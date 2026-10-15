#![cfg(target_os = "macos")]

use std::{path::PathBuf, process::Command};
use ytlr_core::AppPaths;
use ytlr_engine::Tools;

#[tokio::test]
async fn upgraded_macos_bundle_replaces_legacy_managed_ffmpeg() {
    // Isolate process-wide tool environment overrides from parallel tests.
    if let Some(home) = std::env::var_os("YTLR_TOOL_SELECTION_TEST_HOME") {
        let paths = AppPaths::resolve(Some(home.into())).unwrap();
        let tools = Tools::new(paths);
        let expected = PathBuf::from(std::env::var_os("YTLR_TOOL_SELECTION_EXPECTED").unwrap());
        let install = PathBuf::from(std::env::var_os("YTLR_TOOL_SELECTION_INSTALL").unwrap());
        for name in ["ffmpeg", "ffprobe"] {
            assert_eq!(tools.locate(name).unwrap(), expected.join(name));
            assert_eq!(
                tools.redistributable_tool(name).await.unwrap(),
                install.join(name),
                "reinstall must copy the upgraded engine instead of the stale managed binary"
            );
        }
        let managed = PathBuf::from(std::env::var_os("YTLR_TOOL_SELECTION_MANAGED").unwrap());
        assert_eq!(tools.locate("yt-dlp").unwrap(), managed.join("yt-dlp"));
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(dir.path().into())).unwrap();
    let bundled = dir.path().join("bundled");
    let override_dir = dir.path().join("override");
    let write_tools = |directory: &std::path::Path| {
        std::fs::create_dir_all(directory).unwrap();
        for name in ["ffmpeg", "ffprobe", "yt-dlp"] {
            let path = directory.join(name);
            std::fs::write(&path, "#!/bin/sh\necho 'ffmpeg version test-build'\n").unwrap();
            ytlr_engine::tools::make_executable(&path).unwrap();
        }
    };
    write_tools(&bundled);
    write_tools(&override_dir);
    for (generation, bundle_available, explicit_override, rollback_from) in [
        ("2026-09-21.2-test", true, false, None),
        ("2026-10-15-test", true, false, None),
        ("2027-01-01-test", true, false, None),
        ("2026-09-21.2-test", false, false, None),
        ("2026-09-21.2-test", true, true, None),
        ("2026-09-21.2-test", true, false, Some("2026-10-15-test")),
        ("2026-09-20-test", true, false, Some("2026-09-21.2-test")),
    ] {
        let managed = paths.tools_dir().join(generation);
        write_tools(&managed);
        std::fs::write(paths.tools_dir().join("active.json"),
            serde_json::to_vec(&serde_json::json!({"current": rollback_from.unwrap_or(generation), "previous": rollback_from.map(|_| generation)})).unwrap()).unwrap();
        if let Some(from) = rollback_from {
            write_tools(&paths.tools_dir().join(from));
            Tools::new(paths.clone()).rollback().unwrap();
        }
        let expected = if explicit_override {
            &override_dir
        } else if generation == "2026-09-21.2-test" && bundle_available && rollback_from.is_none() {
            &bundled
        } else {
            &managed
        };
        let install = if explicit_override {
            &override_dir
        } else if bundle_available {
            &bundled
        } else {
            &managed
        };
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "upgraded_macos_bundle_replaces_legacy_managed_ffmpeg",
                "--nocapture",
            ])
            .env("YTLR_TOOL_SELECTION_TEST_HOME", dir.path())
            .env("YTLR_TOOL_SELECTION_EXPECTED", expected)
            .env("YTLR_TOOL_SELECTION_INSTALL", install)
            .env("YTLR_TOOL_SELECTION_MANAGED", &managed)
            .env_remove("YTLR_FFMPEG")
            .env_remove("YTLR_FFPROBE")
            .env_remove("YTLR_YT_DLP")
            .env_remove("YTLR_BUNDLED_TOOLS");
        if bundle_available {
            child.env("YTLR_BUNDLED_TOOLS", &bundled);
        }
        if explicit_override {
            child.env("YTLR_FFMPEG", override_dir.join("ffmpeg"));
            child.env("YTLR_FFPROBE", override_dir.join("ffprobe"));
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{generation}/{bundle_available}/{explicit_override}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
