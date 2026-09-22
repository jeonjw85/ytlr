use anyhow::{Context, Result, bail};
use reqwest::Method;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{path::Path, process::Stdio, time::Duration};
use ytlr_core::{AppPaths, ServiceEndpoint};

fn compatible(version: &str) -> bool {
    version
        .split('.')
        .take(2)
        .eq(env!("CARGO_PKG_VERSION").split('.').take(2))
}

fn version_mismatch(version: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "실행 중인 서비스 버전({})과 앱 버전({})이 다릅니다. 녹화를 마무리하고 `ytlr shutdown` 후 다시 실행하세요.",
        version,
        env!("CARGO_PKG_VERSION")
    )
}

async fn decode_json(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .context("녹화 서비스 응답을 읽지 못했습니다.")?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) if status.is_success() => Ok(value),
        Ok(value) => bail!("{}", value["error"].as_str().unwrap_or("요청 처리 실패")),
        Err(_) => bail!(
            "녹화 서비스가 이 앱과 호환되지 않습니다. 녹화를 마무리하고 `ytlr shutdown` 후 다시 실행하세요."
        ),
    }
}

#[derive(Clone)]
pub struct Client {
    pub paths: AppPaths,
    http: reqwest::Client,
    bundled_tools: Option<std::path::PathBuf>,
    endpoint: Option<ServiceEndpoint>,
}

impl Client {
    pub fn new(paths: AppPaths) -> Result<Self> {
        Ok(Self {
            paths,
            bundled_tools: None,
            endpoint: None,
            http: reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(100))
                .build()?,
        })
    }

    pub fn with_bundled_tools(mut self, path: std::path::PathBuf) -> Self {
        self.bundled_tools = Some(path);
        self
    }

    pub fn with_endpoint(mut self, endpoint: ServiceEndpoint) -> Self {
        self.endpoint = Some(endpoint);
        self
    }
    pub async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let endpoint = match &self.endpoint {
            Some(endpoint) => endpoint.clone(),
            None => self.paths.endpoint()?,
        };
        if !path.starts_with('/') || path.contains("://") {
            bail!("잘못된 API 경로");
        }
        if path != "/health" && path != "/shutdown" && !compatible(&endpoint.version) {
            return Err(version_mismatch(&endpoint.version));
        }
        let send_json = !matches!(method, Method::GET | Method::HEAD | Method::DELETE);
        let mut request = self
            .http
            .request(method, format!("http://127.0.0.1:{}{path}", endpoint.port))
            .bearer_auth(endpoint.token);
        if let Some(body) = body
            && send_json
        {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .context("녹화 서비스에 연결할 수 없습니다.")?;
        Ok(serde_json::from_value(decode_json(response).await?)?)
    }
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(Method::GET, path, None::<&Value>).await
    }
    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        self.request(Method::POST, path, Some(body)).await
    }
    pub async fn replica_record(
        &self,
        body: &impl Serialize,
        request_id: &str,
    ) -> Result<ytlr_core::RecordingJob> {
        let endpoint = match &self.endpoint {
            Some(endpoint) => endpoint.clone(),
            None => self.paths.endpoint()?,
        };
        if !compatible(&endpoint.version) {
            bail!("원격 서비스 버전을 앱과 동일하게 업데이트해 주세요.");
        }
        let response = self
            .http
            .post(format!("http://127.0.0.1:{}/jobs", endpoint.port))
            .bearer_auth(endpoint.token)
            .header("x-ytlr-fanout", "1")
            .header("idempotency-key", request_id)
            .json(body)
            .send()
            .await
            .context("이중 녹화 원격에 연결할 수 없습니다.")?;
        Ok(serde_json::from_value(decode_json(response).await?)?)
    }
    pub async fn ensure(&self, executable: &Path) -> Result<()> {
        if self.endpoint.is_some() {
            self.get::<Value>("/health")
                .await
                .context("원격 녹화 서비스에 연결할 수 없습니다.")?;
            return Ok(());
        }
        if self.get::<Value>("/health").await.is_ok() {
            return Ok(());
        }
        self.paths.initialize()?;
        let log_path = self.paths.root.join("service.log");
        // Keep startup logs bounded between launches.
        if std::fs::metadata(&log_path).is_ok_and(|m| m.len() > 2 * 1024 * 1024) {
            let _ = std::fs::rename(&log_path, self.paths.root.join("service.previous.log"));
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        let mut command = std::process::Command::new(executable);
        if let Some(path) = &self.bundled_tools {
            command.env("YTLR_BUNDLED_TOOLS", path);
        }
        command
            .arg("--data-dir")
            .arg(&self.paths.root)
            .args(["run", "--headless"])
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x00000008 | 0x08000000);
        }
        let mut child = command.spawn().context("녹화 서비스 실행 실패")?;
        // Reap when a long-lived GUI owns the child; dropping a CLI does not kill it.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        for _ in 0..120 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if self.get::<Value>("/health").await.is_ok() {
                return Ok(());
            }
        }
        bail!(
            "서비스 시작 시간 초과. {}에서 로그를 확인하세요.",
            self.paths.root.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn incompatible_service_is_not_mutated() {
        let d = tempfile::tempdir().unwrap();
        let client = Client::new(AppPaths::resolve(Some(d.path().into())).unwrap())
            .unwrap()
            .with_endpoint(ServiceEndpoint {
                port: 1,
                token: "unused".into(),
                pid: 0,
                version: "0.1.0".into(),
            });
        let error = client
            .post::<Value>("/jobs", &serde_json::json!({}))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("버전"));
        assert!(!compatible("0.1.99"));
        assert!(compatible(env!("CARGO_PKG_VERSION")));
    }

    #[tokio::test]
    async fn empty_error_body_is_not_a_decode_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().route(
                    "/jobs/{id}",
                    axum::routing::get(|| async { axum::http::StatusCode::NOT_FOUND }),
                ),
            )
            .await
            .unwrap();
        });
        let d = tempfile::tempdir().unwrap();
        let client = Client::new(AppPaths::resolve(Some(d.path().into())).unwrap())
            .unwrap()
            .with_endpoint(ServiceEndpoint {
                port,
                token: "unused".into(),
                pid: 0,
                version: env!("CARGO_PKG_VERSION").into(),
            });
        let error = client
            .request::<Value>(Method::DELETE, "/jobs/abc", Some(&serde_json::json!({})))
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.to_lowercase().contains("decoding"));
        assert!(error.contains("shutdown"));
    }
}
