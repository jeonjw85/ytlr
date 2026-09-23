use std::fs;
use ytlr_core::*;
use ytlr_engine::{Tools, checkpoint, cleanup, salvage_attempt, verify_ledger};

#[tokio::test]
#[ignore = "requires ffmpeg and ffprobe; run explicitly in media integration tests"]
async fn cleanup_revalidates_preserves_results_and_keeps_backup_ledger_truthful() {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::resolve(Some(dir.path().join("app"))).unwrap();
    paths.initialize().unwrap();
    let tools = Tools::new(paths.clone());
    let store = Store::open(&paths.database(), &paths.default_settings()).unwrap();
    let req: RecordRequest =
        serde_json::from_value(serde_json::json!({"url":"https://youtu.be/abcdefghijk"})).unwrap();
    let job = store.add_job(&req, None, false).unwrap();
    let attempt = job.output_dir.join("attempt-0001");
    fs::create_dir_all(&attempt).unwrap();
    assert!(
        std::process::Command::new("ffmpeg")
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
                "3",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-g",
                "10",
                "-c:a",
                "aac",
                "-f",
                "segment",
                "-segment_time",
                "1",
                "-reset_timestamps",
                "1",
                "-segment_list"
            ])
            .arg(attempt.join("segments.csv"))
            .arg(attempt.join("part-%03d.mkv"))
            .status()
            .unwrap()
            .success()
    );
    checkpoint(&attempt, &mut Default::default()).unwrap();
    let mut output = salvage_attempt(&tools, &attempt, &job.recording_options)
        .await
        .unwrap()
        .unwrap();
    output.sha256 = Some(file_digest(&output.path).unwrap().1);
    let job = store
        .update_job(&job.id, |j| {
            j.state = JobState::Stopped;
            j.attempt = 1;
            j.outputs = vec![output.clone()];
        })
        .unwrap();
    fs::write(attempt.join("source.part"), b"keep incomplete source").unwrap();
    fs::write(job.output_dir.join("export-manual.mp4"), b"keep export").unwrap();
    let plan = cleanup::preview(&tools, &job).await.unwrap();
    assert_eq!(plan.files.len(), 3);
    let candidate = job.output_dir.join(&plan.files[0].path);
    let original = fs::read(&candidate).unwrap();
    fs::write(&candidate, b"changed").unwrap();
    assert!(
        cleanup::execute(
            &tools,
            &job,
            &cleanup::CleanupRequest {
                plan_id: plan.id.clone(),
                all: true,
                files: vec![]
            }
        )
        .await
        .is_err()
    );
    fs::write(&candidate, original).unwrap();
    assert!(
        cleanup::execute(
            &tools,
            &job,
            &cleanup::CleanupRequest {
                plan_id: plan.id.clone(),
                all: false,
                files: vec!["../outside".into()]
            }
        )
        .await
        .is_err()
    );
    let result_bytes = fs::read(&output.path).unwrap();
    fs::write(&output.path, b"bad result").unwrap();
    assert!(cleanup::preview(&tools, &job).await.is_err());
    fs::write(&output.path, &result_bytes).unwrap();
    let removed = cleanup::execute(
        &tools,
        &job,
        &cleanup::CleanupRequest {
            plan_id: plan.id.clone(),
            all: false,
            files: vec![plan.files[0].path.clone()],
        },
    )
    .await
    .unwrap();
    assert_eq!(removed, plan.files[0].bytes);
    assert!(!candidate.exists());
    assert!(verify_ledger(&attempt).unwrap().is_empty());
    assert_eq!(fs::read(&output.path).unwrap(), result_bytes);
    assert!(attempt.join("source.part").exists());
    assert!(job.output_dir.join("export-manual.mp4").exists());
    assert!(
        cleanup::execute(
            &tools,
            &job,
            &cleanup::CleanupRequest {
                plan_id: plan.id,
                all: true,
                files: vec![]
            }
        )
        .await
        .is_err()
    );
    let next = cleanup::preview(&tools, &job).await.unwrap();
    assert_eq!(next.files.len(), 2);
    {
        use fs2::FileExt;
        let backup_lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(job.output_dir.join("backup.lock"))
            .unwrap();
        backup_lock.try_lock_exclusive().unwrap();
        assert!(
            cleanup::preview(&tools, &job).await.is_err(),
            "cleanup must not overlap a backup"
        );
    }
    let backup = dir.path().join("backup");
    fs::create_dir(&backup).unwrap();
    mirror_job(&BackupSpec {
        job: job.clone(),
        root: backup.clone(),
        device: None,
        identity: None,
    })
    .unwrap();
    let backed_attempt = backup
        .join(&job.video_id)
        .join(&job.id)
        .join("attempt-0001");
    assert!(verify_ledger(&backed_attempt).unwrap().is_empty());
    assert!(
        backed_attempt
            .join(output.path.file_name().unwrap())
            .is_file()
    );
    cleanup::execute(
        &tools,
        &job,
        &cleanup::CleanupRequest {
            plan_id: next.id,
            all: true,
            files: vec![],
        },
    )
    .await
    .unwrap();
    assert!(
        cleanup::preview(&tools, &job)
            .await
            .unwrap()
            .files
            .is_empty()
    );
    assert!(verify_ledger(&attempt).unwrap().is_empty());
    assert_eq!(fs::read(&output.path).unwrap(), result_bytes);
    let mut partial = job.clone();
    partial.continuity_uncertain = true;
    assert!(cleanup::preview(&tools, &partial).await.is_err());
}
