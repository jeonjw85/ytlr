use std::{collections::HashMap, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use ytlr_core::{AppPaths, GapStatus, TimelineGap};
use ytlr_engine::{StreamSource, Tools, VideoInfo, recovery};

#[tokio::test]
#[ignore = "requires ffmpeg and ffprobe; run explicitly in media integration tests"]
async fn dated_hls_recovery_verifies_actual_media_and_rejects_wrong_ranges() {
    check_hls_recovery(false).await;
}

#[tokio::test]
#[ignore = "requires ffmpeg and ffprobe; run explicitly in media integration tests"]
async fn audio_only_hls_recovery_does_not_require_a_video_timeline() {
    check_hls_recovery(true).await;
}

async fn check_hls_recovery(audio_only: bool) {
    let d = tempfile::tempdir().unwrap();
    let tools = Tools::new(AppPaths::resolve(Some(d.path().join("app"))).unwrap());
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
            "9",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "10",
            "-c:a",
            "aac",
            "-f",
            "hls",
            "-hls_time",
            "3",
            "-hls_list_size",
            "0",
            "-hls_segment_filename",
        ])
        .arg(d.path().join("part-%03d.ts"))
        .args(if audio_only {
            vec!["-map", "1:a:0"]
        } else {
            vec!["-map", "0:v:0", "-map", "1:a:0"]
        })
        .arg(d.path().join("original.m3u8"))
        .status()
        .unwrap();
    assert!(generated.success());
    let mut files = HashMap::new();
    files.insert("/live.m3u8".to_string(), b"#EXTM3U\n#EXT-X-PROGRAM-DATE-TIME:2026-01-01T00:00:00Z\n#EXTINF:3,\npart-000.ts\n#EXTINF:3,\npart-001.ts\n#EXTINF:3,\npart-002.ts\n#EXT-X-ENDLIST\n".to_vec());
    for n in 0..3 {
        files.insert(
            format!("/part-{n:03}.ts"),
            std::fs::read(d.path().join(format!("part-{n:03}.ts"))).unwrap(),
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
                let mut buf = [0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap();
                let path = std::str::from_utf8(&buf[..n])
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
    let info = VideoInfo {
        id: "abcdefghijk".into(),
        title: "fixture".into(),
        channel: "fixture".into(),
        live_status: "is_live".into(),
        format: "fixture".into(),
        resolution: None,
        expected_tracks: 1,
        sources: vec![StreamSource {
            url: format!("http://{address}/live.m3u8"),
            headers: HashMap::new(),
            video: !audio_only,
            audio: true,
        }],
    };
    let mut gap = TimelineGap {
        after_attempt: 1,
        started_at: "2026-01-01T00:00:03Z".into(),
        ended_at: Some("2026-01-01T00:00:06Z".into()),
        seconds: 3.0,
        status: GapStatus::Unrecovered,
        evidence: None,
    };
    let result = recovery::recover(
        &tools,
        &info,
        &gap,
        &d.path().join("recovery"),
        &ytlr_core::RecordingOptions {
            audio_only,
            max_height: None,
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(result.output.has_video, !audio_only);
    assert!(result.output.has_audio);
    assert!((result.output.duration - 3.0).abs() < 0.5);
    assert_eq!(
        result.video_start.as_deref(),
        if audio_only {
            None
        } else {
            Some("2026-01-01T00:00:03+00:00")
        }
    );
    assert!(result.output.path.is_file());
    gap.started_at = "2025-12-31T23:59:30Z".into();
    assert!(
        recovery::recover(
            &tools,
            &info,
            &gap,
            &d.path().join("wrong-time"),
            &ytlr_core::RecordingOptions {
                audio_only,
                max_height: None
            },
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    assert!(!d.path().join("wrong-time/candidate.mkv").exists());
    server.abort();
}
