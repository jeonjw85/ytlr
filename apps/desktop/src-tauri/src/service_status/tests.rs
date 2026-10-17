use super::*;
use fs2::FileExt;
use std::{ffi::OsString, fs::OpenOptions};

#[test]
fn counts_live_recordings_separately_from_downloads_and_transitions() {
    let snapshot: StatusSnapshot = serde_json::from_value(serde_json::json!({
        "jobs": [
            {"state":"recording"},
            {"state":"recording", "kind":"download"},
            {"state":"preparing"},
            {"state":"reconnecting"},
            {"state":"finalizing"},
            {"state":"queued"},
            {"state":"waiting"},
            {"state":"completed"},
            {"state":"partial"},
            {"state":"stopped"},
            {"state":"failed"}
        ]
    }))
    .unwrap();
    let summary = StatusSummary::from_snapshot(&snapshot);
    assert_eq!(summary.recordings, 1);
    assert_eq!(summary.downloads, 1);
    assert_eq!(summary.preparing, 1);
    assert_eq!(summary.reconnecting, 1);
    assert_eq!(summary.finalizing, 1);
    assert_eq!(summary.waiting, 2);
    assert!(summary.label(Language::En).contains("Recording 1"));
    assert!(summary.label(Language::En).contains("Downloading 1"));
    assert!(summary.label(Language::Ko).contains("녹화 1"));
}

#[test]
fn empty_server_is_running_without_claiming_to_record() {
    let summary = StatusSummary::from_snapshot(&serde_json::from_str(r#"{"jobs":[]}"#).unwrap());
    assert_eq!(summary.recordings, 0);
    assert_eq!(summary.downloads, 0);
    assert!(summary.label(Language::En).contains("Local server running"));
    assert!(summary.label(Language::Ko).contains("로컬 서버 실행 중"));
}

#[test]
fn every_download_state_uses_the_same_transition_buckets_as_live_jobs() {
    for (state, want) in [
        ("queued", [0, 0, 0, 0, 0, 1]),
        ("waiting", [0, 0, 0, 0, 0, 1]),
        ("preparing", [0, 0, 1, 0, 0, 0]),
        ("recording", [0, 1, 0, 0, 0, 0]),
        ("reconnecting", [0, 0, 0, 1, 0, 0]),
        ("finalizing", [0, 0, 0, 0, 1, 0]),
        ("completed", [0; 6]),
        ("partial", [0; 6]),
        ("stopped", [0; 6]),
        ("failed", [0; 6]),
    ] {
        let snapshot = serde_json::from_value(serde_json::json!({
            "jobs": [{"state":state, "kind":"download"}]
        }))
        .unwrap();
        let s = StatusSummary::from_snapshot(&snapshot);
        assert_eq!(
            [
                s.recordings,
                s.downloads,
                s.preparing,
                s.reconnecting,
                s.finalizing,
                s.waiting
            ],
            want,
            "{state}"
        );
    }
}

#[test]
fn helper_mode_requires_an_explicit_absolute_data_directory() {
    let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
    assert!(mode_paths(&[]).unwrap().is_none());
    assert!(mode_paths(&args(&["--autostart"])).unwrap().is_none());
    for invalid in [
        vec!["--service-status"],
        vec!["--service-status", "--data-dir"],
        vec!["--service-status", "--data-dir", ""],
        vec!["--service-status", "--data-dir", "relative"],
        vec!["--service-status", "--data-dir", "/data", "--autostart"],
        vec!["--autostart", "--service-status"],
    ] {
        assert!(mode_paths(&args(&invalid)).is_err(), "{invalid:?}");
    }
    assert_eq!(
        mode_paths(&args(&[
            "--service-status",
            "--data-dir",
            "/data with spaces"
        ]))
        .unwrap()
        .unwrap()
        .root,
        std::path::PathBuf::from("/data with spaces")
    );
}

#[test]
fn status_lock_is_unique_per_data_directory_and_released_on_exit() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(first.path().into())).unwrap();
    let other = AppPaths::resolve(Some(second.path().into())).unwrap();
    let guard = try_status_lock(&paths).unwrap().unwrap();
    assert!(try_status_lock(&paths).unwrap().is_none());
    assert!(try_status_lock(&other).unwrap().is_some());
    drop(guard);
    assert!(try_status_lock(&paths).unwrap().is_some());
}

#[test]
fn service_readiness_rejects_stale_endpoints_and_an_updater_held_lock() {
    let d = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(d.path().into())).unwrap();
    assert!(!service_running(&paths).unwrap());
    std::fs::write(paths.endpoint_file(), b"{}").unwrap();
    assert!(!service_running(&paths).unwrap());
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(paths.root.join("service.lock"))
        .unwrap();
    assert!(!service_running(&paths).unwrap());
    lock.lock_exclusive().unwrap();
    assert!(service_running(&paths).unwrap());
    std::fs::remove_file(paths.endpoint_file()).unwrap();
    assert!(
        !service_running(&paths).unwrap(),
        "updater lock must not keep a stopped server visible"
    );
}

#[test]
fn status_language_is_validated_and_persisted_without_secrets() {
    let d = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(d.path().into())).unwrap();
    assert_eq!(load_language(&paths), Language::En);
    assert_eq!(Language::parse("ko").unwrap(), Language::Ko);
    assert_eq!(Language::parse("en").unwrap(), Language::En);
    assert!(Language::parse("fr").is_err());
    save_language(&paths, Language::Ko).unwrap();
    assert_eq!(load_language(&paths), Language::Ko);
    let saved: serde_json::Value = serde_json::from_slice(
        &std::fs::read(paths.root.join("service-status-preferences.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved, serde_json::json!({"language":"ko"}));
    std::fs::write(
        paths.root.join("service-status-preferences.json"),
        b"invalid",
    )
    .unwrap();
    assert_eq!(load_language(&paths), Language::En);
    std::fs::write(
        paths.root.join("service-status-preferences.json"),
        br#"{"language":"fr"}"#,
    )
    .unwrap();
    assert_eq!(load_language(&paths), Language::En);
}
