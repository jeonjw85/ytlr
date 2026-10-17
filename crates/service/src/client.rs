use anyhow::{Context, Result, bail};
use reqwest::Method;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{path::Path, process::Stdio, time::Duration};
use ytlr_core::{AppPaths, SERVICE_API_VERSION, ServiceEndpoint};

fn compatible(endpoint: &ServiceEndpoint) -> bool {
    endpoint.api_version == SERVICE_API_VERSION
        && endpoint
            .version
            .split('.')
            .take(2)
            .eq(env!("CARGO_PKG_VERSION").split('.').take(2))
}

fn version_mismatch(endpoint: &ServiceEndpoint) -> anyhow::Error {
    anyhow::anyhow!(
        "실행 중인 서비스 버전({} / API {})과 앱 버전({} / API {})이 다릅니다. 녹화를 마무리하고 `ytlr shutdown` 후 다시 실행하세요.",
        endpoint.version,
        endpoint.api_version,
        env!("CARGO_PKG_VERSION"),
        SERVICE_API_VERSION
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
    pub fn endpoint_info(&self) -> Result<ServiceEndpoint> {
        self.endpoint
            .clone()
            .map(Ok)
            .unwrap_or_else(|| self.paths.endpoint())
    }
    pub async fn file_chunk(&self, job: &str, body: &impl Serialize) -> Result<Vec<u8>> {
        let endpoint = self.endpoint_info()?;
        if !compatible(&endpoint) {
            return Err(version_mismatch(&endpoint));
        }
        let response = self
            .http
            .post(format!(
                "http://127.0.0.1:{}/jobs/{job}/file-chunk",
                endpoint.port
            ))
            .bearer_auth(endpoint.token)
            .header("x-ytlr-api-version", SERVICE_API_VERSION.to_string())
            .json(body)
            .send()
            .await?;
        if !response.status().is_success() {
            decode_json(response).await?;
            bail!("파일 전송 실패");
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 1024 * 1024 {
                bail!("파일 전송 응답 크기 오류");
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
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

    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self> {
        self.http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(timeout)
            .build()?;
        Ok(self)
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
        if path != "/health" && path != "/shutdown" && !compatible(&endpoint) {
            return Err(version_mismatch(&endpoint));
        }
        let send_json = !matches!(method, Method::GET | Method::HEAD | Method::DELETE);
        let body = if send_json {
            body.map(serde_json::to_value).transpose()?
        } else {
            None
        };
        let requested_format = if method == Method::POST && matches!(path, "/downloads" | "/jobs") {
            body.as_ref()
                .and_then(|value| value.get("download_format"))
                .filter(|format| !format.is_null())
        } else {
            None
        };
        if let Some(format) = requested_format {
            // Use the same endpoint for negotiation and submission, including SSH tunnels.
            let health = self
                .http
                .get(format!("http://127.0.0.1:{}/health", endpoint.port))
                .bearer_auth(&endpoint.token)
                .header("x-ytlr-api-version", SERVICE_API_VERSION.to_string())
                .send()
                .await
                .context("다운로드 형식 지원 여부를 확인할 수 없습니다.")?;
            let health = decode_json(health).await?;
            if !health["download_formats"]
                .as_array()
                .is_some_and(|formats| formats.contains(format))
            {
                bail!(
                    "연결된 서비스가 선택한 다운로드 형식을 지원하지 않습니다. 해당 기기의 서비스를 업데이트하고 다시 시작해 주세요."
                );
            }
        }
        let mut request = self
            .http
            .request(method, format!("http://127.0.0.1:{}{path}", endpoint.port))
            .bearer_auth(endpoint.token)
            .header("x-ytlr-api-version", SERVICE_API_VERSION.to_string());
        if path.starts_with("/jobs/")
            && (path.contains("/cleanup")
                || path.ends_with("/clip")
                || path.contains("/export")
                || path.ends_with("/file-info"))
        {
            request = request.timeout(Duration::from_secs(3600));
        }
        if let Some(body) = &body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .context("녹화 서비스에 연결할 수 없습니다.")?;
        let value = decode_json(response).await?;
        if let Some(format) = requested_format
            && value.get("download_format") != Some(format)
        {
            bail!(
                "서비스가 요청한 다운로드 형식을 저장하지 않았습니다. 작업 목록을 확인하고 서비스를 업데이트해 주세요."
            );
        }
        Ok(serde_json::from_value(value)?)
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
        if !compatible(&endpoint) {
            bail!("원격 서비스 버전을 앱과 동일하게 업데이트해 주세요.");
        }
        let response = self
            .http
            .post(format!("http://127.0.0.1:{}/jobs", endpoint.port))
            .bearer_auth(endpoint.token)
            .header("x-ytlr-api-version", SERVICE_API_VERSION.to_string())
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
    async fn custom_timeout_bounds_polling_without_changing_other_clients() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = ServiceEndpoint {
            api_version: SERVICE_API_VERSION,
            port: listener.local_addr().unwrap().port(),
            token: "unused".into(),
            pid: 0,
            version: env!("CARGO_PKG_VERSION").into(),
        };
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().route(
                    "/snapshot",
                    axum::routing::get(|| async {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        axum::Json(serde_json::json!({"jobs":[]}))
                    }),
                ),
            )
            .await
            .unwrap();
        });
        let d = tempfile::tempdir().unwrap();
        let ordinary = Client::new(AppPaths::resolve(Some(d.path().into())).unwrap())
            .unwrap()
            .with_endpoint(endpoint);
        let poller = ordinary
            .clone()
            .with_timeout(Duration::from_millis(20))
            .unwrap();
        assert!(poller.get::<Value>("/snapshot").await.is_err());
        assert_eq!(
            ordinary.get::<Value>("/snapshot").await.unwrap(),
            serde_json::json!({"jobs":[]})
        );
        server.abort();
    }

    // Model an older service: it accepts the request but drops unknown format fields.
    async fn legacy_download_service(
        health: Value,
        remote: bool,
    ) -> (
        Client,
        std::sync::Arc<ytlr_core::Store>,
        tokio::task::JoinHandle<()>,
        tempfile::TempDir,
    ) {
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::resolve(Some(d.path().into())).unwrap();
        paths.initialize().unwrap();
        let store = std::sync::Arc::new(
            ytlr_core::Store::open(&paths.database(), &paths.default_settings()).unwrap(),
        );
        let old_download = {
            let store = store.clone();
            move |axum::Json(mut request): axum::Json<ytlr_core::RecordRequest>| async move {
                request.download_format = None;
                axum::Json(store.add_job(&request, None, false).unwrap())
            }
        };
        let router = axum::Router::new()
            .route(
                "/health",
                axum::routing::get(move || async move { axum::Json(health) }),
            )
            .route("/downloads", axum::routing::post(old_download.clone()))
            .route("/jobs", axum::routing::post(old_download));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = ServiceEndpoint {
            port: listener.local_addr().unwrap().port(),
            token: "unused".into(),
            pid: 0,
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: SERVICE_API_VERSION,
        };
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = Client::new(paths.clone()).unwrap();
        let client = if remote {
            client.with_endpoint(endpoint)
        } else {
            ytlr_core::atomic_write(
                &paths.root.join("service.json"),
                &serde_json::to_vec(&endpoint).unwrap(),
            )
            .unwrap();
            client
        };
        (client, store, task, d)
    }

    #[tokio::test]
    async fn unsupported_download_format_does_not_create_a_legacy_job() {
        for remote in [false, true] {
            for health in [
                serde_json::json!({"ok": true, "version": "0.2.5"}),
                serde_json::json!({"ok": true, "version": "0.2.5", "download_formats": ["mkv"]}),
            ] {
                let (client, store, task, _d) = legacy_download_service(health, remote).await;
                for path in ["/downloads", "/jobs"] {
                    let result = client.post::<Value>(path, &serde_json::json!({
                        "url": "https://youtu.be/vodvideo001", "kind": "download", "download_format": "mp4"
                    })).await;
                    assert!(
                        store.jobs().unwrap().is_empty(),
                        "unsupported format must be rejected before sending a mutating request"
                    );
                    assert!(result.unwrap_err().to_string().contains("업데이트"));
                }
                // Requests without a selected format keep the legacy contract.
                let legacy: ytlr_core::RecordingJob = client
                    .post(
                        "/downloads",
                        &serde_json::json!({
                            "url": "https://youtu.be/vodvideo001", "kind": "download"
                        }),
                    )
                    .await
                    .unwrap();
                assert_eq!(legacy.download_format, None);
                assert_eq!(store.jobs().unwrap().len(), 1);
                task.abort();
            }
        }
    }

    #[tokio::test]
    async fn download_response_must_acknowledge_the_requested_format() {
        for path in ["/downloads", "/jobs"] {
            let (client, store, task, _d) = legacy_download_service(
                serde_json::json!({"ok": true, "version": "0.2.5", "download_formats": ["mp4"]}),
                true,
            )
            .await;
            let result = client.post::<Value>(path, &serde_json::json!({
                "url": "https://youtu.be/vodvideo001", "kind": "download", "download_format": "mp4"
            })).await;
            assert_eq!(store.jobs().unwrap().len(), 1);
            assert!(
                result.unwrap_err().to_string().contains("형식"),
                "a discarded format must not be reported as success"
            );
            task.abort();
        }
    }

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
                api_version: 0,
            });
        let error = client
            .post::<Value>("/jobs", &serde_json::json!({}))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("버전"));
        let incompatible = ServiceEndpoint {
            port: 1,
            token: String::new(),
            pid: 0,
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: 0,
        };
        assert!(!compatible(&incompatible));
        let compatible_endpoint = ServiceEndpoint {
            api_version: SERVICE_API_VERSION,
            ..incompatible
        };
        assert!(compatible(&compatible_endpoint));
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
                api_version: SERVICE_API_VERSION,
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
