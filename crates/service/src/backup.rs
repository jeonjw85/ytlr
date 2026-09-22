use crate::Service;
use std::{
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
use ytlr_core::*;

/// One supervised process for the whole service. Slow/unplugged backup disks cannot
/// occupy capture's async loop or an unbounded number of blocking threads.
pub async fn run(state: Arc<Service>) {
    let mut checks = std::collections::HashMap::<String, Instant>::new();
    loop {
        tokio::select! { _ = state.shutdown.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(5)) => {} }
        let Ok(settings) = state.store.settings() else {
            continue;
        };
        let Some(root) = settings.backup_root else {
            continue;
        };
        let Ok(jobs) = state.store.jobs() else {
            continue;
        };
        for job in jobs.into_iter().filter(|j| j.attempt > 0) {
            let wait = if job.state.terminal() && job.backup.state == "verified" {
                300
            } else {
                10
            };
            if checks
                .get(&job.id)
                .is_some_and(|t| t.elapsed().as_secs() < wait)
            {
                continue;
            }
            let spec_path = state.paths.root.join(format!("backup-{}.json", job.id));
            let spec = BackupSpec {
                job: job.clone(),
                root: root.clone(),
                device: settings.backup_device,
                identity: settings.backup_identity.clone(),
            };
            let result = async {
                atomic_write(&spec_path, &serde_json::to_vec(&spec)?)?;
                let mut command = tokio::process::Command::new(std::env::current_exe()?);
                command
                    .arg("internal-backup")
                    .arg(&spec_path)
                    .stdin(Stdio::piped())
                    .kill_on_drop(true);
                ytlr_engine::process::hide_console(&mut command);
                let mut child = command
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()?;
                let _lifetime = child.stdin.take();
                let output =
                    tokio::time::timeout(Duration::from_secs(180), child.wait_with_output())
                        .await
                        .map_err(|_| {
                            anyhow::anyhow!("백업 작업 시간 제한 초과 · 다음 주기에 재시도")
                        })??;
                if !output.status.success() {
                    anyhow::bail!("{}", redact(&String::from_utf8_lossy(&output.stderr)));
                }
                Ok::<usize, anyhow::Error>(serde_json::from_slice(&output.stdout)?)
            };
            let result =
                tokio::select! { _ = state.shutdown.cancelled() => return, r = result => r };
            let _ = tokio::fs::remove_file(&spec_path).await;
            checks.insert(job.id.clone(), Instant::now());
            let status = match result {
                Ok(count) => BackupStatus {
                    state: if job.state.terminal() {
                        "verified"
                    } else {
                        "in_progress"
                    }
                    .into(),
                    checked_at: Some(now()),
                    message: "복사된 파일과 SHA-256 검증 완료".into(),
                    destination: Some(root.join(&job.video_id).join(&job.id)),
                    verified_files: count,
                },
                Err(e) => BackupStatus {
                    state: "failed".into(),
                    checked_at: Some(now()),
                    message: redact(&e.to_string()),
                    destination: Some(root.join(&job.video_id).join(&job.id)),
                    verified_files: 0,
                },
            };
            if status.state != job.backup.state || status.message != job.backup.message {
                let _ = state.store.event(
                    &job.id,
                    if status.state == "failed" {
                        "backup_error"
                    } else {
                        "backup"
                    },
                    &status.message,
                );
            }
            let _ = state.store.update_job(&job.id, |j| j.backup = status);
        }
    }
}
