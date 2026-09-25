use anyhow::{Context, Result, bail};
use chrono::{DateTime, Datelike, Duration, FixedOffset, Timelike, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RecordingSchedule {
    pub start_at: Option<String>,
    pub duration_minutes: Option<u32>,
}

impl RecordingSchedule {
    pub fn validate(&self, stop_at: Option<&str>) -> Result<()> {
        let start = self
            .start_at
            .as_deref()
            .map(|s| {
                crate::parse_rfc3339(s)
                    .context("시작 시각은 시간대가 포함된 RFC3339 형식이어야 합니다.")
            })
            .transpose()?;
        if self.duration_minutes.is_some_and(|n| n == 0 || n > 525600) {
            bail!("녹화 시간은 1~525600분이어야 합니다.");
        }
        if let (Some(start), Some(stop)) = (start, stop_at.and_then(crate::parse_rfc3339))
            && stop <= start
        {
            bail!("종료 시각은 시작 시각 이후여야 합니다.");
        }
        Ok(())
    }
}

/// Weekdays refer to the day the window starts (Monday=1). Equal endpoints mean 24h.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WeeklyWindow {
    pub weekdays: Vec<u8>,
    pub start_minute: u16,
    pub end_minute: u16,
    pub utc_offset_minutes: i32,
}

