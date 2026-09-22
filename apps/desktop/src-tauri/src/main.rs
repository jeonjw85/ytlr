#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{Manager, State};
use tauri_plugin_opener::OpenerExt;
use ytlr_core::{AppPaths, RecordingJob, Remote, Snapshot};
use ytlr_service::{client::Client, tunnel::Tunnel};

struct Bridge {
    local: Client,
    executable: PathBuf,
    close_to_tray: AtomicBool,
    paths: AppPaths,
    session: Mutex<Session>,
}

struct TrayLabels {
    show: tauri::menu::MenuItem<tauri::Wry>,
    quit: tauri::menu::MenuItem<tauri::Wry>,
}

#[tauri::command]
fn set_ui_language(labels: State<'_, TrayLabels>, language: String) -> Result<(), String> {
    let (show, quit) = match language.as_str() {
        "en" => ("Open YTLR", "Quit app (keep recording)"),
        "ko" => ("YTLR 열기", "앱 종료 (녹화 유지)"),
        _ => return Err("Unsupported language".into()),
    };
    labels.show.set_text(show).map_err(|e| e.to_string())?;
    labels.quit.set_text(quit).map_err(|e| e.to_string())
}

#[derive(Default)]
struct Session {
    name: Option<String>,
    tunnel: Option<Arc<Tunnel>>,
    generation: u64,
}
impl Session {
    fn select(&mut self, name: Option<String>) -> u64 {
        self.generation += 1;
        self.name = name;
        self.tunnel = None;
        self.generation
    }
    fn lease(&self, expected: &str) -> Result<Option<Arc<Tunnel>>, String> {
        if self.name.as_deref().unwrap_or("local") != expected {
            return Err("연결 대상이 변경되었습니다.".into());
        }
        if self.name.is_some() && self.tunnel.is_none() {
            return Err("원격 연결이 끊겼습니다. 다시 연결해 주세요.".into());
        }
        Ok(self.tunnel.clone())
    }
}

#[tauri::command]
async fn api(
    bridge: State<'_, Bridge>,
    method: String,
    path: String,
    body: Option<serde_json::Value>,
    expected_target: Option<String>,
) -> Result<serde_json::Value, String> {
    if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "DELETE") {
        return Err("지원하지 않는 요청".into());
    }
    let (lease, generation) = {
        let session = bridge.session.lock().map_err(|e| e.to_string())?;
        (
            session.lease(expected_target.as_deref().unwrap_or("local"))?,
            session.generation,
        )
    };
    let client = if let Some(tunnel) = &lease {
        bridge.local.clone().with_endpoint(tunnel.endpoint.clone())
    } else {
        bridge
            .local
            .ensure(&bridge.executable)
            .await
            .map_err(|e| e.to_string())?;
        bridge.local.clone()
    };
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    let value: serde_json::Value = client
        .request(method, &path, body.as_ref())
        .await
        .map_err(|e| e.to_string())?;
    if bridge.session.lock().map_err(|e| e.to_string())?.generation != generation {
        return Err("연결 대상이 변경되었습니다.".into());
    }
    if lease.is_none()
        && path == "/snapshot"
        && let Some(close) = value["settings"]["close_to_tray"].as_bool()
    {
        bridge.close_to_tray.store(close, Ordering::Relaxed);
    }
    Ok(value)
}

