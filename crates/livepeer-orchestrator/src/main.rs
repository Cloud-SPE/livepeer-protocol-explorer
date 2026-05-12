mod backfill_cuts;
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
    /// One-shot historical backfill of `orch_stake_by_round` cut columns
    /// using chain-truth via `BondingManager.getTranscoder()` at each row's
    /// snapshot block. Replaces values that were derived from the stale
    /// `TranscoderUpdate` event payload (which carried *pending*, not
    /// *active*, cuts). Idempotent. Never deletes.
    BackfillOrchCuts {
        /// Bounded concurrency for the eth_call fanout. Chainstack
        /// empirically tolerated 12 sustained; 24+ tripped 429.
        #[arg(long, default_value_t = 12)]
        concurrency: usize,
        /// Path to write the change-log CSV. Defaults to
        /// `/tmp/cuts-backfill-<unix>.csv`.
        #[arg(long)]
        csv_out: Option<PathBuf>,
        /// Compute deltas and write the CSV, but roll back the UPDATEs.
        /// Useful for previewing what would change before committing.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Restrict to a single orch address (case-insensitive). Useful for
        /// spot-checking the fix against one of the known-failing orchs
        /// before running the full sweep.
        #[arg(long)]
        address: Option<String>,
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
        Command::BackfillOrchCuts {
            concurrency,
            csv_out,
            dry_run,
            address,
        } => {
            let rt = Runtime {
                cfg,
                pg,
                archive,
                l1,
                source_sqlite: None,
            };
            backfill_cuts::run(
                &rt,
                backfill_cuts::BackfillCutsOpts {
                    concurrency,
                    csv_out,
                    dry_run,
                    address,
                },
            )
            .await?;
        }
    }
    Ok(())
}

async fn run_migrations(pg: &sqlx::PgPool) -> Result<()> {
    let path = resolve_migrations_path()?;
    info!(path = %path.display(), "loading migrations");
    let migrator = sqlx::migrate::Migrator::new(path.as_path()).await?;
    migrator.run(pg).await?;
    Ok(())
}

// Locate the `migrations/` directory at runtime. The previous compile-time
// path baked in `/build/...` from the rust-builder stage, which doesn't exist
// in the slim runtime image — so `migrate-only` from the prod container would
// fail with FileNotFound. Resolution order:
//   1. `MIGRATIONS_PATH` env override (escape hatch),
//   2. `/opt/livepeer/migrations` (laid down by the runtime stage of Dockerfile),
//   3. compile-time source-tree path (works for `cargo run` from any subdir).
fn resolve_migrations_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("MIGRATIONS_PATH") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Ok(path);
        }
        anyhow::bail!(
            "MIGRATIONS_PATH={} but the directory does not exist",
            path.display()
        );
    }
    let runtime = PathBuf::from("/opt/livepeer/migrations");
    if runtime.exists() {
        return Ok(runtime);
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
    if source.exists() {
        return Ok(source);
    }
    anyhow::bail!(
        "could not locate migrations directory; tried MIGRATIONS_PATH, /opt/livepeer/migrations, and {}",
        source.display()
    );
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
