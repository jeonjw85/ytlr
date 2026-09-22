use crate::*;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::{path::Path, sync::Mutex};

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path, defaults: &Settings) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY CHECK(id=1), body TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS jobs (id TEXT PRIMARY KEY, video_id TEXT NOT NULL, body TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS jobs_video ON jobs(video_id);
            CREATE TABLE IF NOT EXISTS channels (id TEXT PRIMARY KEY, url TEXT NOT NULL UNIQUE, body TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY AUTOINCREMENT, job_id TEXT NOT NULL REFERENCES jobs(id), at TEXT NOT NULL, kind TEXT NOT NULL, message TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS replica_requests (request_id TEXT PRIMARY KEY, job_id TEXT NOT NULL REFERENCES jobs(id));
            PRAGMA user_version=1;")?;
        conn.execute(
            "INSERT OR IGNORE INTO settings VALUES(1, ?)",
            [serde_json::to_string(defaults)?],
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow::anyhow!("상태 DB 잠금 오류"))
    }
    pub fn settings(&self) -> Result<Settings> {
        let raw: String =
            self.lock()?
                .query_row("SELECT body FROM settings WHERE id=1", [], |r| r.get(0))?;
        Ok(serde_json::from_str(&raw)?)
    }
    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        settings.validate()?;
        self.lock()?.execute(
            "UPDATE settings SET body=? WHERE id=1",
            [serde_json::to_string(settings)?],
        )?;
        Ok(())
    }
    pub fn jobs(&self) -> Result<Vec<RecordingJob>> {
        let c = self.lock()?;
        let mut s = c.prepare("SELECT body FROM jobs ORDER BY rowid DESC")?;
        let raws = s
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raws.into_iter()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .collect()
    }
    pub fn job(&self, id: &str) -> Result<RecordingJob> {
        let raw: Option<String> = self
            .lock()?
            .query_row("SELECT body FROM jobs WHERE id=?", [id], |r| r.get(0))
            .optional()?;
        Ok(serde_json::from_str(
            &raw.context("녹화 작업을 찾을 수 없습니다.")?,
        )?)
    }
    pub fn add_job(
        &self,
        request: &RecordRequest,
        channel_id: Option<String>,
        automatic: bool,
    ) -> Result<RecordingJob> {
        self.add_job_keyed(request, channel_id, automatic, None)
    }
    pub fn add_replica_job(&self, request: &RecordRequest, key: &str) -> Result<RecordingJob> {
        if key.is_empty()
            || key.len() > 128
            || !key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            bail!("잘못된 이중 녹화 요청 ID");
        }
        self.add_job_keyed(request, None, false, Some(key))
    }
    fn add_job_keyed(
        &self,
        request: &RecordRequest,
        channel_id: Option<String>,
        automatic: bool,
        key: Option<&str>,
    ) -> Result<RecordingJob> {
        let (url, video_id) = video_url(&request.url)?;
        let settings = self.settings()?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        if let Some(key) = key {
            let existing: Option<String> = tx.query_row("SELECT jobs.body FROM jobs JOIN replica_requests ON jobs.id=replica_requests.job_id WHERE request_id=?", [key], |r| r.get(0)).optional()?;
            if let Some(raw) = existing {
                return Ok(serde_json::from_str(&raw)?);
            }
        }
        let mut matching = None;
        {
            let mut s = tx.prepare("SELECT body FROM jobs WHERE video_id=? ORDER BY rowid DESC")?;
            for raw in s.query_map([&video_id], |r| r.get::<_, String>(0))? {
                let existing: RecordingJob = serde_json::from_str(&raw?)?;
                if automatic || !existing.state.terminal() {
                    matching = Some(existing);
                    break;
                }
            }
        }
        if let Some(job) = matching {
            if let Some(key) = key {
                tx.execute(
                    "INSERT INTO replica_requests VALUES(?,?)",
                    params![key, job.id],
                )?;
            }
            tx.commit()?;
            return Ok(job);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let at = now();
        let job = RecordingJob {
            output_dir: settings.storage_root.join(format!("{video_id}_{id}")),
            id,
            url,
            video_id,
            title: "방송 정보 확인 대기".into(),
            channel: String::new(),
            channel_id,
            state: JobState::Queued,
            message: "녹화 대기".into(),
            created_at: at.clone(),
            updated_at: at,
            started_at: None,
            last_media_at: None,
            format: String::new(),
            bytes: 0,
            media_seconds: 0.0,
            attempt: 0,
            retries: 0,
            priority: request.priority,
            live_from_start: request.live_from_start.unwrap_or(settings.live_from_start),
            continuity_uncertain: false,
            stop_requested: false,
            outputs: vec![],
            attempts: vec![],
            gaps: vec![],
            backup: BackupStatus::default(),
            replica: None,
            replica_origin: key.is_some(),
        };
        tx.execute(
            "INSERT INTO jobs VALUES(?,?,?)",
            params![job.id, job.video_id, serde_json::to_string(&job)?],
        )?;
        if let Some(key) = key {
            tx.execute(
                "INSERT INTO replica_requests VALUES(?,?)",
                params![key, job.id],
            )?;
        }
        tx.commit()?;
        Ok(job)
    }
    /// Read/modify/write under one lock: a progress update must not erase a stop request.
    pub fn update_job(
        &self,
        id: &str,
        change: impl FnOnce(&mut RecordingJob),
    ) -> Result<RecordingJob> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let raw: String = tx.query_row("SELECT body FROM jobs WHERE id=?", [id], |r| r.get(0))?;
        let mut job: RecordingJob = serde_json::from_str(&raw)?;
        change(&mut job);
        job.updated_at = now();
        tx.execute(
            "UPDATE jobs SET body=? WHERE id=?",
            params![serde_json::to_string(&job)?, id],
        )?;
        tx.commit()?;
        Ok(job)
    }

    pub fn enqueue_replica(&self, id: &str, target: &str) -> Result<()> {
        self.update_job(id, |j| {
            if j.replica.is_none() && !j.replica_origin && !j.stop_requested && !j.state.terminal()
            {
                j.replica = Some(ReplicaStatus {
                    target: target.into(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                    state: "pending".into(),
                    remote_job_id: None,
                    remote_state: None,
                    attempts: 0,
                    checked_at: None,
                    next_attempt_at: None,
                    message: "원격 요청 대기".into(),
                });
            }
        })?;
        Ok(())
    }
    pub fn event(&self, id: &str, kind: &str, message: &str) -> Result<()> {
        self.lock()?.execute(
            "INSERT INTO events(job_id, at, kind, message) VALUES(?,?,?,?)",
            params![id, now(), kind, redact(message)],
        )?;
        Ok(())
    }
    pub fn events(&self, id: &str) -> Result<Vec<JobEvent>> {
        let c = self.lock()?;
        let mut s = c.prepare("SELECT id,job_id,at,kind,message FROM events WHERE job_id=? ORDER BY id DESC LIMIT 300")?;
        Ok(s.query_map([id], |r| {
            Ok(JobEvent {
                id: r.get(0)?,
                job_id: r.get(1)?,
                at: r.get(2)?,
                kind: r.get(3)?,
                message: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn channels(&self) -> Result<Vec<Channel>> {
        let c = self.lock()?;
        let mut s = c.prepare("SELECT body FROM channels ORDER BY rowid")?;
        let raws = s
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raws.into_iter()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .collect()
    }
    pub fn add_channel(&self, req: &AddChannelRequest) -> Result<Channel> {
        let url = channel_url(&req.url)?;
        if let Some(channel) = self.channels()?.into_iter().find(|c| c.url == url) {
            return Ok(channel);
        }
        let channel = Channel {
            id: uuid::Uuid::new_v4().to_string(),
            name: if req.name.trim().is_empty() {
                url.clone()
            } else {
                req.name.trim().to_string()
            },
            url,
            enabled: true,
            priority: req.priority,
            last_checked_at: None,
            last_error: None,
        };
        self.lock()?.execute(
            "INSERT INTO channels VALUES(?,?,?)",
            params![channel.id, channel.url, serde_json::to_string(&channel)?],
        )?;
        Ok(channel)
    }
    pub fn save_channel(&self, channel: &Channel) -> Result<()> {
        let count = self.lock()?.execute(
            "UPDATE channels SET body=? WHERE id=?",
            params![serde_json::to_string(channel)?, channel.id],
        )?;
        if count == 0 {
            bail!("채널을 찾을 수 없습니다.");
        }
        Ok(())
    }
    pub fn delete_channel(&self, id: &str) -> Result<()> {
        self.lock()?
            .execute("DELETE FROM channels WHERE id=?", [id])?;
        Ok(())
    }
    pub fn recover_interrupted(&self) -> Result<usize> {
        let mut count = 0;
        for job in self.jobs()? {
            if job
                .gaps
                .iter()
                .any(|g| g.status == GapStatus::Recovered && g.evidence.is_none())
            {
                self.update_job(&job.id, |j| {
                    for gap in &mut j.gaps {
                        if gap.status == GapStatus::Recovered && gap.evidence.is_none() {
                            gap.status = GapStatus::Unrecovered;
                        }
                    }
                    j.continuity_uncertain = true;
                })?;
            }
            if job.state.active() {
                self.update_job(&job.id, |j| {
                    j.state = if j.stop_requested {
                        JobState::Stopped
                    } else {
                        JobState::Queued
                    };
                    j.continuity_uncertain = true;
                    j.message = "이전 실행 중단 감지 · 기존 파일 보존 · 재개 대기".into();
                })?;
                self.event(&job.id, "recovery", "서비스 재시작: 이전 수집 구간을 보존합니다. 구간 사이 연속성은 확인되지 않았습니다.")?;
                count += 1;
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_jobs_preserve_data_and_stop_intent() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(d.path().to_owned())).unwrap();
        let db = Store::open(&paths.database(), &paths.default_settings()).unwrap();
        let req = RecordRequest {
            url: "https://youtu.be/abcdefghijk".into(),
            live_from_start: None,
            priority: 0,
        };
        let a = db.add_job(&req, None, false).unwrap();
        assert_eq!(a.id, db.add_job(&req, None, false).unwrap().id);
        db.update_job(&a.id, |j| {
            j.state = JobState::Recording;
            j.bytes = 321;
            j.attempt = 2;
            j.stop_requested = true;
        })
        .unwrap();
        db.update_job(&a.id, |j| {
            j.bytes += 1;
        })
        .unwrap();
        drop(db);
        let db = Store::open(&paths.database(), &paths.default_settings()).unwrap();
        assert_eq!(db.recover_interrupted().unwrap(), 1);
        let a = db.job(&a.id).unwrap();
        assert_eq!(a.state, JobState::Stopped);
        assert_eq!(a.bytes, 322);
        assert_eq!(a.attempt, 2);
        assert!(a.continuity_uncertain);
        assert_eq!(db.recover_interrupted().unwrap(), 0);
    }
}