impl WeeklyWindow {
    pub fn validate(&self) -> Result<()> {
        if self.weekdays.is_empty()
            || self.weekdays.iter().any(|d| !(1..=7).contains(d))
            || self.start_minute >= 1440
            || self.end_minute >= 1440
            || !(-720..=840).contains(&self.utc_offset_minutes)
        {
            bail!("요일·시간대 설정을 확인하세요.");
        }
        Ok(())
    }
    pub fn occurrence(&self, at: DateTime<Utc>) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        self.validate().ok()?;
        let local = at.with_timezone(&FixedOffset::east_opt(self.utc_offset_minutes * 60)?);
        let minute = local.hour() * 60 + local.minute();
        let overnight = self.end_minute <= self.start_minute;
        let date = if overnight && minute < self.end_minute as u32 {
            local.date_naive().pred_opt()?
        } else if minute >= self.start_minute as u32
            && (overnight || minute < self.end_minute as u32)
        {
            local.date_naive()
        } else {
            return None;
        };
        if !self
            .weekdays
            .contains(&(date.weekday().number_from_monday() as u8))
        {
            return None;
        }
        let start = date
            .and_hms_opt(
                (self.start_minute / 60) as u32,
                (self.start_minute % 60) as u32,
                0,
            )?
            .and_local_timezone(*local.offset())
            .single()?
            .with_timezone(&Utc);
        let minutes = if overnight {
            1440 + self.end_minute - self.start_minute
        } else {
            self.end_minute - self.start_minute
        };
        Some((start, start + Duration::minutes(minutes as i64)))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ChannelRules {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub window: Option<WeeklyWindow>,
    pub duration_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleDecision {
    pub at: String,
    pub title: String,
    pub allowed: bool,
    pub reason: String,
    pub window_start: Option<String>,
    pub stop_at: Option<String>,
}

impl ChannelRules {
    pub fn validate(&self) -> Result<()> {
        for words in [&self.include, &self.exclude] {
            if words.len() > 50
                || words
                    .iter()
                    .any(|w| w.trim().is_empty() || w.chars().count() > 120)
            {
                bail!("키워드는 각각 1~120자, 최대 50개까지 지정할 수 있습니다.");
            }
        }
        if let Some(w) = &self.window {
            w.validate()?;
        }
        RecordingSchedule {
            start_at: None,
            duration_minutes: self.duration_minutes,
        }
        .validate(None)
    }
    pub fn evaluate(&self, title: &str, at: DateTime<Utc>) -> RuleDecision {
        let title_lower = title.to_lowercase();
        let mut d = RuleDecision {
            at: at.to_rfc3339(),
            title: title.into(),
            allowed: true,
            reason: "녹화 조건 일치".into(),
            window_start: None,
            stop_at: None,
        };
        if self
            .exclude
            .iter()
            .any(|w| title_lower.contains(&w.trim().to_lowercase()))
        {
            d.allowed = false;
            d.reason = "제외 키워드 일치".into();
        } else if !self.include.is_empty()
            && !self
                .include
                .iter()
                .any(|w| title_lower.contains(&w.trim().to_lowercase()))
        {
            d.allowed = false;
            d.reason = "포함 키워드 불일치".into();
        }
        if let Some(w) = &self.window {
            if let Some((start, end)) = w.occurrence(at) {
                d.window_start = Some(start.to_rfc3339());
                d.stop_at = Some(end.to_rfc3339());
            } else {
                d.allowed = false;
                d.reason = "녹화 시간대 밖".into();
            }
        }
        d
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AutomationSettings {
    pub notifications: Vec<NotificationTarget>,
    pub retention: RetentionPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NotificationTarget {
    Webhook {
        id: String,
        url_env: String,
    },
    Discord {
        id: String,
        url_env: String,
    },
    Telegram {
        id: String,
        token_env: String,
        chat_id: String,
    },
}
impl NotificationTarget {
    pub fn id(&self) -> &str {
        match self {
            Self::Webhook { id, .. } | Self::Discord { id, .. } | Self::Telegram { id, .. } => id,
        }
    }
}
impl AutomationSettings {
    pub fn validate(&self) -> Result<()> {
        let mut ids = std::collections::HashSet::new();
        if self.notifications.len() > 10 {
            bail!("알림 대상은 최대 10개입니다.");
        }
        for target in &self.notifications {
            if target.id().is_empty() || target.id().len() > 64 || !ids.insert(target.id()) {
                bail!("알림 대상 ID는 고유해야 합니다.");
            }
            let env = match target {
                NotificationTarget::Webhook { url_env, .. }
                | NotificationTarget::Discord { url_env, .. } => url_env,
                NotificationTarget::Telegram {
                    token_env, chat_id, ..
                } => {
                    if chat_id.is_empty() || chat_id.len() > 128 {
                        bail!("Telegram chat ID를 확인하세요.");
                    }
                    token_env
                }
            };
            if env.is_empty()
                || env.len() > 128
                || env.starts_with(|c: char| c.is_ascii_digit())
                || !env.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                bail!("인증 정보의 환경변수 이름을 확인하세요.");
            }
        }
        for days in [
            self.retention.cleanup_after_days,
            self.retention.delete_after_days,
        ]
        .into_iter()
        .flatten()
        {
            if !(1..=36500).contains(&days) {
                bail!("보관 기간은 1~36500일입니다.");
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RetentionPolicy {
    pub cleanup_after_days: Option<u32>,
    pub delete_after_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipRequest {
    pub bookmark_id: String,
    pub end_bookmark_id: Option<String>,
    #[serde(default)]
    pub before_seconds: f64,
    #[serde(default)]
    pub after_seconds: f64,
}
impl ClipRequest {
    pub fn resolve(&self, job: &crate::RecordingJob) -> Result<(crate::MediaOutput, f64, f64)> {
        if !job.state.terminal() {
            bail!("녹화 마무리 후 클립을 만들 수 있습니다.");
        }
        let bookmark = job
            .bookmarks
            .iter()
            .find(|b| b.id == self.bookmark_id)
            .context("북마크를 찾을 수 없습니다.")?;
        let position = bookmark
            .media_seconds
            .context("미디어 시점 미확인 북마크입니다.")?;
        let (start, end) = if let Some(id) = &self.end_bookmark_id {
            let end = job
                .bookmarks
                .iter()
                .find(|b| &b.id == id)
                .context("끝 북마크를 찾을 수 없습니다.")?;
            if end.attempt != bookmark.attempt {
                bail!("재연결 경계를 넘는 클립은 만들 수 없습니다.");
            }
            (
                position,
                end.media_seconds
                    .context("미디어 시점 미확인 북마크입니다.")?,
            )
        } else {
            if !self.before_seconds.is_finite()
                || !self.after_seconds.is_finite()
                || self.before_seconds < 0.0
                || self.after_seconds < 0.0
            {
                bail!("클립 구간을 확인하세요.");
            }
            (
                (position - self.before_seconds).max(0.0),
                position + self.after_seconds,
            )
        };
        let attempt = format!("attempt-{:04}", bookmark.attempt);
        let output = job
            .outputs
            .iter()
            .find(|o| {
                o.path
                    .parent()
                    .and_then(|p| p.file_name())
                    .is_some_and(|n| n == attempt.as_str())
            })
            .context("해당 녹화 시도의 결과 파일이 없습니다.")?
            .clone();
        let end = end.min(output.duration);
        if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start {
            bail!("클립 구간이 비어 있거나 파일 범위 밖입니다.");
        }
        Ok((output, start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overnight_windows_use_start_day_and_exclude_end_boundary() {
        let rules = ChannelRules {
            include: vec!["LIVE".into()],
            exclude: vec!["test".into()],
            window: Some(WeeklyWindow {
                weekdays: vec![1],
                start_minute: 1380,
                end_minute: 60,
                utc_offset_minutes: 540,
            }),
            duration_minutes: None,
        };
        let at = |s| crate::parse_rfc3339(s).unwrap();
        assert!(
            rules
                .evaluate("live concert", at("2026-09-29T00:30:00+09:00"))
                .allowed
        );
        assert!(
            !rules
                .evaluate("live TEST", at("2026-09-29T00:30:00+09:00"))
                .allowed
        );
        assert!(
            !rules
                .evaluate("live", at("2026-09-29T01:00:00+09:00"))
                .allowed
        );
        assert!(
            !rules
                .evaluate("live", at("2026-09-30T00:30:00+09:00"))
                .allowed
        );
    }
    #[test]
    fn schedule_and_configuration_validation() {
        assert!(
            RecordingSchedule {
                start_at: Some("2026-10-01T12:00:00Z".into()),
                duration_minutes: None
            }
            .validate(Some("2026-10-01T11:00:00Z"))
            .is_err()
        );
        assert!(
            RecordingSchedule {
                duration_minutes: Some(0),
                ..Default::default()
            }
            .validate(None)
            .is_err()
        );
        assert!(
            ChannelRules {
                exclude: vec![" ".into()],
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
}
