use crate::*;
use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelHealth {
    pub consecutive_failures: u32,
    pub last_success_at: Option<String>,
    pub error_kind: Option<String>,
    pub incident_open: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PowerStatus {
    pub requested: bool,
    pub active: bool,
    pub waiting_jobs: usize,
    #[serde(default)]
    pub watching_channels: usize,
    pub message: String,
}

pub fn channel_error_kind(message: &str) -> &'static str {
    let text = message.to_lowercase();
    if [
        "cookie",
        "sign in",
        "login",
        "members-only",
        "authentication",
        "403",
        "401",
        "인증",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        "authentication"
    } else if [
        "timeout",
        "timed out",
        "network",
        "connection",
        "dns",
        "resolve host",
        "연결",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        "network"
    } else if ["엔진", "도구", "executable", "yt-dlp", "ffmpeg"]
        .iter()
        .any(|s| text.contains(s))
    {
        "engine"
    } else {
        "extraction"
    }
}

impl Store {
    /// Commit diagnostics and outgoing incident notifications together, preserving concurrent rule edits.
    pub fn complete_channel_scan(
        &self,
        id: &str,
        error: Option<&str>,
        decisions: Vec<RuleDecision>,
    ) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let raw: Option<String> = tx
            .query_row("SELECT body FROM channels WHERE id=?", [id], |r| r.get(0))
            .optional()?;
        let Some(raw) = raw else {
            return Ok(());
        };
        let mut channel: Channel = serde_json::from_str(&raw)?;
        if !channel.enabled {
            return Ok(());
        }
        let at = now();
        channel.last_checked_at = Some(at.clone());
        channel.last_error = error.map(redact);
        channel.decisions = decisions
            .into_iter()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let event = if let Some(error) = error {
            channel.health.consecutive_failures =
                channel.health.consecutive_failures.saturating_add(1);
            channel.health.error_kind = Some(channel_error_kind(error).into());
            if channel.health.consecutive_failures >= 3 && !channel.health.incident_open {
                channel.health.incident_open = true;
                Some((
                    "channel_failed",
                    format!("채널 감시 연속 실패: {}", channel.name),
                ))
            } else {
                None
            }
        } else {
            channel.health.consecutive_failures = 0;
            channel.health.error_kind = None;
            channel.health.last_success_at = Some(at.clone());
            let open = channel.health.incident_open;
            channel.health.incident_open = false;
            open.then(|| {
                (
                    "channel_recovered",
                    format!("채널 감시 정상화: {}", channel.name),
                )
            })
        };
        tx.execute(
            "UPDATE channels SET body=? WHERE id=?",
            params![serde_json::to_string(&channel)?, id],
        )?;
        if let Some((kind, message)) = event {
            tx.execute(
                "INSERT INTO channel_events(channel_id,at,kind,message) VALUES(?,?,?,?)",
                params![id, at, kind, redact(&message)],
            )?;
            let settings: String =
                tx.query_row("SELECT body FROM settings WHERE id=1", [], |r| r.get(0))?;
            let settings: Settings = serde_json::from_str(&settings)?;
            let body = serde_json::to_string(
                &serde_json::json!({"channel_id":id,"title":channel.name,"at":at,"kind":kind,"message":redact(&message),"error_kind":channel.health.error_kind}),
            )?;
            for target in settings.automation.notifications {
                tx.execute(
                    "INSERT INTO notification_outbox(target,body,next_at) VALUES(?,?,?)",
                    params![target.id(), body, at],
                )?;
            }
        }
        tx.execute("DELETE FROM channel_events WHERE channel_id=? AND id NOT IN (SELECT id FROM channel_events WHERE channel_id=? ORDER BY id DESC LIMIT 100)",params![id,id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn channel_events(&self, id: &str) -> Result<Vec<serde_json::Value>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT at,kind,message FROM channel_events WHERE channel_id=? ORDER BY id DESC LIMIT 100")?;
        Ok(stmt.query_map([id], |r|Ok(serde_json::json!({"at":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,"message":r.get::<_,String>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn test_notification(&self, target_id: &str) -> Result<i64> {
        let settings = self.settings()?;
        if !settings
            .automation
            .notifications
            .iter()
            .any(|t| t.id() == target_id)
        {
            bail!("저장된 알림 대상을 찾을 수 없습니다.");
        }
        let at = now();
        let body = serde_json::to_string(
            &serde_json::json!({"kind":"notification_test","at":at,"title":"YTLR","message":"알림 연결 테스트입니다."}),
        )?;
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO notification_outbox(target,body,next_at) VALUES(?,?,?)",
            params![target_id, body, at],
        )?;
        Ok(conn.last_insert_rowid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_incidents_survive_reopening_and_notify_once_per_transition() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(dir.path().into())).unwrap();
        let mut settings = paths.default_settings();
        settings
            .automation
            .notifications
            .push(NotificationTarget::Webhook {
                id: "test".into(),
                url_env: "TEST_URL".into(),
            });
        let store = Store::open(&paths.database(), &settings).unwrap();
        let channel = store
            .add_channel(
                &serde_json::from_value(serde_json::json!({"url":"https://youtube.com/@test"}))
                    .unwrap(),
            )
            .unwrap();
        for _ in 0..5 {
            store
                .complete_channel_scan(&channel.id, Some("HTTP 403 sign in required"), vec![])
                .unwrap();
        }
        assert_eq!(store.notifications(false).unwrap().len(), 1);
        drop(store);
        let store = Store::open(&paths.database(), &settings).unwrap();
        assert_eq!(
            store.channels().unwrap()[0].health.error_kind.as_deref(),
            Some("authentication")
        );
        store
            .complete_channel_scan(&channel.id, None, vec![])
            .unwrap();
        store
            .complete_channel_scan(&channel.id, None, vec![])
            .unwrap();
        assert_eq!(store.notifications(false).unwrap().len(), 2);
        assert!(
            store.channels().unwrap()[0]
                .health
                .last_success_at
                .is_some()
        );
        store.test_notification("test").unwrap();
        assert!(store.test_notification("missing").is_err());
    }
}
