use crate::{atomic_write, sync_dir};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

/// Resolve existing ancestors too: /disk/link/../backup must not bypass overlap checks.
fn resolved(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(fs::canonicalize(path)?);
    }
    let parent = path.parent().context("저장 경로 오류")?;
    let mut result = resolved(parent)?;
    for component in path.strip_prefix(parent)?.components() {
        match component {
            Component::Normal(name) => result.push(name),
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            _ => bail!("저장 경로 오류"),
        }
    }
    Ok(result)
}

pub fn overlapping_storage(primary: &Path, backup: &Path) -> bool {
    match (resolved(primary), resolved(backup)) {
        (Ok(a), Ok(b)) => a.starts_with(&b) || b.starts_with(&a),
        _ => true,
    }
}

pub fn backup_device(root: &Path) -> Result<Option<u64>> {
    let meta = fs::metadata(root).context("백업 디스크가 연결되어 있지 않습니다.")?;
    if !meta.is_dir() {
        bail!("백업 대상은 기존 폴더여야 합니다.");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Some(meta.dev()))
    }
    #[cfg(not(unix))]
    {
        Ok(None)
    }
}

pub fn identify_backup(root: &Path) -> Result<String> {
    backup_device(root)?;
    let marker = root.join(".ytlr-backup-volume");
    let id = uuid::Uuid::new_v4().to_string();
    match OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&marker)
    {
        Ok(mut file) => {
            file.write_all(id.as_bytes())?;
            file.sync_all()?;
            sync_dir(root)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    read_identity(root)
}

fn read_identity(root: &Path) -> Result<String> {
    let path = root.join(".ytlr-backup-volume");
    if !fs::symlink_metadata(&path)?.is_file() {
        bail!("백업 볼륨 확인 파일이 올바르지 않습니다.");
    }
    let mut value = String::new();
    File::open(path)?.take(128).read_to_string(&mut value)?;
    uuid::Uuid::parse_str(&value).context("백업 볼륨 확인 파일 오류")?;
    Ok(value)
}

fn safe_relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("백업 파일이 작업 폴더를 벗어납니다.");
    }
    Ok(())
}

fn prepare_parent(root: &Path, relative: &Path) -> Result<PathBuf> {
    safe_relative(relative)?;
    let mut current = root.to_owned();
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            current.push(component.as_os_str());
            match fs::symlink_metadata(&current) {
                Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                    bail!("백업 경로에 링크 또는 파일이 있습니다.")
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&current)?;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(root.join(relative))
}

pub fn file_digest(path: &Path) -> Result<(u64, String)> {
    file_digest_with_progress(path, |_| Ok(()))
}

pub fn file_digest_with_progress(
    path: &Path,
    mut progress: impl FnMut(u64) -> Result<()>,
) -> Result<(u64, String)> {
    if !fs::symlink_metadata(path)?.is_file() {
        bail!("일반 파일만 백업할 수 있습니다.");
    }
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut length = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        length += n as u64;
        progress(length)?;
    }
    Ok((length, hex::encode(hasher.finalize())))
}

