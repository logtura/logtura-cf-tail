use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub account_id: String,
    pub api_token: String,
    pub scripts: Vec<String>,
    #[serde(default = "default_api_base")]
    pub api_base: String,
    #[serde(default = "default_tail_refresh_margin_secs")]
    pub tail_refresh_margin_secs: u64,
    #[serde(default = "default_reconnect_min_secs")]
    pub reconnect_min_secs: u64,
    #[serde(default = "default_reconnect_max_secs")]
    pub reconnect_max_secs: u64,
}

fn default_api_base() -> String {
    "https://api.cloudflare.com/client/v4".into()
}

fn default_tail_refresh_margin_secs() -> u64 {
    60
}

fn default_reconnect_min_secs() -> u64 {
    1
}

fn default_reconnect_max_secs() -> u64 {
    30
}

pub fn load(path: &Path) -> Result<Config> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file {}", path.display()))?;
    let interpolated = interpolate_env(&raw)?;
    let cfg: Config = toml::from_str(&interpolated).context("parsing TOML")?;
    validate(&cfg)?;
    Ok(cfg)
}

fn interpolate_env(input: &str) -> Result<String> {
    let re = regex::Regex::new(r"\$\{([A-Z_][A-Z0-9_]*)(?::-([^}]*))?\}").unwrap();
    let mut errors = Vec::new();
    let out = re.replace_all(input, |caps: &regex::Captures<'_>| {
        let name = &caps[1];
        match std::env::var(name) {
            Ok(v) => v,
            Err(_) => {
                if let Some(default) = caps.get(2) {
                    default.as_str().to_string()
                } else {
                    errors.push(name.to_string());
                    String::new()
                }
            }
        }
    });
    if !errors.is_empty() {
        return Err(anyhow!(
            "missing env vars referenced in config: {}",
            errors.join(", ")
        ));
    }
    Ok(out.into_owned())
}

pub fn validate(cfg: &Config) -> Result<()> {
    if cfg.account_id.trim().is_empty() {
        return Err(anyhow!("account_id must not be empty"));
    }
    if cfg.api_token.trim().is_empty() {
        return Err(anyhow!("api_token must not be empty"));
    }
    if cfg.scripts.is_empty() {
        return Err(anyhow!("scripts must contain at least one worker name"));
    }
    for script in &cfg.scripts {
        if script.trim().is_empty() {
            return Err(anyhow!("scripts must not contain empty worker names"));
        }
    }
    if cfg.tail_refresh_margin_secs == 0 {
        return Err(anyhow!("tail_refresh_margin_secs must be >= 1"));
    }
    if cfg.reconnect_min_secs == 0 {
        return Err(anyhow!("reconnect_min_secs must be >= 1"));
    }
    if cfg.reconnect_max_secs < cfg.reconnect_min_secs {
        return Err(anyhow!("reconnect_max_secs must be >= reconnect_min_secs"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let cfg: Config = toml::from_str(
            r#"
            account_id = "acct"
            api_token = "tok"
            scripts = ["a", "b"]
            "#,
        )
        .unwrap();
        validate(&cfg).unwrap();
        assert_eq!(cfg.api_base, "https://api.cloudflare.com/client/v4");
        assert_eq!(cfg.tail_refresh_margin_secs, 60);
    }

    #[test]
    fn rejects_empty_scripts() {
        let cfg: Config = toml::from_str(
            r#"
            account_id = "acct"
            api_token = "tok"
            scripts = []
            "#,
        )
        .unwrap();
        assert!(validate(&cfg).is_err());
    }
}
