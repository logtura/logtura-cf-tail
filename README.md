# logtura-cf-tail

Cloudflare Workers Tail API bridge for Vector's `exec` source.

`wrangler tail` is convenient, but it brings Node, Wrangler, and one long-lived subprocess per Worker into production forwarder images. This binary talks to Cloudflare's Tail API directly, opens the returned WebSocket tail session, and emits newline-delimited JSON on stdout for Vector to consume.

## Usage with Vector

```yaml
sources:
  cf_workers:
    type: exec
    command: ["logtura-cf-tail", "--config", "/etc/logtura/cf-tail.toml"]
    mode: streaming
    include_stderr: false
    decoding: { codec: json }
    framing: { method: newline_delimited }
```

Diagnostics go to stderr. stdout is event-only.

## Example config

```toml
account_id = "${CLOUDFLARE_ACCOUNT_ID}"
api_token = "${CLOUDFLARE_API_TOKEN}"
scripts = ["api-worker", "billing-worker"]
```

Optional fields:

```toml
api_base = "https://api.cloudflare.com/client/v4"
tail_refresh_margin_secs = 60
reconnect_min_secs = 1
reconnect_max_secs = 30
```

`${VAR}` and `${VAR:-default}` are expanded from the process environment before TOML parsing.

## Behavior

- Starts one Cloudflare tail session per script.
- Connects to each returned WebSocket URL.
- Emits each JSON WebSocket message as one compact JSON line.
- Recreates tail sessions before their `expires_at` timestamp.
- Reconnects with exponential backoff after transient HTTP or WebSocket failures.

The output is intentionally close to `wrangler tail --format json`, so existing Vector remap transforms can normalize the same event shape.

## Live Cloudflare test

Normal `cargo test` runs only unit tests and skips the live Cloudflare Tail API test unless it is explicitly enabled.

To run the live test, create a dedicated Cloudflare API token scoped to one test Worker, not a production-wide token. The token should be limited to the account that owns the test Worker and only needs Workers Tail read access for this test.

```bash
export LOGTURA_CF_TAIL_TEST_ACCOUNT_ID="..."
export LOGTURA_CF_TAIL_TEST_API_TOKEN="..."
export LOGTURA_CF_TAIL_TEST_SCRIPT="my-test-worker"
cargo test --test live_cloudflare -- --nocapture
```

The test does not read generic `CLOUDFLARE_*` variables, does not list Workers, and does not auto-pick a script.

## Installation

Pre-built static binaries are intended to be attached to GitHub Releases:

```bash
curl -L https://github.com/logtura/logtura-cf-tail/releases/latest/download/logtura-cf-tail-x86_64-unknown-linux-musl \
  -o /usr/local/bin/logtura-cf-tail
chmod +x /usr/local/bin/logtura-cf-tail
```

Or build from source:

```bash
cargo install --git https://github.com/logtura/logtura-cf-tail
```

## License

Apache-2.0.
