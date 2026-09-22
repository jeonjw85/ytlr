use crate::RecordingJob;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GapStatus {
    Unrecovered,
    Recovered,
    Collected,
    Unrecoverable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureAttempt {
    pub n: u32,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub media_seconds: f64,
    pub bytes: u64,
    #[serde(default)]
    pub last_media_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineGap {
    pub after_attempt: u32,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub seconds: f64,
    pub status: GapStatus,
    #[serde(default)]
    pub evidence: Option<RecoveryEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryEvidence {
    pub requested_start: String,
    pub requested_end: String,
    pub video_start: String,
    pub video_end: String,
    pub audio_start: String,
    pub audio_end: String,
    pub output: crate::MediaOutput,
    pub message: String,
}

pub fn parse_rfc3339(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

pub fn wall_seconds(from: &str, to: &str) -> Option<f64> {
    Some((parse_rfc3339(to)? - parse_rfc3339(from)?).num_milliseconds() as f64 / 1000.0)
}

pub fn note_attempt_start(job: &mut RecordingJob, n: u32, started_at: &str) {
    if job.attempts.iter().any(|attempt| attempt.n == n) {
        return;
    }
    if let Some(prev) = job.attempts.iter().find(|attempt| attempt.n + 1 == n) {
        let gap_start = prev
            .last_media_at
            .clone()
            .or_else(|| job.last_media_at.clone())
            .or_else(|| prev.ended_at.clone())
            .unwrap_or_else(|| prev.started_at.clone());
        if let Some(seconds) = wall_seconds(&gap_start, started_at).map(|s| s.max(0.0))
            && (seconds >= 1.0 || job.continuity_uncertain)
        {
            job.gaps.push(TimelineGap {
                after_attempt: prev.n,
                started_at: gap_start,
                ended_at: Some(started_at.into()),
                seconds,
                status: GapStatus::Unrecovered,
                evidence: None,
            });
        }
    }
    job.attempts.push(CaptureAttempt {
        n,
        started_at: started_at.into(),
        ended_at: None,
        media_seconds: 0.0,
        bytes: 0,
        last_media_at: None,
    });
}

pub fn note_attempt_end(
    job: &mut RecordingJob,
    n: u32,
    ended_at: &str,
    media_seconds: f64,
    bytes: u64,
) {
    if let Some(attempt) = job.attempts.iter_mut().find(|attempt| attempt.n == n) {
        attempt.ended_at = Some(ended_at.into());
        attempt.media_seconds = media_seconds;
        attempt.bytes = bytes;
    }
}

pub fn mark_gap_fill(job: &mut RecordingJob, after_attempt: u32, evidence: RecoveryEvidence) {
    if let Some(gap) = job
        .gaps
        .iter_mut()
        .rev()
        .find(|gap| gap.after_attempt == after_attempt && gap.status == GapStatus::Unrecovered)
    {
        // Receipt times are not media anchors. PDT coverage alone cannot certify a splice.
        gap.status = GapStatus::Collected;
        gap.evidence = Some(evidence);
    }
}

pub fn note_media_received(job: &mut RecordingJob, n: u32, at: &str) {
    let first = job
        .attempts
        .iter()
        .find(|a| a.n == n)
        .is_some_and(|a| a.last_media_at.is_none());
    if let Some(attempt) = job.attempts.iter_mut().find(|a| a.n == n) {
        attempt.last_media_at = Some(at.into());
    }
    if first && let Some(gap) = job.gaps.iter_mut().find(|g| g.after_attempt + 1 == n) {
        gap.ended_at = Some(at.into());
        gap.seconds = wall_seconds(&gap.started_at, at).unwrap_or(0.0).max(0.0);
    }
}

pub fn fillable_gaps(job: &RecordingJob) -> Vec<TimelineGap> {
    job.gaps
        .iter()
        .filter(|gap| gap.status == GapStatus::Unrecovered && (3.0..3600.0).contains(&gap.seconds))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn job() -> RecordingJob {
        RecordingJob {
            id: "j".into(),
            url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            video_id: "abcdefghijk".into(),
            title: "t".into(),
            channel: String::new(),
            channel_id: None,
            state: JobState::Recording,
            message: String::new(),
            created_at: "2026-01-01T00:00:00+00:00".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
            started_at: None,
            last_media_at: None,
            output_dir: "/tmp/x".into(),
            format: String::new(),
            bytes: 0,
            media_seconds: 0.0,
            attempt: 0,
            retries: 0,
            priority: 0,
            live_from_start: true,
            continuity_uncertain: false,
            stop_requested: false,
            outputs: vec![],
            attempts: vec![],
            gaps: vec![],
            backup: BackupStatus::default(),
            replica: None,
            replica_origin: false,
        }
    }

    #[test]
    fn restart_boundary_is_a_measured_gap() {
        let mut job = job();
        note_attempt_start(&mut job, 1, "2026-01-01T00:00:00+00:00");
        note_attempt_end(&mut job, 1, "2026-01-01T00:10:00+00:00", 600.0, 100);
        note_attempt_start(&mut job, 2, "2026-01-01T00:12:30+00:00");
        assert_eq!(job.gaps.len(), 1);
        assert_eq!(job.gaps[0].after_attempt, 1);
        assert_eq!(job.gaps[0].seconds, 150.0);
        assert_eq!(job.gaps[0].status, GapStatus::Unrecovered);
        assert_eq!(fillable_gaps(&job).len(), 1);
        assert!(job.gaps[0].evidence.is_none());
    }

    #[test]
    fn crash_restart_records_even_a_short_gap() {
        let mut job = job();
        job.continuity_uncertain = true;
        note_attempt_start(&mut job, 1, "2026-01-01T00:00:00.000+00:00");
        note_attempt_end(&mut job, 1, "2026-01-01T00:00:10.000+00:00", 10.0, 1);
        note_attempt_start(&mut job, 2, "2026-01-01T00:00:10.200+00:00");
        assert_eq!(job.gaps.len(), 1);
        assert!(job.gaps[0].seconds < 1.0);
    }

    #[test]
    fn subsecond_flush_delay_is_not_a_gap() {
        let mut job = job();
        note_attempt_start(&mut job, 1, "2026-01-01T00:00:00.000+00:00");
        note_attempt_end(&mut job, 1, "2026-01-01T00:00:10.000+00:00", 10.0, 1);
        note_attempt_start(&mut job, 2, "2026-01-01T00:00:10.400+00:00");
        assert!(job.gaps.is_empty());
    }

    #[test]
    fn first_media_includes_reconnect_setup_delay() {
        let mut job = job();
        note_attempt_start(&mut job, 1, "2026-01-01T00:00:00Z");
        note_media_received(&mut job, 1, "2026-01-01T00:00:10Z");
        note_attempt_end(&mut job, 1, "2026-01-01T00:00:30Z", 10.0, 1);
        note_attempt_start(&mut job, 2, "2026-01-01T00:00:40Z");
        note_media_received(&mut job, 2, "2026-01-01T00:00:50Z");
        assert_eq!(job.gaps[0].seconds, 40.0);
    }
}
