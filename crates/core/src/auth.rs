use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

fn read_bounded(path: &Path, limit: u64) -> Result<String> {
    use std::io::Read;
    if !path.is_absolute() || !path.is_file() {
        bail!("인증 파일은 존재하는 절대 파일 경로여야 합니다.");
    }
    let mut bytes = vec![];
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("인증 파일 크기 제한 초과");
    }
    String::from_utf8(bytes).context("인증 파일은 UTF-8 텍스트여야 합니다.")
}

pub fn validate_po_token_file(path: &Path) -> Result<()> {
    load_po_token(path).map(|_| ())
}
pub fn load_po_token(path: &Path) -> Result<String> {
    let text = read_bounded(path, 8192)?;
    let rows: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .collect();
    if rows.len() != 1 {
        bail!("PO Token은 client.context+TOKEN 형식의 한 줄이어야 합니다.");
    }
    let token = rows[0];
    let (prefix, value) = token
        .split_once('+')
        .context("PO Token에 client.context+ 접두어가 필요합니다.")?;
    let (client, context) = prefix
        .split_once('.')
        .context("PO Token 컨텍스트가 없습니다.")?;
    if client.is_empty()
        || !client
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        || !matches!(context, "gvs" | "player" | "subs")
        || value.len() < 16
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'='))
    {
        bail!("PO Token 형식 오류: 예시는 mweb.gvs+TOKEN 입니다.");
    }
    Ok(token.into())
}

fn cookies_text(path: &Path) -> Result<String> {
    let text = read_bounded(path, 10 * 1024 * 1024)?;
    if text.contains('\0') {
        bail!("쿠키 파일은 텍스트여야 합니다.");
    }
    if !text.lines().next().is_some_and(|s| {
        s.starts_with("# Netscape HTTP Cookie File") || s.starts_with("# HTTP Cookie File")
    }) {
        bail!("Netscape cookies.txt 헤더가 필요합니다.");
    }
    for row in text
        .lines()
        .filter(|s| !s.is_empty() && (!s.starts_with('#') || s.starts_with("#HttpOnly_")))
    {
        let fields: Vec<_> = row.split('\t').collect();
        if fields.len() != 7
            || !matches!(fields[1], "TRUE" | "FALSE")
            || !matches!(fields[3], "TRUE" | "FALSE")
            || fields[4].parse::<i64>().is_err()
        {
            bail!("Netscape 쿠키 행 형식 오류");
        }
    }
    Ok(text)
}
pub fn validate_cookies_file(path: &Path) -> Result<()> {
    cookies_text(path).map(|_| ())
}

/// A per-invocation jar/config: concurrent extractors never rewrite the user's jar
/// or race over a shared extractor.conf. Private files live outside recording/backup roots.
pub struct ExtractorConfig {
    pub path: Option<PathBuf>,
    files: Vec<PathBuf>,
    secrets: Vec<String>,
}
impl ExtractorConfig {
    pub fn create(root: &Path, cookies: Option<&Path>, token: Option<&Path>) -> Result<Self> {
        let mut result = Self {
            path: None,
            files: vec![],
            secrets: vec![],
        };
        if cookies.is_none() && token.is_none() {
            return Ok(result);
        }
        let directory = root.join("auth");
        std::fs::create_dir_all(&directory)?;
        crate::private_dir(&directory)?;
        let mut config = String::new();
        if let Some(path) = cookies {
            let text = cookies_text(path)?;
            for line in text.lines() {
                if let Some(value) = line.split('\t').nth(6)
                    && !value.is_empty()
                {
                    result.secrets.push(value.into());
                }
            }
            let copy = directory.join(format!("{}.cookies", uuid::Uuid::new_v4()));
            crate::atomic_write(&copy, text.as_bytes())?;
            config.push_str(&format!(
                "--cookies {}\n",
                serde_json::to_string(&copy.to_string_lossy())?
            ));
            result.files.push(copy);
        }
        if let Some(path) = token {
            let value = load_po_token(path)?;
            let (prefix, secret) = value.split_once('+').unwrap();
            let client = prefix.split('.').next().unwrap();
            result.secrets.push(secret.into());
            config.push_str(&format!(
                "--extractor-args \"youtube:player_client={client};po_token={value}\"\n"
            ));
        }
        let path = directory.join(format!("{}.conf", uuid::Uuid::new_v4()));
        crate::atomic_write(&path, config.as_bytes())?;
        result.files.push(path.clone());
        result.path = Some(path);
        Ok(result)
    }
    pub fn scrub(&self, value: &str) -> String {
        let mut clean = value.to_owned();
        for secret in &self.secrets {
            clean = clean.replace(secret, "[REDACTED]");
        }
        crate::redact(&clean)
    }
}
impl Drop for ExtractorConfig {
    fn drop(&mut self) {
        for path in &self.files {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_config_injection_and_contextless_tokens() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("token");
        for invalid in [
            "just_a_raw_token_without_context",
            "web.gvs+abcdefghi;player_client=tv",
            "web.gvs+abcdefghijklmnop\n--exec=bad",
            "WEB.gvs+abcdefghijklmnop",
        ] {
            std::fs::write(&p, invalid).unwrap();
            assert!(load_po_token(&p).is_err());
        }
    }
    #[test]
    fn concurrent_configs_are_isolated_and_cleaned_without_rewriting_user_cookies() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("token");
        let cookie = d.path().join("cookies");
        std::fs::write(&p, "mweb.gvs+abcdefghijklmnop").unwrap();
        let jar =
            "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tCOOKIE_SECRET\n";
        std::fs::write(&cookie, jar).unwrap();
        let a = ExtractorConfig::create(d.path(), Some(&cookie), Some(&p)).unwrap();
        let b = ExtractorConfig::create(d.path(), Some(&cookie), Some(&p)).unwrap();
        assert_ne!(a.path, b.path);
        let path = a.path.clone().unwrap();
        assert!(
            !a.scrub("ERROR COOKIE_SECRET abcdefghijklmnop")
                .contains("COOKIE_SECRET")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
                0
            );
        }
        drop(a);
        assert!(!path.exists());
        assert!(b.path.as_ref().unwrap().exists());
        assert_eq!(std::fs::read_to_string(cookie).unwrap(), jar);
    }
}
