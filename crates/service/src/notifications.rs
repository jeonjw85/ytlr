use crate::Service;
use anyhow::{Context, Result, bail};
use std::{sync::Arc, time::Duration};
use ytlr_core::*;

async fn deliver(
    client: &reqwest::Client,
    target: &NotificationTarget,
    delivery: &NotificationDelivery,
) -> Result<()> {
    let text: String = format!(
        "YTLR · {}\n{}\n{}",
        delivery.body["kind"].as_str().unwrap_or(""),
        delivery.body["title"]
            .as_str()
            .or_else(|| delivery.body["job_id"].as_str())
            .or_else(|| delivery.body["channel_id"].as_str())
            .unwrap_or(""),
        delivery.body["message"].as_str().unwrap_or("")
    )
    .chars()
    .take(1800)
    .collect();
    let (url, payload) = match target {
        NotificationTarget::Webhook { url_env, .. } => (
            std::env::var(url_env).context("Webhook 환경변수가 설정되지 않았습니다.")?,
            serde_json::json!({"delivery_id":delivery.id,"event":delivery.body}),
        ),
        NotificationTarget::Discord { url_env, .. } => (
            std::env::var(url_env).context("Discord 환경변수가 설정되지 않았습니다.")?,
            serde_json::json!({"content":text,"allowed_mentions":{"parse":[]}}),
        ),
        NotificationTarget::Telegram {
            token_env, chat_id, ..
        } => {
            let token =
                std::env::var(token_env).context("Telegram 환경변수가 설정되지 않았습니다.")?;
            if token.is_empty()
                || !token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-:".contains(&b))
            {
                bail!("Telegram 인증 정보 형식 오류");
            }
            (
                format!("https://api.telegram.org/bot{token}/sendMessage"),
                serde_json::json!({"chat_id":chat_id,"text":text}),
            )
        }
    };
    let url = reqwest::Url::parse(&url).map_err(|_| anyhow::anyhow!("알림 URL 형식 오류"))?;
    if url.scheme() != "https"
        && !(url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
    {
        bail!("알림 URL에는 HTTPS가 필요합니다.");
    }
    // Never include reqwest errors or response bodies: webhook URLs contain credentials.
    let response = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("알림 연결 실패 또는 시간 초과"))?;
    if !response.status().is_success() {
        let reason = match response.status().as_u16() {
            401 | 403 => "인증 또는 권한 오류",
            404 => "알림 대상 없음",
            429 => "전송 요청 한도 초과",
            400 => "알림 대상 또는 요청 형식 오류",
            _ => "알림 서버 오류",
        };
        bail!("{reason} (HTTP {})", response.status().as_u16());
    }
    if matches!(target, NotificationTarget::Telegram { .. }) {
        let result: serde_json::Value = response
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("Telegram 응답 형식 오류"))?;
        if result["ok"] != true {
            bail!("Telegram 전송 거절");
        }
    }
    Ok(())
}

pub async fn run(state: Arc<Service>) {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return;
    };
    loop {
        tokio::select! { _ = state.shutdown.cancelled() => return, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
        let (Ok(settings), Ok(deliveries)) =
            (state.store.settings(), state.store.notifications(true))
        else {
            continue;
        };
        for delivery in deliveries {
            let result = if let Some(target) = settings
                .automation
                .notifications
                .iter()
                .find(|t| t.id() == delivery.target)
            {
                tokio::select! { _ = state.shutdown.cancelled() => return, r = deliver(&client, target, &delivery) => r }
            } else {
                Err(anyhow::anyhow!(
                    "알림 대상이 제거되었습니다. 같은 ID로 다시 등록하면 재시도합니다."
                ))
            };
            let _ = state.store.finish_notification(
                delivery.id,
                delivery.attempts.saturating_add(1),
                result.err().map(|e| e.to_string()).as_deref(),
            );
        }
    }
}
