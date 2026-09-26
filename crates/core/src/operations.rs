use crate::*;
use anyhow::{Result, bail};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperationTask {
    Export { index: usize },
    Clip { request: ClipRequest },
    RangeClip { path: String, start: f64, end: f64 },
    Preview { path: String },
    Recover,
    Cleanup { files: Vec<String> },
    Download { remote: String, path: String },
}
impl OperationTask {
    pub fn cancellable(&self) -> bool {
        !matches!(self, Self::Recover | Self::Cleanup { .. })
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::RangeClip { path, start, end } => {
                relative_media_path(path)?;
                if !start.is_finite() || !end.is_finite() || *start < 0.0 || end <= start {
                    bail!("클립 구간을 확인하세요.");
                }
            }
            Self::Preview { path } | Self::Download { path, .. } => {
                relative_media_path(path)?;
            }
            Self::Cleanup { files } => {
                if files.is_empty() || files.len() > 100000 {
                    bail!("정리할 파일을 선택하세요.");
                }
                for path in files {
                    relative_media_path(path)?;
                }
            }
            _ => {}
        }
        if let Self::Download { remote, .. } = self
            && (remote.is_empty() || remote.len() > 64)
        {
            bail!("원격 이름을 확인하세요.");
        }
        Ok(())
    }
}
pub fn relative_media_path(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 4096
        || value.contains(['\\', '\0', ':'])
        || value
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
        || std::path::Path::new(value).is_absolute()
    {
        bail!("잘못된 파일 경로입니다.");
    }
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub modified: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationRequest {
    pub job_id: String,
    pub task: OperationTask,
    #[serde(default)]
    pub source_path: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    pub request: OperationRequest,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub attempts: u32,
    pub cancel_requested: bool,
    pub progress: Option<f64>,
    pub bytes_done: u64,
    pub total_bytes: Option<u64>,
    pub message: String,
    pub error: Option<String>,
    pub result: Option<serde_json::Value>,
    pub input: Option<Artifact>,
    pub destination: Option<PathBuf>,
}
impl Operation {
    pub fn terminal(&self) -> bool {
        matches!(self.state.as_str(), "completed" | "failed" | "cancelled")
    }
}

impl Store {
    pub fn operations(&self) -> Result<Vec<Operation>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT body FROM operations ORDER BY rowid DESC")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .collect()
    }
    pub fn operation(&self, id: &str) -> Result<Operation> {
        let raw: String =
            self.lock()?
                .query_row("SELECT body FROM operations WHERE id=?", [id], |r| r.get(0))?;
        Ok(serde_json::from_str(&raw)?)
    }
    pub fn enqueue_operations(&self, requests: &[OperationRequest]) -> Result<Vec<Operation>> {
        if requests.is_empty() || requests.len() > 100 {
            bail!("한 번에 1~100개 작업을 추가할 수 있습니다.");
        }
        for req in requests {
            req.task.validate()?;
            uuid::Uuid::parse_str(&req.job_id)
                .map_err(|_| anyhow::anyhow!("녹화 작업 ID가 올바르지 않습니다."))?;
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let mut existing = {
            let mut stmt = tx.prepare("SELECT body FROM operations")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|s| serde_json::from_str::<Operation>(&s))
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        if existing.iter().filter(|o| !o.terminal()).count() + requests.len() > 1000 {
            bail!("대기 작업 상한에 도달했습니다.");
        }
        let mut result = vec![];
        for req in requests {
            let fingerprint = serde_json::to_value(req)?;
            if let Some(op) = existing.iter().find(|o| {
                !o.terminal()
                    && serde_json::to_value(&o.request).ok().as_ref() == Some(&fingerprint)
            }) {
                result.push(op.clone());
                continue;
            }
            let at = now();
            let op = Operation {
                id: uuid::Uuid::new_v4().to_string(),
                request: req.clone(),
                state: "queued".into(),
                created_at: at.clone(),
                updated_at: at,
                attempts: 0,
                cancel_requested: false,
                progress: None,
                bytes_done: 0,
                total_bytes: None,
                message: "작업 대기".into(),
                error: None,
                result: None,
                input: None,
                destination: None,
            };
            tx.execute(
                "INSERT INTO operations(id,body) VALUES(?,?)",
                params![op.id, serde_json::to_string(&op)?],
            )?;
            existing.push(op.clone());
            result.push(op);
        }
        tx.commit()?;
        Ok(result)
    }
    pub fn update_operation(
        &self,
        id: &str,
        change: impl FnOnce(&mut Operation) -> Result<()>,
    ) -> Result<Operation> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let raw: String =
            tx.query_row("SELECT body FROM operations WHERE id=?", [id], |r| r.get(0))?;
        let mut op: Operation = serde_json::from_str(&raw)?;
        change(&mut op)?;
        op.updated_at = now();
        tx.execute(
            "UPDATE operations SET body=? WHERE id=?",
            params![serde_json::to_string(&op)?, id],
        )?;
        tx.commit()?;
        Ok(op)
    }
    pub fn cancel_operation(&self, id: &str) -> Result<Operation> {
        self.update_operation(id, |o| {
            if o.terminal() {
                bail!("이미 끝난 작업입니다.");
            }
            if o.state == "running" && !o.request.task.cancellable() {
                bail!("원본을 변경하는 작업은 완료될 때까지 기다려 주세요.");
            }
            o.cancel_requested = true;
            if o.state == "queued" {
                o.state = "cancelled".into();
                o.message = "대기 작업 취소".into();
            }
            Ok(())
        })
    }
    pub fn retry_operation(&self, id: &str) -> Result<Operation> {
        self.update_operation(id, |o| {
            if !matches!(o.state.as_str(), "failed" | "cancelled") {
                bail!("실패하거나 취소된 작업만 재시도할 수 있습니다.");
            }
            o.state = "queued".into();
            o.cancel_requested = false;
            o.error = None;
            o.progress = None;
            o.message = "재검증 후 재시도 대기".into();
            Ok(())
        })
    }
    pub fn recover_operations(&self) -> Result<()> {
        for op in self
            .operations()?
            .into_iter()
            .filter(|o| o.state == "running")
        {
            if matches!(op.request.task, OperationTask::Recover)
                && let Ok(job) = self.job(&op.request.job_id)
                && job.state == JobState::Finalizing
            {
                self.update_job(&job.id, |j| {
                    j.state = JobState::Partial;
                    j.stop_requested = true;
                    j.recovery_error = Some("중단된 백그라운드 복구를 다시 확인합니다.".into());
                })?;
            }
            self.update_operation(&op.id, |o| {
                o.state = if o.cancel_requested {
                    "cancelled"
                } else {
                    "queued"
                }
                .into();
                o.message = "서비스 재시작 · 파일 재검증 대기".into();
                Ok(())
            })?;
        }
        Ok(())
    }
    pub fn job_has_operations(&self, id: &str) -> Result<bool> {
        Ok(self.operations()?.iter().any(|o| {
            o.request.job_id == id
                && !o.terminal()
                && !matches!(o.request.task, OperationTask::Download { .. })
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_survives_restarts_and_cancellation_is_not_lost() {
        let d = tempfile::tempdir().unwrap();
        let p = AppPaths::resolve(Some(d.path().into())).unwrap();
        let store = Store::open(&p.database(), &p.default_settings()).unwrap();
        let request = OperationRequest {
            job_id: uuid::Uuid::new_v4().to_string(),
            task: OperationTask::Export { index: 0 },
            source_path: None,
        };
        let a = store
            .enqueue_operations(&[request.clone(), request])
            .unwrap();
        assert_eq!(a[0].id, a[1].id);
        store
            .update_operation(&a[0].id, |o| {
                o.state = "running".into();
                Ok(())
            })
            .unwrap();
        drop(store);
        let store = Store::open(&p.database(), &p.default_settings()).unwrap();
        store.recover_operations().unwrap();
        assert_eq!(store.operation(&a[0].id).unwrap().state, "queued");
        store.cancel_operation(&a[0].id).unwrap();
        store.recover_operations().unwrap();
        assert_eq!(store.operation(&a[0].id).unwrap().state, "cancelled");
        assert_eq!(store.retry_operation(&a[0].id).unwrap().state, "queued");
        for p in ["../cookie.txt", "/etc/passwd", "a\\b", "a//b", "a/../b"] {
            assert!(relative_media_path(p).is_err());
        }
    }

    #[test]
    fn interrupted_recovery_does_not_restart_live_capture() {
        let d = tempfile::tempdir().unwrap();
        let p = AppPaths::resolve(Some(d.path().into())).unwrap();
        let store = Store::open(&p.database(), &p.default_settings()).unwrap();
        let req: RecordRequest =
            serde_json::from_value(serde_json::json!({"url":"https://youtu.be/abcdefghijk"}))
                .unwrap();
        let job = store.add_job(&req, None, false).unwrap();
        let op = store
            .enqueue_operations(&[OperationRequest {
                job_id: job.id.clone(),
                task: OperationTask::Recover,
                source_path: None,
            }])
            .unwrap()
            .remove(0);
        store
            .update_operation(&op.id, |o| {
                o.state = "running".into();
                Ok(())
            })
            .unwrap();
        store
            .update_job(&job.id, |j| {
                j.attempt = 1;
                j.state = JobState::Finalizing;
                j.stop_requested = false;
            })
            .unwrap();
        store.recover_operations().unwrap();
        store.recover_interrupted().unwrap();
        assert_eq!(store.job(&job.id).unwrap().state, JobState::Partial);
        assert_eq!(store.operation(&op.id).unwrap().state, "queued");
    }
}
