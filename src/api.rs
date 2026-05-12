use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct TailSession {
    pub id: String,
    pub expires_at: DateTime<Utc>,
    pub url: String,
}

#[derive(Debug, Deserialize)]
struct CfEnvelope<T> {
    success: bool,
    errors: Option<Vec<CfError>>,
    result: T,
}

#[derive(Debug, Deserialize)]
struct CfError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct TailResult {
    id: String,
    expires_at: DateTime<Utc>,
    url: String,
}

pub async fn create_tail(
    client: &Client,
    api_base: &str,
    account_id: &str,
    api_token: &str,
    script: &str,
) -> Result<TailSession> {
    let url = format!(
        "{}/accounts/{}/workers/scripts/{}/tails",
        api_base.trim_end_matches('/'),
        urlencoding::encode(account_id),
        urlencoding::encode(script),
    );
    let res = client
        .post(&url)
        .bearer_auth(api_token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .with_context(|| format!("creating tail for {script}"))?;
    let status = res.status();
    let body = res
        .text()
        .await
        .with_context(|| format!("reading tail create response for {script}"))?;
    let env: CfEnvelope<TailResult> = serde_json::from_str(&body)
        .with_context(|| format!("parsing Cloudflare tail response for {script}: {body}"))?;
    if !status.is_success() || !env.success {
        let msg = env
            .errors
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.message)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(anyhow!(
            "Cloudflare tail create failed for {script}: HTTP {status}{}",
            if msg.is_empty() {
                String::new()
            } else {
                format!(": {msg}")
            }
        ));
    }
    Ok(TailSession {
        id: env.result.id,
        expires_at: env.result.expires_at,
        url: env.result.url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn creates_tail_session() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/accounts/acct/workers/scripts/my-worker/tails"))
            .and(header("authorization", "Bearer tok"))
            .and(body_json(serde_json::json!({})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": true,
                "result": {
                    "id": "tail-id",
                    "expires_at": "2026-05-12T16:00:00Z",
                    "url": "wss://example.com/tail"
                }
            })))
            .mount(&server)
            .await;

        let session = create_tail(&Client::new(), &server.uri(), "acct", "tok", "my-worker")
            .await
            .unwrap();

        assert_eq!(session.id, "tail-id");
        assert_eq!(session.url, "wss://example.com/tail");
    }
}
