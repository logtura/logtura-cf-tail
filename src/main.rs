use logtura_cf_tail_lib::{config, tail};

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the TOML config file.
    #[arg(long, short)]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let cfg = config::load(&args.config)
        .with_context(|| format!("loading config from {}", args.config.display()))?;
    tracing::info!(
        account_id = %cfg.account_id,
        scripts = cfg.scripts.len(),
        "logtura-cf-tail starting"
    );

    tail::run(cfg).await
}
