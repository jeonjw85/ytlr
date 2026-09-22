use crate::{Tools, process};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use ytlr_core::{MediaOutput, atomic_write, now, sync_dir};

#[derive(Debug, Deserialize)]
struct Probe {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<Format>,
}
#[derive(Debug, Deserialize)]
struct Stream {
    codec_type: Option<String>,
    duration: Option<String>,
}
#[derive(Debug, Deserialize)]
struct Format {
    duration: Option<String>,
}

pub async fn probe(tools: &Tools, path: &Path) -> Result<MediaOutput> {
    let mut cmd = tokio::process::Command::new(tools.require("ffprobe")?);
    cmd.args([
        "-v",
        "error",
        "-show_entries",
        "stream=codec_type,duration:format=duration",
        "-of",
        "json",
    ])
    .arg(path)
    .kill_on_drop(true);
    process::hide_console(&mut cmd);
    let result = tokio::time::timeout(Duration::from_secs(60), cmd.output()).await??;
    if !result.status.success() {
        bail!("미디어 헤더 검사 실패");
    }
    let data: Probe = serde_json::from_slice(&result.stdout)?;
    let duration = data
        .format
        .and_then(|f| f.duration)
        .and_then(|d| d.parse::<f64>().ok())
        .or_else(|| {
            data.streams
                .iter()
                .filter_map(|s| s.duration.as_ref()?.parse::<f64>().ok())
                .max_by(f64::total_cmp)
        })
        .unwrap_or(0.0);
    Ok(MediaOutput {
        path: path.to_owned(),
        bytes: fs::metadata(path)?.len(),
        duration,
        has_video: data
            .streams
            .iter()
            .any(|s| s.codec_type.as_deref() == Some("video")),
        has_audio: data
            .streams
            .iter()
            .any(|s| s.codec_type.as_deref() == Some("audio")),
        verification: "container_probe".into(),
        sha256: None,
    })
}

pub fn files_under(root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = vec![];
    if !root.exists() {
        return Ok(found);
    }
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Commit only native fragment files finalized by yt-dlp's .part -> -FragN rename.
/// A durable ledger records hashes; incomplete .part files are retained but never certified.
pub fn checkpoint(root: &Path, known: &mut HashSet<PathBuf>) -> Result<u64> {
    let mut total = 0;
    let ledger = root.join("durable-fragments.jsonl");
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ledger)?;
    let closed_segments = closed_segments(root)?;
    for path in files_under(root)? {
        let size = match fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            // Native downloads rename temporary files concurrently with our scan.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        if path != ledger {
            total += size;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let complete_fragment = name
            .rsplit_once("-Frag")
            .is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            || closed_segments.contains(&path);
        if !complete_fragment || known.contains(&path) {
            continue;
        }
        let mut f = fs::OpenOptions::new().read(true).write(true).open(&path)?;
        f.sync_all()?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let n = f.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        writeln!(
            log,
            "{}",
            serde_json::json!({"file": path.strip_prefix(root)?, "bytes": size, "sha256": hex::encode(hash.finalize()), "committed_at": now()})
        )?;
        known.insert(path);
    }
    log.sync_all()?;
    sync_dir(root)?;
    Ok(total)
}

pub fn closed_segments(root: &Path) -> Result<HashSet<PathBuf>> {
    let path = root.join("segments.csv");
    let contents = if path.exists() {
        fs::read(&path)?
    } else {
        vec![]
    };
    // A list entry is a commit notification only after the terminating newline.
    let end = contents
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1);
    let mut csv = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(&contents[..end]);
    let mut result = HashSet::new();
    for row in csv.records() {
        let row = row?;
        if let Some(name) = row.get(0) {
            let name = Path::new(name)
                .file_name()
                .context("세그먼트 파일명 오류")?;
            let name_str = name.to_string_lossy();
            if name_str.starts_with("part-") && name_str.ends_with(".mkv") {
                result.insert(root.join(name));
            }
        }
    }
    // Recover committed segments even when FFmpeg's own list was not flushed at power loss.
    if let Ok(ledger) = fs::read(root.join("durable-fragments.jsonl")) {
        let end = ledger
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |i| i + 1);
        for line in ledger[..end].split(|b| *b == b'\n') {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(line)
                && let Some(name) = value["file"].as_str()
                && name.starts_with("part-")
                && name.ends_with(".mkv")
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
            {
                result.insert(root.join(name));
            }
        }
    }
    Ok(result)
}

