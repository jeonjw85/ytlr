use crate::Service;
use anyhow::Result;
use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use ytlr_core::*;
use ytlr_engine::*;

pub async fn schedule(state: Arc<Service>) {
    let mut tasks = JoinSet::new();
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            _ = tick.tick() => {
                if state.installing.load(Ordering::SeqCst) { continue; }
                let Ok(settings) = state.store.settings() else { continue; };
                let Ok(mut jobs) = state.store.jobs() else { continue; };
                jobs.sort_by(|a,b| b.priority.cmp(&a.priority).then_with(|| a.created_at.cmp(&b.created_at)));
                for job in jobs {
                    if !matches!(job.state, JobState::Queued | JobState::Waiting | JobState::Reconnecting) || job.stop_requested { continue; }
                    if state.next_checks.lock().await.get(&job.id).is_some_and(|at| *at > Instant::now()) { continue; }
                    let mut active = state.active.lock().await;
                    if state.installing.load(Ordering::SeqCst) { break; }
                    if active.contains_key(&job.id) { continue; }
                    if active.len() >= settings.max_recordings {
                        let _ = state.store.update_job(&job.id, |j| { j.message = "동시 녹화 상한 도달 · 대기 중 앞부분이 누락될 수 있습니다.".into(); });
                        continue;
                    }
                    if state.tool_status.read().await.iter().any(|t| t.error.is_some()) {
                        let errors = state.tool_status.read().await.iter().filter_map(|t| t.error.as_ref().map(|e| format!("{}: {}", t.name, e))).collect::<Vec<_>>().join("; ");
                        let _ = state.store.update_job(&job.id, |j| { j.message = format!("엔진 재확인 대기 · {errors}"); });
                        continue;
                    }
                    if let Err(e) = ensure_storage(&job.output_dir, settings.min_free_bytes) {
                        let _ = state.store.update_job(&job.id, |j| { j.message = e.to_string(); });
                        continue;
                    }
                    let cancel = state.shutdown.child_token();
                    active.insert(job.id.clone(), cancel.clone());
                    let s = state.clone();
                    tasks.spawn(async move {
                        let result = run_job(s.clone(), job.clone(), cancel).await;
                        if let Err(e) = result {
                            let _ = s.store.event(&job.id, "error", &format!("{e:#}"));
                            let _ = s.store.update_job(&job.id, |j| { j.state = JobState::Failed; j.message = redact(&format!("{e:#}")); });
                        }
                        s.active.lock().await.remove(&job.id);
                    });
                }
            }
        }
    }
    for cancel in state.active.lock().await.values() {
        cancel.cancel();
    }
    while tasks.join_next().await.is_some() {}
}

fn auth(state: &Service) -> (Option<std::path::PathBuf>, Option<std::path::PathBuf>) {
    let settings = state.store.settings().ok();
    (
        settings.as_ref().and_then(|s| s.cookies_path.clone()),
        settings.and_then(|s| s.po_token_path),
    )
}

