use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use ytlr_core::{AppPaths, ToolStatus, atomic_write};

pub const BUNDLE_VERSION: &str = "2026-09-21.2";
const YTDLP: &str = "2026.08.19";
const DENO: &str = "v2.9.7";

#[derive(Clone)]
pub struct Tools {
    pub paths: AppPaths,
}

#[derive(Debug, Serialize, Deserialize)]
struct ActiveBundle {
    current: String,
    previous: Option<String>,
}

#[derive(Clone)]
pub struct Asset {
    pub name: &'static str,
    pub url: String,
    pub sha256: &'static str,
    pub zip: bool,
    pub tar_xz: bool,
}

pub fn assets() -> Result<Vec<Asset>> {
    let (yt, ytsha, target, denosha, ff, ffsha, probesha) =
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => (
                "yt-dlp_macos",
                "0f192b7ec147ab6288885d6351d9ab67367640029b4377576ef46dd79cf7b202",
                "aarch64-apple-darwin",
                "5cd46d6268f6f78f5d88bdc7159d20bd44cdaa4b3303474839f87ec6fe7ae25c",
                "darwin-arm64",
                "a90e3db6a3fd35f6074b013f948b1aa45b31c6375489d39e572bea3f18336584",
                "bb2db6f5d8cef919da12fbf592119a987202a8c060a886f3cab091f9cab90b64",
            ),
            ("macos", "x86_64") => (
                "yt-dlp_macos",
                "0f192b7ec147ab6288885d6351d9ab67367640029b4377576ef46dd79cf7b202",
                "x86_64-apple-darwin",
                "95daaff11c116a52ad54785e7914c8e9c9cdcaba793c5ed929c74ca2d8e6259a",
                "darwin-x64",
                "ebdddc936f61e14049a2d4b549a412b8a40deeff6540e58a9f2a2da9e6b18894",
                "fa3add0ce901f7241abe0dfc0155d958fc834aca3f8ce61f87cc712ae669c1e0",
            ),
            ("linux", "aarch64") => (
                "yt-dlp_linux_aarch64",
                "b16e4dab368a816cd05d477d698a605a6ae87ccee1c8ffd38fa21d7254141fcc",
                "aarch64-unknown-linux-gnu",
                "c832298b1ad4422481334855f6003e0f54145762c5a134f20a489511d2f65bbf",
                "linux-arm64",
                "6bb182d0d75d23028db82e9e4f723ca69b853d055698486e6984ddb2c06fb8ce",
                "d17ae9b4c297d48e2521ba14e417bb0537c6ff77c584cdbcd6bb0d8d0307a2e8",
            ),
            ("linux", "x86_64") => (
                "yt-dlp_linux",
                "58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a",
                "x86_64-unknown-linux-gnu",
                "c6527f24f4b16031d3ae4fa9f658d5f11534c8d84ce7dc8502420280919c3490",
                "linux-x64",
                "e7e7fb30477f717e6f55f9180a70386c62677ef8a4d4d1a5d948f4098aa3eb99",
                "4f231a1960d83e403d08f7971e271707bec278a9ae18e21b8b5b03186668450d",
            ),
            ("windows", "x86_64") => (
                "yt-dlp.exe",
                "66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a",
                "x86_64-pc-windows-msvc",
                "a0c3101b4158d1dfb7d6a78a7bf0f3de80c96bb423c152beec8beb22786f2238",
                "win32-x64",
                "04e1307997530f9cf2fe35cba2ca7e8875ca91da02f89d6c7243df819c94ad00",
                "3a7e2dc003dc2cd1472827e4c7c4f056ae1ae0ae7c5bbc580c99b49827351ba4",
            ),
            _ => bail!("이 플랫폼의 자동 엔진 설치는 아직 지원되지 않습니다."),
        };
    let mut result = vec![
        Asset {
            name: "yt-dlp",
            url: format!("https://github.com/yt-dlp/yt-dlp/releases/download/{YTDLP}/{yt}"),
            sha256: ytsha,
            zip: false,
            tar_xz: false,
        },
        Asset {
            name: "deno",
            url: format!(
                "https://github.com/denoland/deno/releases/download/{DENO}/deno-{target}.zip"
            ),
            sha256: denosha,
            zip: true,
            tar_xz: false,
        },
        Asset {
            name: "ffmpeg",
            url: format!(
                "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffmpeg-{ff}"
            ),
            sha256: ffsha,
            zip: false,
            tar_xz: false,
        },
        Asset {
            name: "ffprobe",
            url: format!(
                "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffprobe-{ff}"
            ),
            sha256: probesha,
            zip: false,
            tar_xz: false,
        },
    ];
    // The third-party macOS builds contain --enable-nonfree. macOS bundles use
    // our official-source build; standalone users may supply a local free build.
    if cfg!(target_os = "macos") {
        result.retain(|a| !a.name.starts_with("ff"));
    }
    if cfg!(target_os = "linux") {
        result.retain(|a| !a.name.starts_with("ff"));
        let (arch, hash) = match std::env::consts::ARCH {
            "aarch64" => (
                "linuxarm64",
                "1c4187e0b8db8e89ffed67aafcec1cffd29d0007d9a1aba34500e609ba1aeb25",
            ),
            "x86_64" => (
                "linux64",
                "dfe7728e01099e22a6fc7a65355ed4f1b65468e1dbe0015f07c45286b486cc59",
            ),
            _ => bail!("지원되지 않는 Linux 아키텍처"),
        };
        result.push(Asset { name: "ffmpeg", url: format!("https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-09-21-13-55/ffmpeg-n8.1.3-{arch}-gpl-8.1.tar.xz"), sha256: hash, zip: false, tar_xz: true });
    }
    Ok(result)
}

