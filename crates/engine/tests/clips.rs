use std::process::Command;
use ytlr_core::{AppPaths, RecordingOptions};
use ytlr_engine::{Tools, media::remux_range, probe};

#[tokio::test]
#[ignore = "requires ffmpeg and ffprobe; run explicitly in media integration tests"]
async fn clips_preserve_original_and_audio_video_tracks() {
    let d = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(d.path().to_owned())).unwrap();
    let tools = Tools::new(paths);
    let source = d.path().join("source.mkv");
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
                "sine=frequency=440:sample_rate=48000",
                "-t",
                "12",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-g",
                "10",
                "-c:a",
                "aac"
            ])
            .arg(&source)
            .status()
            .unwrap()
            .success()
    );
    let original = std::fs::read(&source).unwrap();
    let clip = d.path().join("clip.mp4");
    let result = remux_range(
        &tools,
        std::slice::from_ref(&source),
        &clip,
        &RecordingOptions::default(),
        Some((3.0, 7.0)),
    )
    .await
    .unwrap();
    assert!(result.has_audio && result.has_video);
    assert!(result.duration >= 3.5 && result.duration < 5.5);
    assert_eq!(original, std::fs::read(&source).unwrap());
    assert!(
        remux_range(
            &tools,
            std::slice::from_ref(&source),
            &clip,
            &RecordingOptions::default(),
            Some((3.0, 7.0))
        )
        .await
        .is_err(),
        "never overwrite clips"
    );
    let audio = d.path().join("audio.mka");
    remux_range(
        &tools,
        std::slice::from_ref(&source),
        &audio,
        &RecordingOptions {
            audio_only: true,
            max_height: None,
        },
        Some((1.0, 4.0)),
    )
    .await
    .unwrap();
    let media = probe(&tools, &audio).await.unwrap();
    assert!(media.has_audio && !media.has_video);
    assert!(
        remux_range(
            &tools,
            &[source],
            &d.path().join("bad.mp4"),
            &RecordingOptions::default(),
            Some((7.0, 3.0))
        )
        .await
        .is_err()
    );
}