async fn run_job(state: Arc<Service>, job: RecordingJob, cancel: CancellationToken) -> Result<()> {
    state.store.update_job(&job.id, |j| {
        j.state = JobState::Preparing;
        j.message = "방송 정보 확인 중".into();
    })?;
    let (cookie_file, po_token) = auth(&state);
    let info = tokio::select! {
        _ = cancel.cancelled() => { interrupted_state(&state, &job.id)?; return Ok(()); },
        result = inspect(&state.tools, &job.url, cookie_file.as_deref(), po_token.as_deref()) => result,
    };
    let info = match info {
        Ok(info) => info,
        Err(e) => {
            retry(&state, &job.id, &e.to_string()).await?;
            return Ok(());
        }
    };
    state.store.update_job(&job.id, |j| {
        j.title = info.title.clone();
        j.channel = info.channel.clone();
        j.format = info.format.clone();
    })?;
    if info.live_status == "is_upcoming" {
        state.store.update_job(&job.id, |j| {
            j.state = JobState::Waiting;
            j.message = "예약 방송 시작 대기".into();
        })?;
        state
            .next_checks
            .lock()
            .await
            .insert(job.id.clone(), Instant::now() + Duration::from_secs(30));
        return Ok(());
    }
    if info.live_status != "is_live" {
        if job.attempt > 0 {
            finish(&state, &job.id, false).await?;
        } else {
            state.store.update_job(&job.id, |j| {
                j.state = JobState::Failed;
                j.message = "현재 라이브 또는 예약 방송이 아닙니다.".into();
            })?;
        }
        return Ok(());
    }
    let settings = state.store.settings()?;
    let started_at = now();
    let job = state.store.update_job(&job.id, |j| {
        j.attempt += 1;
        j.state = JobState::Recording;
        j.message = "스트림 연결 중".into();
        if j.attempt > 1 {
            j.continuity_uncertain = true;
        }
        if j.started_at.is_none() {
            j.started_at = Some(started_at.clone());
        }
        note_attempt_start(j, j.attempt, &started_at);
    })?;
    state.store.event(
        &job.id,
        "recording",
        &format!("수집 시도 {} 시작 · 원본은 별도 경로에 보존", job.attempt),
    )?;
    let result = capture(
        state.tools.clone(),
        state.store.clone(),
        job.clone(),
        info,
        settings,
        cancel.clone(),
    )
    .await?;
    state.store.update_job(&job.id, |j| {
        note_attempt_end(
            j,
            job.attempt,
            &result.ended_at,
            result.media_seconds,
            result.bytes,
        );
    })?;
    if state.store.job(&job.id)?.stop_requested {
        finish(&state, &job.id, false).await?;
    } else if cancel.is_cancelled() {
        interrupted_state(&state, &job.id)?;
    } else if result.success && !result.interrupted {
        // Exit status alone does not prove the broadcast ended.
        match inspect(
            &state.tools,
            &job.url,
            auth(&state).0.as_deref(),
            auth(&state).1.as_deref(),
        )
        .await
        {
            Ok(info)
                if matches!(
                    info.live_status.as_str(),
                    "post_live" | "was_live" | "not_live"
                ) =>
            {
                finish(&state, &job.id, true).await?
            }
            _ => {
                retry(
                    &state,
                    &job.id,
                    "수집 프로세스가 종료됐지만 방송 종료를 확인하지 못했습니다.",
                )
                .await?
            }
        }
    } else {
        retry(&state, &job.id, &result.reason).await?;
    }
    Ok(())
}

fn interrupted_state(state: &Service, id: &str) -> Result<()> {
    state.store.update_job(id, |j| {
        j.state = if j.stop_requested {
            JobState::Stopped
        } else {
            JobState::Queued
        };
        j.continuity_uncertain |= j.attempt > 0;
        j.message = "서비스 종료 · 원본 보존 · 다음 실행에서 재개".into();
    })?;
    Ok(())
}

async fn retry(state: &Service, id: &str, message: &str) -> Result<()> {
    let settings = state.store.settings()?;
    let job = state.store.update_job(id, |j| {
        j.retries += 1;
        j.continuity_uncertain |= j.attempt > 0;
        j.state = if j.retries > settings.max_retries {
            JobState::Failed
        } else {
            JobState::Reconnecting
        };
        j.message = if message.is_empty() {
            "수집 오류 · 재연결 대기".into()
        } else {
            redact(message)
        };
    })?;
    state.store.event(
        id,
        "retry",
        &format!("재시도 {}: {}", job.retries, job.message),
    )?;
    if job.state == JobState::Failed && job.attempt > 0 {
        finish(state, id, false).await?;
    }
    let delay = 5 * 2u64.pow(job.retries.min(5));
    state
        .next_checks
        .lock()
        .await
        .insert(id.into(), Instant::now() + Duration::from_secs(delay));
    Ok(())
}

