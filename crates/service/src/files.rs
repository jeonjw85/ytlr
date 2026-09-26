use crate::{Service, http::ApiResult};
use anyhow::{Context, Result, bail};
use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use fs2::FileExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use ytlr_core::*;

pub struct PlaybackTicket {
    pub job_id: String,
    pub path: String,
    pub expires: Instant,
}

pub fn media_path(job: &RecordingJob, path: &str) -> Result<PathBuf> {
    relative_media_path(path)?;
    if !job.state.terminal() {
        bail!("녹화 완료 후 파일을 사용할 수 있습니다.");
    }
    let root = &job.output_dir;
    if std::fs::symlink_metadata(root)?.file_type().is_symlink() {
        bail!("녹화 폴더 링크는 사용할 수 없습니다.");
    }
    let candidate = root.join(path);
    let is_output = job.outputs.iter().any(|o| o.path == candidate);
    let derived = !path.contains('/')
        && ["export-", "clip-", "preview-"]
            .iter()
            .any(|p| path.starts_with(p));
    if !is_output && !derived {
        bail!("공유할 수 있는 녹화 결과 파일이 아닙니다.");
    }
    if !matches!(
        candidate.extension().and_then(|s| s.to_str()),
        Some("mkv" | "mp4" | "mka" | "m4a" | "webm" | "opus")
    ) {
        bail!("지원하지 않는 미디어 파일입니다.");
    }
    let mut current = root.clone();
    for part in path.split('/') {
        current.push(part);
        if std::fs::symlink_metadata(&current)?
            .file_type()
            .is_symlink()
        {
            bail!("미디어 파일 링크는 사용할 수 없습니다.");
        }
    }
    if !candidate.canonicalize()?.starts_with(root.canonicalize()?) || !candidate.is_file() {
        bail!("파일이 녹화 폴더 밖에 있습니다.");
    }
    Ok(candidate)
}
fn modified(meta: &std::fs::Metadata) -> Result<String> {
    Ok(meta
        .modified()?
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string())
}
pub fn describe_with_progress(
    job: &RecordingJob,
    path: &str,
    progress: impl FnMut(u64) -> Result<()>,
) -> Result<Artifact> {
    let file = media_path(job, path)?;
    let before = std::fs::metadata(&file)?;
    let hash = file_digest_with_progress(&file, progress)?;
    if job
        .outputs
        .iter()
        .find(|o| o.path == file)
        .and_then(|o| o.sha256.as_deref())
        .is_some_and(|expected| expected != hash.1)
    {
        bail!("검증된 원본 결과 파일의 해시가 변경되었습니다.");
    }
    let verification = file.with_extension("verification.json");
    if let Ok(meta) = std::fs::symlink_metadata(&verification) {
        if !meta.is_file() || meta.len() > 1024 * 1024 {
            bail!("미디어 검증 기록이 올바르지 않습니다.");
        }
        let verified: MediaOutput = serde_json::from_slice(&std::fs::read(verification)?)
            .map_err(|_| anyhow::anyhow!("미디어 검증 기록이 손상되었습니다."))?;
        if verified
            .sha256
            .as_deref()
            .is_some_and(|expected| expected != hash.1)
        {
            bail!("검증된 미디어 파일의 해시가 변경되었습니다.");
        }
    }
    let after = std::fs::metadata(&file)?;
    if before.len() != after.len()
        || hash.0 != after.len()
        || modified(&before)? != modified(&after)?
    {
        bail!("검사 중 원본 파일이 변경되었습니다.");
    }
    Ok(Artifact {
        path: path.into(),
        bytes: hash.0,
        sha256: hash.1,
        modified: modified(&after)?,
    })
}
pub fn file_lock(job: &RecordingJob) -> Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(job.output_dir.join("backup.lock"))?;
    FileExt::try_lock_shared(&file).context("파일 정리 또는 백업이 진행 중입니다.")?;
    Ok(file)
}
#[derive(Deserialize)]
pub struct FileRequest {
    pub path: String,
}
pub async fn catalog(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
) -> ApiResult<Vec<Value>> {
    let job = s.store.job(&id)?;
    let files = tokio::task::spawn_blocking(move || -> Result<Vec<Value>> {
        let mut paths = job
            .outputs
            .iter()
            .filter_map(|o| {
                o.path
                    .strip_prefix(&job.output_dir)
                    .ok()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
            })
            .collect::<Vec<_>>();
        if job.output_dir.is_dir() {
            for entry in std::fs::read_dir(&job.output_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    paths.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
        }
        paths.sort();
        paths.dedup();
        Ok(paths
            .into_iter()
            .filter_map(|path| {
                let p = media_path(&job, &path).ok()?;
                let bytes = std::fs::metadata(p).ok()?.len();
                Some(json!({"preview":path.starts_with("preview-"),"path":path,"bytes":bytes}))
            })
            .collect())
    })
    .await??;
    Ok(Json(files))
}
pub async fn info(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    Json(req): Json<FileRequest>,
) -> ApiResult<Artifact> {
    let job = {
        let mut active = s.active.lock().await;
        if s.shutdown.is_cancelled() || active.contains_key(&id) {
            return Err(anyhow::anyhow!("파일 처리 중입니다. 완료 후 다시 시도하세요.").into());
        }
        let job = s.store.job(&id)?;
        active.insert(id.clone(), s.shutdown.child_token());
        job
    };
    // The hash task retains its reservation even if its HTTP client disconnects.
    let cancel = s.shutdown.child_token();
    let _on_disconnect = cancel.clone().drop_guard();
    let task = tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let _lock = file_lock(&job)?;
            describe_with_progress(&job, &req.path, |_| {
                if cancel.is_cancelled() {
                    bail!("파일 검사가 중단되었습니다.");
                }
                Ok(())
            })
        })
        .await;
        s.active.lock().await.remove(&id);
        result?
    });
    Ok(Json(task.await??))
}
#[derive(Deserialize, serde::Serialize)]
pub struct ChunkRequest {
    pub path: String,
    pub offset: u64,
    pub length: usize,
    pub modified: String,
    pub bytes: u64,
}
pub async fn chunk(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    Json(req): Json<ChunkRequest>,
) -> Result<Response, crate::http::ApiError> {
    if req.length == 0 || req.length > 1024 * 1024 {
        return Err(anyhow::anyhow!("전송 청크 크기 오류").into());
    }
    let job = s.store.job(&id)?;
    let _lock = file_lock(&job)?;
    let path = media_path(&job, &req.path)?;
    s.note_file_activity();
    let mut file = tokio::fs::File::open(path).await?;
    let meta = file.metadata().await?;
    if modified(&meta)? != req.modified || meta.len() != req.bytes {
        return Err(anyhow::anyhow!("원격 원본 파일이 변경되었습니다.").into());
    }
    if req.offset >= meta.len() {
        return Err(anyhow::anyhow!("전송 위치가 파일 범위를 벗어납니다.").into());
    }
    file.seek(std::io::SeekFrom::Start(req.offset)).await?;
    let len = req.length.min((meta.len() - req.offset) as usize);
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).await?;
    Ok((
        [
            ("content-type", "application/octet-stream"),
            ("cache-control", "no-store"),
        ],
        buf,
    )
        .into_response())
}
pub async fn ticket(
    State(s): State<Arc<Service>>,
    Path(id): Path<String>,
    Json(req): Json<FileRequest>,
) -> ApiResult<Value> {
    if !req.path.starts_with("preview-") || !req.path.ends_with(".mp4") {
        return Err(anyhow::anyhow!("미리보기 파일을 먼저 생성하세요.").into());
    }
    media_path(&s.store.job(&id)?, &req.path)?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let mut tickets = s.playback.lock().await;
    tickets.retain(|_, t| t.expires > Instant::now());
    if let Some((token, ticket)) = tickets
        .iter_mut()
        .find(|(_, t)| t.job_id == id && t.path == req.path)
    {
        ticket.expires = Instant::now() + Duration::from_secs(86400);
        return Ok(Json(json!({"ticket":token})));
    }
    if tickets.len() >= 128 {
        return Err(anyhow::anyhow!("미리보기 연결 상한에 도달했습니다.").into());
    }
    tickets.insert(
        token.clone(),
        PlaybackTicket {
            job_id: id,
            path: req.path,
            expires: Instant::now() + Duration::from_secs(86400),
        },
    );
    Ok(Json(json!({"ticket":token})))
}

