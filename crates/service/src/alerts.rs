use crate::Service;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use ytlr_core::*;

fn conditions(
    job: &RecordingJob,
    settings: &Settings,
    at: chrono::DateTime<chrono::Utc>,
) -> BTreeMap<String, String> {
    let mut issues = BTreeMap::new();
    let current_attempt = job.attempts.iter().find(|a| a.n == job.attempt);
    let receiving = job.state == JobState::Recording
        && current_attempt
            .and_then(|a| a.last_media_at.as_deref())
            .and_then(parse_rfc3339)
            .is_some_and(|last| (at - last).num_seconds() < settings.stall_timeout_secs as i64);
    if job.state == JobState::Recording {
        let last = job
            .attempts
            .iter()
            .find(|a| a.n == job.attempt)
            .map(|a| a.last_media_at.as_deref().unwrap_or(&a.started_at));
        if last
            .and_then(parse_rfc3339)
            .is_some_and(|last| (at - last).num_seconds() >= settings.stall_timeout_secs as i64)
        {
            issues.insert("stalled".into(), "미디어 수신이 멈췄습니다.".into());
        }
    }
    if job.state == JobState::Reconnecting && job.retries >= 3 {
        issues.insert(
            "reconnecting".into(),
            "녹화 연결이 반복해서 끊어지고 있습니다.".into(),
        );
    }
    if job.state == JobState::Reconnecting && job.message.contains("수신 정지") {
        issues.insert("stalled".into(), "미디어 수신이 멈췄습니다.".into());
    }
    if job.state == JobState::Failed {
        issues.insert(
            "recording_failed".into(),
            "녹화에 실패했습니다. 작업 이벤트를 확인하세요.".into(),
        );
    }
    // Retrying alone is not evidence of recovery. Resolve only after media arrives,
    // or after the job ends/stops. This prevents repeated recovery notifications.
    if !receiving && !job.state.terminal() && !job.stop_requested {
        for alert in job.alerts.iter().filter(|a| {
            a.active
                && matches!(
                    a.kind.as_str(),
                    "stalled" | "reconnecting" | "recording_failed"
                )
        }) {
            issues
                .entry(alert.kind.clone())
                .or_insert_with(|| alert.message.clone());
        }
    }
    if job.recovery_error.is_some() {
        issues.insert(
            "recovery_failed".into(),
            "자동 복구에 실패했습니다. 원본은 보존됩니다.".into(),
        );
    }
    if job.replica.as_ref().is_some_and(|r| {
        r.state == "failed"
            || r.attempts >= 3
            || matches!(r.remote_state, Some(JobState::Failed | JobState::Partial))
    }) {
        issues.insert(
            "replica_failed".into(),
            "원격 이중 녹화 상태를 확인하세요.".into(),
        );
    }
    issues
}

fn reconcile(
    job: &mut RecordingJob,
    desired: &BTreeMap<String, String>,
    at: &str,
) -> Vec<(String, String)> {
    let mut events = vec![];
    for alert in &mut job.alerts {
        if alert.active && !desired.contains_key(&alert.kind) {
            alert.active = false;
            alert.resolved_at = Some(at.into());
            events.push((
                "alert_resolved".into(),
                format!("{} · 알림 해제", alert.message),
            ));
        }
    }
    for (kind, message) in desired {
        if job.alerts.iter().any(|a| &a.kind == kind && a.active) {
            continue;
        }
        job.alerts.retain(|a| &a.kind != kind);
        job.alerts.push(JobAlert {
            id: uuid::Uuid::new_v4().to_string(),
            kind: kind.clone(),
            message: message.clone(),
            active: true,
            opened_at: at.into(),
            resolved_at: None,
        });
        events.push(("alert_opened".into(), message.clone()));
    }
    events
}

pub async fn run(state: Arc<Service>) {
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! { _ = state.shutdown.cancelled() => return, _ = tick.tick() => {} }
        let (Ok(settings), Ok(jobs)) = (state.store.settings(), state.store.jobs()) else {
            continue;
        };
        for job in jobs {
            let desired = conditions(&job, &settings, chrono::Utc::now());
            let current: BTreeMap<_, _> = job
                .alerts
                .iter()
                .filter(|a| a.active)
                .map(|a| (a.kind.clone(), a.message.clone()))
                .collect();
            if desired == current {
                continue;
            }
            let mut events = vec![];
            let result = state.store.update_job(&job.id, |j| {
                let desired = conditions(j, &settings, chrono::Utc::now());
                events = reconcile(j, &desired, &now());
            });
            if result.is_ok() {
                let _ = state.store.persist_job_manifest(&job.id);
                for (kind, message) in events {
                    let _ = state.store.event(&job.id, &kind, &message);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incidents_are_deduplicated_and_only_media_resolves_retries() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(d.path().to_owned())).unwrap();
        let settings = paths.default_settings();
        let store = Store::open(&paths.database(), &settings).unwrap();
        let req: RecordRequest =
            serde_json::from_value(serde_json::json!({"url":"https://youtu.be/abcdefghijk"}))
                .unwrap();
        let mut job = store.add_job(&req, None, false).unwrap();
        let at = chrono::Utc::now();
        job.state = JobState::Reconnecting;
        job.retries = 3;
        let desired = conditions(&job, &settings, at);
        assert_eq!(reconcile(&mut job, &desired, &at.to_rfc3339()).len(), 1);
        let first_id = job.alerts[0].id.clone();
        assert!(reconcile(&mut job, &desired, &at.to_rfc3339()).is_empty());
        job.state = JobState::Preparing;
        assert_eq!(conditions(&job, &settings, at), desired);
        job.state = JobState::Recording;
        job.attempt = 1;
        note_attempt_start(&mut job, 1, &at.to_rfc3339());
        assert_eq!(conditions(&job, &settings, at), desired);
        note_media_received(&mut job, 1, &at.to_rfc3339());
        job.last_media_at = Some(at.to_rfc3339());
        let desired = conditions(&job, &settings, at);
        assert_eq!(reconcile(&mut job, &desired, &at.to_rfc3339()).len(), 1);
        assert!(!job.alerts[0].active);
        assert!(reconcile(&mut job, &desired, &at.to_rfc3339()).is_empty());
        job.state = JobState::Reconnecting;
        let desired = conditions(&job, &settings, at);
        reconcile(&mut job, &desired, &at.to_rfc3339());
        assert_ne!(job.alerts[0].id, first_id);
    }
}
