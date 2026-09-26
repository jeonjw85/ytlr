use crate::Bridge;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_opener::OpenerExt;
use ytlr_core::*;

#[derive(Default, Serialize, Deserialize, Clone)]
pub struct StartupPreferences {
    pub start_hidden: bool,
}
pub struct StartupState(pub Mutex<StartupPreferences>);

pub fn setup(app: &tauri::AppHandle) {
    let bridge = app.state::<Bridge>();
    let preferences: StartupPreferences =
        std::fs::read(bridge.paths.root.join("startup-preferences.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
    let automatic = std::env::args().any(|a| a == "--autostart");
    if automatic
        && preferences.start_hidden
        && let Some(window) = app.get_webview_window("main")
    {
        let _ = window.hide();
    }
    app.manage(StartupState(Mutex::new(preferences)));
    if automatic {
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let bridge = handle.state::<Bridge>();
            let _gate = bridge.local_gate.read().await;
            let _ = bridge.local.ensure(&bridge.executable).await;
        });
    }
}
#[tauri::command]
pub fn startup_status(
    app: tauri::AppHandle,
    state: State<'_, StartupState>,
) -> Result<serde_json::Value, String> {
    Ok(
        serde_json::json!({"enabled":app.autolaunch().is_enabled().map_err(|e|e.to_string())?,"start_hidden":state.0.lock().map_err(|e|e.to_string())?.start_hidden}),
    )
}
#[tauri::command]
pub fn set_startup(
    app: tauri::AppHandle,
    bridge: State<'_, Bridge>,
    state: State<'_, StartupState>,
    enabled: bool,
    start_hidden: bool,
) -> Result<serde_json::Value, String> {
    let manager = app.autolaunch();
    let previous = manager.is_enabled().map_err(|e| e.to_string())?;
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|e| e.to_string())?;
    let prefs = StartupPreferences { start_hidden };
    if let Err(e) = atomic_write(
        &bridge.paths.root.join("startup-preferences.json"),
        &serde_json::to_vec(&prefs).map_err(|e| e.to_string())?,
    ) {
        let _ = if previous {
            manager.enable()
        } else {
            manager.disable()
        };
        return Err(e.to_string());
    }
    *state.0.lock().map_err(|e| e.to_string())? = prefs;
    Ok(serde_json::json!({"enabled":enabled,"start_hidden":start_hidden}))
}
#[tauri::command]
pub fn load_configuration(path: String) -> Result<ConfigurationBundle, String> {
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > 4 * 1024 * 1024 {
        return Err("설정 파일이 너무 큽니다.".into());
    }
    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|_| "설정 백업 파일 형식이 올바르지 않습니다.".into())
}
#[tauri::command]
pub fn save_configuration(path: String, bundle: ConfigurationBundle) -> Result<(), String> {
    atomic_write(
        std::path::Path::new(&path),
        &serde_json::to_vec_pretty(&bundle).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn media_url(
    bridge: State<'_, Bridge>,
    id: String,
    path: String,
    expected_target: String,
) -> Result<String, String> {
    let (lease, generation) = {
        let session = bridge.session.lock().map_err(|e| e.to_string())?;
        (session.lease(&expected_target)?, session.generation)
    };
    let _gate = if lease.is_none() {
        Some(bridge.local_gate.read().await)
    } else {
        None
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
    let result: serde_json::Value = client
        .post(
            &format!("/jobs/{id}/preview-ticket"),
            &serde_json::json!({"path":path}),
        )
        .await
        .map_err(|e| e.to_string())?;
    if bridge.session.lock().map_err(|e| e.to_string())?.generation != generation {
        return Err("연결 대상이 변경되었습니다.".into());
    }
    let token = result["ticket"].as_str().ok_or("미리보기 연결 오류")?;
    Ok(format!(
        "http://127.0.0.1:{}/play/{token}",
        client.endpoint_info().map_err(|e| e.to_string())?.port
    ))
}
#[tauri::command]
pub async fn queue_download(
    bridge: State<'_, Bridge>,
    id: String,
    path: String,
    expected_target: String,
) -> Result<Vec<Operation>, String> {
    let remote = {
        let session = bridge.session.lock().map_err(|e| e.to_string())?;
        let _ = session.lease(&expected_target)?;
        session.name.clone().ok_or("원격 서버를 먼저 선택하세요.")?
    };
    let _gate = bridge.local_gate.read().await;
    bridge
        .local
        .ensure(&bridge.executable)
        .await
        .map_err(|e| e.to_string())?;
    bridge
        .local
        .post(
            "/operations",
            &vec![OperationRequest {
                job_id: id,
                source_path: None,
                task: OperationTask::Download { remote, path },
            }],
        )
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn local_downloads(bridge: State<'_, Bridge>) -> Result<Vec<Operation>, String> {
    let _gate = bridge.local_gate.read().await;
    bridge
        .local
        .ensure(&bridge.executable)
        .await
        .map_err(|e| e.to_string())?;
    let ops: Vec<Operation> = bridge
        .local
        .get("/operations")
        .await
        .map_err(|e| e.to_string())?;
    Ok(ops
        .into_iter()
        .filter(|o| matches!(o.request.task, OperationTask::Download { .. }))
        .collect())
}
#[tauri::command]
pub async fn download_action(
    bridge: State<'_, Bridge>,
    id: String,
    action: String,
) -> Result<Operation, String> {
    if !matches!(action.as_str(), "cancel" | "retry") {
        return Err("지원하지 않는 작업".into());
    }
    let _gate = bridge.local_gate.read().await;
    let ops: Vec<Operation> = bridge
        .local
        .get("/operations")
        .await
        .map_err(|e| e.to_string())?;
    if !ops
        .iter()
        .any(|o| o.id == id && matches!(o.request.task, OperationTask::Download { .. }))
    {
        return Err("다운로드 작업을 찾을 수 없습니다.".into());
    }
    bridge
        .local
        .post(
            &format!("/operations/{id}/{action}"),
            &serde_json::json!({}),
        )
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn open_operation(
    app: tauri::AppHandle,
    bridge: State<'_, Bridge>,
    id: String,
    download: bool,
    expected_target: String,
) -> Result<(), String> {
    let generation = if !download {
        let session = bridge.session.lock().map_err(|e| e.to_string())?;
        session.lease(&expected_target)?;
        if session.name.is_some() {
            return Err("원격 파일을 먼저 다운로드하세요.".into());
        }
        Some(session.generation)
    } else {
        None
    };
    let _gate = bridge.local_gate.read().await;
    let ops: Vec<Operation> = bridge
        .local
        .get("/operations")
        .await
        .map_err(|e| e.to_string())?;
    if let Some(generation) = generation
        && bridge.session.lock().map_err(|e| e.to_string())?.generation != generation
    {
        return Err("연결 대상이 변경되었습니다.".into());
    }
    let op = ops
        .into_iter()
        .find(|o| {
            o.id == id
                && o.state == "completed"
                && (!download || matches!(o.request.task, OperationTask::Download { .. }))
        })
        .ok_or("완료된 작업을 찾을 수 없습니다.")?;
    let path = op
        .result
        .as_ref()
        .and_then(|r| r["path"].as_str())
        .ok_or("결과 파일이 없습니다.")?;
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| e.to_string())
}
