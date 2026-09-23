use crate::{Service, scheduler};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use fs2::FileExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    sync::{Arc, atomic::Ordering},
};
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
        .route("/jobs/{id}/schedule", put(schedule))
        .route("/jobs/{id}/bookmarks", post(bookmark_add))
        .route(
            "/jobs/{id}/bookmarks/{bookmark_id}",
            put(bookmark_edit).delete(bookmark_delete),
        )
        .route("/jobs/{id}/storage", get(job_storage))
        .route("/jobs/{id}/cleanup/preview", post(cleanup_preview))
        .route(
            "/jobs/{id}/cleanup",
            post(cleanup_execute).layer(DefaultBodyLimit::max(4 * 1024 * 1024)),
        )
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
    let public_compatibility_path = matches!(req.uri().path(), "/health" | "/shutdown");
    let wrong_api_version = !public_compatibility_path
        && req
            .headers()
            .get("x-ytlr-api-version")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u32>().ok())
            != Some(SERVICE_API_VERSION);
    if wrong_api_version
        || req.headers().contains_key("origin")
        || req
            .headers()
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            != Some(&format!("Bearer {}", state.token))
    {
        return (
            if wrong_api_version {
                StatusCode::UPGRADE_REQUIRED
            } else {
                StatusCode::UNAUTHORIZED
            },
            Json(json!({"error":if wrong_api_version {"서비스 API 버전이 호환되지 않습니다."} else {"인증되지 않은 로컬 요청"}})),
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
    let _permit = s.finalizer.acquire().await?;
    let job = {
        let active = s.active.lock().await;
        if active.contains_key(&id) {
            return Err(anyhow::anyhow!("진행 중인 녹화는 삭제할 수 없습니다.").into());
        }
        let job = s.store.job(&id)?;
        if !job.state.terminal() {
            return Err(anyhow::anyhow!("녹화가 끝난 후 삭제할 수 있습니다.").into());
        }
        let lock = if job.output_dir.is_dir() {
            let lock = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(job.output_dir.join("backup.lock"))?;
            lock.try_lock_exclusive().map_err(|_| {
                anyhow::anyhow!("백업 또는 정리가 진행 중입니다. 완료 후 다시 시도하세요.")
            })?;
            Some(lock)
        } else {
            None
        };
        s.store.delete_job(&id)?;
        (job, lock)
    };
    if let Err(error) = tokio::fs::remove_dir_all(&job.0.output_dir).await
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

#[derive(Deserialize)]
struct ScheduleRequest {
    stop_at: Option<String>,
}

async fn schedule(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    Json(req): Json<ScheduleRequest>,
) -> ApiResult<RecordingJob> {
    let _active = s.active.lock().await;
    let job = s.store.set_schedule(&id, req.stop_at.as_deref())?;
    s.store.event(
        &id,
        "schedule",
        if job.stop_at.is_some() {
            "종료 예약 변경"
        } else {
            "종료 예약 취소"
        },
    )?;
    Ok(Json(job))
}

async fn bookmark_add(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    Json(req): Json<BookmarkRequest>,
) -> ApiResult<RecordingJob> {
    Ok(Json(s.store.add_bookmark(&id, &req)?))
}

async fn bookmark_edit(
    State(s): State<Arc<Service>>,
    Path((id, bookmark_id)): Path<(String, String)>,
    Json(req): Json<BookmarkRequest>,
) -> ApiResult<RecordingJob> {
    let job = s.store.edit_bookmark(&id, &bookmark_id, Some(&req))?;
    s.store.persist_job_manifest(&id)?;
    Ok(Json(job))
}

async fn bookmark_delete(
    State(s): State<Arc<Service>>,
    Path((id, bookmark_id)): Path<(String, String)>,
) -> ApiResult<RecordingJob> {
    let job = s.store.edit_bookmark(&id, &bookmark_id, None)?;
    s.store.persist_job_manifest(&id)?;
    Ok(Json(job))
}

async fn job_storage(State(s): State<Arc<Service>>, Path(id): Path<String>) -> ApiResult<Value> {
    let job = s.store.job(&id)?;
    let sizes =
        tokio::task::spawn_blocking(move || ytlr_engine::cleanup::inventory(&job)).await??;
    Ok(Json(serde_json::to_value(sizes)?))
}

async fn cleanup_preview(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    cleanup_job(s, id, None).await
}

async fn cleanup_execute(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    Json(req): Json<ytlr_engine::cleanup::CleanupRequest>,
) -> ApiResult<Value> {
    cleanup_job(s, id, Some(req)).await
}

async fn cleanup_job(
    s: Arc<Service>,
    id: String,
    req: Option<ytlr_engine::cleanup::CleanupRequest>,
) -> ApiResult<Value> {
    {
        let mut active = s.active.lock().await;
        if active.contains_key(&id) || !s.store.job(&id)?.state.terminal() {
            return Err(anyhow::anyhow!("진행 중인 작업은 정리할 수 없습니다.").into());
        }
        active.insert(id.clone(), s.shutdown.child_token());
    }
    // The operation owns its lifetime: disconnecting a client must not release
    // the maintenance guard while hashing/deletion is still in progress.
    let task = tokio::spawn(async move {
        let result = async {
            let _permit = s.finalizer.acquire().await?;
            let job = s.store.job(&id)?;
            if let Some(req) = req {
                let removed = ytlr_engine::cleanup::execute(&s.tools, &job, &req).await?;
                let root = job.output_dir.clone();
                let recount = tokio::task::spawn_blocking(move || -> anyhow::Result<u64> {
                    Ok(ytlr_engine::files_under(&root)?
                        .iter()
                        .map(|p| std::fs::metadata(p).map(|m| m.len()))
                        .collect::<std::io::Result<Vec<_>>>()?
                        .iter()
                        .sum())
                })
                .await;
                let mut warnings = vec![];
                match recount {
                    Ok(Ok(bytes)) => {
                        if let Err(error) = s
                            .store
                            .update_job(&id, |j| j.bytes = bytes)
                            .and_then(|_| s.store.persist_job_manifest(&id))
                        {
                            warnings.push(format!("정리 후 작업 정보 갱신 실패: {error}"));
                        }
                    }
                    Ok(Err(error)) => warnings.push(format!("정리 후 용량 계산 실패: {error}")),
                    Err(error) => warnings.push(format!("정리 후 용량 작업 실패: {error}")),
                }
                let _ = s.store.event(
                    &id,
                    "cleanup",
                    &format!("선택한 세그먼트 정리 완료 · {removed} bytes · 검증된 결과 보존"),
                );
                for warning in &warnings {
                    let _ = s.store.event(&id, "cleanup_warning", warning);
                }
                Ok::<_, anyhow::Error>(json!({"reclaimed_bytes":removed,"warnings":warnings}))
            } else {
                Ok(serde_json::to_value(
                    ytlr_engine::cleanup::preview(&s.tools, &job).await?,
                )?)
            }
        }
        .await;
        if let Err(e) = &result {
            let _ = s.store.event(&id, "cleanup_error", &e.to_string());
        }
        s.active.lock().await.remove(&id);
        result
    });
    Ok(Json(task.await??))
}
async fn stop(State(s): State<Arc<Service>>, Path(id): Path<String>) -> ApiResult<RecordingJob> {
    let active = s.active.lock().await;
    let running = active.contains_key(&id);
    let job = s.store.update_job(&id, |j| {
        j.stop_requested = true;
        if !running && !j.state.terminal() {
            if j.attempt > 0 {
                j.state = JobState::Finalizing;
                j.message = "중지 요청 · 저장된 파일 정리 대기".into();
            } else {
                j.state = JobState::Stopped;
                j.message = "중지했습니다. 저장된 파일은 보관됩니다.".into();
            }
        }
    })?;
    if let Some(cancel) = active.get(&id) {
        cancel.cancel();
    }
    Ok(Json(job))
}
async fn retry(State(s): State<Arc<Service>>, Path(id): Path<String>) -> ApiResult<RecordingJob> {
    let active = s.active.lock().await;
    if active.contains_key(&id) {
        return Err(anyhow::anyhow!("작업이 실행 중입니다.").into());
    }
    let job = s.store.update_job(&id, |j| {
        j.state = JobState::Queued;
        j.stop_at = None;
        j.recovery_error = None;
        j.stop_requested = false;
        j.retries = 0;
        j.message = "재시도 대기".into();
    })?;
    drop(active);
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
        j.stop_at = None;
        j.state = JobState::Finalizing;
        j.message = "복구 작업 대기".into();
    })?;
    drop(active);
    tokio::spawn(async move {
        if let Err(e) = scheduler::finish(&s, &id, false).await {
            let _ = s.store.update_job(&id, |j| {
                j.state = JobState::Partial;
                j.recovery_error = Some(redact(&e.to_string()));
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
    let extension = if job.recording_options.audio_only {
        "mka"
    } else {
        "mp4"
    };
    let dest = job
        .output_dir
        .join(format!("export-{}.{extension}", uuid::Uuid::new_v4()));
    let result_path = dest.clone();
    tokio::spawn(async move {
        let Ok(_permit) = s.finalizer.acquire().await else {
            return;
        };
        match ytlr_engine::remux(&s.tools, &[output.path], &dest, &job.recording_options).await {
            Ok(_) => {
                let _ = s.store.event(
                    &id,
                    "export",
                    &format!(
                        "{} 내보내기 완료: {}",
                        extension.to_uppercase(),
                        dest.display()
                    ),
                );
            }
            Err(e) => {
                let _ = s.store.event(
                    &id,
                    "export_error",
                    &format!("{} 호환성/내보내기 오류: {e}", extension.to_uppercase()),
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
    Json(body): Json<Value>,
) -> ApiResult<Channel> {
    let mut channel = s
        .store
        .channels()?
        .into_iter()
        .find(|channel| channel.id == id)
        .ok_or_else(|| anyhow::anyhow!("채널을 찾을 수 없습니다."))?;
    if let Some(value) = body.get("url") {
        channel.url = channel_url(serde_json::from_value(value.clone())?)?;
    }
    if let Some(value) = body.get("name") {
        channel.name = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = body.get("enabled") {
        channel.enabled = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = body.get("priority") {
        channel.priority = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = body.get("recording_options") {
        channel.recording_options = serde_json::from_value(value.clone())?;
    }
    if let Some(value) = body.get("live_from_start") {
        channel.live_from_start = serde_json::from_value(value.clone())?;
    }
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
                storage: RwLock::new(vec![]),
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
                    stop_at: None,
                    recording_options: RecordingOptions::default(),
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
                    .header("x-ytlr-api-version", SERVICE_API_VERSION)
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
                api_version: ytlr_core::SERVICE_API_VERSION,
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
