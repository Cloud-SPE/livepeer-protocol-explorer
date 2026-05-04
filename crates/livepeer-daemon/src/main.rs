mod rpc_manager;
mod supervisor;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use livepeer_core::{config::Config, db, tracing_init};
use std::path::PathBuf;

const SERVICE: &str = "livepeer-daemon";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Near-head follow-mode daemon for the Livepeer pipeline.")]
struct Cli {
    #[arg(long, env = "STATIC_CONFIG", default_value = "config/arbitrum.yaml", global = true)]
    static_config: PathBuf,

    #[arg(long, env = "ENV_CONFIG", default_value = "config/env/dev.yaml", global = true)]
    env_config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Follow {
        #[arg(long, default_value_t = 50_000)]
        max_start_lag_blocks: u64,
        #[arg(long)]
        version: Option<String>,
        #[arg(long, default_value_t = false)]
        include_tentative: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_init::init("info");
    let cli = Cli::parse();
    let cfg = Config::load(&cli.static_config, &cli.env_config).context("loading config")?;
    let pg = db::connect(
        &cfg.database_url().context("DATABASE_URL")?,
        cfg.env.postgres.pool_max_connections,
    )
    .await
    .context("connecting to Postgres")?;

    match cli.command {
        Command::Follow {
            max_start_lag_blocks,
            version,
            include_tentative,
        } => {
            let version = version.unwrap_or_else(|| cfg.static_.pricing.default_valuation_version.clone());
            let rpc = rpc_manager::RpcManager::new(&cfg).await?;
            supervisor::run_follow(
                &pg,
                &cfg,
                rpc,
                supervisor::FollowConfig {
                    max_start_lag_blocks,
                    valuation_version: version,
                    include_tentative,
                },
            )
            .await?;
        }
    }
    Ok(())
}
