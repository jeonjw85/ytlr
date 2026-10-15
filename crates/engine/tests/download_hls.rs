use std::{collections::HashMap, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use ytlr_core::{AppPaths, new_recording_job};
use ytlr_engine::{Tools, capture_args};

#[tokio::test]
#[ignore = "requires yt-dlp, deno, ffmpeg and ffprobe; run explicitly with media tools"]
async fn hls_download_remuxes_transport_stream_segments_into_real_mp4() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(dir.path().join("app"))).unwrap();
    let tools = Tools::new(paths.clone());
    let generated = std::process::Command::new("ffmpeg")
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
            "2",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-g",
            "10",
            "-f",
            "hls",
            "-hls_time",
            "1",
            "-hls_segment_filename",
        ])
        .arg(dir.path().join("part-%03d.ts"))
        .arg(dir.path().join("video.m3u8"))
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let mut files = HashMap::new();
    for name in ["video.m3u8", "part-000.ts", "part-001.ts"] {
        files.insert(
            format!("/{name}"),
            std::fs::read(dir.path().join(name)).unwrap(),
        );
    }
    let files = Arc::new(files);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let files = files.clone();
            tokio::spawn(async move {
                let mut request = [0; 8192];
                let count = socket.read(&mut request).await.unwrap();
                let path = std::str::from_utf8(&request[..count])
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let bytes = &files[path];
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            bytes.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                socket.write_all(bytes).await.unwrap();
            });
        }
    });
    let request = serde_json::from_value(serde_json::json!({
        "url": "https://youtu.be/vodhls00001", "kind": "download", "download_format": "mp4",
        "recording_options": {"max_height": 480}
    }))
    .unwrap();
    let job = new_recording_job(&request, &paths.default_settings(), None, false).unwrap();
    let attempt = job.output_dir.join("attempt-0001");
    std::fs::create_dir_all(&attempt).unwrap();
    let info = dir.path().join("info.json");
    std::fs::write(&info, serde_json::to_vec(&serde_json::json!({
        "id": "vodhls00001", "title": "Combined HLS", "extractor": "generic", "extractor_key": "Generic",
        "webpage_url": "https://youtu.be/vodhls00001", "live_status": "not_live", "is_live": false,
        "duration": 2, "formats": [{
            "format_id": "hls", "url": format!("http://{address}/video.m3u8"), "protocol": "m3u8_native",
            "ext": "mp4", "height": 90, "vcodec": "avc1", "acodec": "aac"
        }]
    })).unwrap()).unwrap();
    let mut args = capture_args(&tools, &job, &attempt, None, None)
        .unwrap()
        .args;
    args.truncate(args.len() - 2);
    let output = tokio::process::Command::new(tools.require("yt-dlp").unwrap())
        .args(args)
        .arg("--load-info-json")
        .arg(info)
        .output()
        .await
        .unwrap();
    server.abort();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let inspected = tokio::process::Command::new(tools.require("ffprobe").unwrap())
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=format_name:stream=codec_type",
            "-of",
            "json",
        ])
        .arg(attempt.join("source.mp4"))
        .output()
        .await
        .unwrap();
    assert!(inspected.status.success());
    let metadata: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert!(
        metadata["format"]["format_name"]
            .as_str()
            .unwrap()
            .split(',')
            .any(|name| name == "mp4"),
        "wrong container: {metadata}"
    );
    for kind in ["video", "audio"] {
        assert!(
            metadata["streams"]
                .as_array()
                .unwrap()
                .iter()
                .any(|stream| stream["codec_type"] == kind)
        );
    }
}