#[tauri::command]
async fn open_job(
    app: tauri::AppHandle,
    bridge: State<'_, Bridge>,
    id: String,
    index: Option<usize>,
) -> Result<(), String> {
    let remote = bridge
        .session
        .lock()
        .map_err(|e| e.to_string())?
        .name
        .is_some();
    if remote {
        return Err("원격 서버의 파일은 이 컴퓨터에서 열 수 없습니다.".into());
    }
    let snapshot: Snapshot = bridge
        .local
        .get("/snapshot")
        .await
        .map_err(|e| e.to_string())?;
    let job: RecordingJob = snapshot
        .jobs
        .into_iter()
        .find(|j| j.id == id)
        .ok_or("녹화 작업을 찾을 수 없습니다.")?;
    let path = if let Some(index) = index {
        job.outputs
            .get(index)
            .ok_or("출력 파일을 찾을 수 없습니다.")?
            .path
            .clone()
    } else {
        job.output_dir
    };
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_diagnostics(
    bridge: State<'_, Bridge>,
    id: String,
    path: String,
) -> Result<(), String> {
    let lease = {
        let session = bridge.session.lock().map_err(|e| e.to_string())?;
        session.lease(session.name.as_deref().unwrap_or("local"))?
    };
    let client = if let Some(tunnel) = &lease {
        bridge.local.clone().with_endpoint(tunnel.endpoint.clone())
    } else {
        bridge.local.clone()
    };
    let events: serde_json::Value = client
        .get(&format!("/jobs/{id}/events"))
        .await
        .map_err(|e| e.to_string())?;
    ytlr_core::atomic_write(
        std::path::Path::new(&path),
        serde_json::to_string_pretty(&events)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_remotes(bridge: State<'_, Bridge>) -> Result<Vec<Remote>, String> {
    bridge.paths.remotes().map_err(|e| e.to_string())
}

#[tauri::command]
fn save_remote(bridge: State<'_, Bridge>, remote: Remote) -> Result<Vec<Remote>, String> {
    let mut remotes = bridge.paths.remotes().map_err(|e| e.to_string())?;
    remotes.retain(|item| item.name != remote.name && item.id != remote.id);
    let mut remote = remote;
    if remote.id.trim().is_empty() {
        remote.id = remote.name.clone();
    }
    remote.validate().map_err(|e| e.to_string())?;
    remotes.push(remote);
    bridge
        .paths
        .save_remotes(&remotes)
        .map_err(|e| e.to_string())?;
    Ok(remotes)
}

#[tauri::command]
fn delete_remote(bridge: State<'_, Bridge>, name: String) -> Result<Vec<Remote>, String> {
    let mut remotes = bridge.paths.remotes().map_err(|e| e.to_string())?;
    remotes.retain(|item| item.name != name);
    bridge
        .paths
        .save_remotes(&remotes)
        .map_err(|e| e.to_string())?;
    if let Ok(mut session) = bridge.session.lock()
        && session.name.as_ref() == Some(&name)
    {
        session.select(Some(name));
    }
    Ok(remotes)
}

#[tauri::command]
async fn connect_remote(bridge: State<'_, Bridge>, name: Option<String>) -> Result<String, String> {
    let name = name.filter(|n| !n.is_empty() && n != "local");
    let generation = bridge
        .session
        .lock()
        .map_err(|e| e.to_string())?
        .select(name.clone());
    let Some(name) = name else {
        return Ok("local".into());
    };
    let remote = bridge
        .paths
        .remotes()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|item| item.name == name)
        .ok_or("원격을 찾을 수 없습니다.")?;
    let tunnel = ytlr_service::tunnel::connect(&remote)
        .await
        .map_err(|e| e.to_string())?;
    let mut session = bridge.session.lock().map_err(|e| e.to_string())?;
    if session.generation != generation {
        return Err("연결 전환이 취소되었습니다.".into());
    }
    session.tunnel = Some(Arc::new(tunnel));
    Ok(name)
}

#[tauri::command]
fn connection(bridge: State<'_, Bridge>) -> Result<String, String> {
    Ok(bridge
        .session
        .lock()
        .map_err(|e| e.to_string())?
        .name
        .clone()
        .unwrap_or_else(|| "local".into()))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let paths = AppPaths::resolve(None)?;
            let name = if cfg!(windows) { "ytlr.exe" } else { "ytlr" };
            let executable = std::env::current_exe()?.parent().unwrap().join(name);
            let executable = if executable.exists() {
                executable
            } else {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../target/debug")
                    .join(name)
            };
            app.manage(Bridge {
                local: Client::new(paths.clone())?
                    .with_bundled_tools(app.path().resource_dir()?.join("tools")),
                executable,
                close_to_tray: AtomicBool::new(true),
                paths,
                session: Mutex::new(Session::default()),
            });
            use tauri::menu::{Menu, MenuItem};
            let show = MenuItem::with_id(app, "show", "Open YTLR", true, None::<&str>)?;
            let quit =
                MenuItem::with_id(app, "quit", "Quit app (keep recording)", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            app.manage(TrayLabels { show, quit });
            let mut rgba = vec![0u8; 20 * 20 * 4];
            for y in 0..20 {
                for x in 0..20 {
                    let p = (y * 20 + x) * 4;
                    if (x as i32 - 10).pow(2) + (y as i32 - 10).pow(2) < 64 {
                        rgba[p..p + 4].copy_from_slice(&[235, 93, 107, 255]);
                    }
                }
            }
            tauri::tray::TrayIconBuilder::new()
                .icon(tauri::image::Image::new_owned(rgba, 20, 20))
                .tooltip("YTLR")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && window
                    .state::<Bridge>()
                    .close_to_tray
                    .load(Ordering::Relaxed)
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            set_ui_language,
            api,
            open_job,
            save_diagnostics,
            list_remotes,
            save_remote,
            delete_remote,
            connect_remote,
            connection
        ])
        .run(tauri::generate_context!())
        .expect("데스크톱 앱 실행 실패");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_remote_selection_never_falls_back_to_local() {
        let mut session = Session::default();
        assert!(session.lease("local").unwrap().is_none());
        session.select(Some("server".into()));
        assert!(session.lease("server").is_err());
        assert!(session.lease("local").is_err());
        session.select(None);
        assert!(session.lease("local").unwrap().is_none());
    }
}
