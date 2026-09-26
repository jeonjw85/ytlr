use crate::*;
use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableSettings {
    pub max_recordings: usize,
    pub scan_interval_secs: u64,
    pub stall_timeout_secs: u64,
    pub min_free_bytes: u64,
    pub max_retries: u32,
    pub live_from_start: bool,
    pub prevent_sleep: bool,
    pub keep_awake_waiting: bool,
    pub automation: AutomationSettings,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableChannel {
    pub url: String,
    pub name: String,
    pub enabled: bool,
    pub priority: i32,
    pub recording_options: RecordingOptions,
    pub live_from_start: Option<bool>,
    pub rules: ChannelRules,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigurationBundle {
    pub format_version: u32,
    pub exported_at: String,
    pub source_storage_root: PathBuf,
    pub settings: PortableSettings,
    pub channels: Vec<PortableChannel>,
    pub schedules: Vec<RecordRequest>,
}
#[derive(Debug, Deserialize)]
pub struct ConfigurationImport {
    pub bundle: ConfigurationBundle,
    #[serde(default)]
    pub replace_channels: bool,
    #[serde(default)]
    pub apply_settings: bool,
    pub storage_root: Option<PathBuf>,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ImportReport {
    pub added_channels: usize,
    pub replaced_channels: usize,
    pub skipped_channels: usize,
    pub added_schedules: usize,
    pub skipped_schedules: usize,
    pub expired_schedules: usize,
    pub storage_root: PathBuf,
    pub retention: RetentionPolicy,
}

pub fn new_recording_job(
    request: &RecordRequest,
    settings: &Settings,
    channel_id: Option<String>,
    replica_origin: bool,
) -> Result<RecordingJob> {
    request.recording_options.validate()?;
    let stop_at = normalize_stop_at(request.stop_at.as_deref())?;
    request.schedule.validate(stop_at.as_deref())?;
    let mut schedule = request.schedule.clone();
    schedule.start_at = normalize_stop_at(schedule.start_at.as_deref())?;
    let (url, video_id) = video_url(&request.url)?;
    let id = uuid::Uuid::new_v4().to_string();
    let at = now();
    Ok(RecordingJob {
        schedule,
        protected: false,
        finished_at: None,
        stop_at,
        bookmarks: vec![],
        alerts: vec![],
        recovery_error: None,
        recording_options: request.recording_options.clone(),
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
        resolution: None,
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
        replica_origin,
    })
}

impl Store {
    pub fn export_configuration(&self) -> Result<ConfigurationBundle> {
        let conn = self.lock()?;
        let raw: String =
            conn.query_row("SELECT body FROM settings WHERE id=1", [], |r| r.get(0))?;
        let settings: Settings = serde_json::from_str(&raw)?;
        let channels = {
            let mut stmt = conn.prepare("SELECT body FROM channels ORDER BY rowid")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|s| serde_json::from_str::<Channel>(&s))
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let jobs = {
            let mut stmt = conn.prepare("SELECT body FROM jobs ORDER BY rowid")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|s| serde_json::from_str::<RecordingJob>(&s))
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(ConfigurationBundle {
            format_version: 1,
            exported_at: now(),
            source_storage_root: settings.storage_root.clone(),
            settings: PortableSettings {
                max_recordings: settings.max_recordings,
                scan_interval_secs: settings.scan_interval_secs,
                stall_timeout_secs: settings.stall_timeout_secs,
                min_free_bytes: settings.min_free_bytes,
                max_retries: settings.max_retries,
                live_from_start: settings.live_from_start,
                prevent_sleep: settings.prevent_sleep,
                keep_awake_waiting: settings.keep_awake_waiting,
                automation: settings.automation,
            },
            channels: channels
                .into_iter()
                .map(|c| PortableChannel {
                    url: c.url,
                    name: c.name,
                    enabled: c.enabled,
                    priority: c.priority,
                    recording_options: c.recording_options,
                    live_from_start: c.live_from_start,
                    rules: c.rules,
                })
                .collect(),
            schedules: jobs
                .into_iter()
                .filter(|j| {
                    j.channel_id.is_none()
                        && j.attempt == 0
                        && !j.state.terminal()
                        && !j.stop_requested
                })
                .map(|j| RecordRequest {
                    url: j.url,
                    schedule: j.schedule,
                    stop_at: j.stop_at,
                    recording_options: j.recording_options,
                    live_from_start: Some(j.live_from_start),
                    priority: j.priority,
                })
                .collect(),
        })
    }
    pub fn import_configuration(
        &self,
        req: &ConfigurationImport,
        preview: bool,
    ) -> Result<ImportReport> {
        if req.bundle.format_version != 1 {
            bail!("지원하지 않는 설정 파일 버전입니다.");
        }
        if req.bundle.channels.len() > 1000 || req.bundle.schedules.len() > 1000 {
            bail!("가져오기 항목이 너무 많습니다.");
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let raw: String = tx.query_row("SELECT body FROM settings WHERE id=1", [], |r| r.get(0))?;
        let mut settings: Settings = serde_json::from_str(&raw)?;
        if req.apply_settings {
            let p = &req.bundle.settings;
            settings.max_recordings = p.max_recordings;
            settings.scan_interval_secs = p.scan_interval_secs;
            settings.stall_timeout_secs = p.stall_timeout_secs;
            settings.min_free_bytes = p.min_free_bytes;
            settings.max_retries = p.max_retries;
            settings.live_from_start = p.live_from_start;
            settings.prevent_sleep = p.prevent_sleep;
            settings.keep_awake_waiting = p.keep_awake_waiting;
            settings.automation = p.automation.clone();
        }
        if let Some(root) = &req.storage_root {
            settings.storage_root = root.clone();
        }
        settings.validate()?;
        let mut report = ImportReport {
            storage_root: settings.storage_root.clone(),
            retention: settings.automation.retention.clone(),
            ..Default::default()
        };
        let mut seen = HashSet::new();
        for incoming in &req.bundle.channels {
            let url = channel_url(&incoming.url)?;
            if !seen.insert(url.clone()) {
                bail!("설정 파일에 중복 채널이 있습니다.");
            }
            incoming.rules.validate()?;
            incoming.recording_options.validate()?;
            let existing: Option<String> = tx
                .query_row(
                    "SELECT body FROM channels WHERE json_extract(body,'$.url')=?",
                    [&url],
                    |r| r.get(0),
                )
                .optional()?;
            let mut channel = if let Some(raw) = existing {
                if !req.replace_channels {
                    report.skipped_channels += 1;
                    continue;
                }
                report.replaced_channels += 1;
                serde_json::from_str::<Channel>(&raw)?
            } else {
                report.added_channels += 1;
                Channel {
                    id: uuid::Uuid::new_v4().to_string(),
                    url: url.clone(),
                    name: String::new(),
                    enabled: true,
                    priority: 0,
                    recording_options: RecordingOptions::default(),
                    live_from_start: None,
                    rules: ChannelRules::default(),
                    decisions: vec![],
                    health: ChannelHealth::default(),
                    last_checked_at: None,
                    last_error: None,
                }
            };
            channel.name = incoming.name.clone();
            channel.enabled = incoming.enabled;
            channel.priority = incoming.priority;
            channel.recording_options = incoming.recording_options.clone();
            channel.live_from_start = incoming.live_from_start;
            channel.rules = incoming.rules.clone();
            if !preview {
                tx.execute("INSERT INTO channels(id,url,body) VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET url=excluded.url,body=excluded.body",params![channel.id,url,serde_json::to_string(&channel)?])?;
            }
        }
        let existing_jobs = {
            let mut stmt = tx.prepare("SELECT body FROM jobs")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|s| serde_json::from_str::<RecordingJob>(&s))
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut videos = existing_jobs
            .iter()
            .filter(|j| !j.state.terminal())
            .map(|j| j.video_id.clone())
            .collect::<HashSet<_>>();
        for incoming in &req.bundle.schedules {
            let job = new_recording_job(incoming, &settings, None, false)?;
            if job
                .stop_at
                .as_deref()
                .and_then(parse_rfc3339)
                .is_some_and(|at| at <= chrono::Utc::now())
            {
                report.expired_schedules += 1;
                continue;
            }
            let canonical = RecordRequest {
                url: job.url.clone(),
                schedule: job.schedule.clone(),
                stop_at: job.stop_at.clone(),
                recording_options: job.recording_options.clone(),
                live_from_start: Some(job.live_from_start),
                priority: job.priority,
            };
            let key = hex::encode(Sha256::digest(serde_json::to_vec(&canonical)?));
            if !videos.insert(job.video_id.clone())
                || tx
                    .query_row(
                        "SELECT 1 FROM configuration_imports WHERE key=?",
                        [&key],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some()
            {
                report.skipped_schedules += 1;
                continue;
            }
            report.added_schedules += 1;
            if !preview {
                tx.execute(
                    "INSERT INTO jobs(id,video_id,body) VALUES(?,?,?)",
                    params![job.id, job.video_id, serde_json::to_string(&job)?],
                )?;
                tx.execute(
                    "INSERT INTO configuration_imports(key,job_id) VALUES(?,?)",
                    params![key, job.id],
                )?;
            }
        }
        if !preview {
            tx.execute(
                "UPDATE settings SET body=? WHERE id=1",
                [serde_json::to_string(&settings)?],
            )?;
            tx.commit()?;
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_is_read_only_and_import_is_atomic_and_deduplicated() {
        let a = tempfile::tempdir().unwrap();
        let p = AppPaths::resolve(Some(a.path().into())).unwrap();
        let source = Store::open(&p.database(), &p.default_settings()).unwrap();
        source
            .add_channel(
                &serde_json::from_value(serde_json::json!({"url":"https://youtube.com/@test"}))
                    .unwrap(),
            )
            .unwrap();
        source.add_job(&serde_json::from_value(serde_json::json!({"url":"https://youtu.be/abcdefghijk","schedule":{"start_at":"2030-01-01T00:00:00Z"}})).unwrap(),None,false).unwrap();
        let d = tempfile::tempdir().unwrap();
        let p = AppPaths::resolve(Some(d.path().into())).unwrap();
        let dest = Store::open(&p.database(), &p.default_settings()).unwrap();
        let mut req = ConfigurationImport {
            bundle: source.export_configuration().unwrap(),
            replace_channels: false,
            apply_settings: true,
            storage_root: None,
        };
        assert_eq!(
            dest.import_configuration(&req, true)
                .unwrap()
                .added_channels,
            1
        );
        assert!(dest.channels().unwrap().is_empty());
        req.bundle.schedules[0].url = "invalid".into();
        assert!(dest.import_configuration(&req, false).is_err());
        assert!(dest.channels().unwrap().is_empty());
        req.bundle = source.export_configuration().unwrap();
        let report = dest.import_configuration(&req, false).unwrap();
        assert_eq!(report.added_schedules, 1);
        let report = dest.import_configuration(&req, false).unwrap();
        assert_eq!(report.skipped_channels, 1);
        assert_eq!(report.skipped_schedules, 1);
        assert!(dest.jobs().unwrap()[0].output_dir.starts_with(d.path()));
    }
}
