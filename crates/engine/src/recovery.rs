//! Bounded recovery from a frozen, explicitly dated HLS media playlist.
//! A duration or a guessed live_start_index is never evidence of coverage.
use crate::{StreamSource, Tools, VideoInfo, media, process};
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use std::{path::Path, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};
use tokio_util::sync::CancellationToken;
use url::Url;
use ytlr_core::{RecoveryEvidence, TimelineGap, atomic_write, parse_rfc3339};

#[derive(Debug, Clone)]
pub struct Segment {
    pub uri: Url,
    pub start_ms: i64,
    pub end_ms: i64,
}
#[derive(Debug)]
pub struct Playlist {
    pub segments: Vec<Segment>,
    pub init: Option<Url>,
}

pub fn parse_playlist(text: &str, base: &Url) -> Result<Playlist> {
    if !text.starts_with("#EXTM3U") {
        bail!("HLS 미디어 목록이 아닙니다.");
    }
    let mut segments = vec![];
    let mut init = None;
    let mut time = None;
    let mut duration = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with("#EXT-X-STREAM-INF")
            || line.starts_with("#EXT-X-BYTERANGE")
            || line == "#EXT-X-DISCONTINUITY"
            || line == "#EXT-X-GAP"
            || (line.starts_with("#EXT-X-KEY:") && line != "#EXT-X-KEY:METHOD=NONE")
        {
            bail!("암호화·불연속·바이트 범위 HLS는 자동 복구하지 않습니다.");
        }
        if let Some(value) = line.strip_prefix("#EXT-X-PROGRAM-DATE-TIME:") {
            let next = parse_rfc3339(value)
                .context("잘못된 HLS 절대 시각")?
                .timestamp_millis();
            if time.is_some_and(|t: i64| (t - next).abs() > 50) {
                bail!("HLS 타임라인이 연속적이지 않습니다.");
            }
            time = Some(next);
        } else if let Some(value) = line.strip_prefix("#EXTINF:") {
            let seconds: f64 = value.split(',').next().unwrap_or_default().parse()?;
            if !seconds.is_finite() || !(0.001..=120.0).contains(&seconds) {
                bail!("잘못된 HLS 조각 길이");
            }
            duration = Some((seconds * 1000.0).round() as i64);
        } else if let Some(value) = line.strip_prefix("#EXT-X-MAP:") {
            if init.is_some() || !segments.is_empty() || value.contains("BYTERANGE") {
                bail!("변경되는 초기화 구간은 자동 복구하지 않습니다.");
            }
            let uri = value
                .strip_prefix("URI=\"")
                .and_then(|s| s.strip_suffix('"'))
                .context("지원되지 않는 초기화 구간")?;
            init = Some(base.join(uri)?);
        } else if !line.is_empty() && !line.starts_with('#') {
            let start_ms =
                time.context("PROGRAM-DATE-TIME이 없어 구간의 절대 시각을 확인할 수 없습니다.")?;
            let end_ms = start_ms
                .checked_add(duration.take().context("조각 길이가 없습니다.")?)
                .context("시각 범위 오류")?;
            segments.push(Segment {
                uri: base.join(line)?,
                start_ms,
                end_ms,
            });
            time = Some(end_ms);
            if segments.len() > 10000 {
                bail!("HLS 목록 크기 제한 초과");
            }
        }
    }
    Ok(Playlist { segments, init })
}

pub fn select_segments(playlist: &Playlist, start: i64, end: i64) -> Result<Vec<Segment>> {
    if end <= start {
        bail!("복구 구간이 비어 있습니다.");
    }
    let selected: Vec<_> = playlist
        .segments
        .iter()
        .filter(|s| s.start_ms < end && s.end_ms > start)
        .cloned()
        .collect();
    if selected.first().is_none_or(|s| s.start_ms > start)
        || selected.last().is_none_or(|s| s.end_ms < end)
        || selected
            .windows(2)
            .any(|s| (s[0].end_ms - s[1].start_ms).abs() > 50)
    {
        bail!("요청한 시각 구간 전체가 HLS 윈도우에 없습니다.");
    }
    Ok(selected)
}

