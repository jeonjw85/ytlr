use crate::{Service, client::Client, files};
use anyhow::{Context, Result, bail};
use axum::{
    Json,
    extract::{Path, State},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use ytlr_core::*;

fn status(s: &Service, id: &str, message: &str, progress: Option<f64>) {
    let _ = s.store.update_operation(id, |o| {
        o.message = message.into();
        o.progress = progress;
        Ok(())
    });
}
pub async fn list(State(s): State<Arc<Service>>) -> crate::http::ApiResult<Vec<Operation>> {
    Ok(Json(s.store.operations()?))
}
pub async fn enqueue(
    State(s): State<Arc<Service>>,
    Json(mut reqs): Json<Vec<OperationRequest>>,
) -> crate::http::ApiResult<Vec<Operation>> {
    let active = s.active.lock().await;
    if s.shutdown.is_cancelled() {
        return Err(anyhow::anyhow!("서비스 종료 중입니다.").into());
    }
    for req in &mut reqs {
        req.source_path = None;
        if let OperationTask::Download { remote, .. } = &req.task {
            if !s.paths.remotes()?.iter().any(|r| &r.name == remote) {
                return Err(anyhow::anyhow!("등록된 원격 서버를 찾을 수 없습니다.").into());
            }
        } else {
            if active.contains_key(&req.job_id) {
                return Err(anyhow::anyhow!("파일 처리 완료 후 작업을 추가하세요.").into());
            }
            let job = s.store.job(&req.job_id)?;
            if !job.state.terminal() {
                return Err(anyhow::anyhow!("녹화 완료 후 작업을 추가하세요.").into());
            }
            if matches!(req.task, OperationTask::Cleanup { .. }) && job.protected {
                return Err(anyhow::anyhow!("중요 녹화 보호를 해제하세요.").into());
            }
            if let OperationTask::Export { index } = req.task {
                let output = job.outputs.get(index).context("결과 파일이 없습니다.")?;
                req.source_path = Some(
                    output
                        .path
                        .strip_prefix(&job.output_dir)?
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
            if let OperationTask::Clip { request } = &req.task {
                let (output, start, end) = request.resolve(&job)?;
                let path = output
                    .path
                    .strip_prefix(&job.output_dir)?
                    .to_string_lossy()
                    .replace('\\', "/");
                req.source_path = Some(path.clone());
                req.task = OperationTask::RangeClip { path, start, end };
            }
        }
    }
    let operations = s.store.enqueue_operations(&reqs)?;
    drop(active);
    Ok(Json(operations))
}
pub async fn cancel(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
) -> crate::http::ApiResult<Operation> {
    let active = s.op_active.lock().await;
    let op = s.store.cancel_operation(&id)?;
    if let Some(token) = active.get(&id) {
        token.cancel();
    }
    Ok(Json(op))
}
pub async fn retry(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
) -> crate::http::ApiResult<Operation> {
    // Use the same lock order as claims/deletion so a retry cannot revive a job
    // after deletion or recording restart has already reserved it.
    let active = s.active.lock().await;
    let running = s.op_active.lock().await;
    if s.shutdown.is_cancelled() || running.contains_key(&id) {
        return Err(anyhow::anyhow!("작업 종료를 기다려 주세요.").into());
    }
    let operation = s.store.operation(&id)?;
    if let OperationTask::Download { remote, .. } = &operation.request.task {
        if !s.paths.remotes()?.iter().any(|r| &r.name == remote) {
            return Err(anyhow::anyhow!("등록된 원격 서버를 찾을 수 없습니다.").into());
        }
    } else {
        let job = s.store.job(&operation.request.job_id)?;
        if active.contains_key(&job.id) || !job.state.terminal() {
            return Err(anyhow::anyhow!("파일 처리 완료 후 작업을 재시도하세요.").into());
        }
        if job.protected && matches!(operation.request.task, OperationTask::Cleanup { .. }) {
            return Err(anyhow::anyhow!("중요 녹화 보호를 해제하세요.").into());
        }
    }
    Ok(Json(s.store.retry_operation(&id)?))
}

pub async fn run(s: Arc<Service>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {_=s.shutdown.cancelled()=>return,_=tick.tick()=>{}}
        if s.installing.load(std::sync::atomic::Ordering::SeqCst) {
            continue;
        }
        let Ok(operations) = s.store.queued_operations() else {
            continue;
        };
        // Reserve while holding the same lock as idle shutdown.
        let active = s.active.lock().await;
        let Some(op) = operations
            .into_iter()
            .rev()
            .find(|o| o.state == "queued" && !active.contains_key(&o.request.job_id))
        else {
            continue;
        };
        if s.shutdown.is_cancelled()
            || s.installing.load(std::sync::atomic::Ordering::SeqCst)
            || active.contains_key(&op.request.job_id)
        {
            continue;
        }
        let mut running = s.op_active.lock().await;
        let token = s.shutdown.child_token();
        let Ok(op) = s.store.update_operation(&op.id, |o| {
            if o.state != "queued" || o.cancel_requested {
                bail!("작업이 취소되었습니다.");
            }
            o.state = "running".into();
            o.attempts += 1;
            o.error = None;
            o.message = "작업 시작".into();
            Ok(())
        }) else {
            continue;
        };
        running.insert(op.id.clone(), token.clone());
        drop(running);
        drop(active);
        let result = execute(s.clone(), op.clone(), token.clone())
            .await
            .map_err(|error| redact(&error.to_string()));
        loop {
            let mut running = s.op_active.lock().await;
            let saved = s.store.update_operation(&op.id, |o| {
                match &result {
                    Ok(value) => {
                        let partial = value.get("completed") == Some(&Value::Bool(false));
                        o.state = if partial { "failed" } else { "completed" }.into();
                        o.cancel_requested = false;
                        o.progress = if partial { None } else { Some(1.0) };
                        o.result = Some(value.clone());
                        o.message = if partial {
                            "일부 파일 정리가 남아 있습니다."
                        } else {
                            "작업 완료"
                        }
                        .into();
                        if partial {
                            o.error = Some("남은 파일을 재검증하려면 작업을 재시도하세요.".into());
                        }
                    }
                    Err(error) => {
                        if o.cancel_requested {
                            o.state = "cancelled".into();
                            o.message = "작업 취소 · 받은 파일 보존".into();
                        } else if s.shutdown.is_cancelled() {
                            o.state = "queued".into();
                            o.message = "서비스 종료 · 재검증 후 재개 대기".into();
                        } else {
                            o.state = "failed".into();
                            o.error = Some(error.clone());
                            o.message = "작업 실패".into();
                        }
                    }
                }
                Ok(())
            });
            if saved.is_ok() {
                running.remove(&op.id);
                break;
            }
            drop(running);
            // Keep the operation reserved while its terminal state is not durable.
            // On shutdown, the stored running state is revalidated on next start.
            tokio::select! {
                _ = s.shutdown.cancelled() => { s.op_active.lock().await.remove(&op.id); return; }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
    }
}

async fn execute(s: Arc<Service>, op: Operation, cancel: CancellationToken) -> Result<Value> {
    match &op.request.task {
        OperationTask::Download { remote, path } => download(&s, &op, remote, path, cancel).await,
        OperationTask::Cleanup { files } => {
            status(&s, &op.id, "정리 대상과 원본 해시 재검증 중", None);
            crate::http::queued_cleanup(s, op.request.job_id.clone(), files.clone()).await
        }
        OperationTask::Recover => {
            status(&s, &op.id, "원본 복구 및 미디어 검사 중", None);
            crate::http::queued_recovery(s, op.request.job_id.clone()).await
        }
        _ => {
            {
                let mut active = s.active.lock().await;
                if s.shutdown.is_cancelled() || active.contains_key(&op.request.job_id) {
                    bail!("파일 처리 중입니다. 다시 시도하세요.");
                }
                active.insert(op.request.job_id.clone(), cancel.clone());
            }
            let result = render(&s, &op, cancel).await;
            s.active.lock().await.remove(&op.request.job_id);
            result
        }
    }
}

async fn render(s: &Service, op: &Operation, cancel: CancellationToken) -> Result<Value> {
    let _permit = s.finalizer.acquire().await?;
    let job = s.store.job(&op.request.job_id)?;
    let _file_lock = files::file_lock(&job)?;
    let (path, range, preview) = match &op.request.task {
        OperationTask::Export { index } => (
            if let Some(path) = &op.request.source_path {
                files::media_path(&job, path)?
            } else {
                job.outputs
                    .get(*index)
                    .context("결과 파일이 없습니다.")?
                    .path
                    .clone()
            },
            None,
            false,
        ),
        OperationTask::Clip { request } => {
            let (output, start, end) = request.resolve(&job)?;
            (output.path, Some((start, end)), false)
        }
        OperationTask::RangeClip { path, start, end } => {
            (files::media_path(&job, path)?, Some((*start, *end)), false)
        }
        OperationTask::Preview { path } => (files::media_path(&job, path)?, None, true),
        _ => unreachable!(),
    };
    let relative = path
        .strip_prefix(&job.output_dir)?
        .to_string_lossy()
        .replace('\\', "/");
    status(s, &op.id, "원본 파일 해시 확인 중", None);
    let j = job.clone();
    let p = relative.clone();
    let check = cancel.clone();
    let store = s.store.clone();
    let operation_id = op.id.clone();
    let input = tokio::task::spawn_blocking(move || {
        let mut updated = std::time::Instant::now();
        files::describe_with_progress(&j, &p, |bytes| {
            if check.is_cancelled() {
                bail!("파일 검사가 중단되었습니다.");
            }
            if updated.elapsed() >= Duration::from_millis(250) {
                store.update_operation(&operation_id, |o| {
                    o.bytes_done = bytes;
                    Ok(())
                })?;
                updated = std::time::Instant::now();
            }
            Ok(())
        })
    })
    .await??;
    if op.input.as_ref().is_some_and(|old| {
        old.sha256 != input.sha256 || old.path != input.path || old.bytes != input.bytes
    }) {
        bail!("원본이 변경되어 이전 작업을 재개할 수 없습니다.");
    }
    s.store.update_operation(&op.id, |o| {
        o.input = Some(input.clone());
        o.total_bytes = Some(input.bytes);
        Ok(())
    })?;
    let prefix = if preview {
        "preview"
    } else if range.is_some() {
        "clip"
    } else {
        "export"
    };
    let ext = if preview || !job.recording_options.audio_only {
        "mp4"
    } else {
        "mka"
    };
    let mut dest = op
        .destination
        .clone()
        .unwrap_or_else(|| job.output_dir.join(format!("{prefix}-{}.{ext}", op.id)));
    if dest.is_file() {
        if let Ok(bytes) = std::fs::read(dest.with_extension("operation.json"))
            && let Ok(receipt) = serde_json::from_slice::<Value>(&bytes)
            && receipt["request"] == serde_json::to_value(&op.request)?
            && receipt["source_sha256"] == input.sha256
        {
            let output: MediaOutput = serde_json::from_value(receipt["output"].clone())?;
            let p = dest.clone();
            let check = cancel.clone();
            let digest = tokio::task::spawn_blocking(move || {
                file_digest_with_progress(&p, |_| {
                    if check.is_cancelled() {
                        bail!("파일 검사가 중단되었습니다.");
                    }
                    Ok(())
                })
            })
            .await??;
            if output.path == dest
                && output.sha256.as_deref() == Some(&digest.1)
                && output.bytes == digest.0
            {
                return Ok(json!({"output":output,"path":dest,"source":relative}));
            }
            bail!("이전 결과 파일이 변경되었습니다.");
        }
        // A crash between publication and receipt leaves an ambiguous derived file.
        // Preserve it and publish the retry under a fresh name.
        dest = job
            .output_dir
            .join(format!("{prefix}-{}-{}.{ext}", op.id, op.attempts));
    }
    s.store.update_operation(&op.id, |o| {
        o.destination = Some(dest.clone());
        Ok(())
    })?;
    ensure_storage(&job.output_dir, s.store.settings()?.min_free_bytes)?;
    let input_media = ytlr_engine::probe(&s.tools, &path).await?;
    let duration = range.map(|(a, b)| b - a).unwrap_or(input_media.duration);
    let supervisor = std::env::current_exe()?;
    let output = ytlr_engine::render::render(
        &s.tools,
        ytlr_engine::render::RenderRequest {
            supervisor: Some(&supervisor),
            min_free_bytes: s.store.settings()?.min_free_bytes,
            source: &path,
            destination: &dest,
            options: &job.recording_options,
            range,
            preview,
        },
        cancel,
        |bytes, seconds| {
            let _ = s.store.update_operation(&op.id, |o| {
                o.bytes_done = bytes;
                o.progress = (duration > 0.0 && seconds >= 0.0)
                    .then(|| (seconds / duration).clamp(0.0, 0.99));
                o.message = if seconds < 0.0 {
                    "결과 파일 해시 검증 중"
                } else {
                    "미디어 처리 중"
                }
                .into();
                Ok(())
            });
        },
    )
    .await?;
    atomic_write(
        &dest.with_extension("operation.json"),
        &serde_json::to_vec(
            &json!({"request":op.request,"source_sha256":input.sha256,"output":output}),
        )?,
    )?;
    let root = job.output_dir.clone();
    let bytes = tokio::task::spawn_blocking(move || -> Result<u64> {
        Ok(ytlr_engine::files_under(&root)?
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum())
    })
    .await??;
    s.store.update_job(&job.id, |j| j.bytes = bytes)?;
    s.store.persist_job_manifest(&job.id)?;
    s.store.event(
        &job.id,
        "export",
        &format!("파일 처리 완료: {}", dest.display()),
    )?;
    Ok(json!({"output":output,"path":dest,"source":relative,"approximate":range.is_some()}))
}

async fn download(
    s: &Service,
    op: &Operation,
    remote: &str,
    path: &str,
    cancel: CancellationToken,
) -> Result<Value> {
    status(s, &op.id, "SSH 연결 및 원격 파일 확인 중", None);
    let remote = s
        .paths
        .remotes()?
        .into_iter()
        .find(|r| r.name == remote)
        .context("원격 설정을 찾을 수 없습니다.")?;
    let tunnel = tokio::select! {_=cancel.cancelled()=>bail!("전송 취소"),r=crate::tunnel::connect(&remote)=>r?};
    let client = Client::new(s.paths.clone())?.with_endpoint(tunnel.endpoint.clone());
    let info_path = format!("/jobs/{}/file-info", op.request.job_id);
    let info_body = json!({"path":path});
    let input: Artifact = tokio::select! {_=cancel.cancelled()=>bail!("전송 취소"),r=client.post(&info_path,&info_body)=>r?};
    if op
        .input
        .as_ref()
        .is_some_and(|a| a.sha256 != input.sha256 || a.bytes != input.bytes || a.path != input.path)
    {
        bail!("원격 파일이 변경되어 이어받을 수 없습니다. 새 다운로드를 추가하세요.");
    }
    let settings = s.store.settings()?;
    let dest = op.destination.clone().unwrap_or_else(|| {
        settings.storage_root.join("downloads").join(format!(
            "{}-{}",
            op.id,
            path.rsplit('/').next().unwrap_or("recording")
        ))
    });
    let parent = dest.parent().context("다운로드 경로 오류")?;
    tokio::fs::create_dir_all(parent).await?;
    ensure_storage(parent, s.store.settings()?.min_free_bytes)?;
    s.store.update_operation(&op.id, |o| {
        o.input = Some(input.clone());
        o.destination = Some(dest.clone());
        o.total_bytes = Some(input.bytes);
        Ok(())
    })?;
    if dest.exists() {
        let p = dest.clone();
        let check = cancel.clone();
        let digest = tokio::task::spawn_blocking(move || {
            file_digest_with_progress(&p, |_| {
                if check.is_cancelled() {
                    bail!("파일 검사가 중단되었습니다.");
                }
                Ok(())
            })
        })
        .await??;
        if digest == (input.bytes, input.sha256.clone()) {
            return Ok(json!({"path":dest,"bytes":input.bytes,"sha256":input.sha256}));
        }
        bail!("다운로드 대상 파일이 이미 존재하며 내용이 다릅니다.");
    }
    let partial = dest.with_extension("partial");
    if std::fs::symlink_metadata(&partial).is_ok_and(|m| !m.is_file()) {
        bail!("다운로드 임시 파일 경로가 올바르지 않습니다.");
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&partial)
        .await?;
    let mut offset = file.metadata().await?.len();
    if offset > input.bytes {
        file.set_len(0).await?;
        offset = 0;
    }
    while offset < input.bytes {
        ensure_storage(parent, s.store.settings()?.min_free_bytes)?;
        let req = files::ChunkRequest {
            path: path.into(),
            offset,
            length: 1024 * 1024,
            modified: input.modified.clone(),
            bytes: input.bytes,
        };
        let chunk = tokio::select! {_=cancel.cancelled()=>bail!("전송 취소"),r=client.file_chunk(&op.request.job_id,&req)=>r?};
        if chunk.is_empty() || chunk.len() as u64 > input.bytes - offset {
            bail!("원격 파일 전송 길이 오류");
        }
        file.write_all(&chunk).await?;
        file.sync_data().await?;
        offset += chunk.len() as u64;
        s.store.update_operation(&op.id, |o| {
            o.bytes_done = offset;
            o.progress = Some(offset as f64 / input.bytes.max(1) as f64);
            o.message = "원격 파일 다운로드 중".into();
            Ok(())
        })?;
    }
    file.sync_all().await?;
    drop(file);
    status(s, &op.id, "다운로드 SHA-256 검증 중", None);
    let p = partial.clone();
    let check = cancel.clone();
    let digest = tokio::task::spawn_blocking(move || {
        file_digest_with_progress(&p, |_| {
            if check.is_cancelled() {
                bail!("전송 취소");
            }
            Ok(())
        })
    })
    .await??;
    if digest != (input.bytes, input.sha256.clone()) {
        tokio::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&partial)
            .await?
            .sync_all()
            .await?;
        s.store.update_operation(&op.id, |o| {
            o.bytes_done = 0;
            o.progress = None;
            Ok(())
        })?;
        bail!("다운로드 해시 불일치. 재시도하면 처음부터 다시 받습니다.");
    }
    if cancel.is_cancelled() {
        bail!("전송 취소");
    }
    std::fs::hard_link(&partial, &dest)?;
    tokio::fs::remove_file(&partial).await?;
    sync_dir(parent)?;
    Ok(json!({"path":dest,"bytes":input.bytes,"sha256":input.sha256}))
}
