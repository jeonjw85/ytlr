use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use url::Url;

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Waiting,
    Preparing,
    Recording,
    Reconnecting,
    Finalizing,
    Completed,
    Partial,
    Stopped,
    Failed,
}

impl JobState {
    pub fn active(&self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Recording | Self::Reconnecting | Self::Finalizing
        )
    }
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Partial | Self::Stopped | Self::Failed
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub storage_root: PathBuf,
    pub max_recordings: usize,
    pub scan_interval_secs: u64,
    pub stall_timeout_secs: u64,
    pub min_free_bytes: u64,
    pub max_retries: u32,
    pub live_from_start: bool,
    pub notifications: bool,
    pub close_to_tray: bool,
    #[serde(default)]
    pub backup_root: Option<PathBuf>,
    #[serde(default)]
    pub backup_device: Option<u64>,
    #[serde(default)]
    pub backup_identity: Option<String>,
    #[serde(default)]
    pub cookies_path: Option<PathBuf>,
    #[serde(default)]
    pub replica_remote: Option<String>,
    #[serde(default)]
    pub po_token_path: Option<PathBuf>,
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        if !self.storage_root.is_absolute() {
            bail!("저장 경로는 절대 경로여야 합니다.");
        }
        if let Some(backup) = &self.backup_root {
            if !backup.is_absolute() {
                bail!("백업 경로는 절대 경로여야 합니다.");
            }
            if crate::overlapping_storage(&self.storage_root, backup) {
                bail!("백업 폴더는 녹화 저장 폴더와 겹치면 안 됩니다.");
            }
        }
        if let Some(cookies) = &self.cookies_path {
            crate::validate_cookies_file(cookies)?;
        }
        if let Some(token) = &self.po_token_path {
            crate::validate_po_token_file(token)?;
        }
        if !(1..=16).contains(&self.max_recordings) {
            bail!("동시 녹화 수는 1~16이어야 합니다.");
        }
        if !(15..=3600).contains(&self.scan_interval_secs) {
            bail!("감시 주기는 15~3600초여야 합니다.");
        }
        if !(30..=1800).contains(&self.stall_timeout_secs) {
            bail!("수신 정지 제한은 30~1800초여야 합니다.");
        }
        if self.max_retries > 100 {
            bail!("재시도 상한은 100입니다.");
        }
        if let Some(name) = &self.replica_remote
            && (name.trim().is_empty()
                || name.len() > 64
                || name.chars().any(|c| "/\\".contains(c) || c.is_whitespace()))
        {
            bail!("이중 녹화 원격 이름을 확인해 주세요.");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingJob {
    pub id: String,
    pub url: String,
    pub video_id: String,
    pub title: String,
    pub channel: String,
    pub channel_id: Option<String>,
    pub state: JobState,
    pub message: String,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub last_media_at: Option<String>,
    pub output_dir: PathBuf,
    pub format: String,
    pub bytes: u64,
    pub media_seconds: f64,
    pub attempt: u32,
    pub retries: u32,
    pub priority: i32,
    pub live_from_start: bool,
    pub continuity_uncertain: bool,
    pub stop_requested: bool,
    pub outputs: Vec<MediaOutput>,
    #[serde(default)]
    pub attempts: Vec<crate::CaptureAttempt>,
    #[serde(default)]
    pub gaps: Vec<crate::TimelineGap>,
    #[serde(default)]
    pub backup: crate::BackupStatus,
    #[serde(default)]
    pub replica: Option<ReplicaStatus>,
    #[serde(default)]
    pub replica_origin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicaStatus {
    pub target: String,
    pub request_id: String,
    pub state: String,
    pub remote_job_id: Option<String>,
    pub remote_state: Option<JobState>,
    pub attempts: u32,
    pub checked_at: Option<String>,
    pub next_attempt_at: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaOutput {
    pub path: PathBuf,
    pub bytes: u64,
    pub duration: f64,
    pub has_video: bool,
    pub has_audio: bool,
    pub verification: String,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub url: String,
    pub name: String,
    pub enabled: bool,
    pub priority: i32,
    pub last_checked_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobEvent {
    pub id: i64,
    pub job_id: String,
    pub at: String,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordRequest {
    pub url: String,
    #[serde(default)]
    pub live_from_start: Option<bool>,
    #[serde(default)]
    pub priority: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddChannelRequest {
    pub url: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub priority: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStatus {
    pub name: String,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub managed: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: String,
    pub jobs: Vec<RecordingJob>,
    pub channels: Vec<Channel>,
    pub settings: Settings,
    pub free_bytes: Option<u64>,
    pub tools: Vec<ToolStatus>,
    pub installing_tools: bool,
    pub tool_message: Option<String>,
    pub replica_targets: Vec<String>,
}

pub fn video_url(input: &str) -> Result<(String, String)> {
    let u = youtube_url(input)?;
    let host = u.host_str().unwrap_or_default();
    let id = if host == "youtu.be" {
        u.path().trim_matches('/').to_string()
    } else if u.path() == "/watch" {
        u.query_pairs()
            .find(|(k, _)| k == "v")
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default()
    } else {
        let mut parts = u.path().trim_matches('/').split('/');
        match parts.next() {
            Some("live" | "shorts" | "embed") => parts.next().unwrap_or_default().to_string(),
            _ => String::new(),
        }
    };
    if id.len() != 11
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        bail!("유효한 유튜브 영상 URL을 입력해 주세요. 채널은 채널 화면에서 추가합니다.");
    }
    Ok((format!("https://www.youtube.com/watch?v={id}"), id))
}

pub fn channel_url(input: &str) -> Result<String> {
    let u = youtube_url(input)?;
    if u.host_str() == Some("youtu.be") {
        bail!("채널 URL을 입력해 주세요.");
    }
    let parts: Vec<_> = u.path().trim_matches('/').split('/').collect();
    let base = match parts.as_slice() {
        [handle, ..] if handle.starts_with('@') && handle.len() > 1 => format!("/{handle}"),
        [kind @ ("channel" | "c" | "user"), id, ..] if !id.is_empty() => format!("/{kind}/{id}"),
        _ => bail!("@핸들 또는 /channel/ID 형태의 채널 URL을 입력해 주세요."),
    };
    Ok(format!("https://www.youtube.com{base}/streams"))
}

fn youtube_url(input: &str) -> Result<Url> {
    let u = Url::parse(input.trim())?;
    if !matches!(u.scheme(), "https" | "http")
        || !matches!(
            u.host_str(),
            Some("youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be")
        )
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
    {
        bail!("유튜브 HTTPS URL을 입력해 주세요.");
    }
    Ok(u)
}

/// Logs are exportable. Never persist signed media URLs, cookies or raw extractor JSON here.
pub fn redact(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("cookie:") || lower.contains("authorization:") || lower.contains("po_token=")
    {
        return "[인증 정보 포함 메시지 생략]".into();
    }
    message
        .split_whitespace()
        .map(|word| {
            if word.contains("://") {
                "[URL]".to_string()
            } else if word.to_ascii_lowercase().contains("token=")
                || word.to_ascii_lowercase().contains("po_token")
                || word.to_ascii_lowercase().contains("cookie:")
            {
                "[REDACTED]".to_string()
            } else {
                word.chars().take(500).collect()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(2000)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExtractorConfig, load_po_token, validate_cookies_file, validate_po_token_file};
    #[test]
    fn canonical_video_identity_and_untrusted_urls() {
        assert_eq!(
            video_url("https://youtu.be/abcdefghijk?t=2").unwrap(),
            video_url("https://www.youtube.com/live/abcdefghijk").unwrap()
        );
        for input in [
            "file:///etc/passwd",
            "https://youtube.com.evil.test/watch?v=abcdefghijk",
            "https://youtube.com@evil.test/watch?v=abcdefghijk",
            "https://www.youtube.com/watch?v=../../x",
            "https://youtu.be/abcdefghijk:22",
        ] {
            assert!(video_url(input).is_err(), "{input}");
        }
    }
    #[test]
    fn channels_normalize_to_streams() {
        assert_eq!(
            channel_url("https://youtube.com/@example/live").unwrap(),
            "https://www.youtube.com/@example/streams"
        );
        assert!(channel_url("https://youtube.com/watch?v=abcdefghijk").is_err());
    }
    #[test]
    fn signed_urls_do_not_enter_logs() {
        let text = redact("ERROR https://r1.googlevideo.com/videoplayback?secret=abc token=secret");
        assert!(!text.contains("secret"));
    }

    #[test]
    fn cookies_file_must_be_netscape_text() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.txt");
        assert!(validate_cookies_file(&missing).is_err());
        let relative = std::path::Path::new("cookies.txt");
        assert!(validate_cookies_file(relative).is_err());
        let binary = dir.path().join("cookies.bin");
        std::fs::write(&binary, [0u8, 1, 2]).unwrap();
        assert!(validate_cookies_file(&binary).is_err());
        let ok = dir.path().join("cookies.txt");
        std::fs::write(
            &ok,
            "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tvalue\n",
        )
        .unwrap();
        assert!(validate_cookies_file(&ok).is_ok());
    }

    #[test]
    fn po_token_file_rejects_empty_and_binary() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("token.txt");
        std::fs::write(&empty, "").unwrap();
        assert!(validate_po_token_file(&empty).is_err());
        let ok = dir.path().join("po.txt");
        std::fs::write(&ok, "# comment\nmweb.gvs+abcdefghijklmnopqrstuvwxyz\n").unwrap();
        assert!(validate_po_token_file(&ok).is_ok());
        assert!(!load_po_token(&ok).unwrap().contains("comment"));
        let conf = ExtractorConfig::create(dir.path(), None, Some(&ok)).unwrap();
        let written = std::fs::read_to_string(conf.path.as_ref().unwrap()).unwrap();
        assert!(written.contains("po_token="));
        assert!(!redact(&written).contains("abcdefghijklmnopqrstuvwxyz"));
    }
}
