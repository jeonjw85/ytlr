use crate::{Service, scheduler};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
use ytlr_core::*;

struct ApiError(anyhow::Error);
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":redact(&self.0.to_string())})),
        )
            .into_response()
    }
}
type ApiResult<T> = Result<Json<T>, ApiError>;

pub fn router(state: Arc<Service>) -> Router {
    Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"ok":true,"version":env!("CARGO_PKG_VERSION")})) }),
        )
        .route("/snapshot", get(snapshot))
        .route("/jobs", post(record))
        .route("/jobs/{id}/stop", post(stop))
        .route("/jobs/{id}/retry", post(retry))
        .route("/jobs/{id}/recover", post(recover))
        .route("/jobs/{id}/export", post(export))
        .route("/jobs/{id}/events", get(events))
        .route("/channels", post(channel_add))
        .route("/channels/{id}", put(channel_update).delete(channel_delete))
        .route("/settings", put(settings))
        .route("/tools/install", post(install))
        .route("/tools/rollback", post(rollback))
        .route("/tools/refresh", post(refresh))
        .route("/shutdown", post(shutdown))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), authorize))
        .with_state(state)
}

async fn authorize(State(state): State<Arc<Service>>, req: Request, next: Next) -> Response {
    // No browser CORS: only the native GUI bridge/CLI can call this loopback API.
    if req.headers().contains_key("origin")
        || req
            .headers()
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            != Some(&format!("Bearer {}", state.token))
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"인증되지 않은 로컬 요청"})),
        )
            .into_response();
    }
    next.run(req).await
}

async fn snapshot(State(s): State<Arc<Service>>) -> ApiResult<Snapshot> {
    Ok(Json(s.snapshot().await?))
}
async fn record(
    State(s): State<Arc<Service>>,
    headers: HeaderMap,
    Json(req): Json<RecordRequest>,
) -> ApiResult<RecordingJob> {
    let forwarded = headers.get("x-ytlr-fanout").is_some();
    let job = if forwarded {
        let key = headers
            .get("idempotency-key")
            .and_then(|s| s.to_str().ok())
            .ok_or_else(|| anyhow::anyhow!("이중 녹화 요청 ID가 없습니다."))?;
        s.store.add_replica_job(&req, key)?
    } else {
        s.store.add_job(&req, None, false)?
    };
    if !forwarded {
        s.fanout_job(&job);
    }
    Ok(Json(s.store.job(&job.id)?))
}
async fn events(State(s): State<Arc<Service>>, Path(id): Path<String>) -> ApiResult<Vec<JobEvent>> {
    Ok(Json(s.store.events(&id)?))
}
async fn stop(State(s): State<Arc<Service>>, Path(id): Path<String>) -> ApiResult<RecordingJob> {
    let active = s.active.lock().await;
    let running = active.contains_key(&id);
    let job = s.store.update_job(&id, |j| {
        j.stop_requested = true;
        if !running && !j.state.terminal() {
            j.state = JobState::Stopped;
            j.message = "중지했습니다. 저장된 파일은 보관됩니다.".into();
        }
    })?;
    if let Some(cancel) = active.get(&id) {
        cancel.cancel();
    }
    Ok(Json(job))
}
async fn retry(State(s): State<Arc<Service>>, Path(id): Path<String>) -> ApiResult<RecordingJob> {
    if s.active.lock().await.contains_key(&id) {
        return Err(anyhow::anyhow!("작업이 실행 중입니다.").into());
    }
    let job = s.store.update_job(&id, |j| {
        j.state = JobState::Queued;
        j.stop_requested = false;
        j.retries = 0;
        j.message = "재시도 대기".into();
    })?;
    s.next_checks.lock().await.remove(&id);
    Ok(Json(job))
}
async fn recover(State(s): State<Arc<Service>>, Path(id): Path<String>) -> ApiResult<Value> {
    let mut active = s.active.lock().await;
    if active.contains_key(&id) {
        return Err(anyhow::anyhow!("진행 중인 녹화를 먼저 중지하세요.").into());
    }
    let job = s.store.job(&id)?;
    if job.attempt == 0 {
        return Err(anyhow::anyhow!("복구할 수집 자료가 없습니다.").into());
    }
    active.insert(id.clone(), s.shutdown.child_token());
    s.store.update_job(&id, |j| {
        j.stop_requested = false;
        j.state = JobState::Finalizing;
        j.message = "복구 작업 대기".into();
    })?;
    drop(active);
    tokio::spawn(async move {
        if let Err(e) = scheduler::finish(&s, &id, false).await {
            let _ = s.store.update_job(&id, |j| {
                j.state = JobState::Partial;
                j.message = format!("복구 실패 · 원본 보존: {e}");
            });
        }
        s.active.lock().await.remove(&id);
    });
    Ok(Json(json!({"accepted":true})))
}

