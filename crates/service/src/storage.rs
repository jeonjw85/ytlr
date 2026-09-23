use crate::Service;
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use ytlr_core::{JobState, StorageStatus};

// Use a rolling window because fragment/segment sizes are published in bursts.
#[derive(Default)]
struct Rate {
    samples: VecDeque<(Instant, u64)>,
}

impl Rate {
    fn observe(&mut self, at: Instant, bytes: u64) -> Option<f64> {
        if self
            .samples
            .back()
            .is_some_and(|(_, previous)| bytes < *previous)
        {
            self.samples.clear();
        }
        self.samples.push_back((at, bytes));
        while self.samples.len() > 2 && at.duration_since(self.samples[1].0).as_secs() >= 30 {
            self.samples.pop_front();
        }
        let (start, previous) = *self.samples.front()?;
        let elapsed = at.duration_since(start).as_secs_f64();
        (elapsed >= 10.0 && bytes > previous).then(|| (bytes - previous) as f64 / elapsed)
    }
}

fn status(path: PathBuf, reserve: u64, rate: Option<f64>) -> StorageStatus {
    let (free, total) = if path.is_dir() {
        (
            ytlr_core::available_space(&path).ok(),
            fs2::total_space(&path).ok(),
        )
    } else {
        (None, None)
    };
    let remaining = free
        .zip(rate)
        .map(|(free, rate)| free.saturating_sub(reserve) as f64 / rate);
    let low_space = free
        .is_some_and(|free| free <= reserve.saturating_mul(2).max(5 * 1024_u64.pow(3)))
        || remaining.is_some_and(|seconds| seconds <= 30.0 * 60.0);
    StorageStatus {
        total_bytes: total,
        path,
        free_bytes: free,
        bytes_per_second: rate,
        remaining_seconds: remaining,
        low_space,
    }
}

fn volume_key(path: &std::path::Path) -> String {
    ytlr_core::backup_device(path)
        .ok()
        .flatten()
        .map(|device| format!("device:{device}"))
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

pub async fn run(state: Arc<Service>) {
    let mut rates = HashMap::<String, Rate>::new();
    let mut warned = BTreeSet::new();
    let mut tick = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! { _ = state.shutdown.cancelled() => return, _ = tick.tick() => {} }
        let (Ok(settings), Ok(jobs)) = (state.store.settings(), state.store.jobs()) else {
            continue;
        };
        let mut roots = BTreeSet::from([settings.storage_root]);
        let now = Instant::now();
        let mut volume_rates = HashMap::<String, f64>::new();
        rates.retain(|id, _| {
            jobs.iter()
                .any(|j| &j.id == id && j.state == JobState::Recording)
        });
        for job in jobs.iter().filter(|j| !j.state.terminal()) {
            if let Some(root) = job.output_dir.parent() {
                roots.insert(root.to_owned());
            }
            if job.state == JobState::Recording {
                let rate = rates
                    .entry(job.id.clone())
                    .or_default()
                    .observe(now, job.bytes)
                    .unwrap_or(0.0);
                if let Some(root) = job.output_dir.parent() {
                    *volume_rates.entry(volume_key(root)).or_default() += rate;
                }
            }
        }
        let statuses: Vec<_> = roots
            .into_iter()
            .map(|root| {
                let rate = volume_rates.get(&volume_key(&root)).copied();
                status(
                    root,
                    settings.min_free_bytes,
                    rate.filter(|rate| *rate > 0.0),
                )
            })
            .collect();
        let low: BTreeSet<_> = statuses
            .iter()
            .filter(|s| s.low_space)
            .map(|s| s.path.clone())
            .collect();
        let mut next_warned = BTreeSet::new();
        for job in jobs.iter().filter(|j| !j.state.terminal()) {
            if job
                .output_dir
                .parent()
                .is_some_and(|root| low.contains(root))
            {
                next_warned.insert(job.id.clone());
                if !warned.contains(&job.id) {
                    let _ = state.store.event(
                        &job.id,
                        "storage_warning",
                        "저장 공간이 부족해지고 있습니다. 저장 디스크의 여유 공간을 확인하세요.",
                    );
                }
            }
        }
        warned = next_warned;
        *state.storage.write().await = statuses;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_handles_bursts_resets_and_stalls() {
        let mut rate = Rate::default();
        let start = Instant::now();
        assert_eq!(rate.observe(start, 100), None);
        assert_eq!(
            rate.observe(start + Duration::from_secs(10), 1100),
            Some(100.0)
        );
        assert_eq!(
            rate.observe(start + Duration::from_secs(20), 1100),
            Some(50.0)
        );
        assert_eq!(rate.observe(start + Duration::from_secs(40), 1100), None);
        assert_eq!(rate.observe(start + Duration::from_secs(50), 0), None);
    }

    #[test]
    fn unavailable_disk_has_no_estimate() {
        let d = tempfile::tempdir().unwrap();
        let result = status(d.path().join("missing"), 1024, Some(100.0));
        assert!(result.free_bytes.is_none());
        assert!(result.remaining_seconds.is_none());
        assert!(!result.low_space);
        let result = status(d.path().to_owned(), u64::MAX, Some(100.0));
        assert!(result.low_space);
        assert_eq!(result.remaining_seconds, Some(0.0));
    }
}
