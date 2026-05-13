use chrono::Utc;
use logtura_cf_tail_lib::api::create_tail;
use reqwest::Client;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn live_env() -> Option<(String, String, String)> {
    let account_id = std::env::var("LOGTURA_CF_TAIL_TEST_ACCOUNT_ID").ok()?;
    let api_token = std::env::var("LOGTURA_CF_TAIL_TEST_API_TOKEN").ok()?;
    let script = std::env::var("LOGTURA_CF_TAIL_TEST_SCRIPT").ok()?;
    Some((account_id, api_token, script))
}

#[test]
fn emits_live_events_with_script_name_when_configured() {
    let Some((account_id, api_token, script)) = live_env() else {
        eprintln!(
            "skipping live Cloudflare Tail event-shape test; set LOGTURA_CF_TAIL_TEST_ACCOUNT_ID, LOGTURA_CF_TAIL_TEST_API_TOKEN, LOGTURA_CF_TAIL_TEST_SCRIPT"
        );
        return;
    };

    let mut cfg = tempfile::NamedTempFile::new().expect("create temp config");
    writeln!(cfg, "account_id = {account_id:?}").expect("write account_id");
    writeln!(cfg, "api_token = {api_token:?}").expect("write api_token");
    writeln!(cfg, "scripts = [{script:?}]").expect("write scripts");

    let mut child = Command::new(env!("CARGO_BIN_EXE_logtura-cf-tail"))
        .arg("--config")
        .arg(cfg.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn logtura-cf-tail");

    let stdout = child.stdout.take().expect("child stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = tx.send(Err(
                        "logtura-cf-tail exited before emitting a live event".to_string()
                    ));
                    return;
                }
                Ok(_) if line.trim().is_empty() => continue,
                Ok(_) => {
                    let _ = tx.send(Ok(line.clone()));
                    return;
                }
                Err(err) => {
                    let _ = tx.send(Err(format!("read live event: {err}")));
                    return;
                }
            }
        }
    });

    let line = match rx.recv_timeout(Duration::from_secs(90)) {
        Ok(Ok(line)) => line,
        Ok(Err(err)) => {
            let _ = child.kill();
            panic!("{err}");
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let _ = child.kill();
            panic!("timed out waiting for a live Cloudflare tail event for {script}");
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let _ = child.kill();
            panic!("live event reader disconnected");
        }
    };
    let _ = child.kill();
    let event: Value = serde_json::from_str(&line).expect("event is JSON");
    assert_eq!(
        event.get("scriptName").and_then(Value::as_str),
        Some(script.as_str())
    );
}

#[tokio::test]
async fn creates_real_cloudflare_tail_session_when_explicitly_enabled() {
    let Some((account_id, api_token, script)) = live_env() else {
        eprintln!(
            "skipping live Cloudflare Tail API test; set LOGTURA_CF_TAIL_TEST_ACCOUNT_ID, LOGTURA_CF_TAIL_TEST_API_TOKEN, LOGTURA_CF_TAIL_TEST_SCRIPT"
        );
        return;
    };

    let session = create_tail(
        &Client::new(),
        "https://api.cloudflare.com/client/v4",
        &account_id,
        &api_token,
        &script,
    )
    .await
    .expect("create Cloudflare tail session");

    assert!(!session.id.trim().is_empty());
    assert!(
        session.url.starts_with("wss://") || session.url.starts_with("ws://"),
        "unexpected tail WebSocket URL: {}",
        session.url
    );
    assert!(
        session.expires_at > Utc::now(),
        "tail session should expire in the future"
    );
}
