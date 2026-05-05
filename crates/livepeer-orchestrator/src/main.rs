mod bootstrap;
mod replay;
mod reset;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use livepeer_core::{
    config::Config,
    db,
    rpc::{cache, Provider},
    tracing_init,
};
use std::path::{Path, PathBuf};
use tracing::info;

const SERVICE: &str = "livepeer-orchestrator";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Bounded orchestration for bootstrap and replay flows.")]
struct Cli {
    #[arg(
        long,
        env = "STATIC_CONFIG",
        default_value = "config/arbitrum.yaml",
        global = true
    )]
    static_config: PathBuf,

    #[arg(
        long,
        env = "ENV_CONFIG",
        default_value = "config/env/dev.yaml",
        global = true
    )]
    env_config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    MigrateOnly,
    Bootstrap {
        #[arg(long)]
        from_block: Option<u64>,
        #[arg(long)]
        to_block: Option<u64>,
        #[arg(long, env = "SOURCE_SQLITE")]
        source_sqlite: Option<PathBuf>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long, default_value_t = false)]
        include_tentative: bool,
        #[arg(long, default_value_t = false)]
        no_resume: bool,
        #[arg(long, default_value_t = false)]
        skip_seed_import: bool,
        #[arg(long, default_value_t = false)]
        skip_cross_check: bool,
    },
    Replay {
        #[arg(long)]
        from_block: Option<u64>,
        #[arg(long)]
        to_block: Option<u64>,
        #[arg(long, env = "SOURCE_SQLITE")]
        source_sqlite: Option<PathBuf>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long, default_value_t = false)]
        include_tentative: bool,
        #[arg(long, default_value_t = false)]
        keep_raw_events: bool,
        #[arg(long, default_value_t = false)]
        allow_live_rpc: bool,
        #[arg(long, default_value_t = false)]
        skip_seed_import: bool,
        #[arg(long, default_value_t = false)]
        skip_cross_check: bool,
    },
}

pub struct Runtime {
    pub cfg: Config,
    pub pg: sqlx::PgPool,
    pub archive: Provider,
    pub l1: Option<Provider>,
    pub source_sqlite: Option<PathBuf>,
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
    let archive = Provider::new(
        "chainstack",
        cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?,
    )?;
    let l1 = match cfg.env.l1.as_ref() {
        Some(l1_cfg) => {
            let l1_url = std::env::var(l1_cfg.url_env.as_str())
                .with_context(|| format!("{} missing", l1_cfg.url_env))?;
            Some(Provider::new("l1-eth", l1_url)?)
        }
        None => None,
    };
    info!(service = SERVICE, "config + db ready");

    match cli.command {
        Command::MigrateOnly => {
            run_migrations(&pg).await?;
        }
        Command::Bootstrap {
            from_block,
            to_block,
            source_sqlite,
            version,
            include_tentative,
            no_resume,
            skip_seed_import,
            skip_cross_check,
        } => {
            let rt = Runtime {
                cfg,
                pg,
                archive,
                l1,
                source_sqlite,
            };
            bootstrap::run(
                &rt,
                bootstrap::BootstrapOpts {
                    from_block,
                    to_block,
                    version,
                    include_tentative,
                    no_resume,
                    skip_seed_import,
                    skip_cross_check,
                },
            )
            .await?;
        }
        Command::Replay {
            from_block,
            to_block,
            source_sqlite,
            version,
            include_tentative,
            keep_raw_events,
            allow_live_rpc,
            skip_seed_import,
            skip_cross_check,
        } => {
            let rt = Runtime {
                cfg,
                pg,
                archive,
                l1,
                source_sqlite,
            };
            cache::set_cache_only_mode(!allow_live_rpc);
            replay::run(
                &rt,
                replay::ReplayOpts {
                    from_block,
                    to_block,
                    version,
                    include_tentative,
                    keep_raw_events,
                    allow_live_rpc,
                    skip_seed_import,
                    skip_cross_check,
                },
            )
            .await?;
        }
    }
    Ok(())
}

async fn run_migrations(pg: &sqlx::PgPool) -> Result<()> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
    let migrator = sqlx::migrate::Migrator::new(path.as_path()).await?;
    migrator.run(pg).await?;
    Ok(())
}

async fn resolve_to_block(archive: &Provider, to_block: Option<u64>) -> Result<u64> {
    match to_block {
        Some(n) => Ok(n),
        None => {
            let head = archive.eth_block_number().await?;
            Ok(head.saturating_sub(50))
        }
    }
}