pub fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

impl Tools {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths }
    }
    pub fn locate(&self, name: &str) -> Option<PathBuf> {
        if let Some(p) = std::env::var_os(format!(
            "YTLR_{}",
            name.replace('-', "_").to_ascii_uppercase()
        ))
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        {
            return Some(p);
        }
        let exe = executable_name(name);
        if let Ok(raw) = std::fs::read(self.paths.tools_dir().join("active.json"))
            && let Ok(bundle) = serde_json::from_slice::<ActiveBundle>(&raw)
        {
            let p = self.paths.tools_dir().join(bundle.current).join(&exe);
            if p.is_file() {
                return Some(p);
            }
        }
        if let Some(root) = std::env::var_os("YTLR_BUNDLED_TOOLS") {
            let path = PathBuf::from(root).join(&exe);
            if path.is_file() {
                return Some(path);
            }
        }
        if let Ok(current) = std::env::current_exe()
            && let Some(parent) = current.parent()
        {
            for base in [
                parent.join("tools"),
                parent.join("../Resources/tools"),
                parent.to_path_buf(),
            ] {
                let p = base.join(&exe);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|p| p.join(&exe))
                .find(|p| p.is_file())
        })
    }
    pub fn require(&self, name: &str) -> Result<PathBuf> {
        self.locate(name).with_context(|| format!("{name}이 없습니다. 설정에서 녹화 엔진을 설치하거나 `ytlr tools install`을 실행하세요."))
    }
    pub async fn doctor(&self) -> Vec<ToolStatus> {
        let mut result = vec![];
        for name in ["yt-dlp", "deno", "ffmpeg", "ffprobe"] {
            let path = self.locate(name);
            let mut status = ToolStatus {
                name: name.into(),
                managed: path
                    .as_ref()
                    .is_some_and(|p| p.starts_with(self.paths.tools_dir())),
                path: path.clone(),
                version: None,
                error: None,
            };
            if let Some(path) = path {
                match tool_version(&path, name).await {
                    Ok(v) => {
                        if cfg!(target_os = "linux") && v.contains("johnvansickle") {
                            status.error = Some("이 빌드는 DNS 해석 오류가 있습니다. 검증된 엔진을 다시 설치해 주세요.".into());
                        }
                        status.version = Some(v);
                    }
                    Err(e) => status.error = Some(e.to_string()),
                }
            } else {
                status.error = Some("설치 필요".into());
            }
            result.push(status);
        }
        result
    }
    /// Stage a complete pinned bundle, then atomically switch. Existing binaries stay intact.
    pub async fn install(&self, progress: impl Fn(String) + Send + Sync) -> Result<()> {
        let bundle = format!("{}-{}", BUNDLE_VERSION, uuid::Uuid::new_v4());
        let dest = self.paths.tools_dir().join(&bundle);
        tokio::fs::create_dir_all(&dest).await?;
        if cfg!(target_os = "macos") {
            for name in ["ffmpeg", "ffprobe"] {
                let source = self.redistributable_tool(name).await?;
                tokio::fs::copy(source, dest.join(executable_name(name))).await?;
            }
        }
        let client = reqwest::Client::builder()
            .user_agent("YTLiveRecord/0.1")
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(900))
            .build()?;
        for asset in assets()? {
            progress(format!("{} 다운로드 및 SHA-256 검증 중", asset.name));
            let response = client.get(&asset.url).send().await?.error_for_status()?;
            let partial = dest.join(format!("{}.download", asset.name));
            let mut file = tokio::fs::File::create(&partial).await?;
            let mut hash = Sha256::new();
            let mut stream = response.bytes_stream();
            let mut size = 0u64;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                size += chunk.len() as u64;
                if size > 512 * 1024 * 1024 {
                    bail!("엔진 다운로드 크기 제한 초과");
                }
                hash.update(&chunk);
                file.write_all(&chunk).await?;
            }
            file.sync_all().await?;
            drop(file);
            if hex::encode(hash.finalize()) != asset.sha256 {
                bail!("{} 체크섬이 일치하지 않습니다.", asset.name);
            }
            if asset.tar_xz {
                let input = partial.clone();
                let folder = dest.clone();
                tokio::task::spawn_blocking(move || -> Result<()> {
                    let reader = xz2::read::XzDecoder::new(std::fs::File::open(input)?);
                    let mut archive = tar::Archive::new(reader);
                    let mut found = std::collections::HashSet::new();
                    for entry in archive.entries()? {
                        let mut entry = entry?;
                        let name = entry
                            .path()?
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if !matches!(name.as_str(), "ffmpeg" | "ffprobe")
                            || !entry.header().entry_type().is_file()
                        {
                            continue;
                        }
                        if entry.size() > 512 * 1024 * 1024 || !found.insert(name.clone()) {
                            bail!("잘못된 FFmpeg 아카이브 구성");
                        }
                        let target = folder.join(&name);
                        let mut file = std::fs::OpenOptions::new()
                            .create_new(true)
                            .write(true)
                            .open(&target)?;
                        std::io::copy(&mut entry, &mut file)?;
                        file.sync_all()?;
                        make_executable(&target)?;
                    }
                    if found.len() != 2 {
                        bail!("FFmpeg 아카이브에 필요한 실행 파일이 없습니다.");
                    }
                    Ok(())
                })
                .await??;
                for name in ["ffmpeg", "ffprobe"] {
                    tool_version(&dest.join(name), name).await?;
                }
                tokio::fs::remove_file(partial).await?;
                continue;
            }
            let target = dest.join(executable_name(asset.name));
            if asset.zip {
                let input = partial.clone();
                let target = target.clone();
                tokio::task::spawn_blocking(move || -> Result<()> {
                    let mut zip = zip::ZipArchive::new(std::fs::File::open(input)?)?;
                    let mut entry = zip.by_name(&executable_name("deno"))?;
                    let mut bytes = Vec::new();
                    (&mut entry)
                        .take(512 * 1024 * 1024)
                        .read_to_end(&mut bytes)?;
                    atomic_write(&target, &bytes)
                })
                .await??;
                tokio::fs::remove_file(partial).await?;
            } else {
                tokio::fs::rename(partial, &target).await?;
            }
            make_executable(&target)?;
            tool_version(&target, asset.name).await?;
        }
        let pointer = self.paths.tools_dir().join("active.json");
        let previous = std::fs::read(&pointer)
            .ok()
            .and_then(|r| serde_json::from_slice::<ActiveBundle>(&r).ok())
            .map(|b| b.current);
        atomic_write(
            &pointer,
            &serde_json::to_vec(&ActiveBundle {
                current: bundle,
                previous,
            })?,
        )?;
        progress("녹화 엔진 설치 완료".into());
        Ok(())
    }
    pub fn rollback(&self) -> Result<()> {
        let pointer = self.paths.tools_dir().join("active.json");
        let bundle: ActiveBundle = serde_json::from_slice(&std::fs::read(&pointer)?)?;
        let previous = bundle.previous.context("이전 엔진 버전이 없습니다.")?;
        atomic_write(
            &pointer,
            &serde_json::to_vec(&ActiveBundle {
                current: previous,
                previous: Some(bundle.current),
            })?,
        )
    }

    pub async fn redistributable_tool(&self, name: &str) -> Result<PathBuf> {
        let mut candidates = vec![];
        if let Some(path) = self.locate(name) {
            candidates.push(path);
        }
        if let Some(root) = std::env::var_os("YTLR_BUNDLED_TOOLS") {
            candidates.push(PathBuf::from(root).join(executable_name(name)));
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(parent) = exe.parent()
        {
            candidates.extend([
                parent.join("tools").join(name),
                parent.join("../Resources/tools").join(name),
            ]);
        }
        if let Some(paths) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&paths).map(|p| p.join(executable_name(name))));
        }
        for path in candidates.into_iter().filter(|p| p.is_file()) {
            let mut cmd = tokio::process::Command::new(&path);
            cmd.arg("-version").kill_on_drop(true);
            crate::process::hide_console(&mut cmd);
            if let Ok(Ok(output)) =
                tokio::time::timeout(Duration::from_secs(20), cmd.output()).await
                && output.status.success()
                && !String::from_utf8_lossy(&output.stdout).contains("--enable-nonfree")
                && !String::from_utf8_lossy(&output.stderr).contains("--enable-nonfree")
            {
                return Ok(path);
            }
        }
        bail!(
            "배포 가능한 {name}이 필요합니다. macOS 앱 내장 도구 또는 직접 설치한 FFmpeg를 사용하세요."
        )
    }
}

pub fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

async fn tool_version(path: &Path, name: &str) -> Result<String> {
    let mut cmd = tokio::process::Command::new(path);
    cmd.arg(if name.starts_with("ff") {
        "-version"
    } else {
        "--version"
    })
    .kill_on_drop(true);
    crate::process::hide_console(&mut cmd);
    let output = crate::process::output_timeout(cmd, Duration::from_secs(20))
        .await
        .context("엔진 응답 시간 초과")?;
    if !output.status.success() {
        bail!("{name} 실행 실패");
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("unknown")
        .chars()
        .take(150)
        .collect())
}
