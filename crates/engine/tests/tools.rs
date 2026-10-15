#![cfg(target_os = "macos")]

use std::{
    path::{Path, PathBuf},
    process::Command,
};
use ytlr_core::AppPaths;
use ytlr_engine::Tools;

fn write_tools(directory: &Path, mp3: bool) {
    std::fs::create_dir_all(directory).unwrap();
    for name in ["ffmpeg", "ffprobe", "yt-dlp"] {
        let path = directory.join(name);
        let encoders = if mp3 {
            " A....D libmp3lame           libmp3lame MP3 (MPEG audio layer 3) (codec mp3)"
        } else {
            " A....D aac                  AAC (Advanced Audio Coding)"
        };
        std::fs::write(&path, format!("#!/bin/sh\ncase \"$*\" in\n  '-hide_banner -encoders') echo '{encoders}' ;;\n  *) echo 'ffmpeg version test-build' ;;\nesac\n")).unwrap();
        ytlr_engine::tools::make_executable(&path).unwrap();
    }
}

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
    write_tools(&bundled, true);
    write_tools(&override_dir, false);
    for (generation, mp3, bundle_available, explicit_override, rollback_from, expected_source) in [
        ("2026-09-21.2-test", false, true, false, None, "bundled"),
        ("2026-10-15-relabeled", false, true, false, None, "bundled"),
        ("2027-01-01-relabeled", false, true, false, None, "bundled"),
        ("2026-09-21.2-capable", true, true, false, None, "managed"),
        ("2026-10-15-test", true, true, false, None, "managed"),
        ("2027-01-01-test", true, true, false, None, "managed"),
        ("2026-09-21.2-test", false, false, false, None, "managed"),
        ("2026-09-21.2-test", false, true, true, None, "override"),
        (
            "2026-09-21.2-test",
            false,
            true,
            false,
            Some("2026-10-15-test"),
            "managed",
        ),
        (
            "2026-09-20-test",
            false,
            true,
            false,
            Some("2026-09-21.2-test"),
            "managed",
        ),
    ] {
        let managed = paths.tools_dir().join(generation);
        write_tools(&managed, mp3);
        std::fs::write(paths.tools_dir().join("active.json"),
            serde_json::to_vec(&serde_json::json!({"current": rollback_from.unwrap_or(generation), "previous": rollback_from.map(|_| generation)})).unwrap()).unwrap();
        if let Some(from) = rollback_from {
            write_tools(&paths.tools_dir().join(from), true);
            Tools::new(paths.clone()).rollback().unwrap();
        }
        let expected = match expected_source {
            "bundled" => &bundled,
            "override" => &override_dir,
            _ => &managed,
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

#[tokio::test]
async fn reinstalled_legacy_ffmpeg_can_upgrade_when_a_capable_bundle_appears() {
    if let Some(home) = std::env::var_os("YTLR_TOOL_REINSTALL_TEST_HOME") {
        let paths = AppPaths::resolve(Some(home.into())).unwrap();
        let legacy = paths.tools_dir().join("2026-09-21.2-test");
        write_tools(&legacy, false);
        let pointer = paths.tools_dir().join("active.json");
        let activate = |name: &str| {
            std::fs::write(
                &pointer,
                serde_json::to_vec(&serde_json::json!({"current": name, "previous": null}))
                    .unwrap(),
            )
            .unwrap();
        };
        activate("2026-09-21.2-test");
        let tools = Tools::new(paths.clone());
        let reinstalled = paths.tools_dir().join("2026-10-15-reinstalled");
        std::fs::create_dir_all(&reinstalled).unwrap();
        for name in ["ffmpeg", "ffprobe"] {
            let source = tools.redistributable_tool(name).await.unwrap();
            assert_eq!(source, legacy.join(name));
            std::fs::copy(source, reinstalled.join(name)).unwrap();
        }
        activate("2026-10-15-reinstalled");
        assert_eq!(tools.locate("ffmpeg").unwrap(), reinstalled.join("ffmpeg"));
        let bundled = PathBuf::from(std::env::var_os("YTLR_BUNDLED_TOOLS").unwrap());
        write_tools(&bundled, true);
        let ffmpeg = bundled.join("ffmpeg");
        let script = std::fs::read_to_string(&ffmpeg).unwrap();
        std::fs::write(&ffmpeg, format!("#!/bin/sh\nif [ ! -e \"$0.failed-once\" ]; then : > \"$0.failed-once\"; exit 1; fi\n{script}")).unwrap();
        assert_eq!(tools.locate("ffmpeg").unwrap(), reinstalled.join("ffmpeg"));
        for name in ["ffmpeg", "ffprobe"] {
            assert_eq!(
                tools.clone().locate(name).unwrap(),
                bundled.join(name),
                "a transient probe failure must not permanently hide the MP3-capable bundle"
            );
        }
        // Refresh a cached result if a replaceable tool changes at the same path.
        write_tools(&reinstalled, true);
        assert_eq!(tools.locate("ffmpeg").unwrap(), reinstalled.join("ffmpeg"));
        write_tools(&reinstalled, false);
        assert_eq!(tools.locate("ffmpeg").unwrap(), bundled.join("ffmpeg"));
        write_tools(&bundled, false);
        for name in ["ffmpeg", "ffprobe"] {
            assert_eq!(
                tools.locate(name).unwrap(),
                reinstalled.join(name),
                "a bundle without an MP3 encoder is not an upgrade"
            );
        }
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "reinstalled_legacy_ffmpeg_can_upgrade_when_a_capable_bundle_appears",
            "--nocapture",
        ])
        .env("YTLR_TOOL_REINSTALL_TEST_HOME", dir.path())
        .env("YTLR_BUNDLED_TOOLS", dir.path().join("bundled"))
        .env_remove("YTLR_FFMPEG")
        .env_remove("YTLR_FFPROBE")
        .env_remove("YTLR_YT_DLP")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