/// Read packet counts from a closed, short segment; headers alone cannot detect missing audio.
pub async fn segment_has_both_tracks(tools: &Tools, path: &Path) -> Result<bool> {
    let mut cmd = tokio::process::Command::new(tools.require("ffprobe")?);
    cmd.args([
        "-v",
        "error",
        "-count_packets",
        "-show_entries",
        "stream=codec_type,nb_read_packets",
        "-of",
        "json",
    ])
    .arg(path)
    .kill_on_drop(true);
    process::hide_console(&mut cmd);
    let result = tokio::time::timeout(Duration::from_secs(30), cmd.output()).await??;
    if !result.status.success() {
        return Ok(false);
    }
    let data: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    let has = |kind| {
        data["streams"].as_array().is_some_and(|streams| {
            streams.iter().any(|s| {
                s["codec_type"].as_str() == Some(kind)
                    && s["nb_read_packets"]
                        .as_str()
                        .and_then(|n| n.parse::<u64>().ok())
                        .is_some_and(|n| n > 0)
            })
        })
    };
    Ok(has("audio") && has("video"))
}

pub async fn discover_outputs(tools: &Tools, root: &Path) -> Result<Vec<MediaOutput>> {
    let mut outputs = vec![];
    let mut seen_attempts = HashSet::new();
    let mut files = files_under(root)?;
    files.sort_by_key(|p| {
        if p.extension().is_some_and(|e| e == "mkv") {
            0
        } else {
            1
        }
    });
    for path in files {
        let parent = path.parent().context("출력 경로 오류")?.to_owned();
        if !parent
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("attempt-"))
            || seen_attempts.contains(&parent)
        {
            continue;
        }
        if !path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .starts_with("part-")
            && path
                .extension()
                .is_some_and(|ext| ext == "mkv" || ext == "mp4" || ext == "webm")
            && let Ok(media) = probe(tools, &path).await
            && media.has_video
            && media.has_audio
            && media.duration > 0.0
        {
            outputs.push(media);
            seen_attempts.insert(parent);
        }
    }
    Ok(outputs)
}

pub fn verify_ledger(root: &Path) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Entry {
        file: PathBuf,
        bytes: u64,
        sha256: String,
    }
    let ledger = root.join("durable-fragments.jsonl");
    if !ledger.exists() {
        return Ok(vec![]);
    }
    let contents = fs::read(&ledger)?;
    let end = contents
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1);
    let mut issues = vec![];
    for line in contents[..end]
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
    {
        let entry: Entry = match serde_json::from_slice(line) {
            Ok(entry) => entry,
            Err(_) => {
                issues.push("손상된 저장 확정 기록".into());
                continue;
            }
        };
        if entry
            .file
            .components()
            .any(|p| !matches!(p, std::path::Component::Normal(_)))
        {
            issues.push("잘못된 저장 확정 파일 경로".into());
            continue;
        }
        let file = root.join(&entry.file);
        let good = (|| -> Result<bool> {
            let metadata = fs::symlink_metadata(&file)?;
            if !metadata.is_file() || metadata.len() != entry.bytes {
                return Ok(false);
            }
            let mut reader = File::open(&file)?;
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let n = reader.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
            }
            Ok(hex::encode(hasher.finalize()) == entry.sha256)
        })()
        .unwrap_or(false);
        if !good {
            issues.push(format!(
                "저장 자료 누락/해시 불일치: {}",
                entry.file.display()
            ));
        }
    }
    if end != contents.len() {
        issues.push("마지막 저장 확정 기록이 중단되었습니다.".into());
    }
    Ok(issues)
}

/// Never concat unknown retry boundaries. Salvage each attempt independently with stream-copy.
pub async fn salvage_attempt(tools: &Tools, root: &Path) -> Result<Option<MediaOutput>> {
    let mut segments: Vec<_> = closed_segments(root)?.into_iter().collect();
    segments.sort();
    if !segments.is_empty() {
        return concat_segments(tools, root, &segments).await.map(Some);
    }
    let folder = root.to_owned();
    tokio::task::spawn_blocking(move || rebuild_native_fragments(&folder)).await??;
    let files = files_under(root)?;
    let mut video: Option<(PathBuf, u64)> = None;
    let mut audio: Option<(PathBuf, u64)> = None;
    let mut combined: Option<(PathBuf, u64)> = None;
    for path in files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if !name.starts_with("source")
            || name.contains("-Frag")
            || name.ends_with(".json")
            || name.ends_with(".ytdl")
        {
            continue;
        }
        if let Ok(media) = probe(tools, &path).await {
            if media.has_video && media.has_audio {
                if combined
                    .as_ref()
                    .is_none_or(|(_, size)| *size < media.bytes)
                {
                    combined = Some((path, media.bytes));
                }
            } else if media.has_video {
                if video.as_ref().is_none_or(|(_, size)| *size < media.bytes) {
                    video = Some((path, media.bytes));
                }
            } else if media.has_audio && audio.as_ref().is_none_or(|(_, size)| *size < media.bytes)
            {
                audio = Some((path, media.bytes));
            }
        }
    }
    let mut inputs = vec![];
    if let Some((p, _)) = combined {
        inputs.push(p);
    } else if let (Some((v, _)), Some((a, _))) = (video, audio) {
        inputs.extend([v, a]);
    }
    if inputs.is_empty() {
        return Ok(None);
    }
    let dest = root.join(format!("recovered-{}.mkv", uuid::Uuid::new_v4()));
    remux(tools, &inputs, &dest).await.map(Some)
}

