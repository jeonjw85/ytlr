use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use ytlr_core::*;
use ytlr_service::client::Client;

#[derive(Parser)]
#[command(
    name = "ytlr",
    version,
    about = "YTLR — 복구 가능한 유튜브 라이브 녹화"
)]
struct Args {
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    remote: Option<String>,
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    /// 라이브 URL 녹화 또는 예약 방송 대기
    Record {
        url: String,
        #[arg(long)]
        from_now: bool,
        #[arg(long, default_value_t = 0)]
        priority: i32,
    },
    /// 채널을 추가하고 자동 감시
    Watch {
        url: String,
    },
    /// 녹화 현황
    Status {
        #[arg(long)]
        json: bool,
    },
    Stop {
        id: String,
    },
    Retry {
        id: String,
    },
    Recover {
        id: String,
    },
    Export {
        id: String,
        #[arg(long, default_value_t = 0)]
        index: usize,
    },
    Events {
        id: String,
    },
    Channel {
        #[command(subcommand)]
        action: ChannelAction,
    },
    Settings {
        #[arg(long)]
        storage: Option<PathBuf>,
        #[arg(long)]
        max_recordings: Option<usize>,
        #[arg(long)]
        reserve_gib: Option<u64>,
        #[arg(long)]
        backup: Option<PathBuf>,
        #[arg(long)]
        cookies: Option<PathBuf>,
        #[arg(long)]
        replica: Option<String>,
        #[arg(long)]
        po_token: Option<PathBuf>,
    },
    Tools {
        #[command(subcommand)]
        action: ToolsAction,
    },
    /// 설치 도구 및 저장 공간 진단
    Doctor,
    /// 포그라운드 서비스 (systemd/launchd에서도 사용)
    Run {
        #[arg(long)]
        headless: bool,
    },
    /// 백그라운드 서비스 실행
    Start,
    /// 수집을 정리하고 서비스 종료 (다음 실행에서 작업 재개)
    Shutdown,
    Remote {
        #[command(subcommand)]
        action: RemoteAction,
    },
    #[command(hide = true)]
    InternalBackup {
        spec: PathBuf,
    },
    #[command(hide = true)]
    InternalWorker {
        spec: String,
    },
}
#[derive(Subcommand)]
enum ChannelAction {
    Add {
        url: String,
        #[arg(long, default_value = "")]
        name: String,
    },
    List,
    Remove {
        id: String,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
}
#[derive(Subcommand)]
enum RemoteAction {
    Add {
        name: String,
        #[arg(long)]
        ssh: String,
        #[arg(long = "remote-data-dir")]
        remote_data_dir: PathBuf,
        #[arg(long)]
        identity: Option<PathBuf>,
        #[arg(long)]
        known_hosts: Option<PathBuf>,
        #[arg(long)]
        executable: Option<String>,
        /// Replace an existing remote with the same name.
        #[arg(long)]
        force: bool,
    },
    List,
    Remove {
        name: String,
    },
}
#[derive(Subcommand)]
enum ToolsAction {
    Install,
    Rollback,
    /// 배포용 도구 복사. 관리되는 검증된 번들만 사용합니다.
    Bundle {
        output: PathBuf,
    },
}

#[tokio::main]
async fn main() {
    if let Err(e) = execute().await {
        eprintln!("오류: {e:#}");
        std::process::exit(1);
    }
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn settings_path(path: PathBuf, remote: bool) -> Result<PathBuf> {
    if remote {
        if !path.to_string_lossy().starts_with('/') {
            bail!("원격 설정에는 서버의 절대 경로를 입력하세요.");
        }
        return Ok(path);
    }
    Ok(if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    })
}

async fn execute() -> Result<()> {
    let args = Args::parse();
    if args.remote.is_some()
        && matches!(
            args.command,
            Action::Run { .. }
                | Action::Start
                | Action::InternalWorker { .. }
                | Action::InternalBackup { .. }
                | Action::Remote { .. }
                | Action::Tools {
                    action: ToolsAction::Bundle { .. }
                }
        )
    {
        bail!("이 명령은 원격 대상에서 실행할 수 없습니다.");
    }
    if let Action::InternalBackup { spec } = &args.command {
        let spec: BackupSpec = serde_json::from_slice(&std::fs::read(spec)?)?;
        use tokio::io::AsyncReadExt;
        let work = tokio::task::spawn_blocking(move || mirror_job(&spec));
        let mut stdin = tokio::io::stdin();
        let mut byte = [0u8; 1];
        tokio::select! {
            result = work => {
                print_json(&result??)?;
                std::io::Write::flush(&mut std::io::stdout())?;
                // Tokio's stdin uses a blocking read. Do not wait for that read
                // during runtime shutdown while the parent awaits our exit.
                std::process::exit(0);
            },
            _ = stdin.read(&mut byte) => std::process::exit(130),
        }
    }
    if let Action::InternalWorker { spec } = &args.command {
        let code =
            ytlr_engine::process::worker(serde_json::from_slice(&std::fs::read(spec)?)?).await?;
        std::process::exit(code);
    }
    let paths = AppPaths::resolve(args.data_dir)?;
    if matches!(args.command, Action::Run { .. }) {
        return ytlr_service::run(paths).await;
    }
    if let Action::Tools {
        action: ToolsAction::Bundle { output },
    } = &args.command
    {
        paths.initialize()?;
        let tools = ytlr_engine::Tools::new(paths.clone());
        tools.install(|msg| eprintln!("{msg}")).await?;
        std::fs::create_dir_all(output)?;
        for name in ["yt-dlp", "deno", "ffmpeg", "ffprobe"] {
            let source = if name.starts_with("ff") {
                tools.redistributable_tool(name).await?
            } else {
                tools.require(name)?
            };
            if !name.starts_with("ff") && !source.starts_with(paths.tools_dir()) {
                bail!("배포에는 검증된 관리 번들만 사용할 수 있습니다.");
            }
            std::fs::copy(source, output.join(ytlr_engine::executable_name(name)))?;
        }
        return Ok(());
    }
    if let Action::Remote { action } = args.command {
        paths.initialize()?;
        let mut remotes = paths.remotes()?;
        match action {
            RemoteAction::List => print_json(&remotes)?,
            RemoteAction::Remove { name } => {
                remotes.retain(|r| r.name != name);
                paths.save_remotes(&remotes)?;
                print_json(&json!({"ok": true}))?;
            }
            RemoteAction::Add {
                name,
                ssh,
                remote_data_dir,
                identity,
                known_hosts,
                executable,
                force,
            } => {
                if remotes.iter().any(|remote| remote.name == name) && !force {
                    bail!(
                        "원격 '{}'이(가) 이미 존재합니다. 교체하려면 --force를 사용하세요.",
                        name
                    );
                }
                if force {
                    remotes.retain(|r| r.name != name);
                }
                let remote = Remote {
                    id: name.clone(),
                    name,
                    ssh,
                    identity,
                    remote_data_dir,
                    known_hosts,
                    executable,
                };
                remote.validate()?;
                remotes.push(remote.clone());
                paths.save_remotes(&remotes)?;
                print_json(&remote)?;
            }
        }
        return Ok(());
    }
    let mut tunnel = None;
    let mut client = Client::new(paths.clone())?;
    if let Some(name) = &args.remote {
        if matches!(
            args.command,
            Action::Run { .. } | Action::Start | Action::InternalWorker { .. }
        ) {
            bail!("이 명령은 --remote와 함께 사용할 수 없습니다.");
        }
        let remote = paths
            .remotes()?
            .into_iter()
            .find(|r| r.name == *name)
            .with_context(|| format!("원격을 찾을 수 없습니다: {name}"))?;
        let connected = ytlr_service::tunnel::connect(&remote).await?;
        client = client.with_endpoint(connected.endpoint.clone());
        tunnel = Some(connected);
    }
    let _tunnel = tunnel;
    if !matches!(args.command, Action::Shutdown) {
        client.ensure(&std::env::current_exe()?).await?;
    }
    match args.command {
        Action::Start => println!("녹화 서비스 실행 중"),
        Action::Record {
            url,
            from_now,
            priority,
        } => print_json(
            &client
                .post::<RecordingJob>(
                    "/jobs",
                    &RecordRequest {
                        url,
                        live_from_start: if from_now { Some(false) } else { None },
                        priority,
                    },
                )
                .await?,
        )?,
        Action::Watch { url } => print_json(
            &client
                .post::<Channel>(
                    "/channels",
                    &AddChannelRequest {
                        url,
                        name: String::new(),
                        priority: 0,
                    },
                )
                .await?,
        )?,
        Action::Status { json } => {
            let data: Snapshot = client.get("/snapshot").await?;
            if json {
                print_json(&data)?;
            } else {
                println!(
                    "YTLR {} · 작업 {}개 · 동시 녹화 상한 {}",
                    data.version,
                    data.jobs.len(),
                    data.settings.max_recordings
                );
                for job in data.jobs {
                    println!(
                        "{}  {:?}  {}\n  {}",
                        job.id, job.state, job.title, job.message
                    );
                }
            }
        }
        Action::Stop { id } => print_json(
            &client
                .post::<Value>(&format!("/jobs/{id}/stop"), &json!({}))
                .await?,
        )?,
        Action::Retry { id } => print_json(
            &client
                .post::<Value>(&format!("/jobs/{id}/retry"), &json!({}))
                .await?,
        )?,
        Action::Recover { id } => print_json(
            &client
                .post::<Value>(&format!("/jobs/{id}/recover"), &json!({}))
                .await?,
        )?,
        Action::Export { id, index } => print_json(
            &client
                .post::<Value>(&format!("/jobs/{id}/export"), &json!({"index":index}))
                .await?,
        )?,
        Action::Events { id } => {
            print_json(&client.get::<Value>(&format!("/jobs/{id}/events")).await?)?
        }
        Action::Channel { action } => match action {
            ChannelAction::Add { url, name } => print_json(
                &client
                    .post::<Channel>(
                        "/channels",
                        &AddChannelRequest {
                            url,
                            name,
                            priority: 0,
                        },
                    )
                    .await?,
            )?,
            ChannelAction::List => {
                print_json(&client.get::<Snapshot>("/snapshot").await?.channels)?
            }
            ChannelAction::Remove { id } => print_json(
                &client
                    .request::<Value>(
                        reqwest::Method::DELETE,
                        &format!("/channels/{id}"),
                        None::<&Value>,
                    )
                    .await?,
            )?,
            action @ (ChannelAction::Enable { .. } | ChannelAction::Disable { .. }) => {
                let enabled = matches!(action, ChannelAction::Enable { .. });
                let id = match action {
                    ChannelAction::Enable { id } | ChannelAction::Disable { id } => id,
                    _ => unreachable!(),
                };
                let channels = client.get::<Snapshot>("/snapshot").await?.channels;
                let mut channel = channels
                    .into_iter()
                    .find(|c| c.id == id)
                    .ok_or_else(|| anyhow::anyhow!("채널을 찾을 수 없습니다."))?;
                channel.enabled = enabled;
                print_json(
                    &client
                        .request::<Value>(
                            reqwest::Method::PUT,
                            &format!("/channels/{id}"),
                            Some(&channel),
                        )
                        .await?,
                )?;
            }
        },
        Action::Settings {
            storage,
            max_recordings,
            reserve_gib,
            backup,
            cookies,
            replica,
            po_token,
        } => {
            let mut settings = client.get::<Snapshot>("/snapshot").await?.settings;
            let changed = storage.is_some()
                || max_recordings.is_some()
                || reserve_gib.is_some()
                || backup.is_some()
                || cookies.is_some()
                || replica.is_some()
                || po_token.is_some();
            if let Some(storage) = storage {
                settings.storage_root = settings_path(storage, args.remote.is_some())?;
            }
            if let Some(n) = max_recordings {
                settings.max_recordings = n;
            }
            if let Some(n) = reserve_gib {
                settings.min_free_bytes = n
                    .checked_mul(1024 * 1024 * 1024)
                    .ok_or_else(|| anyhow::anyhow!("공간 설정값이 너무 큽니다."))?;
            }
            if let Some(backup) = backup {
                settings.backup_root = if backup.as_os_str() == "-" {
                    None
                } else {
                    Some(settings_path(backup, args.remote.is_some())?)
                };
            }
            if let Some(cookies) = cookies {
                settings.cookies_path = if cookies.as_os_str() == "-" {
                    None
                } else {
                    Some(settings_path(cookies, args.remote.is_some())?)
                };
            }
            if let Some(replica) = replica {
                settings.replica_remote = if replica == "-" { None } else { Some(replica) };
            }
            if let Some(po_token) = po_token {
                settings.po_token_path = if po_token.as_os_str() == "-" {
                    None
                } else {
                    Some(settings_path(po_token, args.remote.is_some())?)
                };
            }
            if changed {
                settings = client
                    .request(reqwest::Method::PUT, "/settings", Some(&settings))
                    .await?;
            }
            print_json(&settings)?;
        }
        Action::Doctor => {
            let _: Value = client.post("/tools/refresh", &json!({})).await?;
            let status = client.get::<Snapshot>("/snapshot").await?;
            print_json(
                &json!({"tools":status.tools,"free_bytes":status.free_bytes,"storage_root":status.settings.storage_root,"data_dir":paths.root}),
            )?;
            if status.tools.iter().any(|t| t.error.is_some()) {
                bail!("필요한 엔진이 없습니다. `ytlr tools install`로 설치할 수 있습니다.");
            }
        }
        Action::Tools {
            action: ToolsAction::Install,
        } => {
            let _: Value = client.post("/tools/install", &json!({})).await?;
            let mut last_message = None;
            loop {
                let s = client.get::<Snapshot>("/snapshot").await?;
                if s.tool_message != last_message {
                    eprintln!("{}", s.tool_message.as_deref().unwrap_or("설치 중"));
                    last_message = s.tool_message.clone();
                }
                if !s.installing_tools {
                    if s.tool_message
                        .as_deref()
                        .is_some_and(|m| m.starts_with("설치 실패"))
                    {
                        bail!("{}", s.tool_message.unwrap_or_default());
                    }
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        Action::Tools {
            action: ToolsAction::Rollback,
        } => print_json(&client.post::<Value>("/tools/rollback", &json!({})).await?)?,
        Action::Shutdown => print_json(&client.post::<Value>("/shutdown", &json!({})).await?)?,
        Action::Run { .. }
        | Action::InternalWorker { .. }
        | Action::InternalBackup { .. }
        | Action::Remote { .. }
        | Action::Tools {
            action: ToolsAction::Bundle { .. },
        } => unreachable!(),
    }
    Ok(())
}