pub async fn finish(state: &Service, id: &str, natural_end: bool) -> Result<()> {
    state.store.update_job(id, |j| {
        j.state = JobState::Finalizing;
        j.message = "원본 검사·복구 중".into();
    })?;
    let _permit = state.finalizer.acquire().await?;
    let job = state.store.job(id)?;
    let root = job.output_dir.clone();
    let issues = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
        let mut issues = vec![];
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                issues.extend(verify_ledger(&entry.path())?);
            }
        }
        Ok(issues)
    })
    .await??;
    let integrity_ok = issues.is_empty();
    for issue in issues {
        state.store.event(id, "integrity", &issue)?;
    }
    if !integrity_ok {
        state
            .store
            .update_job(id, |j| j.continuity_uncertain = true)?;
    }
    fill_gaps(state, id).await?;
    let job = state.store.job(id)?;
    let mut outputs = discover_outputs(&state.tools, &job.output_dir).await?;
    for n in 1..=job.attempt {
        let dir = job.output_dir.join(format!("attempt-{n:04}"));
        if !dir.exists() || outputs.iter().any(|o| o.path.starts_with(&dir)) {
            continue;
        }
        match salvage_attempt(&state.tools, &dir).await {
            Ok(Some(output)) => {
                outputs.push(output);
            }
            Ok(None) => {
                state.store.event(id, "recovery", &format!("시도 {n}: 병합 가능한 영상·음성 쌍을 찾지 못했습니다. 원본 조각은 보존됩니다."))?;
            }
            Err(e) => {
                state
                    .store
                    .event(id, "recovery", &format!("시도 {n} 복구 실패: {e}"))?;
            }
        }
    }
    let total_bytes = files_under(&job.output_dir)?
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    let duration = outputs.iter().map(|o| o.duration).sum();
    let previous_hashes: std::collections::HashMap<_, _> = job
        .outputs
        .iter()
        .filter_map(|o| o.sha256.as_ref().map(|h| (o.path.clone(), h.clone())))
        .collect();
    outputs = tokio::task::spawn_blocking(move || -> Result<Vec<MediaOutput>> {
        for output in &mut outputs {
            let hash = file_digest(&output.path)?.1;
            if previous_hashes
                .get(&output.path)
                .is_some_and(|previous| previous != &hash)
            {
                anyhow::bail!("이전에 검증한 결과 파일이 변경되었습니다. 원본/백업을 보존합니다.");
            }
            output.sha256 = Some(hash);
        }
        Ok(outputs)
    })
    .await??;
    let job = state.store.job(id)?;
    for evidence in job.gaps.iter().filter_map(|g| g.evidence.as_ref()) {
        if evidence.output.path.is_file() {
            outputs.push(evidence.output.clone());
        }
    }
    let open_gaps = job
        .gaps
        .iter()
        .filter(|gap| gap.status != GapStatus::Recovered)
        .count();
    let stopped = job.stop_requested;
    let complete = integrity_ok
        && natural_end
        && !stopped
        && !job.continuity_uncertain
        && job.attempt == 1
        && open_gaps == 0
        && !outputs.is_empty();
    let job = state.store.update_job(id, |j| {
        j.outputs = outputs;
        j.bytes = total_bytes;
        j.media_seconds = duration;
        j.continuity_uncertain |= open_gaps > 0;
        j.state = if complete {
            JobState::Completed
        } else if stopped {
            JobState::Stopped
        } else if j.attempt > 0 {
            JobState::Partial
        } else {
            JobState::Failed
        };
        j.message = if complete {
            "수집 종료 · 컨테이너·영상·음성 검사 통과 (전체 디코딩 검사는 미실행)".into()
        } else if stopped {
            "중지했습니다. 저장된 파일은 보관됩니다.".into()
        } else if open_gaps > 0 {
            format!("부분 보관 · 수신 공백/경계 미검증 {open_gaps}구간 · 원본 보존")
        } else {
            "부분 보관 · 구간 연속성을 확인할 수 없습니다. 원본을 보존합니다.".into()
        };
    })?;
    atomic_write(
        &job.output_dir.join("recording.json"),
        &serde_json::to_vec_pretty(&job)?,
    )?;
    state.store.event(id, "finished", &job.message)?;
    Ok(())
}

