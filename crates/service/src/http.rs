use crate::{Service, scheduler};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
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
        .route("/jobs/{id}", delete(delete_job))
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
        .method_not_allowed_fallback(|| async {
            (
                StatusCode::METHOD_NOT_ALLOWED,
                Json(json!({"error":"지원하지 않는 요청"})),
            )
        })
        .fallback(|| async {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error":"요청 처리 실패"})),
            )
        })
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
async fn delete_job(State(s): State<Arc<Service>>, Path(id): Path<String>) -> ApiResult<Value> {
    let job = {
        let active = s.active.lock().await;
        if active.contains_key(&id) {
            return Err(anyhow::anyhow!("진행 중인 녹화는 삭제할 수 없습니다.").into());
        }
        let job = s.store.job(&id)?;
        if !job.state.terminal() {
            return Err(anyhow::anyhow!("녹화가 끝난 후 삭제할 수 있습니다.").into());
        }
        s.store.delete_job(&id)?;
        job
    };
    if let Err(error) = tokio::fs::remove_dir_all(&job.output_dir).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(
            anyhow::anyhow!("작업은 삭제했지만 녹화 파일 정리에 실패했습니다: {error}").into(),
        );
    }
    Ok(Json(json!({"deleted":true})))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Client;
    use std::{collections::HashMap, sync::atomic::AtomicBool};
    use tokio::sync::{Mutex, RwLock, Semaphore};
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;
    use ytlr_engine::Tools;

    fn service() -> (Arc<Service>, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(d.path().to_owned())).unwrap();
        paths.initialize().unwrap();
        let store = Arc::new(Store::open(&paths.database(), &paths.default_settings()).unwrap());
        (
            Arc::new(Service {
                tools: Tools::new(paths.clone()),
                store,
                paths,
                token: "test-token".into(),
                shutdown: CancellationToken::new(),
                active: Mutex::new(HashMap::new()),
                next_checks: Mutex::new(HashMap::new()),
                finalizer: Semaphore::new(1),
                gap_recovery: Semaphore::new(1),
                tool_status: RwLock::new(vec![]),
                installing: AtomicBool::new(false),
                tool_message: RwLock::new(None),
            }),
            d,
        )
    }

    fn completed_job(state: &Service) -> RecordingJob {
        let job = state
            .store
            .add_job(
                &RecordRequest {
                    url: "https://youtu.be/abcdefghijk".into(),
                    live_from_start: None,
                    priority: 0,
                },
                None,
                false,
            )
            .unwrap();
        std::fs::create_dir_all(&job.output_dir).unwrap();
        std::fs::write(job.output_dir.join("clip.mkv"), b"data").unwrap();
        state
            .store
            .update_job(&job.id, |j| j.state = JobState::Completed)
            .unwrap()
    }

    async fn send(
        state: Arc<Service>,
        method: &str,
        uri: &str,
        body: &'static str,
    ) -> (StatusCode, String) {
        let response = router(state)
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn delete_job_removes_record_and_files() {
        let (state, _d) = service();
        let job = completed_job(&state);
        let output = job.output_dir.clone();
        let (status, body) =
            send(state.clone(), "DELETE", &format!("/jobs/{}", job.id), "{}").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("deleted"));
        assert!(state.store.job(&job.id).is_err());
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn unknown_route_returns_json() {
        let (state, _d) = service();
        let (status, body) = send(state, "DELETE", "/jobs/missing/extra", "{}").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("error"));
    }

    #[tokio::test]
    async fn client_delete_with_json_body_succeeds() {
        let (state, d) = service();
        let job = completed_job(&state);
        let output = job.output_dir.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router(state)).await.unwrap();
        });
        let client = Client::new(AppPaths::resolve(Some(d.path().to_owned())).unwrap())
            .unwrap()
            .with_endpoint(ServiceEndpoint {
                port,
                token: "test-token".into(),
                pid: 0,
                version: env!("CARGO_PKG_VERSION").into(),
            });
        let value: Value = client
            .request(
                reqwest::Method::DELETE,
                &format!("/jobs/{}", job.id),
                Some(&json!({})),
            )
            .await
            .unwrap();
        assert_eq!(value["deleted"], true);
        assert!(!output.exists());
    }
}