fn copy_verified(src: &Path, dest: &Path, expected: &(u64, String)) -> Result<()> {
    // A same-sized file may be corrupt. Verify both source and destination content.
    if file_digest(src)? != *expected {
        bail!("백업 원본 해시 불일치: 원본은 변경하지 않습니다.");
    }
    if let Ok(meta) = fs::symlink_metadata(dest) {
        if !meta.is_file() {
            bail!("백업 대상이 일반 파일이 아닙니다.");
        }
        if file_digest(dest)? == *expected {
            return Ok(());
        }
    }
    let parent = dest.parent().context("백업 경로 오류")?;
    let tmp = parent.join(format!(".backup-{}.partial", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut source = File::open(src)?;
        let mut target = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
        let mut buffer = [0u8; 65536];
        loop {
            let n = source.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            target.write_all(&buffer[..n])?;
        }
        target.sync_all()?;
        drop(target);
        if file_digest(&tmp)? != *expected {
            bail!("백업 사본 검증 실패: 기존 사본을 보존합니다.");
        }
        fs::rename(&tmp, dest)?;
        sync_dir(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[derive(Deserialize)]
struct LedgerEntry {
    file: PathBuf,
    bytes: u64,
    sha256: String,
}

pub fn mirror_committed(src_root: &Path, dest_root: &Path) -> Result<usize> {
    if overlapping_storage(src_root, dest_root) {
        bail!("백업 경로가 원본과 겹칩니다.");
    }
    let mut count = 0;
    let ledger = src_root.join("durable-fragments.jsonl");
    if ledger.is_file() {
        // Freeze the ledger first. Do not copy a later ledger claiming un-copied fragments.
        let bytes = fs::read(&ledger)?;
        let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        for line in bytes[..end]
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
        {
            let entry: LedgerEntry = serde_json::from_slice(line).context("저장 확정 기록 손상")?;
            safe_relative(&entry.file)?;
            let source = src_root.join(&entry.file);
            if !fs::canonicalize(&source)?.starts_with(fs::canonicalize(src_root)?) {
                bail!("원본 파일 경로 이탈");
            }
            let dest = prepare_parent(dest_root, &entry.file)?;
            copy_verified(&source, &dest, &(entry.bytes, entry.sha256))?;
            count += 1;
        }
        atomic_write(&dest_root.join("durable-fragments.jsonl"), &bytes[..end])?;
    }
    for name in ["session.json", "result.json"] {
        let src = src_root.join(name);
        if src.is_file() {
            copy_verified(&src, &dest_root.join(name), &file_digest(&src)?)?;
            count += 1;
        }
    }
    for entry in fs::read_dir(src_root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("ledger-before-cleanup-")
            && name.ends_with(".jsonl")
            && entry.file_type()?.is_file()
        {
            copy_verified(
                &entry.path(),
                &dest_root.join(name.as_ref()),
                &file_digest(&entry.path())?,
            )?;
            count += 1;
        }
    }
    Ok(count)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackupStatus {
    pub state: String,
    pub checked_at: Option<String>,
    pub message: String,
    pub destination: Option<PathBuf>,
    pub verified_files: usize,
}

#[derive(Serialize, Deserialize)]
pub struct BackupSpec {
    pub job: crate::RecordingJob,
    pub root: PathBuf,
    pub device: Option<u64>,
    pub identity: Option<String>,
}

pub fn mirror_job(spec: &BackupSpec) -> Result<usize> {
    use fs2::FileExt;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(spec.job.output_dir.join("backup.lock"))?;
    lock.try_lock_exclusive()
        .context("이 작업의 이전 백업이 아직 진행 중입니다.")?;
    let actual = backup_device(&spec.root)?;
    if let Some(expected) = &spec.identity {
        if &read_identity(&spec.root)? != expected {
            bail!("다른 백업 볼륨입니다. 설정을 확인해 주세요.");
        }
    } else if spec.device.is_some() && actual != spec.device {
        bail!("백업 디스크가 다른 파일시스템으로 바뀌었습니다.");
    }
    let src_root = &spec.job.output_dir;
    if overlapping_storage(src_root, &spec.root) {
        bail!("백업 경로가 원본과 겹칩니다.");
    }
    // Root must already exist. An unplugged mount must never be recreated on the primary disk.
    let relative = PathBuf::from(&spec.job.video_id)
        .join(&spec.job.id)
        .join("recording.json");
    let metadata_dest = prepare_parent(&spec.root, &relative)?;
    let dest_root = metadata_dest.parent().context("백업 경로 오류")?;
    let mut count = 0;
    for entry in fs::read_dir(src_root)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            bail!("작업 폴더의 링크는 백업하지 않습니다.");
        }
        if entry.file_type()?.is_dir()
            && entry.file_name().to_string_lossy().starts_with("attempt-")
        {
            let dest = prepare_parent(
                dest_root,
                &PathBuf::from(entry.file_name()).join("session.json"),
            )?;
            count += mirror_committed(&entry.path(), dest.parent().unwrap())?;
        }
    }
    for output in &spec.job.outputs {
        let relative = output
            .path
            .strip_prefix(src_root)
            .context("결과 파일이 작업 폴더 밖에 있습니다.")?;
        if !fs::canonicalize(&output.path)?.starts_with(fs::canonicalize(src_root)?) {
            bail!("결과 파일 경로 이탈");
        }
        let dest = prepare_parent(dest_root, relative)?;
        let expected = match &output.sha256 {
            Some(hash) => (output.bytes, hash.clone()),
            None => file_digest(&output.path)?,
        };
        copy_verified(&output.path, &dest, &expected)?;
        count += 1;
    }
    for entry in fs::read_dir(src_root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("cleanup-")
            && name.ends_with(".json")
            && name != "cleanup-plan.json"
            && entry.file_type()?.is_file()
        {
            copy_verified(
                &entry.path(),
                &dest_root.join(name.as_ref()),
                &file_digest(&entry.path())?,
            )?;
            count += 1;
        }
    }
    if backup_device(&spec.root)? != actual {
        bail!("백업 중 디스크가 변경되었습니다.");
    }
    atomic_write(&metadata_dest, &serde_json::to_vec_pretty(&spec.job)?)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_size_corruption_is_repaired_without_touching_source() {
        let d = tempfile::tempdir().unwrap();
        let source = d.path().join("original");
        let dest = d.path().join("copy");
        fs::write(&source, b"good").unwrap();
        fs::write(&dest, b"evil").unwrap();
        copy_verified(&source, &dest, &file_digest(&source).unwrap()).unwrap();
        assert_eq!(fs::read(dest).unwrap(), b"good");
        assert_eq!(fs::read(source).unwrap(), b"good");
    }
    #[test]
    fn corrupt_or_missing_committed_source_never_publishes_ledger() {
        let source = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        fs::write(source.path().join("fragment"), b"good").unwrap();
        let hash = file_digest(&source.path().join("fragment")).unwrap().1;
        fs::write(
            source.path().join("durable-fragments.jsonl"),
            format!("{{\"file\":\"fragment\",\"bytes\":4,\"sha256\":\"{hash}\"}}\n"),
        )
        .unwrap();
        mirror_committed(source.path(), dest.path()).unwrap();
        let before = fs::read(dest.path().join("durable-fragments.jsonl")).unwrap();
        fs::write(source.path().join("fragment"), b"evil").unwrap();
        assert!(mirror_committed(source.path(), dest.path()).is_err());
        assert_eq!(fs::read(dest.path().join("fragment")).unwrap(), b"good");
        assert_eq!(
            fs::read(dest.path().join("durable-fragments.jsonl")).unwrap(),
            before
        );
        fs::remove_file(source.path().join("fragment")).unwrap();
        assert!(mirror_committed(source.path(), dest.path()).is_err());
    }
    #[test]
    fn partial_ledger_rows_are_not_certified() {
        let source = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        fs::write(source.path().join("durable-fragments.jsonl"), b"{\"file\":").unwrap();
        mirror_committed(source.path(), dest.path()).unwrap();
        assert_eq!(
            fs::read(dest.path().join("durable-fragments.jsonl")).unwrap(),
            b""
        );
    }
    #[test]
    fn overlap_checks_resolve_aliases_and_parent_components() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("rec")).unwrap();
        assert!(overlapping_storage(
            &d.path().join("rec"),
            &d.path().join("rec/../rec/backup")
        ));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.path().join("rec"), d.path().join("link")).unwrap();
            assert!(overlapping_storage(
                &d.path().join("rec"),
                &d.path().join("link/backup")
            ));
        }
        assert!(!overlapping_storage(
            &d.path().join("rec"),
            &d.path().join("backup")
        ));
    }
}
