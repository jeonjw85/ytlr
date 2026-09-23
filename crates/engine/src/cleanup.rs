use crate::{Tools, closed_segments, files_under, probe, segment_has_tracks, verify_ledger};
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};
use ytlr_core::{JobState, MediaOutput, RecordingJob, atomic_write, file_digest, now, sync_dir};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CleanupFile {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupPlan {
    pub id: String,
    pub job_id: String,
    pub created_at: String,
    pub files: Vec<CleanupFile>,
    pub retained: Vec<MediaOutput>,
    pub reclaimable_bytes: u64,
}

#[derive(Deserialize)]
pub struct CleanupRequest {
    pub plan_id: String,
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub files: Vec<PathBuf>,
}

#[derive(Deserialize)]
struct LedgerEntry {
    file: PathBuf,
    bytes: u64,
    sha256: String,
}

fn committed_files(root: &Path) -> Result<BTreeMap<PathBuf, (u64, String)>> {
    let data = fs::read(root.join("durable-fragments.jsonl"))
        .context("완성 세그먼트의 저장 확정 기록이 없습니다.")?;
    let end = data
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |i| i + 1);
    if end != data.len() {
        bail!("저장 확정 기록의 마지막 항목이 완성되지 않았습니다.");
    }
    let mut entries = BTreeMap::new();
    for line in data[..end]
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let entry: LedgerEntry = serde_json::from_slice(line)?;
        if entry.file.as_os_str().is_empty()
            || entry
                .file
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || entries
                .insert(entry.file, (entry.bytes, entry.sha256))
                .is_some()
        {
            bail!("저장 확정 기록의 파일 경로가 올바르지 않습니다.");
        }
    }
    Ok(entries)
}

fn recover_quarantined(root: &Path) -> Result<()> {
    let mut synced = BTreeSet::new();
    for pending in files_under(root)? {
        let Some(name) = pending.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(body) = name
            .strip_prefix(".cleanup-")
            .and_then(|name| name.strip_suffix(".pending"))
        else {
            continue;
        };
        if body.len() < 38 || body.as_bytes().get(36) != Some(&b'-') {
            continue;
        }
        let (plan_id, original) = body.split_at(36);
        let original = original.strip_prefix('-').context("격리 파일명 오류")?;
        uuid::Uuid::parse_str(plan_id).context("격리 파일의 정리 계획 ID 오류")?;
        if original.is_empty()
            || !matches!(
                Path::new(original).components().next(),
                Some(Component::Normal(_))
            )
            || Path::new(original).components().count() != 1
        {
            bail!("격리 파일의 원본 경로가 올바르지 않습니다.");
        }
        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(format!("cleanup-{plan_id}.json")))?)?;
        let plan: CleanupPlan = serde_json::from_value(receipt["plan"].clone())?;
        let selected: BTreeSet<PathBuf> = serde_json::from_value(receipt["selected"].clone())?;
        let relative = pending.strip_prefix(root)?;
        let expected_relative = relative.parent().unwrap_or(Path::new("")).join(original);
        let expected = plan
            .files
            .iter()
            .find(|file| file.path == expected_relative)
            .context("격리 파일이 정리 계획에 없습니다.")?;
        if plan.id != plan_id
            || !selected.contains(&expected_relative)
            || file_digest(&pending)? != (expected.bytes, expected.sha256.clone())
        {
            bail!("격리 파일이 정리 영수증과 일치하지 않습니다.");
        }
        let parent = pending.parent().context("격리 파일 상위 폴더 오류")?;
        let original_path = parent.join(original);
        if committed_files(parent)?.contains_key(Path::new(original)) {
            if original_path.exists() {
                bail!("격리 파일과 원본 파일이 모두 존재합니다. 자동 복구하지 않습니다.");
            }
            fs::rename(&pending, original_path)?;
        } else {
            fs::remove_file(&pending)?;
        }
        synced.insert(parent.to_owned());
    }
    for parent in synced {
        sync_dir(&parent)?;
    }
    Ok(())
}

