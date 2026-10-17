#![cfg(target_os = "macos")]

use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::MetadataExt,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use ytlr_core::{AppPaths, SERVICE_API_VERSION, ServiceEndpoint};

const DESKTOP: &str = env!("CARGO_BIN_EXE_ytlr-desktop");
const CLIENT_SOCKET: &str = "/tmp/org_ytliverecord_desktop_si.sock";

fn helper_pids(home: &Path) -> Vec<u32> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,command="])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| {
            line.contains(DESKTOP)
                && line.contains("--service-status --data-dir")
                && line.contains(home.to_str().unwrap())
        })
        .filter_map(|line| line.split_whitespace().next()?.parse().ok())
        .collect()
}

struct Processes<'a> {
    home: &'a Path,
    children: Vec<Child>,
    socket_identity: Option<(u64, u64)>,
}

impl Processes<'_> {
    fn spawn(&mut self, args: &[&str]) -> usize {
        let child = Command::new(DESKTOP)
            .args(args)
            .env("YTLR_HOME", self.home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        self.children.push(child);
        self.children.len() - 1
    }
}

impl Drop for Processes<'_> {
    fn drop(&mut self) {
        // Match only this test's unique directory and executable, never other running apps.
        for pid in helper_pids(self.home) {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
        }
        for child in &mut self.children {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        // A force-terminated test client cannot run the plugin's socket cleanup.
        if let Ok(metadata) = std::fs::symlink_metadata(CLIENT_SOCKET)
            && self.socket_identity == Some((metadata.dev(), metadata.ino()))
        {
            let _ = std::fs::remove_file(CLIENT_SOCKET);
        }
    }
}

async fn wait_for(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(12), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("native status lifecycle condition timed out");
}

