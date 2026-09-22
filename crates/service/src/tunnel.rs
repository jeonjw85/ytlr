use anyhow::{Context, Result, bail};
use std::{net::Ipv4Addr, process::Stdio, time::Duration};
use tokio::process::{Child, Command};
use ytlr_core::{Remote, ServiceEndpoint, parse_ssh_target, redact};

pub struct Tunnel {
    _child: Child,
    pub endpoint: ServiceEndpoint,
    pub name: String,
}

fn options(remote: &Remote) -> Result<Vec<String>> {
    let target = parse_ssh_target(&remote.ssh)?;
    let mut args: Vec<String> = [
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=5",
        "-o",
        "ServerAliveCountMax=2",
        "-p",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    args.push(target.port.to_string());
    if let Some(identity) = &remote.identity {
        args.extend(["-i".into(), identity.to_string_lossy().into_owned()]);
    }
    if let Some(hosts) = &remote.known_hosts {
        args.extend([
            "-o".into(),
            format!(
                "UserKnownHostsFile={}",
                serde_json::to_string(&hosts.to_string_lossy())?
            ),
        ]);
    }
    Ok(args)
}

fn target(remote: &Remote) -> Result<String> {
    let target = parse_ssh_target(&remote.ssh)?;
    Ok(format!("{}@{}", target.user, target.host))
}

fn quote(value: &str) -> Result<String> {
    if value.contains(['\0', '\n', '\r']) {
        bail!("SSH 경로에 제어 문자를 사용할 수 없습니다.");
    }
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

pub async fn connect(remote: &Remote) -> Result<Tunnel> {
    remote.validate()?;
    let executable = remote.executable.as_deref().unwrap_or("ytlr");
    let dir = remote.remote_data_dir.to_string_lossy();
    let endpoint_path = format!("{}/service.json", dir.trim_end_matches('/'));
    // 'start' checks a stale endpoint too. All options precede the SSH destination;
    // a '--' after the destination would be executed by the remote shell.
    let script = format!(
        "{} --data-dir {} start >/dev/null && cat {}",
        quote(executable)?,
        quote(&dir)?,
        quote(&endpoint_path)?
    );
    let mut command = Command::new("ssh");
    command
        .args(options(remote)?)
        .arg("--")
        .arg(target(remote)?)
        .arg(script)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    ytlr_engine::process::hide_console(&mut command);
    let output = tokio::time::timeout(Duration::from_secs(45), command.output())
        .await
        .context("SSH 명령 시간 제한 초과")??;
    if !output.status.success() {
        bail!(
            "SSH 연결 실패: {}",
            redact(&String::from_utf8_lossy(&output.stderr))
        );
    }
    let remote_endpoint: ServiceEndpoint =
        serde_json::from_slice(&output.stdout).context("원격 서비스 응답 형식 오류")?;
    if remote_endpoint.port == 0 || remote_endpoint.token.len() < 32 {
        bail!("원격 서비스 인증 정보가 올바르지 않습니다.");
    }
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let mut command = Command::new("ssh");
    command
        .args(options(remote)?)
        .args([
            "-N",
            "-o",
            "ExitOnForwardFailure=yes",
            "-L",
            &format!("127.0.0.1:{port}:127.0.0.1:{}", remote_endpoint.port),
        ])
        .arg("--")
        .arg(target(remote)?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    ytlr_engine::process::hide_console(&mut command);
    let mut child = command.spawn().context("SSH 터널 실행 실패")?;
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(750))
        .build()?;
    for _ in 0..30 {
        if child.try_wait()?.is_some() {
            bail!("SSH 터널이 종료되었습니다. 호스트 키·인증·포워딩 설정을 확인하세요.");
        }
        if let Ok(response) = http
            .get(format!("http://127.0.0.1:{port}/health"))
            .bearer_auth(&remote_endpoint.token)
            .send()
            .await
            && response.status().is_success()
            && response
                .json::<serde_json::Value>()
                .await
                .is_ok_and(|v| v["ok"] == true)
        {
            return Ok(Tunnel {
                _child: child,
                name: remote.name.clone(),
                endpoint: ServiceEndpoint {
                    port,
                    ..remote_endpoint
                },
            });
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    // kill_on_drop also handles task cancellation, timeout and partial setup failures.
    bail!("터널은 열렸지만 원격 서비스 인증/상태 확인에 실패했습니다.")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_paths_preserve_quotes_without_executing_them() {
        assert_eq!(quote("/a'b").unwrap(), "'/a'\"'\"'b'");
        assert!(quote("a\nb").is_err());
    }
}