/// Rebuild a missing/truncated aggregate from contiguous native fragments, without replacing it.
pub fn rebuild_native_fragments(root: &Path) -> Result<()> {
    let mut groups = std::collections::BTreeMap::<String, Vec<(u64, PathBuf)>>::new();
    for path in files_under(root)? {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if let Some((prefix, n)) = name.rsplit_once("-Frag")
            && prefix.starts_with("source.")
            && let Ok(n) = n.parse::<u64>()
        {
            groups.entry(prefix.into()).or_default().push((n, path));
        }
    }
    for (prefix, mut pieces) in groups {
        pieces.sort_by_key(|(n, _)| *n);
        if pieces.first().is_none_or(|(n, _)| *n > 1)
            || pieces.windows(2).any(|w| w[1].0 != w[0].0 + 1)
        {
            continue;
        }
        let total: u64 = pieces
            .iter()
            .map(|(_, p)| fs::metadata(p).map(|m| m.len()))
            .collect::<std::io::Result<Vec<_>>>()?
            .iter()
            .sum();
        if [
            root.join(&prefix),
            root.join(prefix.trim_end_matches(".part")),
        ]
        .iter()
        .any(|p| fs::metadata(p).is_ok_and(|m| m.len() >= total))
        {
            continue;
        }
        let target = root.join(format!("source.rebuilt-{}.bin", uuid::Uuid::new_v4()));
        let mut output = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&target)?;
        for (_, path) in pieces {
            std::io::copy(&mut File::open(path)?, &mut output)?;
        }
        output.sync_all()?;
        sync_dir(root)?;
    }
    Ok(())
}

async fn concat_segments(tools: &Tools, root: &Path, segments: &[PathBuf]) -> Result<MediaOutput> {
    let mut lines = String::from("ffconcat version 1.0\n");
    let mut signature = None;
    for path in segments {
        let media = probe(tools, path).await?;
        if !media.has_video || !media.has_audio {
            bail!("영상·음성이 없는 세그먼트가 있습니다. 원본을 보존합니다.");
        }
        let mut cmd = tokio::process::Command::new(tools.require("ffprobe")?);
        cmd.args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,codec_name,width,height,sample_rate,channels",
            "-of",
            "json",
        ])
        .arg(path)
        .kill_on_drop(true);
        process::hide_console(&mut cmd);
        let out = tokio::time::timeout(Duration::from_secs(30), cmd.output()).await??;
        if !out.status.success() {
            bail!("세그먼트 포맷 확인 실패");
        }
        let current: serde_json::Value = serde_json::from_slice(&out.stdout)?;
        if signature.as_ref().is_some_and(|s| s != &current) {
            bail!("방송 중 포맷이 변경되어 자동 연결을 중단했습니다. 분할 원본을 사용하세요.");
        }
        signature = Some(current);
        let file = path
            .file_name()
            .context("세그먼트 파일명 오류")?
            .to_string_lossy();
        if !file
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        {
            bail!("잘못된 세그먼트 파일명");
        }
        lines.push_str(&format!("file '{file}'\n"));
    }
    let list = root.join("concat.txt");
    atomic_write(&list, lines.as_bytes())?;
    let dest = root.join(format!("recording-{}.mkv", uuid::Uuid::new_v4()));
    let temp = dest.with_extension("partial");
    let mut cmd = tokio::process::Command::new(tools.require("ffmpeg")?);
    cmd.args([
        "-v", "error", "-nostdin", "-n", "-f", "concat", "-safe", "1", "-i",
    ])
    .arg(&list)
    .args([
        "-map", "0:v:0", "-map", "0:a:0", "-c", "copy", "-f", "matroska",
    ])
    .arg(&temp)
    .kill_on_drop(true);
    process::hide_console(&mut cmd);
    let result = cmd.output().await?;
    if !result.status.success() {
        bail!(
            "분할 파일 연결 실패: {}",
            ytlr_core::redact(&String::from_utf8_lossy(&result.stderr))
        );
    }
    let output = probe(tools, &temp).await?;
    if output.duration <= 0.0 || !output.has_audio || !output.has_video {
        bail!("병합 결과 검증 실패");
    }
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&temp)?
        .sync_all()?;
    fs::hard_link(&temp, &dest)?;
    fs::remove_file(temp)?;
    sync_dir(root)?;
    Ok(MediaOutput {
        path: dest,
        ..output
    })
}

