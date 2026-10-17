use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io,
    path::PathBuf,
};
use ytlr_core::{AppPaths, JobKind, JobState};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Ko,
}

impl Language {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "en" => Ok(Self::En),
            "ko" => Ok(Self::Ko),
            _ => Err("Unsupported language".into()),
        }
    }
}

#[derive(Deserialize)]
pub struct StatusSnapshot {
    jobs: Vec<StatusJob>,
}

#[derive(Deserialize)]
struct StatusJob {
    #[serde(default)]
    kind: JobKind,
    state: JobState,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct StatusSummary {
    pub recordings: usize,
    pub downloads: usize,
    pub preparing: usize,
    pub reconnecting: usize,
    pub finalizing: usize,
    pub waiting: usize,
}

impl StatusSummary {
    pub fn from_snapshot(snapshot: &StatusSnapshot) -> Self {
        let mut summary = Self::default();
        for job in &snapshot.jobs {
            match job.state {
                JobState::Recording if job.kind == JobKind::Download => summary.downloads += 1,
                JobState::Recording => summary.recordings += 1,
                JobState::Preparing => summary.preparing += 1,
                JobState::Reconnecting => summary.reconnecting += 1,
                JobState::Finalizing => summary.finalizing += 1,
                JobState::Queued | JobState::Waiting => summary.waiting += 1,
                _ => {}
            }
        }
        summary
    }

    pub fn label(&self, language: Language) -> String {
        let labels = match language {
            Language::En => [
                "Local server running",
                "Recording",
                "Downloading",
                "Preparing",
                "Reconnecting",
                "Finalizing",
                "Waiting",
            ],
            Language::Ko => [
                "로컬 서버 실행 중",
                "녹화",
                "다운로드",
                "준비",
                "재연결",
                "마무리",
                "대기",
            ],
        };
        let mut parts = vec![labels[0].to_string()];
        for (label, count) in labels[1..].iter().zip([
            self.recordings,
            self.downloads,
            self.preparing,
            self.reconnecting,
            self.finalizing,
            self.waiting,
        ]) {
            if count > 0 {
                parts.push(format!("{label} {count}"));
            }
        }
        parts.join(" · ")
    }
}

pub struct StatusMode {
    pub paths: AppPaths,
    pub wait_for_service: bool,
}

pub fn mode_paths(args: &[OsString]) -> Result<Option<StatusMode>, String> {
    if !args.iter().any(|arg| arg == "--service-status") {
        return Ok(None);
    }
    let wait_for_service = args.len() == 4 && args[3] == "--wait-for-service";
    if (args.len() != 3 && !wait_for_service)
        || args[0] != "--service-status"
        || args[1] != "--data-dir"
    {
        return Err("Usage: --service-status --data-dir ABSOLUTE_PATH [--wait-for-service]".into());
    }
    let root = PathBuf::from(&args[2]);
    if !root.is_absolute() {
        return Err("Service status requires an absolute data directory".into());
    }
    AppPaths::resolve(Some(root))
        .map(|paths| {
            Some(StatusMode {
                paths,
                wait_for_service,
            })
        })
        .map_err(|e| e.to_string())
}

fn lock_contended(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

pub fn try_status_lock(paths: &AppPaths) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(paths.root.join("service-status.lock"))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(e) if lock_contended(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn service_running(paths: &AppPaths) -> io::Result<bool> {
    if !paths.endpoint_file().try_exists()? {
        return Ok(false);
    }
    service_lock_held(paths)
}

fn service_lock_held(paths: &AppPaths) -> io::Result<bool> {
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(paths.root.join("service.lock"))
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    match file.try_lock_exclusive() {
        Ok(()) => Ok(false), // Closing the file immediately releases this probe lock.
        Err(e) if lock_contended(&e) => Ok(true),
        Err(e) => Err(e),
    }
}

async fn wait_for_service(paths: &AppPaths, idle_grace: std::time::Duration) -> io::Result<bool> {
    let started = std::time::Instant::now();
    let mut initializing = false;
    loop {
        if service_running(paths)? {
            return Ok(true);
        }
        let locked = service_lock_held(paths)?;
        if !locked && (initializing || started.elapsed() >= idle_grace) {
            return Ok(false);
        }
        initializing |= locked;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[derive(Default, Deserialize, Serialize)]
struct Preferences {
    language: Language,
}

pub fn save_language(paths: &AppPaths, language: Language) -> Result<(), String> {
    let bytes = serde_json::to_vec(&Preferences { language }).map_err(|e| e.to_string())?;
    ytlr_core::atomic_write(&paths.root.join("service-status-preferences.json"), &bytes)
        .map_err(|e| e.to_string())
}

pub fn load_language(paths: &AppPaths) -> Language {
    std::fs::read(paths.root.join("service-status-preferences.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Preferences>(&bytes).ok())
        .unwrap_or_default()
        .language
}

async fn read_status(client: &ytlr_service::client::Client) -> Result<StatusSnapshot, String> {
    client
        .get("/snapshot")
        .await
        .map_err(|_| "Unable to read local server status".into())
}

#[cfg(test)]
mod tests;

#[cfg(target_os = "macos")]
pub mod macos;
