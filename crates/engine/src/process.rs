use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerSpec {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

pub fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    #[cfg(not(windows))]
    let _ = command;
}

pub fn group(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    hide_console(command);
}

struct ProcessGroup {
    pid: u32,
    armed: bool,
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.pid as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &self.pid.to_string(), "/T", "/F"])
                .creation_flags(0x08000000)
                .spawn();
        }
    }
}

pub async fn output_timeout(
    mut command: Command,
    timeout: Duration,
) -> Result<std::process::Output> {
    group(&mut command);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn()?;
    let mut family = ProcessGroup {
        pid: child.id().context("하위 프로세스 ID 오류")?,
        armed: true,
    };
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .context("하위 프로세스 응답 시간 초과")??;
    family.armed = false;
    Ok(output)
}

pub async fn interrupt(child: &mut Child) {
    if let Some(id) = child.id() {
        #[cfg(unix)]
        {
            // Child was created as a process-group leader; include its ffmpeg descendants.
            unsafe {
                libc::kill(-(id as i32), libc::SIGINT);
            }
        }
        #[cfg(windows)]
        {
            let mut cmd = Command::new("taskkill");
            hide_console(&mut cmd);
            let _ = cmd
                .args(["/PID", &id.to_string(), "/T", "/F"])
                .output()
                .await;
        }
        if tokio::time::timeout(Duration::from_secs(20), child.wait())
            .await
            .is_err()
        {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(id as i32), libc::SIGKILL);
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

/// Independent lifetime guard. The service holds stdin open; EOF also detects SIGKILL.
/// This avoids leaving a downloader behind when the service crashes (including on macOS).
pub async fn worker(spec: WorkerSpec) -> Result<i32> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(
            spec.cwd
                .parent()
                .context("작업 폴더 오류")?
                .join("capture.lock"),
        )?;
    lock.try_lock_exclusive()
        .context("이전 수집 프로세스가 아직 종료 중입니다.")?;
    let mut command = Command::new(&spec.executable);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    group(&mut command);
    let mut child = command.spawn().context("수집 프로세스 시작 실패")?;
    #[cfg(target_os = "macos")]
    let _sleep_guard = if let Some(id) = child.id() {
        Command::new("/usr/bin/caffeinate")
            .args(["-i", "-w", &id.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()
    } else {
        None
    };
    let mut stdin = tokio::io::stdin();
    let mut buf = [0u8; 1];
    tokio::select! {
        status = child.wait() => Ok(status?.code().unwrap_or(1)),
        _ = stdin.read(&mut buf) => { interrupt(&mut child).await; Ok(130) },
        _ = tokio::signal::ctrl_c() => { interrupt(&mut child).await; Ok(130) },
    }
}
