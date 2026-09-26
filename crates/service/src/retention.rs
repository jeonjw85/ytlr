use crate::Service;
use std::{sync::Arc, time::Duration};
use ytlr_core::*;

pub(crate) fn due(job: &RecordingJob, days: Option<u32>) -> bool {
    !job.protected
        && job.state.terminal()
        && days.is_some_and(|days| {
            job.finished_at
                .as_deref()
                .and_then(parse_rfc3339)
                .is_some_and(|at| chrono::Utc::now() - at >= chrono::Duration::days(days as i64))
        })
}
pub async fn run(state: Arc<Service>) {
    let mut tick = tokio::time::interval(Duration::from_secs(3600));
    loop {
        tokio::select! { _ = state.shutdown.cancelled() => return, _ = tick.tick() => {} }
        let (Ok(settings), Ok(jobs)) = (state.store.settings(), state.store.jobs()) else {
            continue;
        };
        let policy = settings.automation.retention;
        for job in jobs {
            if state.store.job_has_operations(&job.id).unwrap_or(true) {
                continue;
            }
            if state.shutdown.is_cancelled() {
                return;
            }
            let result = if due(&job, policy.delete_after_days) {
                crate::http::delete_job_impl(state.clone(), job.id.clone(), true).await
            } else if due(&job, policy.cleanup_after_days) {
                crate::http::automatic_cleanup(state.clone(), job.id.clone()).await
            } else {
                continue;
            };
            let message = match result {
                Ok(value) => format!("보관 정책 처리 완료: {value}"),
                Err(e) => format!("보관 정책 처리 보류: {e}"),
            };
            let _ = state.store.maintenance_event(&job.id, &message);
        }
    }
}