fn range(value: Option<&str>, size: u64) -> Result<(u64, u64, bool)> {
    if size == 0 {
        bail!("빈 파일입니다.");
    }
    let Some(value) = value else {
        return Ok((0, size - 1, false));
    };
    let (start, end) = value
        .strip_prefix("bytes=")
        .and_then(|v| v.split_once('-'))
        .context("잘못된 Range 헤더")?;
    let (start, end) = if start.is_empty() {
        let suffix: u64 = end.parse()?;
        if suffix == 0 {
            bail!("잘못된 Range 헤더");
        }
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start: u64 = start.parse()?;
        let end = if end.is_empty() {
            size - 1
        } else {
            end.parse::<u64>()?.min(size - 1)
        };
        (start, end)
    };
    if start > end || start >= size {
        bail!("파일 범위를 벗어났습니다.");
    }
    Ok((start, end, true))
}
pub async fn play(
    State(s): State<Arc<Service>>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result=async {
        let (id,path)={ let tickets=s.playback.lock().await; let ticket=tickets.get(&token).filter(|t|t.expires>Instant::now()).context("만료된 미리보기 연결")?; (ticket.job_id.clone(),ticket.path.clone()) };
        let job=s.store.job(&id)?; let lock=file_lock(&job)?; let path=media_path(&job,&path)?;
        let mut file=tokio::fs::File::open(path).await?; let size=file.metadata().await?.len();
        let (start,end,partial)=match range(headers.get("range").and_then(|v|v.to_str().ok()),size) { Ok(r)=>r,Err(_)=>return Ok::<_,anyhow::Error>((StatusCode::RANGE_NOT_SATISFIABLE,[("content-range",format!("bytes */{size}"))]).into_response()) };
        file.seek(std::io::SeekFrom::Start(start)).await?;
        let stream=async_stream::stream! {
            let _lock=lock; let mut remaining=end-start+1;
            while remaining>0 { let mut buf=vec![0u8;remaining.min(64*1024) as usize]; match file.read(&mut buf).await {
                Ok(0)=>{yield Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));break;},
                Ok(n)=>{s.note_file_activity();buf.truncate(n); remaining-=n as u64; yield Ok(axum::body::Bytes::from(buf));},
                Err(e)=>{yield Err(e);break;}
            } }
        };
        let body=Body::from_stream(stream);
        let mut response=Response::builder().status(if partial {StatusCode::PARTIAL_CONTENT}else{StatusCode::OK})
            .header("content-type","video/mp4").header("accept-ranges","bytes").header("cache-control","no-store").header("x-content-type-options","nosniff")
            .header("content-length",end-start+1);
        if partial { response=response.header("content-range",format!("bytes {start}-{end}/{size}")); }
        Ok(response.body(body)?)
    }.await;
    result.unwrap_or_else(|_| {
        (StatusCode::NOT_FOUND, "미리보기 파일을 다시 열어 주세요.").into_response()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn media_ranges_handle_seek_suffix_and_invalid_requests() {
        assert_eq!(range(Some("bytes=2-4"), 10).unwrap(), (2, 4, true));
        assert_eq!(range(Some("bytes=-3"), 10).unwrap(), (7, 9, true));
        assert!(range(Some("bytes=10-"), 10).is_err());
        assert!(range(Some("bytes=0-1,4-5"), 10).is_err());
    }
}
