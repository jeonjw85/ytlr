use crate::{Service, client::Client, tunnel};
use anyhow::{Context, Result};
use std::{sync::Arc, time::Duration};
use ytlr_core::*;

async fn deliver(
    state: &Service,
    job: &RecordingJob,
    delivery: &ReplicaStatus,
) -> Result<RecordingJob> {
    let remote = state
        .paths
        .remotes()?
        .into_iter()
        .find(|r| r.name == delivery.target)
        .context("이중 녹화 원격 설정을 찾을 수 없습니다.")?;
    let tunnel = tunnel::connect(&remote).await?;
    let client = Client::new(state.paths.clone())?.with_endpoint(tunnel.endpoint.clone());
    let remote_job = if let Some(id) = &delivery.remote_job_id {
        client
            .get::<Snapshot>("/snapshot")
            .await?
            .jobs
            .into_iter()
            .find(|j| &j.id == id)
            .context("원격 작업을 찾을 수 없습니다.")?
    } else {
        client
            .replica_record(
                &RecordRequest {
                    url: job.url.clone(),
                    live_from_start: Some(job.live_from_start),
                    priority: job.priority,
                },
                &delivery.request_id,
            )
            .await?
    };
    Ok(remote_job)
}

pub async fn run(state: Arc<Service>) {
    loop {
        tokio::select! { _ = state.shutdown.cancelled() => return, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
        let Ok(jobs) = state.store.jobs() else {
            continue;
        };
        for job in jobs {
            let Some(mut replica) = job.replica.clone() else {
                continue;
            };
            if matches!(replica.state.as_str(), "finished" | "failed" | "cancelled") {
                continue;
            }
            if replica
                .next_attempt_at
                .as_deref()
                .and_then(parse_rfc3339)
                .is_some_and(|t| t > chrono::Utc::now())
            {
                continue;
            }
            if job.stop_requested && replica.remote_job_id.is_none() {
                replica.state = "cancelled".into();
                replica.message = "전달 전 사용자가 중지했습니다.".into();
                let _ = state
                    .store
                    .update_job(&job.id, |j| j.replica = Some(replica));
                continue;
            }
            let result = tokio::select! { _ = state.shutdown.cancelled() => return, r = deliver(&state, &job, &replica) => r };
            let previous = replica.state.clone();
            replica.checked_at = Some(now());
            match result {
                Ok(remote) => {
                    replica.remote_job_id = Some(remote.id);
                    let receiving = remote.state == JobState::Recording
                        && remote
                            .last_media_at
                            .as_deref()
                            .and_then(parse_rfc3339)
                            .is_some_and(|t| (chrono::Utc::now() - t).num_seconds() < 30);
                    replica.state = if remote.state.terminal() {
                        "finished"
                    } else if receiving {
                        "recording"
                    } else {
                        "accepted"
                    }
                    .into();
                    replica.message = if receiving {
                        "원격 영상 수신 확인"
                    } else {
                        "원격 요청 접수 · 녹화 상태는 별도 확인"
                    }
                    .into();
                    replica.remote_state = Some(remote.state);
                    replica.attempts = 0;
                    replica.next_attempt_at =
                        Some((chrono::Utc::now() + chrono::Duration::seconds(15)).to_rfc3339());
                }
                Err(e) => {
                    replica.attempts += 1;
                    replica.state = if replica.attempts >= 8 {
                        "failed"
                    } else {
                        "retrying"
                    }
                    .into();
                    replica.message = redact(&e.to_string());
                    replica.next_attempt_at = Some(
                        (chrono::Utc::now()
                            + chrono::Duration::seconds(5 * 2i64.pow(replica.attempts.min(5))))
                        .to_rfc3339(),
                    );
                }
            }
            if previous != replica.state {
                let _ = state.store.event(&job.id, "replica", &replica.message);
            }
            let _ = state
                .store
                .update_job(&job.id, |j| j.replica = Some(replica));
        }
    }
}