fn checked_path(root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("정리 대상 경로가 올바르지 않습니다.");
    }
    if !fs::symlink_metadata(root)?.is_dir() {
        bail!("작업 폴더가 일반 디렉터리가 아닙니다.");
    }
    let mut path = root.to_owned();
    for part in relative.components() {
        path.push(part);
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            bail!("링크가 포함된 파일은 정리할 수 없습니다.");
        }
    }
    Ok(path)
}

pub fn inventory(job: &RecordingJob) -> Result<BTreeMap<String, u64>> {
    let mut sizes = BTreeMap::new();
    for path in files_under(&job.output_dir)? {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let category = if job.outputs.iter().any(|o| o.path == path) {
            "results"
        } else if name.starts_with("export-") && !name.ends_with(".json") {
            "exports"
        } else if name.contains("-Frag") {
            "fragments"
        } else if name.starts_with("part-") {
            "segments"
        } else if name.starts_with("source") {
            "sources"
        } else {
            "metadata_other"
        };
        *sizes.entry(category.into()).or_insert(0) += fs::metadata(path)?.len();
    }
    Ok(sizes)
}

fn eligible(job: &RecordingJob) -> Result<()> {
    if !matches!(job.state, JobState::Completed | JobState::Stopped)
        || job.continuity_uncertain
        || !job.gaps.is_empty()
        || job.outputs.is_empty()
    {
        bail!("연속성 문제가 없고 검증된 결과가 있는 완료·중지 작업만 정리할 수 있습니다.");
    }
    Ok(())
}

fn lock_backup(root: &Path) -> Result<fs::File> {
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("backup.lock"))?;
    lock.try_lock_exclusive()
        .context("백업 또는 정리가 진행 중입니다. 완료 후 다시 시도하세요.")?;
    Ok(lock)
}

