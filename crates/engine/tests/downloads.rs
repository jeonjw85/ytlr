use std::process::Command;
use ytlr_core::{AppPaths, RecordRequest, new_recording_job};
use ytlr_engine::{Tools, capture_args, probe};

#[tokio::test]
#[ignore = "requires yt-dlp, deno, ffmpeg and ffprobe; run explicitly with media tools"]
async fn native_download_reports_postprocessing_even_when_print_silences_logs() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(dir.path().into())).unwrap();
    let tools = Tools::new(paths.clone());
    let video = dir.path().join("video.mp4");
    let audio = dir.path().join("audio.m4a");
    assert!(
        Command::new(tools.require("ffmpeg").unwrap())
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
