use crate::{Tools, probe, process};
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::{Child, ChildStdin},
};
use tokio_util::sync::CancellationToken;
use ytlr_core::*;

pub struct RenderRequest<'a> {
    pub supervisor: Option<&'a Path>,
    pub min_free_bytes: u64,
    pub source: &'a Path,
    pub destination: &'a Path,
    pub options: &'a RecordingOptions,
    pub range: Option<(f64, f64)>,
    pub preview: bool,
}

async fn stop_child(
    child: &mut Child,
    lifetime_pipe: &mut Option<ChildStdin>,
    supervised: bool,
) -> Result<()> {
    lifetime_pipe.take();
    if !supervised {
        child.start_kill()?;
    }
    // The supervisor handles graceful interruption and escalation for its entire
    // child group. Killing it first would orphan FFmpeg. Keep the caller's file
    // reservation until termination is actually observed.
    child
        .wait()
        .await
        .context("미디어 프로세스 종료 확인 실패")?;
    Ok(())
}

pub async fn render(
    tools: &Tools,
    request: RenderRequest<'_>,
    cancel: CancellationToken,
    mut progress: impl FnMut(u64, f64) + Send,
) -> Result<MediaOutput> {
    let RenderRequest {
        supervisor,
        min_free_bytes,
        source,
        destination,
        options,
        range,
        preview,
    } = request;
    if cancel.is_cancelled() {
        bail!("작업이 중단되었습니다.");
    }
    if destination.exists() {
        bail!("출력 파일이 이미 존재합니다.");
    }
    let input = probe(tools, source).await?;
    if let Some((start, end)) = range
        && (!start.is_finite()
            || !end.is_finite()
            || start < 0.0
            || end <= start
            || end > input.duration + 0.5)
    {
        bail!("클립 구간이 파일 범위를 벗어납니다.");
    }
    let parent = destination.parent().context("출력 폴더 오류")?;
    let temp = parent.join(format!(
        ".{}.partial",
        destination
            .file_name()
            .context("출력 파일명 오류")?
            .to_string_lossy()
    ));
    let worker_dir = parent.join(".render-worker");
    let spec_path = tools
        .paths
        .root
        .join("workers")
        .join(format!("render-{}.json", uuid::Uuid::new_v4()));
    // False while another supervisor may own the deterministic temporary path.
    let mut cleanup_temp = false;
    let result = async {
        if supervisor.is_some() {
            std::fs::create_dir_all(&worker_dir)?;
            let directory = std::fs::symlink_metadata(&worker_dir)?;
            if !directory.is_dir() { bail!("미디어 작업 폴더가 올바르지 않습니다."); }
            let lock = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(parent.join("capture.lock"))?;
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    if cancel.is_cancelled() { bail!("작업이 중단되었습니다."); }
                    match lock.try_lock_exclusive() {
                        Ok(()) => break,
                        Err(e) if e.raw_os_error() == fs2::lock_contended_error().raw_os_error() => tokio::time::sleep(Duration::from_millis(100)).await,
                        Err(e) => return Err(e.into()),
                    }
                }
                Ok::<_, anyhow::Error>(())
            }).await.context("이전 미디어 작업 종료 대기 시간 초과")??;
        }
        match std::fs::symlink_metadata(&temp) {
            Ok(metadata) if metadata.is_file() => std::fs::remove_file(&temp)?,
            Ok(_) => bail!("임시 미디어 파일 경로 오류"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.into()),
        }
        cleanup_temp = true;
        let mut cmd = tokio::process::Command::new(tools.require("ffmpeg")?);
        cmd.args(["-hide_banner", "-nostdin", "-v", "error", "-n", "-progress", "pipe:1", "-nostats"]);
        if let Some((start, _)) = range { cmd.arg("-ss").arg(start.to_string()); }
        cmd.arg("-i").arg(source);
        if let Some((start, end)) = range { cmd.arg("-t").arg((end - start).to_string()); }
        if !options.audio_only { cmd.args(["-map", "0:v:0"]); }
        cmd.args(["-map", "0:a:0"]);
        if preview {
            if !options.audio_only {
                let mut enc = tokio::process::Command::new(tools.require("ffmpeg")?);
                enc.args(["-hide_banner", "-encoders"]).kill_on_drop(true);
                process::hide_console(&mut enc);
                let output = tokio::time::timeout(Duration::from_secs(15), enc.output()).await??;
                let listing = String::from_utf8_lossy(&output.stdout);
                if listing.lines().any(|line| line.split_whitespace().nth(1) == Some("libx264")) {
                    cmd.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "25"]);
                } else if listing.lines().any(|line| line.split_whitespace().nth(1) == Some("h264_videotoolbox")) {
                    cmd.args(["-c:v", "h264_videotoolbox", "-allow_sw", "1", "-b:v", "2000k"]);
                } else { bail!("미리보기용 H.264 인코더가 없습니다. 검증된 엔진을 설치하세요."); }
                cmd.args(["-vf", "scale=w='min(1280,iw)':h='min(720,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2", "-pix_fmt", "yuv420p"]);
            }
            cmd.args(["-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart", "-f", "mp4"]);
        } else {
            cmd.args(["-c", "copy", "-f", if options.audio_only { "matroska" } else { "mp4" }]);
        }
        cmd.arg(&temp).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        if let Some(supervisor) = supervisor {
            let spec = process::WorkerSpec {
                executable: tools.require("ffmpeg")?,
                args: cmd.as_std().get_args().map(|s| s.to_string_lossy().into_owned()).collect(),
                cwd: worker_dir.clone(),
            };
            atomic_write(&spec_path, &serde_json::to_vec(&spec)?)?;
            let mut guarded = tokio::process::Command::new(supervisor);
            guarded.arg("internal-worker").arg(&spec_path).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(false);
            cmd = guarded;
        }
        process::hide_console(&mut cmd);
        let mut child = cmd.spawn()?;
        cleanup_temp = false;
        let mut lifetime_pipe = child.stdin.take();
        let mut lines = BufReader::new(child.stdout.take().context("FFmpeg 진행률 연결 오류")?).lines();
        let stderr = child.stderr.take().context("FFmpeg 오류 연결 실패")?;
        let errors = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut output = Vec::new();
            let mut buf = [0u8; 1024];
            while let Ok(n) = reader.read(&mut buf).await {
                if n == 0 { break; }
                if output.len() < 4096 { output.extend_from_slice(&buf[..n.min(4096 - output.len())]); }
            }
            redact(&String::from_utf8_lossy(&output))
        });
        let mut bytes = 0;
        let mut seconds = 0.0;
        let outcome = tokio::select! {
            _ = cancel.cancelled() => Err(anyhow::anyhow!("작업이 중단되었습니다.")),
            result = async {
                while let Some(line) = lines.next_line().await? {
                    if let Some(v) = line.strip_prefix("out_time_us=") { seconds = v.parse::<f64>().unwrap_or(0.0) / 1_000_000.0; }
                    if let Some(v) = line.strip_prefix("total_size=") { bytes = v.parse::<u64>().unwrap_or(bytes); }
                    if line.starts_with("progress=") {
                        if available_space(parent)? < min_free_bytes { bail!("저장 공간 부족으로 미디어 작업을 중단했습니다."); }
                        progress(bytes, seconds);
                    }
                }
                Ok::<_, anyhow::Error>(child.wait().await?)
            } => result,
        };
        if outcome.is_err()
            && let Err(error) = stop_child(&mut child, &mut lifetime_pipe, supervisor.is_some()).await
        {
            errors.abort();
            return Err(error);
        }
        cleanup_temp = true;
        let stderr = errors.await.unwrap_or_default();
        let status = outcome?;
        if !status.success() { bail!("미디어 처리 실패: {stderr}"); }
        if cancel.is_cancelled() { bail!("작업이 중단되었습니다."); }
        let mut media = probe(tools, &temp).await?;
        if !options.accepts(&media) || !media.duration.is_finite() || media.duration <= 0.0 { bail!("결과 파일 검사 실패"); }
        let path = temp.clone();
        progress(media.bytes, -1.0);
        let check = cancel.clone();
        let digest = tokio::task::spawn_blocking(move || file_digest_with_progress(&path, |_| {
            if check.is_cancelled() { bail!("작업이 중단되었습니다."); }
            Ok(())
        })).await??;
        if digest.0 != media.bytes { bail!("검증 중 결과 파일의 크기가 변경되었습니다."); }
        media.sha256 = Some(digest.1);
        if cancel.is_cancelled() { bail!("작업이 중단되었습니다."); }
        std::fs::OpenOptions::new().read(true).write(true).open(&temp)?.sync_all()?;
        std::fs::hard_link(&temp, destination)?;
        media.path = destination.into();
        sync_dir(parent)?;
        atomic_write(&destination.with_extension("verification.json"), &serde_json::to_vec_pretty(&media)?)?;
        Ok(media)
    }.await;
    if cleanup_temp {
        let _ = std::fs::remove_file(&temp);
        let _ = std::fs::remove_dir(&worker_dir);
    }
    let _ = std::fs::remove_file(&spec_path);
    result
}
