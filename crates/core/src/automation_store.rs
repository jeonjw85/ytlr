use crate::*;
use anyhow::{Result, bail};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationDelivery {
    pub id: i64,
    pub target: String,
    pub body: serde_json::Value,
    pub attempts: u32,
    pub next_at: String,
    pub delivered: bool,
    pub error: Option<String>,
}

impl Store {
    pub fn set_full_schedule(
        &self,
        id: &str,
        mut schedule: RecordingSchedule,
        stop_at: Option<&str>,
    ) -> Result<RecordingJob> {
        schedule.start_at = normalize_stop_at(schedule.start_at.as_deref())?;
        let stop_at = normalize_stop_at(stop_at)?;
        schedule.validate(stop_at.as_deref())?;
        self.update_job_checked(id, |job| {
            if job.attempt > 0
                || job.stop_requested
                || !matches!(job.state, JobState::Queued | JobState::Waiting)
            {
                bail!("시작 전 대기 작업의 예약만 변경할 수 있습니다.");
            }
            job.schedule = schedule;
            job.stop_at = stop_at;
            Ok(())
        })
    }
    pub fn set_start_schedule(
        &self,
        id: &str,
        mut schedule: RecordingSchedule,
    ) -> Result<RecordingJob> {
        schedule.start_at = normalize_stop_at(schedule.start_at.as_deref())?;
        self.update_job_checked(id, |job| {
            if job.attempt > 0
                || job.stop_requested
                || !matches!(job.state, JobState::Queued | JobState::Waiting)
            {
                bail!("시작 전 대기 작업의 예약만 변경할 수 있습니다.");
            }
            schedule.validate(job.stop_at.as_deref())?;
            job.schedule = schedule;
            Ok(())
        })
    }
    pub fn notifications(&self, due_only: bool) -> Result<Vec<NotificationDelivery>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(if due_only {
            "SELECT id,target,body,attempts,next_at,delivered,error FROM notification_outbox WHERE delivered=0 AND next_at<=? ORDER BY id LIMIT 20"
        } else {
            "SELECT id,target,body,attempts,next_at,delivered,error FROM notification_outbox WHERE ? IS NOT NULL ORDER BY id DESC LIMIT 100"
        })?;
        let rows = stmt
            .query_map([now()], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, String>(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, target, body, attempts, next_at, delivered, error)| {
                Ok(NotificationDelivery {
                    id,
                    target,
                    body: serde_json::from_str(&body)?,
                    attempts,
                    next_at,
                    delivered,
                    error,
                })
            })
            .collect()
    }
    pub fn finish_notification(&self, id: i64, attempts: u32, error: Option<&str>) -> Result<()> {
        let delay = 5_i64 * 2_i64.pow(attempts.min(10));
        let next = (chrono::Utc::now() + chrono::Duration::seconds(delay.min(3600))).to_rfc3339();
        self.lock()?.execute(
            "UPDATE notification_outbox SET attempts=?,next_at=?,delivered=?,error=? WHERE id=?",
            params![attempts, next, error.is_none(), error, id],
        )?;
        self.lock()?.execute("DELETE FROM notification_outbox WHERE delivered=1 AND id < (SELECT COALESCE(MAX(id),0)-1000 FROM notification_outbox)", [])?;
        Ok(())
    }
    pub fn retry_notification(&self, id: i64) -> Result<()> {
        if self.lock()?.execute(
            "UPDATE notification_outbox SET delivered=0,next_at=?,error=NULL WHERE id=?",
            params![now(), id],
        )? == 0
        {
            bail!("알림을 찾을 수 없습니다.");
        }
        Ok(())
    }
    pub fn maintenance_event(&self, id: &str, message: &str) -> Result<()> {
        self.lock()?.execute(
            "INSERT INTO maintenance_events(at,job_id,message) VALUES(?,?,?)",
            params![now(), id, redact(message)],
        )?;
        self.lock()?.execute("DELETE FROM maintenance_events WHERE id < (SELECT COALESCE(MAX(id),0)-1000 FROM maintenance_events)", [])?;
        Ok(())
    }
    pub fn maintenance_events(&self) -> Result<Vec<serde_json::Value>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT at,job_id,message FROM maintenance_events ORDER BY id DESC LIMIT 100",
        )?;
        Ok(stmt.query_map([], |r| Ok(serde_json::json!({"at":r.get::<_,String>(0)?,"job_id":r.get::<_,String>(1)?,"message":r.get::<_,String>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schedules_outbox_and_occurrences_survive_restart_and_deletion() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(d.path().to_owned())).unwrap();
        let mut settings = paths.default_settings();
        settings
            .automation
            .notifications
            .push(NotificationTarget::Webhook {
                id: "ops".into(),
                url_env: "YTLR_TEST_URL".into(),
            });
        let store = Store::open(&paths.database(), &settings).unwrap();
        let req: RecordRequest = serde_json::from_value(serde_json::json!({"url":"https://youtu.be/abcdefghijk","schedule":{"start_at":"2030-01-01T09:00:00+09:00","duration_minutes":10}})).unwrap();
        let job = store
            .add_window_job(&req, "channel".into(), "window-1", true)
            .unwrap()
            .unwrap();
        assert!(
            store
                .add_window_job(&req, "channel".into(), "window-1", true)
                .unwrap()
                .is_none()
        );
        store.event(&job.id, "recording", "started").unwrap();
        assert!(
            store
                .add_window_job(&req, "channel".into(), "window-2", true)
                .is_err(),
            "an overlapping active job must not consume the next occurrence"
        );
        let delivery = store.notifications(true).unwrap().remove(0);
        store
            .finish_notification(delivery.id, 1, Some("network unavailable"))
            .unwrap();
        drop(store);
        let store = Store::open(&paths.database(), &settings).unwrap();
        assert_eq!(
            store.job(&job.id).unwrap().schedule.start_at.as_deref(),
            Some("2030-01-01T00:00:00+00:00")
        );
        assert!(store.notifications(true).unwrap().is_empty());
        assert_eq!(store.notifications(false).unwrap()[0].attempts, 1);
        store.retry_notification(delivery.id).unwrap();
        assert_eq!(store.notifications(true).unwrap().len(), 1);
        store.delete_job(&job.id).unwrap();
        assert!(
            store
                .add_window_job(&req, "channel".into(), "window-1", true)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.notifications(true).unwrap().len(),
            1,
            "deletion must not discard queued alerts"
        );
        store.finish_notification(delivery.id, 2, None).unwrap();
        assert!(store.notifications(true).unwrap().is_empty());
        assert!(
            store
                .add_window_job(&req, "channel".into(), "window-2", true)
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn schedule_edit_rejects_started_jobs_and_preserves_terminal_age() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(d.path().to_owned())).unwrap();
        let store = Store::open(&paths.database(), &paths.default_settings()).unwrap();
        let req = serde_json::from_value(serde_json::json!({"url":"https://youtu.be/abcdefghijk"}))
            .unwrap();
        let job = store.add_job(&req, None, false).unwrap();
        let schedule = RecordingSchedule {
            start_at: Some("2030-01-01T00:00:00Z".into()),
            duration_minutes: Some(60),
        };
        store
            .set_full_schedule(&job.id, schedule.clone(), Some("2030-01-02T00:00:00Z"))
            .unwrap();
        assert!(
            store
                .set_full_schedule(&job.id, schedule, Some("2029-01-01T00:00:00Z"))
                .is_err()
        );
        assert_eq!(
            store.job(&job.id).unwrap().stop_at.as_deref(),
            Some("2030-01-02T00:00:00+00:00"),
            "invalid full schedule updates are atomic"
        );
        store
            .update_job(&job.id, |j| {
                j.state = JobState::Recording;
                j.attempt = 1;
            })
            .unwrap();
        assert!(
            store
                .set_start_schedule(&job.id, RecordingSchedule::default())
                .is_err()
        );
        let finished = store
            .update_job(&job.id, |j| j.state = JobState::Stopped)
            .unwrap();
        assert!(finished.finished_at.is_some());
        let edited = store.update_job(&job.id, |j| j.protected = true).unwrap();
        assert_eq!(finished.finished_at, edited.finished_at);
        let existing = store
            .add_window_job(&req, "channel".into(), "once", false)
            .unwrap()
            .unwrap();
        assert_eq!(
            existing.id, job.id,
            "legacy once-only channel jobs stay deduplicated"
        );
    }
    #[test]
    fn clip_ranges_reject_unknown_positions_and_cross_attempts() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(d.path().to_owned())).unwrap();
        let store = Store::open(&paths.database(), &paths.default_settings()).unwrap();
        let req = serde_json::from_value(serde_json::json!({"url":"https://youtu.be/abcdefghijk"}))
            .unwrap();
        let mut job = store.add_job(&req, None, false).unwrap();
        job.state = JobState::Completed;
        job.outputs.push(MediaOutput {
            path: job.output_dir.join("attempt-0001/recording.mkv"),
            bytes: 1,
            duration: 60.0,
            has_video: true,
            has_audio: true,
            verification: "container_probe".into(),
            sha256: None,
        });
        job.bookmarks.push(Bookmark {
            id: "a".into(),
            title: "a".into(),
            note: String::new(),
            created_at: now(),
            attempt: 1,
            media_seconds: Some(10.0),
            received_at: now(),
        });
        let mut clip = ClipRequest {
            bookmark_id: "a".into(),
            end_bookmark_id: None,
            before_seconds: 30.0,
            after_seconds: 100.0,
        };
        let (_, start, end) = clip.resolve(&job).unwrap();
        assert_eq!((start, end), (0.0, 60.0));
        let mut b = job.bookmarks[0].clone();
        b.id = "b".into();
        b.attempt = 2;
        b.media_seconds = Some(20.0);
        job.bookmarks.push(b);
        clip.end_bookmark_id = Some("b".into());
        assert!(clip.resolve(&job).is_err());
        job.bookmarks[1].attempt = 1;
        assert_eq!(clip.resolve(&job).unwrap().2, 20.0);
        job.bookmarks[0].media_seconds = None;
        assert!(clip.resolve(&job).is_err());
    }
}
