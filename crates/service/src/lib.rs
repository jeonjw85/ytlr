mod alerts;
mod backup;
pub mod client;
mod http;
mod notifications;
mod replica;
mod retention;
mod scheduler;
mod storage;
pub mod tunnel;

use anyhow::{Context, Result};
use fs2::FileExt;
use std::{
    collections::HashMap,
    fs::OpenOptions,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;
use ytlr_core::*;
use ytlr_engine::Tools;

pub struct Service {
    pub storage: RwLock<Vec<StorageStatus>>,
    pub store: Arc<Store>,
    pub paths: AppPaths,
    pub tools: Tools,
    pub token: String,
    pub shutdown: CancellationToken,
    pub active: Mutex<HashMap<String, CancellationToken>>,
    pub next_checks: Mutex<HashMap<String, std::time::Instant>>,
    pub finalizer: Semaphore,
    pub gap_recovery: Semaphore,
    pub tool_status: RwLock<Vec<ToolStatus>>,
    pub installing: AtomicBool,
    pub tool_message: RwLock<Option<String>>,
}

impl Service {
    pub async fn snapshot(&self) -> Result<Snapshot> {
        let settings = self.store.settings()?;
        Ok(Snapshot {
            storage: self.storage.read().await.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
            jobs: self.store.jobs()?,
            channels: self.store.channels()?,
            free_bytes: available_space(&settings.storage_root).ok(),
            settings,
            tools: self.tool_status.read().await.clone(),
            installing_tools: self.installing.load(Ordering::SeqCst),
            tool_message: self.tool_message.read().await.clone(),
            replica_targets: self.paths.remotes()?.into_iter().map(|r| r.name).collect(),
        })
    }
    pub async fn refresh_tools(&self) {
        let tools = self.tools.doctor().await;
        let _ = atomic_write(
            &self.paths.root.join("tool-health.json"),
            &serde_json::to_vec_pretty(&tools).unwrap_or_default(),
        );
        *self.tool_status.write().await = tools;
    }

    pub fn fanout_job(&self, job: &RecordingJob) {
        let Ok(settings) = self.store.settings() else {
            return;
        };
        let Some(name) = settings.replica_remote.filter(|name| !name.is_empty()) else {
            return;
        };
        if let Err(e) = self.store.enqueue_replica(&job.id, &name) {
            let _ = self.store.event(&job.id, "replica_error", &e.to_string());
        }
    }
}

pub async fn run(paths: AppPaths) -> Result<()> {
    paths.initialize()?;
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(paths.root.join("service.lock"))?;
    lock_file
        .try_lock_exclusive()
        .context("이미 녹화 서비스가 실행 중입니다.")?;
    let store = Arc::new(Store::open(&paths.database(), &paths.default_settings())?);
    store.recover_interrupted()?;
    // Reconcile filesystem-derived usage after a crash between destructive
    // cleanup and its database/manifest publication.
    for job in store.jobs()? {
        if !job.output_dir.is_dir() {
            continue;
        }
        let Ok(files) = ytlr_engine::files_under(&job.output_dir) else {
            continue;
        };
        let bytes = files
            .iter()
            .filter_map(|path| std::fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .sum();
        if bytes != job.bytes && store.update_job(&job.id, |job| job.bytes = bytes).is_ok() {
            let _ = store.persist_job_manifest(&job.id);
        }
    }
    let settings = store.settings()?;
    std::fs::create_dir_all(&settings.storage_root)?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let endpoint = ServiceEndpoint {
        api_version: SERVICE_API_VERSION,
        port: listener.local_addr()?.port(),
        token: token.clone(),
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").into(),
    };
    let state = Arc::new(Service {
        storage: RwLock::new(vec![]),
        tools: Tools::new(paths.clone()),
        store,
        paths: paths.clone(),
        token,
        shutdown: CancellationToken::new(),
        active: Mutex::new(HashMap::new()),
        next_checks: Mutex::new(HashMap::new()),
        finalizer: Semaphore::new(1),
        gap_recovery: Semaphore::new(1),
        tool_status: RwLock::new(vec![]),
        installing: AtomicBool::new(false),
        tool_message: RwLock::new(None),
    });
    state.refresh_tools().await;
    atomic_write(&paths.endpoint_file(), &serde_json::to_vec(&endpoint)?)?;
    let scheduler = tokio::spawn(scheduler::schedule(state.clone()));
    let monitor = tokio::spawn(scheduler::monitor(state.clone()));
    let replica = tokio::spawn(replica::run(state.clone()));
    let recovery = tokio::spawn(scheduler::recover_gaps(state.clone()));
    let backup = tokio::spawn(backup::run(state.clone()));
    let storage = tokio::spawn(storage::run(state.clone()));
    let alerts = tokio::spawn(alerts::run(state.clone()));
    let notifications = tokio::spawn(notifications::run(state.clone()));
    let retention = tokio::spawn(retention::run(state.clone()));
    let checker = state.clone();
    let tool_checker = tokio::spawn(async move {
        loop {
            tokio::select! { _ = checker.shutdown.cancelled() => return, _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {} }
            let failed = checker
                .tool_status
                .read()
                .await
                .iter()
                .any(|t| t.error.is_some());
            if failed && !checker.installing.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::select! { _ = checker.shutdown.cancelled() => return, _ = checker.refresh_tools() => {} }
            }
        }
    });
    let shutdown = state.shutdown.clone();
    let signal = tokio::spawn(async move {
        #[cfg(unix)]
        {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("signal handler");
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
        shutdown.cancel();
    });
    let shutdown = state.shutdown.clone();
    let result = axum::serve(listener, http::router(state.clone()))
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await;
    state.shutdown.cancel();
    let _ = scheduler.await;
    monitor.abort();
    let _ = replica.await;
    let _ = recovery.await;
    let _ = backup.await;
    let _ = storage.await;
    let _ = alerts.await;
    let _ = notifications.await;
    let _ = retention.await;
    signal.abort();
    let _ = tool_checker.await;
    let _ = std::fs::remove_file(paths.endpoint_file());
    FileExt::unlock(&lock_file)?;
    result?;
    Ok(())
}
