use crate::Service;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use ytlr_core::*;

#[cfg(not(windows))]
struct Guard(std::process::Child);
#[cfg(not(windows))]
impl Guard {
    fn new() -> std::io::Result<Self> {
        use std::process::{Command, Stdio};
        #[cfg(target_os = "macos")]
        let mut command = {
            let mut c = Command::new("/usr/bin/caffeinate");
            c.args(["-i", "-w", &std::process::id().to_string()]);
            c
        };
        #[cfg(not(target_os = "macos"))]
        let mut command = {
            let mut c = Command::new("systemd-inhibit");
            c.args([
                "--what=sleep",
                "--mode=block",
                "--who=YTLR",
                "--why=Recording or scheduled work",
                "sh",
                "-c",
                "read -r lifetime",
            ]);
            c
        };
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(Self)
    }
    fn active(&mut self) -> bool {
        self.0.try_wait().is_ok_and(|s| s.is_none())
    }
}
#[cfg(not(windows))]
impl Drop for Guard {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_some()) {
            return;
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGTERM);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(windows)]
struct Guard {
    stop: std::sync::mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
#[cfg(windows)]
impl Guard {
    fn new() -> std::io::Result<Self> {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetThreadExecutionState(flags: u32) -> u32;
        }
        let (stop, rx) = std::sync::mpsc::channel();
        let (ready, status) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let success = unsafe { SetThreadExecutionState(0x80000001) } != 0;
            let _ = ready.send(success);
            if success {
                let _ = rx.recv();
                unsafe {
                    SetThreadExecutionState(0x80000000);
                }
            }
        });
        if !status.recv().unwrap_or(false) {
            let _ = thread.join();
            return Err(std::io::Error::other("Windows sleep inhibition failed"));
        }
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
    fn active(&mut self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}
#[cfg(windows)]
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub async fn run(state: Arc<Service>) {
    let mut guard: Option<Guard> = None;
    let mut retry_at = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! { _=state.shutdown.cancelled()=>return, _=tick.tick()=>{} }
        let (Ok(settings), Ok(jobs)) = (state.store.settings(), state.store.jobs()) else {
            continue;
        };
        let waiting_jobs = jobs
            .iter()
            .filter(|j| !j.state.terminal() && !j.state.active() && !j.stop_requested)
            .count();
        let watching_channels = state
            .store
            .channels()
            .map(|cs| cs.iter().filter(|c| c.enabled).count())
            .unwrap_or(0);
        let reading = state
            .file_activity
            .lock()
            .ok()
            .and_then(|v| *v)
            .is_some_and(|until| until > Instant::now());
        let work = !state.active.lock().await.is_empty()
            || !state.op_active.lock().await.is_empty()
            || reading;
        let requested = settings.prevent_sleep
            && (work
                || (settings.keep_awake_waiting && (waiting_jobs > 0 || watching_channels > 0)));
        if !requested {
            guard = None;
            retry_at = Instant::now();
        }
        if guard.as_mut().is_some_and(|g| !g.active()) {
            guard = None;
            retry_at = Instant::now() + Duration::from_secs(60);
        }
        if requested && guard.is_none() && Instant::now() >= retry_at {
            guard = Guard::new().ok();
            retry_at = Instant::now()
                + if guard.is_none() {
                    Duration::from_secs(60)
                } else {
                    Duration::ZERO
                };
        }
        let active = guard.is_some();
        let message = if active {
            "절전 방지 적용 중"
        } else if requested {
            "절전 방지를 적용할 수 없습니다. 시스템 전원 설정을 확인하세요."
        } else if waiting_jobs > 0 || watching_channels > 0 {
            "예약 또는 채널 감시 대기 중입니다. 컴퓨터가 절전 상태이면 녹화를 시작할 수 없습니다."
        } else {
            "절전 방지 대기"
        };
        *state.power.write().await = PowerStatus {
            requested,
            active,
            waiting_jobs,
            watching_channels,
            message: message.into(),
        };
    }
}