async fn prepare(tools: &Tools, job: &RecordingJob) -> Result<CleanupPlan> {
    eligible(job)?;
    let mut files = vec![];
    let mut retained = vec![];
    for output in &job.outputs {
        let relative = output.path.strip_prefix(&job.output_dir)?;
        let path = checked_path(&job.output_dir, relative)?;
        let expected = output
            .sha256
            .as_ref()
            .context("결과 파일의 검증 해시가 없습니다.")?;
        let hash_path = path.clone();
        let (bytes, hash) = tokio::task::spawn_blocking(move || file_digest(&hash_path)).await??;
        if bytes != output.bytes || &hash != expected {
            bail!("보존할 결과 파일이 변경되었습니다. 정리를 중단합니다.");
        }
        let actual = probe(tools, &path).await?;
        if !job.recording_options.accepts(&actual)
            || actual.duration <= 0.0
            || !segment_has_tracks(tools, &path, &job.recording_options).await?
        {
            bail!("보존할 결과 파일 검증 실패");
        }
        retained.push(output.clone());
        // Only our verified segment-concatenation output proves coverage of these segments.
        if !path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .starts_with("recording-")
        {
            continue;
        }
        let parent = path.parent().context("결과 경로 오류")?;
        if !parent
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .starts_with("attempt-")
        {
            continue;
        }
        let root = parent.to_owned();
        if !tokio::task::spawn_blocking(move || verify_ledger(&root))
            .await??
            .is_empty()
        {
            bail!("원본 조각 검증에 실패했습니다. 정리하지 않습니다.");
        }
        let committed = committed_files(parent)?;
        let mut candidates = vec![];
        let mut seconds = 0.0;
        for segment in closed_segments(parent)? {
            if !segment.exists() {
                continue;
            } // Already documented by a previous cleanup receipt.
            let relative = segment.strip_prefix(&job.output_dir)?.to_owned();
            let segment = checked_path(&job.output_dir, &relative)?;
            if job.outputs.iter().any(|o| o.path == segment) {
                continue;
            }
            let media = probe(tools, &segment).await?;
            if !job.recording_options.accepts(&media)
                || media.duration <= 0.0
                || !segment_has_tracks(tools, &segment, &job.recording_options).await?
            {
                bail!("세그먼트 검증 실패");
            }
            seconds += media.duration;
            let (bytes, sha256) =
                tokio::task::spawn_blocking(move || file_digest(&segment)).await??;
            let attempt_relative = parent.strip_prefix(&job.output_dir)?;
            let ledger_path = relative.strip_prefix(attempt_relative)?;
            if committed.get(ledger_path) != Some(&(bytes, sha256.clone())) {
                bail!("정리 대상 세그먼트가 저장 확정 기록과 일치하지 않습니다.");
            }
            candidates.push(CleanupFile {
                path: relative,
                bytes,
                sha256,
            });
        }
        if seconds > actual.duration + 1.0 {
            bail!("병합 결과가 세그먼트 전체 길이를 포함하지 않습니다.");
        }
        files.extend(candidates);
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    Ok(CleanupPlan {
        id: uuid::Uuid::new_v4().to_string(),
        job_id: job.id.clone(),
        created_at: now(),
        reclaimable_bytes: files.iter().map(|f| f.bytes).sum(),
        files,
        retained,
    })
}

pub async fn preview(tools: &Tools, job: &RecordingJob) -> Result<CleanupPlan> {
    eligible(job)?;
    let _lock = lock_backup(&job.output_dir)?;
    let root = job.output_dir.clone();
    tokio::task::spawn_blocking(move || recover_quarantined(&root)).await??;
    let plan = prepare(tools, job).await?;
    atomic_write(
        &job.output_dir.join("cleanup-plan.json"),
        &serde_json::to_vec_pretty(&plan)?,
    )?;
    Ok(plan)
}

pub async fn execute(tools: &Tools, job: &RecordingJob, req: &CleanupRequest) -> Result<u64> {
    eligible(job)?;
    let _lock = lock_backup(&job.output_dir)?;
    let recovery_root = job.output_dir.clone();
    tokio::task::spawn_blocking(move || recover_quarantined(&recovery_root)).await??;
    let plan: CleanupPlan =
        serde_json::from_slice(&fs::read(job.output_dir.join("cleanup-plan.json"))?)?;
    uuid::Uuid::parse_str(&plan.id).context("정리 계획 ID 오류")?;
    if req.all && !req.files.is_empty() {
        bail!("전체 선택과 개별 파일 선택은 함께 사용할 수 없습니다.");
    }
    if plan.id != req.plan_id || plan.job_id != job.id {
        bail!("정리 미리보기가 만료되었습니다. 다시 확인하세요.");
    }
    let current = prepare(tools, job).await?;
    if plan.files != current.files
        || serde_json::to_value(&plan.retained)? != serde_json::to_value(&current.retained)?
    {
        bail!("미리보기 이후 파일이 변경되었습니다. 다시 확인하세요.");
    }
    let selected: BTreeSet<_> = if req.all {
        plan.files.iter().map(|f| f.path.clone()).collect()
    } else {
        req.files.iter().cloned().collect()
    };
    if selected.is_empty()
        || selected
            .iter()
            .any(|p| !plan.files.iter().any(|f| &f.path == p))
    {
        bail!("미리보기에 포함된 삭제 대상을 선택하세요.");
    }
    let root = job.output_dir.clone();
    // Keep the cross-process lock inside the blocking task even if the HTTP caller disconnects.
    tokio::task::spawn_blocking(move || -> Result<u64> {
        let _lock = _lock;
        let receipt = root.join(format!("cleanup-{}.json", plan.id));
        atomic_write(&receipt, &serde_json::to_vec_pretty(&serde_json::json!({"plan":plan,"selected":selected,"started_at":now(),"completed":false}))?)?;
        let parents: BTreeSet<_> = selected.iter().filter_map(|p| p.parent().map(Path::to_owned)).collect();
        let mut ledgers = vec![];
        // Preserve the original ledger and prepare its replacement before any
        // file is moved out of its committed pathname.
        for parent in &parents {
            let ledger = root.join(parent).join("durable-fragments.jsonl");
            if !ledger.exists() { bail!("정리 대상의 저장 확정 기록이 없습니다."); }
            let data = fs::read(&ledger)?;
            atomic_write(&root.join(parent).join(format!("ledger-before-cleanup-{}.jsonl", plan.id)), &data)?;
            let mut kept = vec![];
            for line in data.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
                let entry: serde_json::Value = serde_json::from_slice(line)?;
                let file = entry["file"].as_str().context("저장 확정 기록 오류")?;
                if !selected.contains(&parent.join(file)) { kept.extend_from_slice(line); kept.push(b'\n'); }
            }
            ledgers.push((ledger, data, kept));
        }

        // Verify every candidate before moving any of them.
        for file in plan.files.iter().filter(|f| selected.contains(&f.path)) {
            let path = checked_path(&root, &file.path)?;
            if file_digest(&path)? != (file.bytes, file.sha256.clone()) { bail!("삭제 직전에 파일이 변경되었습니다. 남은 파일은 보존합니다."); }
        }

        // A same-directory rename prevents a later pathname recheck from
        // deleting an unrelated replacement. Restore moved files on error.
        let mut quarantined = vec![];
        let move_result = (|| -> Result<()> {
            for file in plan.files.iter().filter(|f| selected.contains(&f.path)) {
                let path = checked_path(&root, &file.path)?;
                let parent = path.parent().context("정리 대상 상위 폴더 오류")?;
                let name = path.file_name().context("정리 대상 파일명 오류")?.to_string_lossy();
                let quarantine = parent.join(format!(".cleanup-{}-{name}.pending", plan.id));
                if quarantine.exists() {
                    bail!("이전 정리에서 남은 격리 파일이 있습니다. 원본을 보존하고 정리를 중단합니다.");
                }
                fs::rename(&path, &quarantine)?;
                if file_digest(&quarantine)? != (file.bytes, file.sha256.clone()) {
                    let _ = fs::rename(&quarantine, &path);
                    bail!("격리한 파일이 미리보기와 다릅니다. 정리를 중단합니다.");
                }
                quarantined.push((path, quarantine, file.bytes));
            }
            Ok(())
        })();
        if let Err(error) = move_result {
            for (path, quarantine, _) in quarantined.iter().rev() {
                let _ = fs::rename(quarantine, path);
            }
            return Err(error);
        }

        // Once every source is safely quarantined, publish all shortened
        // ledgers. Roll back both metadata and names if any publication fails.
        let ledger_result = (|| -> Result<()> {
            for (ledger, _, kept) in &ledgers { atomic_write(ledger, kept)?; }
            for parent in &parents { sync_dir(&root.join(parent))?; }
            Ok(())
        })();
        if let Err(error) = ledger_result {
            for (ledger, original, _) in &ledgers { let _ = atomic_write(ledger, original); }
            for (path, quarantine, _) in quarantined.iter().rev() {
                let _ = fs::rename(quarantine, path);
            }
            return Err(error);
        }

        let mut reclaimed = 0;
        let mut pending = vec![];
        for (_, quarantine, bytes) in &quarantined {
            match fs::remove_file(quarantine) {
                Ok(()) => reclaimed += *bytes,
                Err(_) => pending.push(
                    quarantine
                        .strip_prefix(&root)
                        .unwrap_or(quarantine)
                        .to_owned(),
                ),
            }
        }
        for parent in parents { let _ = sync_dir(&root.join(parent)); }
        let completed = pending.is_empty();
        let receipt_data = serde_json::to_vec_pretty(&serde_json::json!({"plan":plan,"selected":selected,"completed_at":now(),"completed":completed,"committed":true,"pending":pending,"reclaimed_bytes":reclaimed}));
        if receipt_data.as_ref().is_ok_and(|data| atomic_write(&receipt, data).is_ok()) && completed {
            let _ = fs::remove_file(root.join("cleanup-plan.json"));
            let _ = sync_dir(&root);
        }
        Ok(reclaimed)
    }).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_paths_reject_traversal_and_links() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("job");
        fs::create_dir(&root).unwrap();
        assert!(checked_path(&root, Path::new("../outside")).is_err());
        assert!(checked_path(&root, d.path()).is_err());
        #[cfg(unix)]
        {
            let outside = d.path().join("outside");
            fs::write(&outside, b"keep").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("part-001.mkv")).unwrap();
            assert!(checked_path(&root, Path::new("part-001.mkv")).is_err());
            assert_eq!(fs::read(outside).unwrap(), b"keep");
        }
    }
}