#[derive(Deserialize)]
struct ExportRequest {
    #[serde(default)]
    index: usize,
}
async fn export(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    Json(req): Json<ExportRequest>,
) -> ApiResult<Value> {
    let job = s.store.job(&id)?;
    if !job.state.terminal() {
        return Err(anyhow::anyhow!("녹화 마무리 후 내보낼 수 있습니다.").into());
    }
    let output = job
        .outputs
        .get(req.index)
        .ok_or_else(|| anyhow::anyhow!("영상 파일을 찾을 수 없습니다."))?
        .clone();
    // Separate artifact; failed export never replaces the original MKV.
    let dest = job
        .output_dir
        .join(format!("export-{}.mp4", uuid::Uuid::new_v4()));
    let result_path = dest.clone();
    tokio::spawn(async move {
        let Ok(_permit) = s.finalizer.acquire().await else {
            return;
        };
        match ytlr_engine::remux(&s.tools, &[output.path], &dest).await {
            Ok(_) => {
                let _ = s.store.event(
                    &id,
                    "export",
                    &format!("MP4 내보내기 완료: {}", dest.display()),
                );
            }
            Err(e) => {
                let _ = s.store.event(
                    &id,
                    "export_error",
                    &format!("MP4 호환성/내보내기 오류: {e}"),
                );
            }
        }
    });
    Ok(Json(json!({"accepted":true,"path":result_path})))
}

async fn channel_add(
    State(s): State<Arc<Service>>,
    Json(req): Json<AddChannelRequest>,
) -> ApiResult<Channel> {
    Ok(Json(s.store.add_channel(&req)?))
}
async fn channel_update(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    Json(mut channel): Json<Channel>,
) -> ApiResult<Channel> {
    channel.id = id;
    channel.url = channel_url(&channel.url)?;
    s.store.save_channel(&channel)?;
    Ok(Json(channel))
}
async fn channel_delete(State(s): State<Arc<Service>>, Path(id): Path<String>) -> ApiResult<Value> {
    s.store.delete_channel(&id)?;
    Ok(Json(json!({"ok":true})))
}
async fn settings(
    State(s): State<Arc<Service>>,
    Json(mut settings): Json<Settings>,
) -> ApiResult<Settings> {
    settings.validate()?;
    if let Some(target) = &settings.replica_remote
        && !s.paths.remotes()?.iter().any(|r| &r.name == target)
    {
        return Err(anyhow::anyhow!("현재 녹화 장비에 등록되지 않은 이중 녹화 대상입니다.").into());
    }
    ensure_storage(&settings.storage_root, 0)?;
    if let Some(backup) = &settings.backup_root {
        settings.backup_device = backup_device(backup)?;
        settings.backup_identity = Some(identify_backup(backup)?);
        for job in s.store.jobs()? {
            if overlapping_storage(&job.output_dir, backup) {
                return Err(anyhow::anyhow!("백업 폴더가 기존 작업 폴더와 겹칩니다.").into());
            }
        }
    } else {
        settings.backup_device = None;
        settings.backup_identity = None;
    }
    s.store.save_settings(&settings)?;
    Ok(Json(settings))
}
async fn install(State(s): State<Arc<Service>>) -> ApiResult<Value> {
    let active = s.active.lock().await;
    if !active.is_empty() {
        return Err(anyhow::anyhow!("진행 중인 작업이 끝난 후 엔진을 설치할 수 있습니다.").into());
    }
    if s.installing.swap(true, Ordering::SeqCst) {
        return Err(anyhow::anyhow!("이미 설치 중입니다.").into());
    }
    drop(active);
    *s.tool_message.write().await = Some("검증된 엔진 번들 설치 준비 중".into());
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let tools = s.tools.clone();
        let installation = tools.install(move |message| {
            let _ = tx.send(message);
        });
        tokio::pin!(installation);
        let result = loop {
            tokio::select! {
                result = &mut installation => break result,
                Some(message) = rx.recv() => { *s.tool_message.write().await = Some(message); }
            }
        };
        *s.tool_message.write().await = Some(match result {
            Ok(()) => "녹화 엔진 설치 완료".into(),
            Err(e) => format!("설치 실패: {e}"),
        });
        s.refresh_tools().await;
        s.installing.store(false, Ordering::SeqCst);
    });
    Ok(Json(json!({"accepted":true})))
}
async fn rollback(State(s): State<Arc<Service>>) -> ApiResult<Value> {
    let active = s.active.lock().await;
    if !active.is_empty() || s.installing.load(Ordering::SeqCst) {
        return Err(anyhow::anyhow!("작업 또는 설치가 진행 중입니다.").into());
    }
    s.tools.rollback()?;
    s.refresh_tools().await;
    Ok(Json(json!({"ok":true})))
}
async fn refresh(State(s): State<Arc<Service>>) -> ApiResult<Value> {
    s.refresh_tools().await;
    Ok(Json(json!({"ok":true})))
}
async fn shutdown(State(s): State<Arc<Service>>) -> ApiResult<Value> {
    s.shutdown.cancel();
    Ok(Json(json!({"ok":true})))
}