pub async fn remux(tools: &Tools, inputs: &[PathBuf], dest: &Path) -> Result<MediaOutput> {
    if dest.exists() {
        bail!("출력 파일이 이미 존재합니다.");
    }
    let parent = dest.parent().context("출력 폴더 오류")?;
    let format = if dest.extension().is_some_and(|x| x == "mp4") {
        "mp4"
    } else {
        "matroska"
    };
    let temp = parent.join(format!(".export-{}.partial", uuid::Uuid::new_v4()));
    let mut cmd = tokio::process::Command::new(tools.require("ffmpeg")?);
    cmd.args(["-hide_banner", "-nostdin", "-v", "error", "-n"]);
    for input in inputs {
        cmd.arg("-i").arg(input);
    }
    if inputs.len() == 2 {
        cmd.args(["-map", "0:v:0", "-map", "1:a:0"]);
    } else {
        cmd.args(["-map", "0:v:0", "-map", "0:a:0"]);
    }
    cmd.args(["-c", "copy", "-f", format])
        .arg(&temp)
        .kill_on_drop(true);
    process::hide_console(&mut cmd);
    let result = cmd.output().await?;
    if !result.status.success() {
        bail!(
            "병합 실패: {}",
            ytlr_core::redact(&String::from_utf8_lossy(&result.stderr))
        );
    }
    let media = probe(tools, &temp).await?;
    if !media.has_video || !media.has_audio || media.duration <= 0.0 {
        bail!("결과 파일의 영상·음성 검증 실패");
    }
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&temp)?
        .sync_all()?;
    // hard_link is an atomic no-clobber publication on the same filesystem.
    fs::hard_link(&temp, dest)?;
    fs::remove_file(&temp)?;
    sync_dir(parent)?;
    let result = MediaOutput {
        path: dest.to_owned(),
        ..media
    };
    atomic_write(
        &dest.with_extension("verification.json"),
        &serde_json::to_vec_pretty(&result)?,
    )?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_does_not_certify_unfinished_fragments() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("source.part-Frag1"), b"completed").unwrap();
        fs::write(d.path().join("source.part-Frag2.part"), b"incomplete").unwrap();
        let mut known = HashSet::new();
        checkpoint(d.path(), &mut known).unwrap();
        checkpoint(d.path(), &mut known).unwrap();
        let ledger = fs::read_to_string(d.path().join("durable-fragments.jsonl")).unwrap();
        assert_eq!(ledger.lines().count(), 1);
        assert!(ledger.contains("Frag1"));
        assert!(!ledger.contains("Frag2"));
        assert!(d.path().join("source.part-Frag2.part").exists());
        assert!(verify_ledger(d.path()).unwrap().is_empty());
        fs::write(d.path().join("source.part-Frag1"), b"corrupted").unwrap();
        assert_eq!(verify_ledger(d.path()).unwrap().len(), 1);
    }

    #[test]
    fn native_reconstruction_preserves_truncated_aggregate_and_skips_gaps() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("source.video.part"), b"ab").unwrap();
        fs::write(d.path().join("source.video.part-Frag1"), b"abc").unwrap();
        fs::write(d.path().join("source.video.part-Frag2"), b"def").unwrap();
        fs::write(d.path().join("source.audio.part-Frag1"), b"123").unwrap();
        fs::write(d.path().join("source.audio.part-Frag3"), b"789").unwrap();
        rebuild_native_fragments(d.path()).unwrap();
        let rebuilt: Vec<_> = files_under(d.path())
            .unwrap()
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "bin"))
            .collect();
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(fs::read(&rebuilt[0]).unwrap(), b"abcdef");
        assert_eq!(fs::read(d.path().join("source.video.part")).unwrap(), b"ab");
    }

    #[test]
    fn active_temporary_file_renames_are_not_disk_failures() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_owned();
        let writer = std::thread::spawn(move || {
            for n in 0..500 {
                let partial = root.join(format!("transient-{n}.part"));
                let complete = root.join(format!("transient-{n}.bin"));
                fs::write(&partial, b"data").unwrap();
                fs::rename(&partial, &complete).unwrap();
                fs::remove_file(complete).unwrap();
            }
        });
        let mut known = HashSet::new();
        for _ in 0..100 {
            checkpoint(d.path(), &mut known).unwrap();
        }
        writer.join().unwrap();
    }
}