#[tokio::test]
#[ignore = "launches native macOS menu bar UI; run explicitly in a logged-in desktop session"]
async fn status_process_outlives_client_and_stops_without_restarting_service() {
    let home = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(home.path().into())).unwrap();
    paths.initialize().unwrap();
    let service_lock: File = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(paths.root.join("service.lock"))
        .unwrap();
    service_lock.lock_exclusive().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let health_requests = Arc::new(AtomicUsize::new(0));
    let failing = Arc::new(AtomicBool::new(false));
    let request_count = requests.clone();
    let health_count = health_requests.clone();
    let fail_requests = failing.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = ServiceEndpoint {
        api_version: SERVICE_API_VERSION,
        port: listener.local_addr().unwrap().port(),
        token: "isolated-status-test".into(),
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").into(),
    };
    let server = tokio::spawn(async move {
        let router = axum::Router::new()
            .route("/health", axum::routing::get(move || {
                let count = health_count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    axum::Json(serde_json::json!({"ok":true}))
                }
            }))
            .route("/snapshot", axum::routing::get(move |headers: axum::http::HeaderMap| {
                let count = request_count.clone();
                let failing = fail_requests.clone();
                async move {
                    assert_eq!(headers.get("authorization").unwrap(), "Bearer isolated-status-test");
                    count.fetch_add(1, Ordering::SeqCst);
                    if failing.load(Ordering::SeqCst) {
                        (axum::http::StatusCode::SERVICE_UNAVAILABLE, axum::Json(serde_json::json!({"error":"temporary failure"})))
                    } else {
                        (axum::http::StatusCode::OK, axum::Json(serde_json::json!({"jobs":[{"kind":"live","state":"recording"}]})))
                    }
                }
            }));
        axum::serve(listener, router).await.unwrap();
    });
    let endpoint_bytes = serde_json::to_vec(&endpoint).unwrap();
    ytlr_core::atomic_write(&paths.endpoint_file(), &endpoint_bytes).unwrap();
    std::fs::write(
        paths.root.join("startup-preferences.json"),
        br#"{"start_hidden":true}"#,
    )
    .unwrap();
    let mut processes = Processes {
        home: home.path(),
        children: Vec::new(),
        socket_identity: None,
    };
    let helper_args = [
        "--service-status",
        "--data-dir",
        home.path().to_str().unwrap(),
    ];

    // Do not contact another user's client singleton, even just to raise its window.
    let main_client = if !Path::new(CLIENT_SOCKET).exists() {
        Some(processes.spawn(&["--autostart"]))
    } else {
        eprintln!("Skipping native client attachment: an existing singleton socket is present");
        processes.spawn(&helper_args);
        None
    };
    wait_for(|| requests.load(Ordering::SeqCst) > 0 && helper_pids(home.path()).len() == 1).await;
    let helper = helper_pids(home.path())[0];
    if let Some(index) = main_client {
        let metadata = std::fs::symlink_metadata(CLIENT_SOCKET).unwrap();
        processes.socket_identity = Some((metadata.dev(), metadata.ino()));
        processes.children[index].kill().unwrap();
        processes.children[index].wait().unwrap();
    }
    let after_exit = requests.load(Ordering::SeqCst);
    wait_for(|| requests.load(Ordering::SeqCst) > after_exit).await;
    assert_eq!(
        helper_pids(home.path()),
        vec![helper],
        "helper must survive client process exit"
    );

    if main_client.is_some() {
        let previous_health = health_requests.load(Ordering::SeqCst);
        let relaunched = processes.spawn(&["--autostart"]);
        wait_for(|| health_requests.load(Ordering::SeqCst) > previous_health).await;
        let metadata = std::fs::symlink_metadata(CLIENT_SOCKET).unwrap();
        processes.socket_identity = Some((metadata.dev(), metadata.ino()));
        assert!(processes.children[relaunched].try_wait().unwrap().is_none());
        assert_eq!(
            helper_pids(home.path()),
            vec![helper],
            "reopening client must reuse the independent indicator"
        );
        processes.children[relaunched].kill().unwrap();
        processes.children[relaunched].wait().unwrap();
    }

    let duplicate = processes.spawn(&helper_args);
    wait_for(|| processes.children[duplicate].try_wait().unwrap().is_some()).await;
    assert!(processes.children[duplicate].wait().unwrap().success());
    assert_eq!(
        helper_pids(home.path()),
        vec![helper],
        "helper singleton must remain unique"
    );

    failing.store(true, Ordering::SeqCst);
    let before_failure = requests.load(Ordering::SeqCst);
    wait_for(|| requests.load(Ordering::SeqCst) > before_failure).await;
    assert_eq!(
        helper_pids(home.path()),
        vec![helper],
        "HTTP failure must not remove a live server indicator"
    );
    failing.store(false, Ordering::SeqCst);
    let before_recovery = requests.load(Ordering::SeqCst);
    wait_for(|| requests.load(Ordering::SeqCst) > before_recovery).await;

    // An updater owns the same exclusive service lock after shutdown, but no endpoint.
    std::fs::remove_file(paths.endpoint_file()).unwrap();
    wait_for(|| helper_pids(home.path()).is_empty()).await;
    assert!(service_lock.try_lock_exclusive().is_ok());
    assert!(
        !paths.endpoint_file().exists(),
        "helper must never restart the service"
    );

    drop(service_lock);
    std::fs::write(paths.endpoint_file(), endpoint_bytes).unwrap();
    let stale = processes.spawn(&helper_args);
    wait_for(|| processes.children[stale].try_wait().unwrap().is_some()).await;
    assert!(processes.children[stale].wait().unwrap().success());
    assert!(
        helper_pids(home.path()).is_empty(),
        "stale endpoint must not create an indicator"
    );
    let malformed = processes.spawn(&["--service-status"]);
    wait_for(|| processes.children[malformed].try_wait().unwrap().is_some()).await;
    assert_eq!(
        processes.children[malformed].wait().unwrap().code(),
        Some(2)
    );
    server.abort();
}
