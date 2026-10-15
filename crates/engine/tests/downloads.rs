use std::process::Command;
use ytlr_core::{AppPaths, RecordRequest, new_recording_job};
use ytlr_engine::{Tools, capture_args, discover_outputs, probe};

#[tokio::test]
#[ignore = "requires yt-dlp, deno, ffmpeg and ffprobe; run explicitly with media tools"]
async fn native_download_prefers_higher_quality_combined_streams() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(dir.path().into())).unwrap();
    let tools = Tools::new(paths.clone());
    for (format, audio_ext, vcodec, acodec) in [
        ("mp4", "m4a", "avc1", "aac"),
        ("webm", "webm", "vp9", "opus"),
    ] {
        let request = serde_json::from_value(serde_json::json!({
            "url": "https://youtu.be/vodvideo001", "kind": "download", "download_format": format,
            "recording_options": {"max_height": 720}
        }))
        .unwrap();
        let job = new_recording_job(&request, &paths.default_settings(), None, false).unwrap();
        let info = dir.path().join("info.json");
        std::fs::write(&info, serde_json::to_vec(&serde_json::json!({
            "id": "vodvideo001", "title": "Combined video", "extractor": "generic", "extractor_key": "Generic",
            "webpage_url": "https://youtu.be/vodvideo001", "live_status": "not_live", "is_live": false,
            "formats": [
                {"format_id": "video480", "url": "https://example.invalid/video", "ext": format, "height": 480, "vcodec": vcodec, "acodec": "none"},
                {"format_id": "audio", "url": "https://example.invalid/audio", "ext": audio_ext, "vcodec": "none", "acodec": acodec},
                {"format_id": "combined720", "url": "https://example.invalid/combined", "ext": format, "height": 720, "vcodec": vcodec, "acodec": acodec}
            ]
        })).unwrap()).unwrap();
        let mut args = capture_args(&tools, &job, &dir.path().join("attempt-0001"), None, None)
            .unwrap()
            .args;
        args.truncate(args.len() - 2);
        let output = Command::new(tools.require("yt-dlp").unwrap())
            .args(args)
            .args([
                "--simulate",
                "--print",
                "selected:%(format_id)s",
                "--load-info-json",
            ])
            .arg(&info)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.lines().any(|line| line == "selected:combined720"),
            "{format}: {stdout}"
        );
    }
}

#[tokio::test]
#[ignore = "requires ffmpeg and ffprobe; run explicitly with media tools"]
async fn recovered_download_remains_visible_and_yields_to_requested_output() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(dir.path().into())).unwrap();
    let tools = Tools::new(paths.clone());
    let request = serde_json::from_value(serde_json::json!({
        "url": "https://youtu.be/vodvideo001", "kind": "download", "download_format": "mp4"
    }))
    .unwrap();
    let job = new_recording_job(&request, &paths.default_settings(), None, false).unwrap();
    let attempt = job.output_dir.join("attempt-0001");
    std::fs::create_dir_all(&attempt).unwrap();
    let source = attempt.join("source.mkv.part");
    let generated = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=sample_rate=48000",
            "-t",
            "1",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-f",
            "matroska",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let recovered = ytlr_engine::salvage_attempt(&tools, &attempt, &job.recording_options)
        .await
        .unwrap()
        .unwrap();
    for _ in 0..2 {
        let outputs = discover_outputs(
            &tools,
            &attempt,
            &job.recording_options,
            job.download_format,
        )
        .await
        .unwrap();
        assert_eq!(
            outputs.len(),
            1,
            "a previous recovery must remain discoverable"
        );
        assert_eq!(outputs[0].path, recovered.path);
    }
    std::fs::remove_file(source).unwrap();
    let outputs = discover_outputs(
        &tools,
        &attempt,
        &job.recording_options,
        job.download_format,
    )
    .await
    .unwrap();
    assert_eq!(
        outputs[0].path, recovered.path,
        "recovery remains usable after source cleanup"
    );
    let desired = attempt.join("source.mp4");
    let converted = Command::new(tools.require("ffmpeg").unwrap())
        .args(["-v", "error", "-i"])
        .arg(&recovered.path)
        .args(["-c", "copy"])
        .arg(&desired)
        .output()
        .unwrap();
    assert!(
        converted.status.success(),
        "{}",
        String::from_utf8_lossy(&converted.stderr)
    );
    let outputs = discover_outputs(
        &tools,
        &attempt,
        &job.recording_options,
        job.download_format,
    )
    .await
    .unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(
        outputs[0].path, desired,
        "the requested format takes priority over recovery"
    );
}