async fn fill_gaps(state: &Service, id: &str) -> Result<()> {
    let Ok(_permit) = state.gap_recovery.try_acquire() else {
        return Ok(());
    };
    let job = state.store.job(id)?;
    if job.stop_requested || state.shutdown.is_cancelled() {
        return Ok(());
    }
    let gaps = fillable_gaps(&job);
    if gaps.is_empty() {
        return Ok(());
    }
    let cancel = state
        .active
        .lock()
        .await
        .get(id)
        .cloned()
        .unwrap_or_else(|| state.shutdown.child_token());
    let (cookie_file, po_token) = auth(state);
    let info = match tokio::select! { _ = cancel.cancelled() => return Ok(()), result = inspect(
        &state.tools,
        &job.url,
        cookie_file.as_deref(),
        po_token.as_deref(),
    ) => result }
    {
        Ok(info) => info,
        Err(e) => {
            state
                .store
                .event(id, "gap", &format!("누락 복구용 방송 확인 실패: {e}"))?;
            return Ok(());
        }
    };
    let settings = state.store.settings()?;
    for gap in gaps.into_iter().take(4) {
        if state.store.job(id)?.stop_requested || cancel.is_cancelled() {
            break;
        }
        ensure_storage(&job.output_dir, settings.min_free_bytes)?;
        let directory = job.output_dir.join(format!(
            "recovery-{}-{}",
            gap.after_attempt,
            uuid::Uuid::new_v4()
        ));
        match ytlr_engine::recovery::recover(&state.tools, &info, &gap, &directory, cancel.clone())
            .await
        {
            Ok(evidence) => {
                let updated = state.store.update_job(id, |j| {
                    j.outputs.push(evidence.output.clone());
                    mark_gap_fill(j, gap.after_attempt, evidence);
                    j.continuity_uncertain = true;
                })?;
                atomic_write(
                    &job.output_dir.join("recording.json"),
                    &serde_json::to_vec_pretty(&updated)?,
                )?;
                state.store.event(id, "gap_candidate", "요청 시각을 포함하는 영상·음성 보충 파일 확보. 원래 녹화와의 접합 경계는 미검증입니다.")?;
            }
            Err(e) => {
                state
                    .store
                    .event(id, "gap_unverified", &format!("구간 보충 미확인: {e}"))?;
            }
        }
    }
    Ok(())
}

pub async fn recover_gaps(state: Arc<Service>) {
    let mut checked = std::collections::HashMap::<(String, u32), Instant>::new();
    loop {
        tokio::select! { _ = state.shutdown.cancelled() => return, _ = tokio::time::sleep(Duration::from_secs(10)) => {} }
        let Ok(settings) = state.store.settings() else {
            continue;
        };
        if state.active.lock().await.len() >= settings.max_recordings {
            continue;
        }
        let Ok(jobs) = state.store.jobs() else {
            continue;
        };
        for job in jobs
            .into_iter()
            .filter(|j| j.state == JobState::Recording && !j.stop_requested)
        {
            let key = (job.id.clone(), job.attempt);
            if checked
                .get(&key)
                .is_some_and(|at| at.elapsed().as_secs() < 300)
                || fillable_gaps(&job).is_empty()
            {
                continue;
            }
            checked.insert(key, Instant::now());
            let _ = fill_gaps(&state, &job.id).await;
        }
    }
}

pub async fn monitor(state: Arc<Service>) {
    let mut checked = std::collections::HashMap::<String, Instant>::new();
    loop {
        tokio::select! { _ = state.shutdown.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(3)) => {} }
        if state.installing.load(Ordering::SeqCst)
            || state
                .tool_status
                .read()
                .await
                .iter()
                .any(|t| t.error.is_some())
        {
            continue;
        }
        let Ok(settings) = state.store.settings() else {
            continue;
        };
        let Ok(channels) = state.store.channels() else {
            continue;
        };
        for mut channel in channels {
            if !channel.enabled
                || checked
                    .get(&channel.id)
                    .is_some_and(|at| at.elapsed().as_secs() < settings.scan_interval_secs)
            {
                continue;
            }
            let cookie_file = settings.cookies_path.clone();
            let po_token = settings.po_token_path.clone();
            let result = tokio::select! { _ = state.shutdown.cancelled() => return, r = scan_channel(&state.tools, &channel.url, cookie_file.as_deref(), po_token.as_deref()) => r };
            checked.insert(channel.id.clone(), Instant::now());
            channel.last_checked_at = Some(now());
            match result {
                Ok(urls) => {
                    channel.last_error = None;
                    for url in urls {
                        let request = RecordRequest {
                            url,
                            live_from_start: None,
                            priority: channel.priority,
                        };
                        if let Ok(job) =
                            state
                                .store
                                .add_job(&request, Some(channel.id.clone()), true)
                        {
                            state.fanout_job(&job);
                        }
                    }
                }
                Err(e) => {
                    channel.last_error = Some(redact(&e.to_string()));
                }
            }
            // Preserve edits made while the network request was in flight.
            if let Ok(channels) = state.store.channels()
                && let Some(mut current) = channels.into_iter().find(|c| c.id == channel.id)
            {
                current.last_checked_at = channel.last_checked_at;
                current.last_error = channel.last_error;
                let _ = state.store.save_channel(&current);
            }
        }
    }
}
