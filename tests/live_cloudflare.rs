use chrono::Utc;
use logtura_cf_tail_lib::api::create_tail;
use reqwest::Client;

fn live_env() -> Option<(String, String, String)> {
    let account_id = std::env::var("LOGTURA_CF_TAIL_TEST_ACCOUNT_ID").ok()?;
    let api_token = std::env::var("LOGTURA_CF_TAIL_TEST_API_TOKEN").ok()?;
    let script = std::env::var("LOGTURA_CF_TAIL_TEST_SCRIPT").ok()?;
    Some((account_id, api_token, script))
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
