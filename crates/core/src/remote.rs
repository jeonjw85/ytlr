use crate::{AppPaths, atomic_write};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Remote {
    pub id: String,
    pub name: String,
    pub ssh: String,
    #[serde(default)]
    pub identity: Option<PathBuf>,
    pub remote_data_dir: PathBuf,
    #[serde(default)]
    pub known_hosts: Option<PathBuf>,
    #[serde(default)]
    pub executable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub user: String,
    pub host: String,
    pub port: u16,
}

pub fn parse_ssh_target(input: &str) -> Result<SshTarget> {
    let input = input.trim();
    if input.is_empty()
        || input.contains(char::is_whitespace)
        || input.contains('/')
        || input.contains('\\')
        || input.contains('@') && input.matches('@').count() != 1
    {
        bail!("SSH 대상은 user@host 또는 user@host:port 형식입니다.");
    }
    let (user, rest) = input
        .split_once('@')
        .context("user@host 형식이 필요합니다.")?;
    if user.is_empty()
        || !user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        bail!("SSH 사용자 이름이 올바르지 않습니다.");
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (
            host,
            port.parse::<u16>()
                .map_err(|_| anyhow::anyhow!("SSH 포트가 올바르지 않습니다."))?,
        ),
        Some(_) => bail!("IPv6 SSH 대상은 아직 지원하지 않습니다."),
        None => (rest, 22u16),
    };
    if !(1..=65535).contains(&port)
        || host.is_empty()
        || host.starts_with('-')
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    {
        bail!("SSH 호스트가 올바르지 않습니다.");
    }
    Ok(SshTarget {
        user: user.into(),
        host: host.into(),
        port,
    })
}

impl Remote {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty()
            || self.name == "local"
            || self
                .name
                .chars()
                .any(|c| "/\\".contains(c) || c.is_control())
        {
            bail!("원격 이름을 확인해 주세요.");
        }
        parse_ssh_target(&self.ssh)?;
        let dir = self.remote_data_dir.to_string_lossy();
        if dir.contains(['\0', '\n', '\r']) {
            bail!("원격 경로에 제어 문자를 사용할 수 없습니다.");
        }
        if !self.remote_data_dir.is_absolute() && !dir.starts_with('/') {
            bail!("원격 데이터 경로는 절대 경로여야 합니다.");
        }
        if let Some(identity) = &self.identity
            && (!identity.is_absolute() || !identity.is_file())
        {
            bail!("SSH 키 파일을 찾을 수 없습니다.");
        }
        if let Some(hosts) = &self.known_hosts
            && (!hosts.is_absolute() || !hosts.is_file())
        {
            bail!("known_hosts 파일을 찾을 수 없습니다.");
        }
        if self
            .executable
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.contains(['\0', '\n', '\r']))
        {
            bail!("원격 실행 파일 경로 오류");
        }
        Ok(())
    }
}

impl AppPaths {
    pub fn remotes_file(&self) -> PathBuf {
        self.root.join("remotes.json")
    }
    pub fn remotes(&self) -> Result<Vec<Remote>> {
        let path = self.remotes_file();
        if !path.exists() {
            return Ok(vec![]);
        }
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }
    pub fn save_remotes(&self, remotes: &[Remote]) -> Result<()> {
        for remote in remotes {
            remote.validate()?;
        }
        self.initialize()?;
        atomic_write(&self.remotes_file(), &serde_json::to_vec_pretty(remotes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_targets_reject_credentials_in_url() {
        let ok = parse_ssh_target("ytlr@studio.local:2222").unwrap();
        assert_eq!(ok.user, "ytlr");
        assert_eq!(ok.host, "studio.local");
        assert_eq!(ok.port, 2222);
        assert_eq!(parse_ssh_target("ytlr@192.168.1.8").unwrap().port, 22);
        for bad in [
            "ssh://ytlr:hunter2@host",
            "ytlr@host/tmp",
            "ytlr@host:0",
            "@host",
            "ytlr@",
            "ytlr host",
            "ytlr@host@x",
        ] {
            assert!(parse_ssh_target(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn remotes_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(dir.path().to_owned())).unwrap();
        let remote = Remote {
            id: "r1".into(),
            name: "studio".into(),
            ssh: "ytlr@studio.local".into(),
            identity: None,
            remote_data_dir: "/var/lib/ytlr".into(),
            known_hosts: None,
            executable: None,
        };
        paths.save_remotes(std::slice::from_ref(&remote)).unwrap();
        assert_eq!(paths.remotes().unwrap(), vec![remote]);
    }
}
