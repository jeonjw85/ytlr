use crate::Bridge;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{fs::File, sync::Mutex, time::Duration};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::{Update, UpdaterExt};

const RELEASES: &str = "https://github.com/jeonjw85/ytlr/releases";
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

fn lock_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

#[derive(Clone, Serialize)]
pub struct UpdateStatus {
    current_version: String,
    available: bool,
    auto_check: bool,
    phase: String,
    version: Option<String>,
    notes: Option<String>,
    checked_at: Option<String>,
    downloaded: u64,
    total: Option<u64>,
    message: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Preferences {
    auto_check: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self { auto_check: true }
    }
}

struct Pending {
    update: Update,
    bytes: Option<Vec<u8>>,
}
pub struct AppUpdates {
    status: Mutex<UpdateStatus>,
    operation: tokio::sync::Mutex<()>,
    pending: tokio::sync::Mutex<Option<Pending>>,
}
impl AppUpdates {
    pub fn installing(&self) -> bool {
        self.status().phase == "installing"
    }
    fn status(&self) -> UpdateStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn change(&self, change: impl FnOnce(&mut UpdateStatus)) {
        change(&mut self.status.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

pub fn setup(app: &AppHandle) {
    let bridge = app.state::<Bridge>();
    let preferences: Preferences =
        std::fs::read(bridge.paths.root.join("updater-preferences.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
    let key_configured = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|v| v.get("pubkey"))
        .and_then(|v| v.as_str())
        .is_some_and(|key| !key.trim().is_empty());
    let reason = if cfg!(debug_assertions) {
        Some("개발 빌드에서는 자동 업데이트를 사용하지 않습니다.")
    } else if cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_none() {
        Some("Linux 자동 업데이트는 AppImage에서 지원됩니다.")
    } else if !key_configured {
        Some("업데이트 서명 공개키가 설정되지 않은 빌드입니다.")
    } else {
        None
    };
    app.manage(AppUpdates {
        status: Mutex::new(UpdateStatus {
            current_version: app.package_info().version.to_string(),
            available: reason.is_none(),
            auto_check: preferences.auto_check,
            phase: "idle".into(),
            version: None,
            notes: None,
            checked_at: None,
            downloaded: 0,
            total: None,
            message: reason.map(str::to_owned),
        }),
        operation: tokio::sync::Mutex::new(()),
        pending: tokio::sync::Mutex::new(None),
    });
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            let status = handle.state::<AppUpdates>().status();
            if status.available && status.auto_check {
                let _ = perform_check(&handle).await;
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

#[tauri::command]
pub fn update_status(updates: State<'_, AppUpdates>) -> UpdateStatus {
    updates.status()
}

#[tauri::command]
pub fn set_update_preferences(
    bridge: State<'_, Bridge>,
    updates: State<'_, AppUpdates>,
    auto_check: bool,
) -> Result<UpdateStatus, String> {
    let preferences = Preferences { auto_check };
    ytlr_core::atomic_write(
        &bridge.paths.root.join("updater-preferences.json"),
        &serde_json::to_vec(&preferences).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    updates.change(|s| s.auto_check = auto_check);
    Ok(updates.status())
}

#[tauri::command]
pub async fn check_app_update(app: AppHandle) -> Result<UpdateStatus, String> {
    perform_check(&app).await
}

async fn perform_check(app: &AppHandle) -> Result<UpdateStatus, String> {
    let updates = app.state::<AppUpdates>();
    let _operation = updates
        .operation
        .try_lock()
        .map_err(|_| "업데이트 작업이 이미 진행 중입니다.")?;
    if !updates.status().available {
        return Ok(updates.status());
    }
    updates.change(|s| {
        s.phase = "checking".into();
        s.message = None;
    });
    let result = async {
        app.updater_builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|e| e.to_string())
    }
    .await;
    match result {
        Ok(update) => {
            let mut pending = updates.pending.lock().await;
            // Keep an already verified download when a periodic check finds the same release.
            if pending.as_ref().map(|p| &p.update.version) != update.as_ref().map(|u| &u.version) {
                *pending = update.clone().map(|update| Pending {
                    update,
                    bytes: None,
                });
            }
            updates.change(|s| {
                s.checked_at = Some(ytlr_core::now());
                s.version = update.as_ref().map(|u| u.version.clone());
                s.notes = update.and_then(|u| u.body);
                s.phase = if pending.as_ref().is_some_and(|p| p.bytes.is_some()) {
                    "ready"
                } else if pending.is_some() {
                    "available"
                } else {
                    "current"
                }
                .into();
                if pending.is_none() {
                    s.downloaded = 0;
                    s.total = None;
                }
            });
        }
        Err(error) => updates.change(|s| {
            s.phase = "error".into();
            s.message = Some(format!("업데이트 확인 실패: {}", ytlr_core::redact(&error)));
        }),
    }
    Ok(updates.status())
}

/// Retain the exclusive service lock until installation has finished. This also
/// prevents CLI/another GUI from starting the old sidecar while it is replaced.
struct PausedService {
    _lock: File,
    was_running: bool,
}

async fn stop_idle_service(bridge: &Bridge) -> Result<PausedService, String> {
    bridge.paths.initialize().map_err(|e| e.to_string())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(bridge.paths.root.join("service.lock"))
        .map_err(|e| e.to_string())?;
    match lock.try_lock_exclusive() {
        Ok(()) => {
            return Ok(PausedService {
                _lock: lock,
                was_running: false,
            });
        }
        Err(e) if lock_contended(&e) => {}
        Err(e) => return Err(e.to_string()),
    }
    let _: serde_json::Value = bridge
        .local
        .post("/shutdown-idle", &serde_json::json!({}))
        .await
        .map_err(|e| e.to_string())?;
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => {
                    return Ok(PausedService {
                        _lock: lock,
                        was_running: true,
                    });
                }
                Err(e) if lock_contended(&e) => {
                    tokio::time::sleep(Duration::from_millis(200)).await
                }
                Err(e) => return Err(e.to_string()),
            }
        }
    })
    .await
    .map_err(|_| "로컬 서비스 종료 대기 시간이 초과되었습니다.".to_string())?
}

async fn update_gate(bridge: &Bridge) -> Result<tokio::sync::RwLockWriteGuard<'_, ()>, String> {
    tokio::time::timeout(Duration::from_secs(2), bridge.local_gate.write())
        .await
        .map_err(|_| "로컬 요청이 처리 중입니다. 완료 후 업데이트를 설치하세요.".into())
}

#[tauri::command]
pub async fn install_app_update(app: AppHandle) -> Result<UpdateStatus, String> {
    let updates = app.state::<AppUpdates>();
    let _operation = updates
        .operation
        .try_lock()
        .map_err(|_| "업데이트 작업이 이미 진행 중입니다.")?;
    let mut pending = updates.pending.lock().await;
    let pending = pending.as_mut().ok_or("먼저 업데이트를 확인하세요.")?;
    if pending.bytes.is_none() {
        updates.change(|s| {
            s.phase = "downloading".into();
            s.downloaded = 0;
            s.total = None;
            s.message = None;
        });
        let mut update = pending.update.clone();
        update.timeout = Some(Duration::from_secs(30 * 60));
        match update
            .download(
                |chunk, total| {
                    updates.change(|s| {
                        s.downloaded += chunk as u64;
                        s.total = total;
                    })
                },
                || {},
            )
            .await
        {
            Ok(bytes) => pending.bytes = Some(bytes),
            Err(e) => {
                updates.change(|s| {
                    s.phase = "error".into();
                    s.message = Some(format!(
                        "업데이트 다운로드 또는 서명 검증 실패: {}",
                        ytlr_core::redact(&e.to_string())
                    ));
                });
                return Ok(updates.status());
            }
        }
    }
    let bridge = app.state::<Bridge>();
    updates.change(|s| {
        s.phase = "installing".into();
        s.message = None;
    });
    let _gate = match update_gate(&bridge).await {
        Ok(gate) => gate,
        Err(e) => {
            updates.change(|s| {
                s.phase = "ready".into();
                s.message = Some(e);
            });
            return Ok(updates.status());
        }
    };
    let service_lock = match stop_idle_service(&bridge).await {
        Ok(lock) => lock,
        Err(e) => {
            updates.change(|s| {
                s.phase = "ready".into();
                s.message = Some(e);
            });
            return Ok(updates.status());
        }
    };
    let update = pending.update.clone();
    let bytes = pending
        .bytes
        .take()
        .ok_or("다운로드한 업데이트가 없습니다.")?;
    let result = tauri::async_runtime::spawn_blocking(move || update.install(bytes)).await;
    match result {
        Ok(Ok(())) => {
            updates.change(|s| s.phase = "restarting".into());
            drop(service_lock);
            app.restart();
        }
        result => {
            let error = match result {
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
                _ => unreachable!(),
            };
            let was_running = service_lock.was_running;
            drop(service_lock);
            // A failed install should not leave previously queued recordings stranded.
            let restore_error = if was_running {
                bridge.local.ensure(&bridge.executable).await.err()
            } else {
                None
            };
            updates.change(|s| {
                s.phase = "error".into();
                s.message = Some(format!(
                    "업데이트 설치 실패: {}{}",
                    ytlr_core::redact(&error),
                    restore_error
                        .map(|e| format!(
                            " · 로컬 서비스 재시작 실패: {}",
                            ytlr_core::redact(&e.to_string())
                        ))
                        .unwrap_or_default()
                ));
            });
        }
    }
    Ok(updates.status())
}

#[tauri::command]
pub fn open_update_releases(app: AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(RELEASES, None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn real_updater_checks_versions_and_verifies_downloads_before_install() {
        use axum::{
            Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get,
        };
        use std::{path::Path, process::Command, sync::Arc};
        use tauri::test::{mock_builder, mock_context, noop_assets};

        let home = tempfile::tempdir().unwrap();
        let cli =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../node_modules/@tauri-apps/cli/tauri.js");
        let key = home.path().join("test.key");
        let artifact = home.path().join("fixture.bin");
        let run = |args: &[&str]| {
            let output = Command::new("node")
                .arg(&cli)
                .arg("signer")
                .args(args)
                .env_remove("TAURI_SIGNING_PRIVATE_KEY")
                .env_remove("TAURI_SIGNING_PRIVATE_KEY_PATH")
                .env_remove("TAURI_SIGNING_PRIVATE_KEY_PASSWORD")
                .output()
                .unwrap();
            // Never print signer output: key generation includes private material.
            assert!(output.status.success(), "test fixture signer failed");
        };
        run(&[
            "generate",
            "--ci",
            "--password",
            "",
            "--write-keys",
            key.to_str().unwrap(),
        ]);
        let original = b"Signed update fixture; never installed".to_vec();
        std::fs::write(&artifact, &original).unwrap();
        run(&[
            "sign",
            "--private-key-path",
            key.to_str().unwrap(),
            "--password",
            "",
            "--app-version",
            "1.2.3",
            artifact.to_str().unwrap(),
        ]);
        let public_key = std::fs::read_to_string(key.with_extension("key.pub")).unwrap();
        let signature = std::fs::read_to_string(artifact.with_extension("bin.sig")).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        struct Feed {
            manifest: serde_json::Value,
            bytes: Vec<u8>,
            status: StatusCode,
        }
        let feed = Arc::new(Mutex::new(Feed {
            manifest: serde_json::json!({"version":"1.2.3","platforms":{"fixture":{"url":format!("{base}/artifact"),"signature":signature.trim()}}}),
            bytes: original.clone(),
            status: StatusCode::OK,
        }));
        let router = Router::new()
            .route(
                "/latest.json",
                get(|State(feed): State<Arc<Mutex<Feed>>>| async move {
                    let f = feed.lock().unwrap();
                    (f.status, Json(f.manifest.clone())).into_response()
                }),
            )
            .route(
                "/artifact",
                get(|State(feed): State<Arc<Mutex<Feed>>>| async move {
                    feed.lock().unwrap().bytes.clone()
                }),
            )
            .with_state(feed.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mut context = mock_context(noop_assets());
        context.config_mut().plugins.0.insert("updater".into(),serde_json::json!({
            "pubkey":public_key.trim(),"requireSignedVersion":true,
            "dangerousInsecureTransportProtocol":true,"endpoints":[format!("{base}/latest.json")]
        }));
        let app = mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap();
        let updater = app
            .updater_builder()
            .target("fixture")
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .build()
            .unwrap();
        let update = updater.check().await.unwrap().unwrap();
        let mut downloaded = 0;
        assert_eq!(
            update
                .download(|chunk, _| downloaded += chunk, || {})
                .await
                .unwrap(),
            original
        );
        assert_eq!(downloaded, original.len());
        feed.lock().unwrap().bytes.push(0);
        assert!(
            update.download(|_, _| {}, || {}).await.is_err(),
            "corrupt artifacts must be rejected"
        );
        feed.lock().unwrap().bytes = original;
        feed.lock().unwrap().manifest["version"] = serde_json::json!("9.9.9");
        assert!(matches!(
            updater
                .check()
                .await
                .unwrap()
                .unwrap()
                .download(|_, _| {}, || {})
                .await,
            Err(tauri_plugin_updater::Error::SignedVersionMismatch { .. })
        ));
        for version in ["0.1.0", "0.0.9"] {
            feed.lock().unwrap().manifest["version"] = serde_json::json!(version);
            assert!(
                updater.check().await.unwrap().is_none(),
                "same or older versions must not update"
            );
        }
        feed.lock().unwrap().status = StatusCode::NOT_FOUND;
        assert!(updater.check().await.is_err());
        server.abort();
    }

    #[tokio::test]
    async fn update_holds_the_sidecar_lock_until_install_finishes() {
        let home = tempfile::tempdir().unwrap();
        let paths = ytlr_core::AppPaths::resolve(Some(home.path().to_owned())).unwrap();
        let bridge = Bridge {
            local_gate: tokio::sync::RwLock::new(()),
            local: ytlr_service::client::Client::new(paths.clone()).unwrap(),
            executable: home.path().join("nonexistent-sidecar"),
            close_to_tray: std::sync::atomic::AtomicBool::new(true),
            paths,
            session: Mutex::new(crate::Session::default()),
        };
        let busy = bridge.local_gate.read().await;
        assert!(
            update_gate(&bridge).await.is_err(),
            "long API work must defer installation instead of hanging"
        );
        drop(busy);
        drop(update_gate(&bridge).await.unwrap());
        let guard = stop_idle_service(&bridge).await.unwrap();
        let contender = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(home.path().join("service.lock"))
            .unwrap();
        assert!(lock_contended(&contender.try_lock_exclusive().unwrap_err()));
        drop(guard);
        contender.try_lock_exclusive().unwrap();
    }
}
