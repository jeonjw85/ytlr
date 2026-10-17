use super::*;
use std::{
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use tauri::{
    Manager,
    menu::{Menu, MenuItem},
    tray::{TrayIcon, TrayIconBuilder},
};
use ytlr_service::client::Client;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

fn context() -> tauri::Context<tauri::Wry> {
    let mut context = crate::context();
    context.config_mut().app.windows.clear();
    context
}

fn client_command(executable: &Path, paths: &AppPaths) -> Command {
    let mut command = Command::new(executable);
    command.env("YTLR_HOME", &paths.root);
    command
}

fn spawn_detached(mut command: Command) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

struct Presentation {
    status: String,
    title: String,
    open: &'static str,
}

fn presentation(summary: Option<&StatusSummary>, language: Language) -> Presentation {
    Presentation {
        status: match summary {
            Some(summary) => summary.label(language),
            None => match language {
                Language::En => "Local server · Unable to connect".into(),
                Language::Ko => "로컬 서버 · 연결 확인 실패".into(),
            },
        },
        title: match summary {
            Some(summary) if summary.recordings > 0 => summary.recordings.to_string(),
            Some(_) => String::new(),
            None => "?".into(),
        },
        open: match language {
            Language::En => "Open YTLR",
            Language::Ko => "YTLR 열기",
        },
    }
}

struct StatusTray {
    _lock: File,
    tray: TrayIcon,
    status: MenuItem<tauri::Wry>,
    open: MenuItem<tauri::Wry>,
}

impl StatusTray {
    fn refresh(&self, summary: Option<&StatusSummary>, language: Language) -> tauri::Result<()> {
        let view = presentation(summary, language);
        self.status.set_text(&view.status)?;
        self.open.set_text(view.open)?;
        self.tray
            .set_tooltip(Some(format!("YTLR · {}", view.status)))?;
        self.tray.set_title(Some(view.title))
    }
}

fn attach(executable: &Path, paths: &AppPaths, wait_for_service: bool) -> io::Result<bool> {
    if !wait_for_service && !service_running(paths)? {
        return Ok(false);
    }
    let Some(lock) = try_status_lock(paths)? else {
        return Ok(false);
    };
    // The helper acquires and retains the authoritative lock in its own process.
    drop(lock);
    let mut command = client_command(executable, paths);
    command
        .arg("--service-status")
        .arg("--data-dir")
        .arg(&paths.root);
    if wait_for_service {
        command.arg("--wait-for-service");
    }
    spawn_detached(command)?;
    Ok(true)
}

pub fn setup(app: &tauri::AppHandle) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let paths = app.state::<crate::Bridge>().paths.clone();
    paths.initialize().map_err(|e| e.to_string())?;
    // Handoff is synchronous; even an immediate Cmd+Q cannot cancel the observer's startup wait.
    let failed = attach(&executable, &paths, true).is_err();
    if failed {
        eprintln!("Unable to start local service indicator; retrying in background");
    }
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut failed = failed;
        loop {
            let result = {
                let bridge = handle.state::<crate::Bridge>();
                let _gate = bridge.local_gate.read().await;
                if handle.state::<crate::updates::AppUpdates>().installing() {
                    Ok(false)
                } else {
                    attach(&executable, &bridge.paths, false)
                }
            };
            if result.is_err() && !failed {
                eprintln!("Unable to start local service indicator; retrying in background");
            }
            failed = result.is_err();
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
    Ok(())
}

pub fn run(mode: StatusMode) -> Result<(), String> {
    let paths = mode.paths;
    if !mode.wait_for_service && !service_running(&paths).map_err(|e| e.to_string())? {
        return Ok(());
    }
    let Some(lock) = try_status_lock(&paths).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    if mode.wait_for_service
        && !tauri::async_runtime::block_on(wait_for_service(&paths, Duration::from_secs(30)))
            .map_err(|e| e.to_string())?
    {
        return Ok(());
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let client = Client::new(paths.clone())
        .and_then(|c| c.with_timeout(POLL_INTERVAL))
        .map_err(|_| "Unable to create local status client".to_string())?;
    let open_paths = paths.clone();
    let open_executable = executable.clone();
    tauri::Builder::default()
        .enable_macos_default_menu(false)
        .setup(move |app| {
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let view = presentation(None, load_language(&paths));
            let status = MenuItem::with_id(app, "status", &view.status, false, None::<&str>)?;
            let open = MenuItem::with_id(app, "open-client", view.open, true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&status, &open])?;
            let mut rgba = vec![0u8; 20 * 20 * 4];
            for y in 0..20_i32 {
                for x in 0..20_i32 {
                    let distance = (x - 10).pow(2) + (y - 10).pow(2);
                    if (30..64).contains(&distance) || distance < 9 {
                        rgba[((y * 20 + x) * 4 + 3) as usize] = 255;
                    }
                }
            }
            let menu_paths = paths.clone();
            let tray = TrayIconBuilder::with_id("local-service")
                .icon(tauri::image::Image::new_owned(rgba, 20, 20))
                .icon_as_template(true)
                .tooltip("YTLR · Local server")
                .menu(&menu)
                .on_menu_event(move |_, event| {
                    if event.id.as_ref() == "open-client"
                        && spawn_detached(client_command(&executable, &menu_paths)).is_err()
                    {
                        eprintln!("Unable to open YTLR client");
                    }
                })
                .build(app)?;
            app.manage(StatusTray {
                _lock: lock,
                tray,
                status,
                open,
            });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    if matches!(service_running(&paths), Ok(false)) {
                        handle.exit(0);
                        return;
                    }
                    let summary = read_status(&client)
                        .await
                        .ok()
                        .map(|snapshot| StatusSummary::from_snapshot(&snapshot));
                    if handle
                        .state::<StatusTray>()
                        .refresh(summary.as_ref(), load_language(&paths))
                        .is_err()
                    {
                        eprintln!("Unable to refresh local service indicator");
                    }
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            });
            Ok(())
        })
        .build(context())
        .map_err(|e| e.to_string())?
        .run(move |_, event| match event {
            tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } => api.prevent_exit(),
            // Finder may target the accessory process because it shares the app bundle.
            tauri::RunEvent::Reopen { .. }
                if spawn_detached(client_command(&open_executable, &open_paths)).is_err() =>
            {
                eprintln!("Unable to open YTLR client");
            }
            _ => {}
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_server(paths: &AppPaths) -> File {
        std::fs::write(paths.endpoint_file(), b"{}").unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(paths.root.join("service.lock"))
            .unwrap();
        lock.lock_exclusive().unwrap();
        lock
    }

    #[test]
    fn launcher_skips_stale_endpoints_and_an_existing_status_process() {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(d.path().into())).unwrap();
        let nonexistent = paths.root.join("nonexistent-client");
        assert!(!attach(&nonexistent, &paths, false).unwrap());
        let service = test_server(&paths);
        let helper = try_status_lock(&paths).unwrap().unwrap();
        assert!(!attach(&nonexistent, &paths, false).unwrap());
        drop(helper);
        drop(service);
        assert!(!attach(&nonexistent, &paths, false).unwrap());
    }

    #[tokio::test]
    async fn startup_handoff_launches_without_a_ready_endpoint() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(d.path().into())).unwrap();
        let executable = paths.root.join("startup-fixture");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$YTLR_HOME/args\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(attach(&executable, &paths, true).unwrap());
        let expected = format!(
            "--service-status\n--data-dir\n{}\n--wait-for-service\n",
            paths.root.display()
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            while std::fs::read_to_string(paths.root.join("args"))
                .ok()
                .as_deref()
                != Some(expected.as_str())
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!paths.endpoint_file().exists());
    }

    #[tokio::test]
    async fn launcher_starts_a_detached_helper_with_an_explicit_data_directory() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(d.path().into())).unwrap();
        let _service = test_server(&paths);
        let executable = paths.root.join("test client with spaces");
        std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$YTLR_HOME/args\"\nprintf '%s' \"$$\" > \"$YTLR_HOME/pid\"\n/bin/ps -o pgid= -p $$ > \"$YTLR_HOME/group\"\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(attach(&executable, &paths, false).unwrap());
        tokio::time::timeout(Duration::from_secs(3), async {
            while std::fs::read_to_string(paths.root.join("group"))
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .is_none()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(paths.root.join("args")).unwrap(),
            format!("--service-status\n--data-dir\n{}\n", paths.root.display())
        );
        assert_eq!(
            std::fs::read_to_string(paths.root.join("pid"))
                .unwrap()
                .trim(),
            std::fs::read_to_string(paths.root.join("group"))
                .unwrap()
                .trim()
        );
    }

    #[test]
    fn helper_context_has_no_windows() {
        assert!(context().config().app.windows.is_empty());
    }

    #[test]
    fn opening_client_does_not_forward_helper_arguments() {
        let paths = AppPaths::resolve(Some("/data with spaces".into())).unwrap();
        let command = client_command(
            std::path::Path::new("/Applications/YTLR.app/Contents/MacOS/YTLR"),
            &paths,
        );
        assert_eq!(
            command.get_program(),
            "/Applications/YTLR.app/Contents/MacOS/YTLR"
        );
        assert_eq!(command.get_args().count(), 0);
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == "YTLR_HOME" && value == Some(paths.root.as_os_str()))
        );
    }

    #[test]
    fn unavailable_status_clears_recording_count_and_is_localized() {
        let active = StatusSummary {
            recordings: 2,
            ..Default::default()
        };
        assert_eq!(presentation(Some(&active), Language::En).title, "2");
        let unavailable = presentation(None, Language::Ko);
        assert_eq!(unavailable.title, "?");
        assert!(unavailable.status.contains("로컬 서버"));
        assert!(!unavailable.status.contains("녹화 2"));
        assert_eq!(unavailable.open, "YTLR 열기");
    }
}