#[tokio::test]
#[ignore = "requires yt-dlp, deno, ffmpeg and ffprobe; run explicitly with media tools"]
async fn native_download_saves_the_requested_container_and_audio_codec() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(dir.path().into())).unwrap();
    let tools = Tools::new(paths.clone());
    let mut formats = vec![];
    for (name, video, codec, ext) in [
        ("video-mp4", true, "libx264", "mp4"),
        ("video-webm", true, "libvpx-vp9", "webm"),
        ("audio-m4a", false, "aac", "m4a"),
        ("audio-webm", false, "libopus", "webm"),
    ] {
        let file = dir.path().join(format!("{name}.{ext}"));
        // Fixture encoders are provided by the test environment; downloads use Tools below.
        let generated = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                if video {
                    "testsrc2=size=160x90:rate=10"
                } else {
                    "sine=sample_rate=48000"
                },
                "-t",
                "1",
                if video { "-c:v" } else { "-c:a" },
                codec,
            ])
            .arg(&file)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        formats.push(serde_json::json!({
            "format_id": name, "url": url::Url::from_file_path(file).unwrap().as_str(),
            "ext": ext, "vcodec": if video { if ext == "mp4" { "avc1" } else { "vp9" } } else { "none" },
            "acodec": if video { "none" } else { if ext == "m4a" { "aac" } else { "opus" } },
            "height": if video { Some(90) } else { None }, "abr": if ext == "m4a" { 128 } else { 160 }
        }));
    }
    for (index, (format, audio_only, container, codec)) in [
        ("mp4", false, "mp4", "aac"),
        ("mkv", false, "matroska", "opus"),
        ("webm", false, "webm", "opus"),
        ("mp3", true, "mp3", "mp3"),
        ("m4a", true, "m4a", "aac"),
        ("m4a", true, "m4a", "aac"),
    ]
    .into_iter()
    .enumerate()
    {
        let request: RecordRequest = serde_json::from_value(serde_json::json!({
            "url": "https://youtu.be/vodvideo001", "kind": "download", "download_format": format,
            "recording_options": {"audio_only": audio_only, "max_height": if audio_only { None } else { Some(480) }}
        })).unwrap();
        let mut job = new_recording_job(&request, &paths.default_settings(), None, false).unwrap();
        job.attempt = 1;
        let attempt = job.output_dir.join("attempt-0001");
        std::fs::create_dir_all(&attempt).unwrap();
        let info = dir.path().join(format!("info-{index}.json"));
        // The last case must transcode Opus when a native M4A stream is unavailable.
        let available: Vec<_> = formats
            .iter()
            .filter(|f| index != 5 || f["ext"] != "m4a")
            .collect();
        std::fs::write(&info, serde_json::to_vec(&serde_json::json!({
            "id": "vodvideo001", "title": "Local video", "extractor": "generic", "extractor_key": "Generic",
            "webpage_url": "https://youtu.be/vodvideo001", "live_status": "not_live", "is_live": false,
            "duration": 1, "formats": available
        })).unwrap()).unwrap();
        let mut args = capture_args(&tools, &job, &attempt, None, None)
            .unwrap()
            .args;
        args.truncate(args.len() - 2);
        let output = Command::new(tools.require("yt-dlp").unwrap())
            .args(args)
            .args(["--enable-file-urls", "--load-info-json"])
            .arg(info)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{format}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = attempt.join(format!("source.{format}"));
        assert!(expected.is_file(), "{format}: requested output is missing");
        let media = probe(&tools, &expected).await.unwrap();
        assert_eq!(media.has_video, !audio_only);
        assert!(media.has_audio && media.duration > 0.0);
        let outputs = discover_outputs(
            &tools,
            &attempt,
            &job.recording_options,
            job.download_format,
        )
        .await
        .unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(
            outputs[0].path, expected,
            "kept source tracks must not replace the converted result"
        );
        let inspected = Command::new(tools.require("ffprobe").unwrap())
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=format_name:stream=codec_name,codec_type",
                "-of",
                "json",
            ])
            .arg(&expected)
            .output()
            .unwrap();
        assert!(inspected.status.success());
        let metadata: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
        assert!(
            metadata["format"]["format_name"]
                .as_str()
                .unwrap()
                .split(',')
                .any(|name| name == container),
            "{format}: {metadata}"
        );
        assert!(
            metadata["streams"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["codec_type"] == "audio" && s["codec_name"] == codec),
            "{format}: {metadata}"
        );
    }
}

