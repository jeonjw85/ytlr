use crate::{
    Tools, checkpoint,
    process::{self, WorkerSpec},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;
use ytlr_core::{
    JobState, RecordingJob, Settings, Store, atomic_write, note_media_received, now, private_dir,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoInfo {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub live_status: String,
    pub format: String,
    pub expected_tracks: usize,
    pub sources: Vec<StreamSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamSource {
    pub url: String,
    pub headers: HashMap<String, String>,
    pub video: bool,
    pub audio: bool,
}

#[derive(Debug)]
pub struct CaptureResult {
    pub success: bool,
    pub interrupted: bool,
    pub reason: String,
    pub media_seconds: f64,
    pub bytes: u64,
    pub started_at: String,
    pub ended_at: String,
}

fn base_args(
    tools: &Tools,
    cookies: Option<&Path>,
    po_token: Option<&Path>,
) -> Result<CommandArgs> {
    let deno = tools.require("deno")?;
    let ffmpeg = tools.require("ffmpeg")?;
    let mut args = vec![
        "--ignore-config".into(),
        "--no-cache-dir".into(),
        "--no-update".into(),
        "--no-colors".into(),
        "--js-runtimes".into(),
        format!("deno:{}", deno.display()),
        "--ffmpeg-location".into(),
        ffmpeg.to_string_lossy().into_owned(),
        "--socket-timeout".into(),
        "20".into(),
        "--retries".into(),
        "3".into(),
        "--extractor-retries".into(),
        "2".into(),
    ];
    let auth = ytlr_core::ExtractorConfig::create(&tools.paths.root, cookies, po_token)?;
    if let Some(conf) = &auth.path {
        args.extend([
            "--config-locations".into(),
            conf.to_string_lossy().into_owned(),
        ]);
    }
    Ok(CommandArgs { args, auth })
}

pub struct CommandArgs {
    pub args: Vec<String>,
    auth: ytlr_core::ExtractorConfig,
}

pub async fn inspect(
    tools: &Tools,
    url: &str,
    cookies: Option<&Path>,
    po_token: Option<&Path>,
) -> Result<VideoInfo> {
    let mut cmd = Command::new(tools.require("yt-dlp")?);
    let arguments = base_args(tools, cookies, po_token)?;
    cmd.args(&arguments.args)
        .args([
            "--no-playlist",
            "--skip-download",
            "--dump-single-json",
            "--ignore-no-formats-error",
            "--",
            url,
        ])
        .kill_on_drop(true);
    process::hide_console(&mut cmd);
    let output = process::output_timeout(cmd, Duration::from_secs(90))
        .await
        .context("방송 정보 확인 실패")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        if err.contains("live event will begin") || err.contains("Premiere will begin") {
            return Ok(VideoInfo {
                id: String::new(),
                title: "예약 방송".into(),
                channel: String::new(),
                live_status: "is_upcoming".into(),
                format: String::new(),
                expected_tracks: 0,
                sources: vec![],
            });
        }
        bail!("방송 정보 확인 실패: {}", arguments.auth.scrub(&err));
    }
    let v: Value = serde_json::from_slice(&output.stdout)?;
    let formats = v["requested_formats"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![v.clone()]);
    let sources = formats
        .iter()
        .filter_map(|f| {
            Some(StreamSource {
                url: f["url"].as_str()?.into(),
                headers: serde_json::from_value(
                    f.get("http_headers")
                        .or_else(|| v.get("http_headers"))
                        .cloned()
                        .unwrap_or(serde_json::json!({})),
                )
                .unwrap_or_default(),
                // YouTube HLS can advertise an audio rendition without identifying its codec.
                // Only the explicit string "none" means the track is absent in yt-dlp metadata.
                video: f["vcodec"].as_str() != Some("none"),
                audio: f["acodec"].as_str() != Some("none"),
            })
        })
        .collect();
    Ok(VideoInfo {
        id: v["id"].as_str().unwrap_or_default().into(),
        title: v["title"].as_str().unwrap_or("라이브 방송").into(),
        channel: v["channel"].as_str().unwrap_or_default().into(),
        live_status: v["live_status"].as_str().unwrap_or("not_live").into(),
        format: v["format"].as_str().unwrap_or_default().into(),
        expected_tracks: v["requested_formats"].as_array().map_or(1, Vec::len),
        sources,
    })
}

pub async fn scan_channel(
    tools: &Tools,
    url: &str,
    cookies: Option<&Path>,
    po_token: Option<&Path>,
) -> Result<Vec<String>> {
    let mut cmd = Command::new(tools.require("yt-dlp")?);
    let arguments = base_args(tools, cookies, po_token)?;
    cmd.args(&arguments.args)
        .args([
            "--flat-playlist",
            "--playlist-end",
            "30",
            "--dump-single-json",
            "--",
            url,
        ])
        .kill_on_drop(true);
    process::hide_console(&mut cmd);
    let result = process::output_timeout(cmd, Duration::from_secs(90)).await?;
    if !result.status.success() {
        bail!(
            "채널 확인 실패: {}",
            arguments
                .auth
                .scrub(&String::from_utf8_lossy(&result.stderr))
        );
    }
    let data: Value = serde_json::from_slice(&result.stdout)?;
    Ok(data["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| matches!(v["live_status"].as_str(), Some("is_live" | "is_upcoming")))
        .filter_map(|v| v["id"].as_str())
        .filter(|id| id.len() == 11)
        .map(|id| format!("https://www.youtube.com/watch?v={id}"))
        .collect())
}

pub fn capture_args(
    tools: &Tools,
    job: &RecordingJob,
    attempt: &Path,
    cookies: Option<&Path>,
    po_token: Option<&Path>,
) -> Result<CommandArgs> {
    let mut command = base_args(tools, cookies, po_token)?;
    let args = &mut command.args;
    args.extend([
        "--no-playlist", "--no-simulate", "--newline", "--progress", "--progress-delta", "1",
        "--keep-fragments", "--keep-video", "--no-overwrites", "--no-post-overwrites",
        "--fragment-retries", "5", "--retry-sleep", "fragment:exp=1:10", "--concurrent-fragments", "2",
        "--skip-unavailable-fragments", "--merge-output-format", "mkv", "--remux-video", "mkv",
        "--format", "bv*+ba/b", "--fixup", "never",
        "--progress-template", "download:__YTLR_PROGRESS__{\"bytes\":%(progress.downloaded_bytes)j,\"track\":%(info.format_id)j,\"status\":%(progress.status)j}",
        "--print", "before_dl:__YTLR_INFO__%(.{id,title,channel,format,resolution,format_id})j",
    ].into_iter().map(String::from));
    // New attempts have distinct filenames. Unknown boundaries are never appended blindly.
    args.push(
        if job.live_from_start && job.attempt == 1 {
            "--live-from-start"
        } else {
            "--no-live-from-start"
        }
        .into(),
    );
    args.extend([
        "--output".into(),
        attempt
            .join("source.%(ext)s")
            .to_string_lossy()
            .into_owned(),
        "--".into(),
        job.url.clone(),
    ]);
    Ok(command)
}

#[derive(Debug, Default)]
pub struct ProgressTracker {
    tracks: HashMap<String, (u64, Instant)>,
    started: Option<Instant>,
}

impl ProgressTracker {
    pub fn observe(&mut self, track: &str, bytes: u64, at: Instant) -> bool {
        self.started.get_or_insert(at);
        let old = self.tracks.entry(track.into()).or_insert((0, at));
        if bytes > old.0 {
            *old = (bytes, at);
            true
        } else {
            false
        }
    }
    pub fn stalled(&self, at: Instant, started: Instant, timeout: Duration) -> bool {
        if self.tracks.is_empty() {
            return at.duration_since(started) > timeout;
        }
        self.tracks
            .values()
            .any(|(_, last)| at.duration_since(*last) > timeout)
    }
}

pub fn ffmpeg_capture_args(info: &VideoInfo, attempt: &Path) -> Result<Vec<String>> {
    if info.sources.is_empty() {
        bail!("접근 가능한 스트림 URL이 없습니다.");
    }
    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-loglevel",
        "warning",
        "-n",
        "-progress",
        "pipe:1",
        "-stats_period",
        "1",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    for source in &info.sources {
        args.extend(["-rw_timeout".into(), "20000000".into()]);
        let headers = source
            .headers
            .iter()
            .filter(|(k, v)| !k.contains(['\r', '\n']) && !v.contains(['\r', '\n']))
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect::<String>();
        if !headers.is_empty() {
            args.extend(["-headers".into(), headers]);
        }
        args.extend(["-i".into(), source.url.clone()]);
    }
    let video = info
        .sources
        .iter()
        .position(|s| s.video)
        .context("영상 트랙을 찾을 수 없습니다.")?;
    let audio = info
        .sources
        .iter()
        .position(|s| s.audio)
        .context("음성 트랙을 찾을 수 없습니다.")?;
    args.extend([
        "-map".into(),
        format!("{video}:v:0"),
        "-map".into(),
        format!("{audio}:a:0"),
    ]);
    args.extend(
        [
            "-c",
            "copy",
            "-f",
            "segment",
            "-segment_format",
            "matroska",
            "-segment_time",
            "30",
            "-reset_timestamps",
            "1",
            "-flush_packets",
            "1",
            "-segment_list_type",
            "csv",
            "-segment_list",
        ]
        .into_iter()
        .map(String::from),
    );
    args.push(attempt.join("segments.csv").to_string_lossy().into_owned());
    args.push(attempt.join("part-%06d.mkv").to_string_lossy().into_owned());
    Ok(args)
}

pub async fn capture(
    tools: Tools,
    store: Arc<Store>,
    job: RecordingJob,
    info: VideoInfo,
    settings: Settings,
    cancel: CancellationToken,
) -> Result<CaptureResult> {
    let attempt = job.output_dir.join(format!("attempt-{:04}", job.attempt));
    tokio::fs::create_dir_all(&attempt).await?;
    private_dir(&job.output_dir)?;
    private_dir(&attempt)?;
    atomic_write(
        &attempt.join("session.json"),
        &serde_json::to_vec_pretty(
            &serde_json::json!({"video_id":job.video_id,"attempt":job.attempt,"started_at":now(),"from_start":job.live_from_start && job.attempt == 1}),
        )?,
    )?;
    let started_at = now();
    let ffmpeg_mode = !(job.live_from_start && job.attempt == 1);
    let arguments = capture_args(
        &tools,
        &job,
        &attempt,
        settings.cookies_path.as_deref(),
        settings.po_token_path.as_deref(),
    )?;
    let spec = if ffmpeg_mode {
        WorkerSpec {
            executable: tools.require("ffmpeg")?,
            args: ffmpeg_capture_args(&info, &attempt)?,
            cwd: attempt.clone(),
        }
    } else {
        WorkerSpec {
            executable: tools.require("yt-dlp")?,
            args: arguments.args.clone(),
            cwd: attempt.clone(),
        }
    };
    // Signed URLs go through a private supervisor file and are excluded from exported logs.
    let runtime = tools.paths.root.join("workers");
    tokio::fs::create_dir_all(&runtime).await?;
    private_dir(&runtime)?;
    let spec_path = runtime.join(format!("{}.json", uuid::Uuid::new_v4()));
    atomic_write(&spec_path, &serde_json::to_vec(&spec)?)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("internal-worker")
        .arg(&spec_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    process::hide_console(&mut command);
    let mut worker = command.spawn().context("녹화 작업 프로세스 시작 실패")?;
    let mut lifetime_pipe = worker.stdin.take();
    let stdout = worker.stdout.take().context("수집 출력 파이프 오류")?;
    let stderr = worker.stderr.take().context("수집 오류 파이프 오류")?;
    let (tx, mut rx) = mpsc::channel::<String>(256);
    let out_tx = tx.clone();
    let out_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if out_tx.send(line).await.is_err() {
                break;
            }
        }
    });
    let err_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).await.is_err() {
                break;
            }
        }
    });
    let mut tick = tokio::time::interval(Duration::from_secs(3));
    let started = Instant::now();
    let mut tracker = ProgressTracker::default();
    let mut reason = String::new();
    let mut interrupted = false;
    let mut known = HashSet::<PathBuf>::new();
    let mut stop_at: Option<Instant> = None;
    let mut finalizing = false;
    let mut checked_segments = HashSet::new();
    let previous_bytes = job.bytes;
    let mut media_seconds = 0.0;
    loop {
        tokio::select! {
            _ = cancel.cancelled(), if !interrupted => {
                interrupted = true;
                reason = "사용자 중지 또는 서비스 종료".into();
                lifetime_pipe.take();
                stop_at = Some(Instant::now());
            }
            line = rx.recv() => {
                let Some(line) = line else { break; };
                if ffmpeg_mode && line.starts_with("out_time_us=") {
                    let time = line.trim_start_matches("out_time_us=").parse::<u64>().unwrap_or(0);
                    if tracker.observe("muxed", time, Instant::now()) {
                        media_seconds = time as f64 / 1_000_000.0;
                        store.update_job(&job.id, |j| { let at = now(); note_media_received(j, job.attempt, &at); j.last_media_at = Some(at); j.media_seconds = media_seconds; j.state = JobState::Recording; j.message = "녹화 중 · 30초 MKV 분할 보존".into(); })?;
                    }
                } else if let Some((_, raw)) = line.split_once("__YTLR_PROGRESS__") {
                    if let Ok(value) = serde_json::from_str::<Value>(raw) {
                        let bytes = value["bytes"].as_u64().unwrap_or(0);
                        let track = value["track"].as_str().unwrap_or("muxed");
                        if tracker.observe(track, bytes, Instant::now()) {
                            store.update_job(&job.id, |j| { let at = now(); note_media_received(j, job.attempt, &at); j.last_media_at = Some(at); j.state = JobState::Recording; j.message = "녹화 중 · 원본 보존".into(); })?;
                        }
                    }
                } else if let Some((_, raw)) = line.split_once("__YTLR_INFO__") {
                    if let Ok(value) = serde_json::from_str::<Value>(raw) {
                        store.update_job(&job.id, |j| {
                            if let Some(title) = value["title"].as_str() { j.title = title.into(); }
                            if let Some(channel) = value["channel"].as_str() { j.channel = channel.into(); }
                            j.format = value["format"].as_str().unwrap_or("최고 화질").into();
                        })?;
                    }
                } else {
                    let lower = line.to_ascii_lowercase();
                    if lower.contains("skipping") || lower.contains("fragment not found") || lower.contains("non-monoton") || lower.contains("failed to open segment") {
                        store.update_job(&job.id, |j| { j.continuity_uncertain = true; })?;
                        store.event(&job.id, "gap", &arguments.auth.scrub(&line))?;
                    } else if lower.contains("error") || lower.contains("warning") {
                        store.event(&job.id, "engine", &arguments.auth.scrub(&line))?;
                        if !interrupted { reason = arguments.auth.scrub(&line); }
                    }
                    if line.starts_with("[Merger]") || line.starts_with("[VideoRemuxer]") {
                        finalizing = true;
                        store.update_job(&job.id, |j| { j.state = JobState::Finalizing; j.message = "병합 중 · 원본 유지".into(); })?;
                    }
                }
            }
            _ = tick.tick() => {
                let root = attempt.clone();
                let mut checkpoint_known = std::mem::take(&mut known);
                let result = tokio::task::spawn_blocking(move || { let result = checkpoint(&root, &mut checkpoint_known); (result, checkpoint_known) }).await?;
                known = result.1;
                match result.0 {
                    Ok(bytes) => {
                        store.update_job(&job.id, |j| { j.bytes = previous_bytes.saturating_add(bytes); })?;
                    },
                    Err(e) => {
                        reason = format!("디스크 기록 확정 실패: {e}");
                        let _ = store.event(&job.id, "storage_error", &reason);
                        if !interrupted { interrupted = true; lifetime_pipe.take(); stop_at = Some(Instant::now()); }
                    }
                }
                if ffmpeg_mode && !interrupted {
                    for segment in crate::closed_segments(&attempt)? {
                        if checked_segments.contains(&segment) { continue; }
                        if !crate::segment_has_both_tracks(&tools, &segment).await? {
                            store.update_job(&job.id, |j| j.continuity_uncertain = true)?;
                            reason = "분할 파일에서 영상 또는 음성 패킷 누락 감지".into();
                            store.event(&job.id, "gap", &reason)?;
                            interrupted = true;
                            lifetime_pipe.take(); stop_at = Some(Instant::now());
                            break;
                        }
                        checked_segments.insert(segment);
                    }
                }
                let disk_low = fs2_free(&attempt).is_some_and(|n| n < settings.min_free_bytes);
                let missing_track = !ffmpeg_mode && info.expected_tracks > tracker.tracks.len() && started.elapsed().as_secs() > settings.stall_timeout_secs;
                if !interrupted && (disk_low || (!finalizing && (missing_track || tracker.stalled(Instant::now(), started, Duration::from_secs(settings.stall_timeout_secs))))) {
                    reason = if disk_low { "저장 공간 부족" } else { "영상 또는 음성 수신 정지 감지" }.into();
                    interrupted = true;
                    lifetime_pipe.take();
                    stop_at = Some(Instant::now());
                    store.event(&job.id, "interrupted", &reason)?;
                }
                if stop_at.is_some_and(|at| at.elapsed() > Duration::from_secs(30)) { let _ = worker.kill().await; break; }
            }
        }
    }
    drop(lifetime_pipe);
    let status = worker.wait().await?;
    let _ = tokio::fs::remove_file(spec_path).await;
    let _ = out_task.await;
    let _ = err_task.await;
    let root = attempt.clone();
    let bytes = tokio::task::spawn_blocking(move || checkpoint(&root, &mut known)).await??;
    store.update_job(&job.id, |j| {
        j.bytes = previous_bytes.saturating_add(bytes);
    })?;
    atomic_write(
        &attempt.join("result.json"),
        &serde_json::to_vec_pretty(
            &serde_json::json!({"exit_code":status.code(),"interrupted":interrupted,"reason":reason,"finished_at":now()}),
        )?,
    )?;
    let ended_at = now();
    let snapshot = store.job(&job.id)?;
    Ok(CaptureResult {
        success: status.success(),
        interrupted,
        reason,
        media_seconds,
        bytes: snapshot.bytes.saturating_sub(previous_bytes),
        started_at,
        ended_at,
    })
}

fn fs2_free(path: &Path) -> Option<u64> {
    ytlr_core::available_space(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_track_cannot_hide_another_tracks_stall() {
        let start = Instant::now();
        let mut p = ProgressTracker::default();
        p.observe("video", 10, start);
        p.observe("audio", 10, start);
        p.observe("audio", 20, start + Duration::from_secs(40));
        assert!(p.stalled(
            start + Duration::from_secs(41),
            start,
            Duration::from_secs(30)
        ));
        assert!(!p.observe("video", 10, start + Duration::from_secs(41)));
    }
}
