use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub version: String,
}

impl AppPaths {
    pub fn resolve(explicit: Option<PathBuf>) -> Result<Self> {
        let root = explicit
            .or_else(|| std::env::var_os("YTLR_HOME").map(PathBuf::from))
            .unwrap_or(
                ProjectDirs::from("org", "YTLiveRecord", "YTLiveRecord")
                    .context("사용자 데이터 디렉터리를 찾을 수 없습니다.")?
                    .data_local_dir()
                    .to_owned(),
            );
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()?.join(root)
        };
        Ok(Self { root })
    }
    pub fn initialize(&self) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        private_dir(&self.root)?;
        fs::create_dir_all(self.tools_dir())?;
        Ok(())
    }
    pub fn database(&self) -> PathBuf {
        self.root.join("state.sqlite3")
    }
    pub fn endpoint_file(&self) -> PathBuf {
        self.root.join("service.json")
    }
    pub fn tools_dir(&self) -> PathBuf {
        self.root.join("tools")
    }
    pub fn endpoint(&self) -> Result<ServiceEndpoint> {
        Ok(serde_json::from_slice(
            &fs::read(self.endpoint_file()).context("녹화 서비스가 실행 중이 아닙니다.")?,
        )?)
    }
    pub fn default_settings(&self) -> crate::Settings {
        crate::Settings {
            storage_root: self.root.join("recordings"),
            max_recordings: 2,
            scan_interval_secs: 60,
            stall_timeout_secs: 120,
            min_free_bytes: 5 * 1024 * 1024 * 1024,
            max_retries: 8,
            live_from_start: false,
            notifications: true,
            close_to_tray: true,
            backup_root: None,
            backup_device: None,
            backup_identity: None,
            cookies_path: None,
            replica_remote: None,
            po_token_path: None,
        }
    }
}

pub fn private_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("파일의 상위 디렉터리가 없습니다.")?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        sync_dir(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

pub fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub fn ensure_storage(path: &Path, reserve: u64) -> Result<u64> {
    fs::create_dir_all(path).context("저장 폴더를 만들 수 없습니다.")?;
    let free = fs2::available_space(path)?;
    if free < reserve {
        bail!("저장 공간 부족: {:.1} GiB 남음", free as f64 / 1073741824.0);
    }
    let probe = path.join(format!(".ytlr-write-check-{}", uuid::Uuid::new_v4()));
    File::create(&probe)
        .context("저장 폴더에 쓸 수 없습니다.")?
        .sync_all()?;
    fs::remove_file(probe)?;
    Ok(free)
}

pub fn available_space(path: &Path) -> Result<u64> {
    Ok(fs2::available_space(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_replacement_and_capacity_checks() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state.json");
        atomic_write(&p, b"first").unwrap();
        atomic_write(&p, b"second").unwrap();
        assert_eq!(fs::read(p).unwrap(), b"second");
        assert!(ensure_storage(d.path(), u64::MAX).is_err());
    }
}
