use crate::api::{create_tail, TailSession};
use crate::config::Config;
use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Output = Arc<Mutex<tokio::io::Stdout>>;
const TRACE_PROTOCOL: &str = "trace-v1";

pub async fn run(cfg: Config) -> Result<()> {
    let client = Client::builder()
        .user_agent(concat!("logtura-cf-tail/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("building HTTP client")?;
    let stdout = Arc::new(Mutex::new(tokio::io::stdout()));
    let mut tasks = Vec::with_capacity(cfg.scripts.len());

    for script in cfg.scripts.clone() {
        let worker_cfg = cfg.clone();
        let worker_client = client.clone();
        let worker_stdout = Arc::clone(&stdout);
        tasks.push(tokio::spawn(async move {
            tail_worker(worker_cfg, worker_client, worker_stdout, script).await;
        }));
    }

    for task in tasks {
        task.await.context("tail worker task panicked")?;
    }
    Ok(())
}

async fn tail_worker(cfg: Config, client: Client, stdout: Output, script: String) {
    let mut backoff = Duration::from_secs(cfg.reconnect_min_secs);
    let max_backoff = Duration::from_secs(cfg.reconnect_max_secs);

    loop {
        match tail_once(&cfg, &client, &stdout, &script).await {
            Ok(()) => {
                tracing::info!(script, "tail session ended; reconnecting");
                backoff = Duration::from_secs(cfg.reconnect_min_secs);
            }
            Err(err) => {
                tracing::warn!(script, error = ?err, "tail session failed; reconnecting after backoff");
                sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
            }
        }
    }
}

async fn tail_once(cfg: &Config, client: &Client, stdout: &Output, script: &str) -> Result<()> {
    let session = create_tail(
        client,
        &cfg.api_base,
        &cfg.account_id,
        &cfg.api_token,
        script,
    )
    .await?;
    tracing::info!(
        script,
        tail_id = %session.id,
        expires_at = %session.expires_at,
        "created Cloudflare tail session"
    );

    stream_session(session, cfg.tail_refresh_margin_secs, stdout, script).await
}

async fn stream_session(
    session: TailSession,
    refresh_margin_secs: u64,
    stdout: &Output,
    script: &str,
) -> Result<()> {
    let refresh_at = refresh_at(session.expires_at, refresh_margin_secs);
    let mut request = session
        .url
        .as_str()
        .into_client_request()
        .with_context(|| format!("building WebSocket request for {script}"))?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(TRACE_PROTOCOL),
    );
    request.headers_mut().insert(
        "User-Agent",
        HeaderValue::from_static(concat!("logtura-cf-tail/", env!("CARGO_PKG_VERSION"))),
    );

    let (ws, _) = connect_async(request)
        .await
        .with_context(|| format!("connecting WebSocket for {script}"))?;
    tracing::info!(script, tail_id = %session.id, "connected Cloudflare tail WebSocket");
    stream_connected_websocket(ws, session, refresh_at, stdout, script).await
}

async fn stream_connected_websocket(
    ws: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    session: TailSession,
    refresh_at: Instant,
    stdout: &Output,
    script: &str,
) -> Result<()> {
    let (mut write, mut read) = ws.split();
    write
        .send(Message::Text(r#"{"debug":false}"#.into()))
        .await
        .with_context(|| format!("sending initial tail options for {script}"))?;

    loop {
        tokio::select! {
            _ = sleep_until(refresh_at) => {
                tracing::info!(script, tail_id = %session.id, "refreshing tail session before expiry");
                return Ok(());
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => emit_json_lines(stdout, &text).await?,
                    Some(Ok(Message::Binary(bytes))) => {
                        let text = String::from_utf8(bytes)
                            .with_context(|| format!("non-utf8 binary WebSocket message for {script}"))?;
                        emit_json_lines(stdout, &text).await?;
                    }
                    Some(Ok(Message::Close(frame))) => {
                        tracing::info!(script, ?frame, "Cloudflare tail WebSocket closed");
                        return Ok(());
                    }
                    Some(Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_))) => {}
                    Some(Err(err)) => return Err(err).with_context(|| format!("reading WebSocket for {script}")),
                    None => return Ok(()),
                }
            }
        }
    }
}

fn refresh_at(expires_at: chrono::DateTime<Utc>, refresh_margin_secs: u64) -> Instant {
    let now = Utc::now();
    let refresh_at = expires_at - chrono::Duration::seconds(refresh_margin_secs as i64);
    if refresh_at <= now {
        Instant::now()
    } else {
        let duration = (refresh_at - now)
            .to_std()
            .unwrap_or_else(|_| Duration::from_secs(0));
        Instant::now() + duration
    }
}

async fn sleep_until(deadline: Instant) {
    tokio::time::sleep_until(deadline).await;
}

pub async fn emit_json_lines(stdout: &Output, raw: &str) -> Result<()> {
    let parsed: Value = serde_json::from_str(raw).context("parsing WebSocket JSON message")?;
    let mut out = stdout.lock().await;
    match parsed {
        Value::Array(items) => {
            for item in items {
                write_json_line(&mut out, &item).await?;
            }
        }
        value => write_json_line(&mut out, &value).await?,
    }
    Ok(())
}

async fn write_json_line(out: &mut tokio::io::Stdout, value: &Value) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let mut line = serde_json::to_vec(value).context("serializing JSON line")?;
    line.push(b'\n');
    out.write_all(&line).await.context("writing JSON line")?;
    out.flush().await.context("flushing JSON line")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use tokio_tungstenite::accept_hdr_async;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    #[test]
    fn refresh_time_is_immediate_when_already_inside_margin() {
        let expires_at = Utc::now() + chrono::Duration::seconds(10);
        let deadline = refresh_at(expires_at, 60);
        assert!(deadline <= Instant::now() + Duration::from_millis(10));
    }

    #[tokio::test]
    #[allow(clippy::result_large_err)]
    async fn websocket_uses_trace_protocol_and_sends_debug_options() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let ws = accept_hdr_async(stream, |req: &Request, response: Response| {
                assert_eq!(
                    req.headers()
                        .get("sec-websocket-protocol")
                        .and_then(|v| v.to_str().ok()),
                    Some(TRACE_PROTOCOL)
                );
                let mut response = response;
                response.headers_mut().insert(
                    "Sec-WebSocket-Protocol",
                    HeaderValue::from_static(TRACE_PROTOCOL),
                );
                Ok(response)
            })
            .await
            .unwrap();

            let (mut write, mut read) = ws.split();
            let first = read.next().await.unwrap().unwrap();
            assert_eq!(first.into_text().unwrap(), r#"{"debug":false}"#);
            write.close().await.unwrap();
            let _ = tx.send(());
        });

        let session = TailSession {
            id: "tail-id".into(),
            expires_at: Utc::now() + chrono::Duration::minutes(5),
            url: format!("ws://{addr}/tail"),
        };
        let stdout = Arc::new(Mutex::new(tokio::io::stdout()));
        stream_session(session, 60, &stdout, "unit-worker")
            .await
            .unwrap();
        rx.await.unwrap();
    }
}