async fn fetch(
    client: &reqwest::Client,
    source: &StreamSource,
    uri: &Url,
    limit: usize,
) -> Result<Vec<u8>> {
    if !matches!(uri.scheme(), "https" | "http") {
        bail!("지원되지 않는 스트림 URL");
    }
    let mut request = client.get(uri.clone());
    for (key, value) in &source.headers {
        if key.eq_ignore_ascii_case("cookie") || key.eq_ignore_ascii_case("authorization") {
            continue;
        }
        request = request.header(key, value);
    }
    let response = request
        .send()
        .await
        .context("HLS 요청 실패")?
        .error_for_status()?;
    let mut data = vec![];
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if data.len().saturating_add(chunk.len()) > limit {
            bail!("복구 데이터 크기 제한 초과");
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

fn timestamp(ms: i64) -> Result<String> {
    Ok(chrono::DateTime::from_timestamp_millis(ms)
        .context("시각 범위 오류")?
        .to_rfc3339())
}

pub async fn recover(
    tools: &Tools,
    info: &VideoInfo,
    gap: &TimelineGap,
    directory: &Path,
    cancel: CancellationToken,
) -> Result<RecoveryEvidence> {
    let work = recover_inner(tools, info, gap, directory);
    tokio::select! {
        _ = cancel.cancelled() => bail!("구간 보충 취소 · 수집 원본 보존"),
        result = tokio::time::timeout(Duration::from_secs(180), work) => result.context("구간 보충 시간 제한 초과")?,
    }
}

async fn recover_inner(
    tools: &Tools,
    info: &VideoInfo,
    gap: &TimelineGap,
    directory: &Path,
) -> Result<RecoveryEvidence> {
    let start = parse_rfc3339(&gap.started_at)
        .context("공백 시작 시각 오류")?
        .timestamp_millis();
    let requested_end = gap
        .ended_at
        .as_deref()
        .context("공백 종료 시각이 없습니다.")?;
    let end = parse_rfc3339(requested_end)
        .context("공백 종료 시각 오류")?
        .timestamp_millis();
    if end <= start || end - start > 3_600_000 {
        bail!("지원하는 복구 구간을 초과합니다.");
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .build()?;
    let mut tracks = vec![];
    let mut video_range = None;
    let mut audio_range = None;
    let mut budget = 512usize * 1024 * 1024;
    // Inspect all manifests before writing anything or fetching media.
    let mut plans = vec![];
    for source in &info.sources {
        let uri = Url::parse(&source.url)?;
        let bytes = fetch(&client, source, &uri, 2 * 1024 * 1024).await?;
        let playlist = parse_playlist(std::str::from_utf8(&bytes)?, &uri)?;
        let segments = select_segments(&playlist, start, end)?;
        plans.push((source, playlist.init, segments));
    }
    if plans.is_empty() {
        bail!("복구할 스트림이 없습니다.");
    }
    tokio::fs::create_dir_all(directory).await?;
    for (index, (source, init, segments)) in plans.iter().enumerate() {
        let path = directory.join(format!("track-{index}.bin"));
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .await?;
        for uri in init.iter().chain(segments.iter().map(|s| &s.uri)) {
            let bytes = fetch(&client, source, uri, budget.min(128 * 1024 * 1024)).await?;
            budget -= bytes.len();
            file.write_all(&bytes).await?;
        }
        file.sync_all().await?;
        drop(file);
        let expected = (segments.last().unwrap().end_ms - segments[0].start_ms) as f64 / 1000.0;
        let probe = media::probe(tools, &path).await?;
        if (probe.duration - expected).abs() > 1.0
            || (source.video && !probe.has_video)
            || (source.audio && !probe.has_audio)
        {
            bail!("받은 미디어의 길이/트랙이 HLS 타임라인과 일치하지 않습니다.");
        }
        // A header is insufficient. Decode the bounded candidate before exposing it.
        let mut decode = Command::new(tools.require("ffmpeg")?);
        decode
            .args(["-v", "error", "-xerror", "-nostdin", "-i"])
            .arg(&path)
            .args(["-map", "0", "-f", "null", "-"])
            .kill_on_drop(true);
        process::hide_console(&mut decode);
        if !decode.output().await?.status.success() {
            bail!("복구 미디어 디코딩 검사 실패");
        }
        let range = (segments[0].start_ms, segments.last().unwrap().end_ms);
        if source.video {
            video_range = Some(range);
        }
        if source.audio {
            audio_range = Some(range);
        }
        tracks.push((path, source.video, source.audio, range));
    }
    let video = video_range.context("영상 타임라인 없음")?;
    let audio = audio_range.context("음성 타임라인 없음")?;
    let dest = directory.join("candidate.mkv");
    let staging = directory.join("candidate.partial");
    let mut cmd = Command::new(tools.require("ffmpeg")?);
    cmd.args(["-v", "error", "-nostdin", "-n"]);
    for (path, _, _, range) in &tracks {
        cmd.args([
            "-ss",
            &format!("{:.3}", (start - range.0) as f64 / 1000.0),
            "-i",
        ])
        .arg(path);
    }
    let vi = tracks.iter().position(|t| t.1).unwrap();
    let ai = tracks.iter().position(|t| t.2).unwrap();
    cmd.args([
        "-map",
        &format!("{vi}:v:0"),
        "-map",
        &format!("{ai}:a:0"),
        "-t",
        &format!("{:.3}", (end - start) as f64 / 1000.0),
        "-c",
        "copy",
        "-f",
        "matroska",
    ])
    .arg(&staging)
    .kill_on_drop(true);
    process::hide_console(&mut cmd);
    if !cmd.output().await?.status.success() {
        bail!("구간 보충 파일 생성 실패");
    }
    let mut output = media::probe(tools, &staging).await?;
    if !output.has_video
        || !output.has_audio
        || (output.duration - (end - start) as f64 / 1000.0).abs() > 1.0
    {
        bail!("구간 보충 결과 검사 실패");
    }
    if !media::segment_has_both_tracks(tools, &staging).await? {
        bail!("영상·음성 패킷이 없습니다.");
    }
    tokio::fs::OpenOptions::new()
        .write(true)
        .open(&staging)
        .await?
        .sync_all()
        .await?;
    tokio::fs::hard_link(&staging, &dest).await?;
    tokio::fs::remove_file(&staging).await?;
    output.path = dest;
    output.sha256 = Some(ytlr_core::file_digest(&output.path)?.1);
    output.verification = "hls_pdt_candidate".into();
    let evidence = RecoveryEvidence {
        requested_start: gap.started_at.clone(), requested_end: requested_end.into(),
        video_start: timestamp(video.0)?, video_end: timestamp(video.1)?,
        audio_start: timestamp(audio.0)?, audio_end: timestamp(audio.1)?, output,
        message: "HLS 절대 시각·트랙 길이 확인. 수신 공백은 추정치이며 기존 녹화와의 접합 경계는 미검증입니다.".into(),
    };
    atomic_write(
        &directory.join("evidence.json"),
        &serde_json::to_vec_pretty(&evidence)?,
    )?;
    Ok(evidence)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn playlist() -> Playlist {
        parse_playlist("#EXTM3U\n#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00Z\n#EXTINF:3.5,\na.ts\n#EXTINF:7.0,\nb.ts\n", &Url::parse("https://example.test/live/list.m3u8").unwrap()).unwrap()
    }
    #[test]
    fn uses_variable_durations_and_absolute_time_not_a_guessed_index() {
        let p = playlist();
        let start = parse_rfc3339("2026-01-01T00:00:03Z")
            .unwrap()
            .timestamp_millis();
        let s = select_segments(&p, start, start + 6000).unwrap();
        assert_eq!(s.len(), 2);
        assert!(select_segments(&p, start, start + 9000).is_err());
        assert!(select_segments(&p, start - 4000, start).is_err());
    }
    #[test]
    fn rejects_undated_discontinuous_or_encrypted_playlists() {
        let base = Url::parse("https://example.test/a").unwrap();
        for text in [
            "#EXTM3U\n#EXTINF:5,\na.ts",
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"x\"",
            "#EXTM3U\n#EXT-X-DISCONTINUITY",
        ] {
            assert!(parse_playlist(text, &base).is_err());
        }
    }
}
