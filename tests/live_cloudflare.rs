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

fn assert_tail_event_shape(event: &Value, expected_script: &str) {
    let obj = event.as_object().expect("tail event is a JSON object");
    assert_eq!(
        obj.get("scriptName").and_then(Value::as_str),
        Some(expected_script),
        "tail event should include the worker script name"
    );
    assert!(
        obj.get("outcome")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty()),
        "tail event should include a non-empty outcome"
    );
    assert!(
        obj.get("eventTimestamp").and_then(Value::as_i64).is_some()
            || obj.get("eventTimestamp").and_then(Value::as_u64).is_some(),
        "tail event should include a numeric eventTimestamp"
    );
    assert!(
        obj.get("logs").and_then(Value::as_array).is_some(),
        "tail event should include logs as an array"
    );
    assert!(
        obj.get("exceptions").and_then(Value::as_array).is_some(),
        "tail event should include exceptions as an array"
    );
    assert!(
        obj.get("event").is_some(),
        "tail event should include the Cloudflare event envelope"
    );

    for log in obj.get("logs").and_then(Value::as_array).unwrap() {
        let log_obj = log.as_object().expect("log entry is an object");
        assert!(
            log_obj
                .get("level")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty()),
            "log entry should include a non-empty level"
        );
        assert!(
            log_obj.get("message").and_then(Value::as_array).is_some(),
            "log entry should include message as an array"
        );
        assert!(
            log_obj.get("timestamp").and_then(Value::as_i64).is_some()
                || log_obj.get("timestamp").and_then(Value::as_u64).is_some(),
            "log entry should include a numeric timestamp"
        );
    }

    for exception in obj.get("exceptions").and_then(Value::as_array).unwrap() {
        let ex_obj = exception.as_object().expect("exception entry is an object");
        assert!(
            ex_obj
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty()),
            "exception entry should include a non-empty name"
        );
        assert!(
            ex_obj.get("message").and_then(Value::as_str).is_some(),
            "exception entry should include a message string"
        );
        assert!(
            ex_obj.get("timestamp").and_then(Value::as_i64).is_some()
                || ex_obj.get("timestamp").and_then(Value::as_u64).is_some(),
            "exception entry should include a numeric timestamp"
        );
        if let Some(stack) = ex_obj.get("stack") {
            assert!(
                stack.as_str().is_some_and(|s| !s.is_empty()),
                "exception stack should be a non-empty string when present"
            );
        }
    }
}

fn stop_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn fixture_tail_events_match_expected_shape() {
    let fixtures = [
        (
            include_str!("fixtures/dirtsignal-canceled-queue.json"),
            "dirtsignal",
        ),
        (
            include_str!("fixtures/dirtsignal-queue-log.json"),
            "dirtsignal",
        ),
        (
            include_str!("fixtures/dirtsignal-exceeded-memory.json"),
            "dirtsignal",
        ),
        (
            include_str!("fixtures/worker-exception.json"),
            "logtura-tail-shape-fixture",
        ),
    ];

    for (raw, script) in fixtures {
        let event: Value = serde_json::from_str(raw).expect("fixture is JSON");
        assert_tail_event_shape(&event, script);
    }
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
            stop_child(&mut child);
            panic!("{err}");
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            stop_child(&mut child);
            panic!("timed out waiting for a live Cloudflare tail event for {script}");
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            stop_child(&mut child);
            panic!("live event reader disconnected");
        }
    };
    stop_child(&mut child);
    let event: Value = serde_json::from_str(&line).expect("event is JSON");
    assert_tail_event_shape(&event, script.as_str());
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