#[tokio::test]
#[ignore = "requires yt-dlp, deno, ffmpeg and ffprobe; run explicitly with media tools"]
async fn native_download_rejects_unavailable_container_or_quality_without_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(dir.path().into())).unwrap();
    let tools = Tools::new(paths.clone());
    for (requested, available, height) in [
        ("mp4", "webm", 90),
        ("webm", "mp4", 90),
        ("mp4", "mp4", 1080),
        ("webm", "webm", 1080),
    ] {
        let request = serde_json::from_value(serde_json::json!({
            "url": "https://youtu.be/vodvideo001", "kind": "download", "download_format": requested,
            "recording_options": {"max_height": 480}
        }))
        .unwrap();
        let job = new_recording_job(&request, &paths.default_settings(), None, false).unwrap();
        let info = dir.path().join("info.json");
        let audio_ext = if available == "mp4" { "m4a" } else { "webm" };
        std::fs::write(&info, serde_json::to_vec(&serde_json::json!({
            "id": "vodvideo001", "title": "Unavailable format", "extractor": "generic", "extractor_key": "Generic",
            "webpage_url": "https://youtu.be/vodvideo001", "live_status": "not_live", "is_live": false,
            "formats": [
                {"format_id": "video", "url": "https://example.invalid/video", "ext": available, "height": height, "vcodec": "vp9", "acodec": "none"},
                {"format_id": "audio", "url": "https://example.invalid/audio", "ext": audio_ext, "vcodec": "none", "acodec": "opus"},
                {"format_id": "combined", "url": "https://example.invalid/combined", "ext": available, "height": height, "vcodec": "vp9", "acodec": "opus"}
            ]
        })).unwrap()).unwrap();
        let mut args = capture_args(&tools, &job, &dir.path().join("attempt-0001"), None, None)
            .unwrap()
            .args;
        args.truncate(args.len() - 2);
        let output = Command::new(tools.require("yt-dlp").unwrap())
            .args(args)
            .args(["--simulate", "--load-info-json"])
            .arg(&info)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{requested}/{available}/{height}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Requested format is not available"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[tokio::test]
#[ignore = "requires yt-dlp, deno, ffmpeg and ffprobe; run explicitly with media tools"]
async fn native_download_reports_postprocessing_even_when_print_silences_logs() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(dir.path().into())).unwrap();
    let tools = Tools::new(paths.clone());
    let video = dir.path().join("video.mp4");
    let audio = dir.path().join("audio.m4a");
    assert!(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=10",
                "-f",
                "lavfi",
                "-i",
                "sine=sample_rate=48000",
                "-map",
                "0:v",
                "-t",
                "1",
                "-c:v",
                "libx264",
            ])
            .arg(&video)
            .args(["-map", "1:a", "-t", "1", "-c:a", "aac"])
            .arg(&audio)
            .status()
            .unwrap()
            .success()
    );
    let info = dir.path().join("info.json");
    std::fs::write(
        &info,
        serde_json::to_vec(&serde_json::json!({
            "id": "vodvideo001", "title": "Local video", "extractor": "generic",
            "extractor_key": "Generic", "webpage_url": "https://youtu.be/vodvideo001",
            "live_status": "not_live", "is_live": false, "duration": 1,
            "formats": [
                {"format_id": "video", "url": url::Url::from_file_path(video).unwrap().as_str(),
                 "ext": "mp4", "vcodec": "avc1", "acodec": "none", "height": 90},
                {"format_id": "audio", "url": url::Url::from_file_path(audio).unwrap().as_str(),
                 "ext": "m4a", "vcodec": "none", "acodec": "aac"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let request: RecordRequest = serde_json::from_value(serde_json::json!({
        "url": "https://youtu.be/vodvideo001", "kind": "download"
    }))
    .unwrap();
    let mut job = new_recording_job(&request, &paths.default_settings(), None, false).unwrap();
    job.attempt = 1;
    let attempt = job.output_dir.join("attempt-0001");
    std::fs::create_dir_all(&attempt).unwrap();
    let mut args = capture_args(&tools, &job, &attempt, None, None)
        .unwrap()
        .args;
    // Exercise the real downloader and merger, using local files rather than YouTube.
    args.truncate(args.len() - 2);
    let output = Command::new(tools.require("yt-dlp").unwrap())
        .args(args)
        .args(["--enable-file-urls", "--load-info-json"])
        .arg(info)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    let media = probe(&tools, &attempt.join("source.mkv")).await.unwrap();
    assert!(media.has_video && media.has_audio && media.duration > 0.0);
    assert!(!stdout.contains("[Merger]") && !stderr.contains("[Merger]"));
    assert!(
        stdout
            .lines()
            .any(|line| line == "__YTLR_FINALIZING__\"vodvideo001\""),
        "missing merge signal: {stdout}\n{stderr}"
    );
}
