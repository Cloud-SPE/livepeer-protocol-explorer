use crate::metrics::Metrics;
use alloy::{
    primitives::{keccak256, Address, B256},
    sol,
    sol_types::{SolCall, SolType},
};
use anyhow::{anyhow, Context, Result};
use livepeer_core::rpc::{BlockTag, Provider};
use serde::Deserialize;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use std::{collections::HashSet, str::FromStr};
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

const ENS_REGISTRY: &str = "0x00000000000c2e074ec69a0dfb2997ba6c7d2e1e";
const ZERO_ADDRESS: &str = "0x0000000000000000000000000000000000000000";
const DEFAULT_TTL_DAYS: i64 = 7;
pub const DEFAULT_BATCH_LIMIT: i64 = 100;
const FAILURE_BREAKER_THRESHOLD: usize = 5;
const FAILURE_BREAKER_COOLDOWN_SECS: u64 = 60;
const L1_WATCH_CHUNK_SIZE: u64 = 10_000;
const L1_WATCH_CHECKPOINT: &str = "enricher_ens_l1_logs";
const NAME_CHANGED_TOPIC0: &str =
    "0xb7d29e911041e8d9b843369e890bcb72c9388692ba48b65ac54e7214c4c348f7";
const TEXT_CHANGED_V1_TOPIC0: &str =
    "0xd8c9334b1a9c2f9da342a0a2b32629c1a229b6445dad78947f674b44444a7550";
const TEXT_CHANGED_V2_TOPIC0: &str =
    "0x03c78c05b3f473334642cabadd0a57a0e2e6b9ef90b572644b11c1230f0b68cf";
const AVATAR_KEY: &str = "avatar";

sol! {
    interface ENSRegistry {
        function resolver(bytes32 node) external view returns (address);
    }

    interface ENSResolver {
        function name(bytes32 node) external view returns (string memory);
        function addr(bytes32 node) external view returns (address);
        function text(bytes32 node, string calldata key) external view returns (string memory);
    }
}

#[derive(Debug, Default)]
pub struct SweepSummary {
    pub orchestrators_seen: u64,
    pub orchestrators_updated: u64,
    pub gateways_seen: u64,
    pub gateways_updated: u64,
    pub named_rows: u64,
    pub avatar_rows: u64,
    pub failures: u64,
}

#[derive(Debug, Default)]
pub struct WatchSummary {
    pub latest_l1_block: u64,
    pub logs_seen: u64,
    pub addresses_refreshed: u64,
}

#[derive(Debug)]
struct EnsProjection {
    ens_name: Option<String>,
    ens_avatar_url: Option<String>,
}

#[derive(Debug)]
struct FailureBreaker {
    consecutive_failures: usize,
    metrics: Arc<Metrics>,
}

impl FailureBreaker {
    async fn on_result<T>(&mut self, result: &Result<T>) {
        if result.is_ok() {
            self.consecutive_failures = 0;
            self.metrics.breaker_open.set(0);
            return;
        }
        self.consecutive_failures += 1;
        if self.consecutive_failures < FAILURE_BREAKER_THRESHOLD {
            return;
        }
        warn!(
            consecutive_failures = self.consecutive_failures,
            cooldown_secs = FAILURE_BREAKER_COOLDOWN_SECS,
            "livepeer-enricher circuit breaker open; cooling down before more L1 ENS reads"
        );
        self.metrics
            .breaker_open_total
            .with_label_values(&["consecutive_failures"])
            .inc();
        self.metrics.breaker_open.set(1);
        sleep(Duration::from_secs(FAILURE_BREAKER_COOLDOWN_SECS)).await;
        self.metrics.breaker_open.set(0);
        self.consecutive_failures = 0;
    }
}

pub async fn watch_once(
    pg: &PgPool,
    l1: &Provider,
    chain_id: i64,
    metrics: &Arc<Metrics>,
) -> Result<WatchSummary> {
    let latest = l1.eth_block_number().await?;
    let start = match load_watch_checkpoint(pg).await? {
        Some(block) => block.saturating_add(1),
        None => {
            advance_watch_checkpoint(pg, latest).await?;
            return Ok(WatchSummary {
                latest_l1_block: latest,
                ..Default::default()
            });
        }
    };

    if start > latest {
        return Ok(WatchSummary {
            latest_l1_block: latest,
            ..Default::default()
        });
    }

    let mut summary = WatchSummary {
        latest_l1_block: latest,
        ..Default::default()
    };
    let mut breaker = FailureBreaker {
        consecutive_failures: 0,
        metrics: metrics.clone(),
    };
    let mut chunk_start = start;
    while chunk_start <= latest {
        let chunk_end = std::cmp::min(chunk_start + L1_WATCH_CHUNK_SIZE - 1, latest);
        let logs = fetch_ens_change_logs(l1, chunk_start, chunk_end).await?;
        let addresses = find_affected_addresses(pg, chain_id, &logs).await?;
        summary.logs_seen += logs.len() as u64;
        summary.addresses_refreshed += addresses.len() as u64;
        for address in addresses {
            let result = refresh_address(pg, l1, chain_id, &address).await;
            if let Err(e) = &result {
                metrics
                    .resolve_failures_total
                    .with_label_values(&[address.entity_label()])
                    .inc();
                warn!(
                    address = %address.address,
                    entity = address.entity_label(),
                    error = %e,
                    "ens watcher refresh failed"
                );
            } else {
                metrics
                    .rows_updated_total
                    .with_label_values(&[address.entity_label()])
                    .inc();
                let projection = result.as_ref().unwrap();
                if projection.ens_name.is_some() {
                    metrics
                        .rows_named_total
                        .with_label_values(&[address.entity_label(), "name"])
                        .inc();
                }
                if projection.ens_avatar_url.is_some() {
                    metrics
                        .rows_named_total
                        .with_label_values(&[address.entity_label(), "avatar"])
                        .inc();
                }
            }
            breaker.on_result(&result).await;
        }
        advance_watch_checkpoint(pg, chunk_end).await?;
        chunk_start = chunk_end + 1;
    }

    if summary.logs_seen > 0 || summary.addresses_refreshed > 0 {
        info!(
            latest_l1_block = summary.latest_l1_block,
            logs_seen = summary.logs_seen,
            addresses_refreshed = summary.addresses_refreshed,
            "ens watcher iteration complete"
        );
    }
    Ok(summary)
}

pub async fn run_once(
    pg: &PgPool,
    l1: &Provider,
    chain_id: i64,
    batch_limit: i64,
    metrics: &Arc<Metrics>,
) -> Result<SweepSummary> {
    let mut summary = SweepSummary::default();
    let mut breaker = FailureBreaker {
        consecutive_failures: 0,
        metrics: metrics.clone(),
    };

    let orchestrators = load_pending_orchestrator_addresses(pg, chain_id, batch_limit).await?;
    summary.orchestrators_seen = orchestrators.len() as u64;
    for address in orchestrators {
        let result = resolve_and_upsert_orchestrator(pg, l1, chain_id, &address).await;
        if let Err(e) = &result {
            summary.failures += 1;
            metrics
                .resolve_failures_total
                .with_label_values(&["orchestrator"])
                .inc();
            warn!(address = %address, error = %e, "orchestrator ENS resolve failed");
        } else {
            let projection = result.as_ref().unwrap();
            summary.orchestrators_updated += 1;
            metrics
                .rows_updated_total
                .with_label_values(&["orchestrator"])
                .inc();
            if projection.ens_name.is_some() {
                summary.named_rows += 1;
                metrics
                    .rows_named_total
                    .with_label_values(&["orchestrator", "name"])
                    .inc();
            }
            if projection.ens_avatar_url.is_some() {
                summary.avatar_rows += 1;
                metrics
                    .rows_named_total
                    .with_label_values(&["orchestrator", "avatar"])
                    .inc();
            }
        }
        breaker.on_result(&result).await;
    }

    let gateways = load_pending_broadcaster_addresses(pg, chain_id, batch_limit).await?;
    summary.gateways_seen = gateways.len() as u64;
    for address in gateways {
        let result = resolve_and_upsert_broadcaster(pg, l1, chain_id, &address).await;
        if let Err(e) = &result {
            summary.failures += 1;
            metrics
                .resolve_failures_total
                .with_label_values(&["broadcaster"])
                .inc();
            warn!(address = %address, error = %e, "gateway ENS resolve failed");
        } else {
            let projection = result.as_ref().unwrap();
            summary.gateways_updated += 1;
            metrics
                .rows_updated_total
                .with_label_values(&["broadcaster"])
                .inc();
            if projection.ens_name.is_some() {
                summary.named_rows += 1;
                metrics
                    .rows_named_total
                    .with_label_values(&["broadcaster", "name"])
                    .inc();
            }
            if projection.ens_avatar_url.is_some() {
                summary.avatar_rows += 1;
                metrics
                    .rows_named_total
                    .with_label_values(&["broadcaster", "avatar"])
                    .inc();
            }
        }
        breaker.on_result(&result).await;
    }

    metrics.sweeps_total.with_label_values(&["ok"]).inc();
    info!(
        orchestrators_seen = summary.orchestrators_seen,
        gateways_seen = summary.gateways_seen,
        "ens sweep completed"
    );
    Ok(summary)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum EnsEntity {
    Orchestrator,
    Broadcaster,
}

impl EnsEntity {
    fn label(self) -> &'static str {
        match self {
            EnsEntity::Orchestrator => "orchestrator",
            EnsEntity::Broadcaster => "broadcaster",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TrackedAddress {
    entity: EnsEntity,
    address: String,
}

impl TrackedAddress {
    fn entity_label(&self) -> &'static str {
        self.entity.label()
    }
}

#[derive(Debug, Clone)]
struct EnsLog {
    topic0: String,
    topics: Vec<String>,
    data: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEnsLog {
    topics: Vec<String>,
    data: String,
}

/// Re-resolution TTL in days. Rows older than this are swept and re-resolved.
/// Configurable via `ENRICHER_TTL_DAYS`; falls back to [`DEFAULT_TTL_DAYS`] when
/// unset or not a positive integer. Read once and cached for the process.
fn ttl_days() -> i64 {
    static TTL_DAYS: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *TTL_DAYS.get_or_init(|| {
        std::env::var("ENRICHER_TTL_DAYS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|&d| d > 0)
            .unwrap_or(DEFAULT_TTL_DAYS)
    })
}

async fn load_pending_orchestrator_addresses(
    pg: &PgPool,
    chain_id: i64,
    limit: i64,
) -> Result<Vec<String>> {
    let should_fill_avatar_cache = crate::avatar::store_dir().is_some();
    let rows = sqlx::query(
        r#"SELECT p.address
             FROM orchestrator_profile p
        LEFT JOIN orchestrator_ens e
               ON e.chain_id = p.chain_id
              AND e.address = p.address
            WHERE p.chain_id = $1
              AND (
                    e.address IS NULL
                 OR e.ens_last_resolved_at IS NULL
                 OR e.ens_last_resolved_at < now() - ($2 * interval '1 day')
                 OR (
                       $4
                   AND e.ens_avatar_url IS NOT NULL
                   AND e.ens_avatar_stored_ext IS NULL
                 )
              )
         ORDER BY COALESCE(e.ens_last_resolved_at, to_timestamp(0)) ASC, p.address ASC
            LIMIT $3"#,
    )
    .bind(chain_id)
    .bind(ttl_days())
    .bind(limit)
    .bind(should_fill_avatar_cache)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(|r| r.get("address")).collect())
}

async fn load_pending_broadcaster_addresses(
    pg: &PgPool,
    chain_id: i64,
    limit: i64,
) -> Result<Vec<String>> {
    let rows = sqlx::query(
        r#"SELECT p.address
             FROM broadcaster_profile p
        LEFT JOIN broadcaster_ens e
               ON e.chain_id = p.chain_id
              AND e.address = p.address
            WHERE p.chain_id = $1
              AND (
                    e.address IS NULL
                 OR e.ens_last_resolved_at IS NULL
                 OR e.ens_last_resolved_at < now() - ($2 * interval '1 day')
              )
         ORDER BY COALESCE(e.ens_last_resolved_at, to_timestamp(0)) ASC, p.address ASC
            LIMIT $3"#,
    )
    .bind(chain_id)
    .bind(ttl_days())
    .bind(limit)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(|r| r.get("address")).collect())
}

async fn resolve_and_upsert_orchestrator(
    pg: &PgPool,
    l1: &Provider,
    chain_id: i64,
    address: &str,
) -> Result<EnsProjection> {
    let projection = resolve_ens_projection(l1, address).await?;
    sqlx::query(
        r#"INSERT INTO orchestrator_ens (
               chain_id, address, ens_name, ens_avatar_url, ens_last_resolved_at
           ) VALUES ($1, $2, $3, $4, now())
           ON CONFLICT (chain_id, address) DO UPDATE
               SET ens_name = EXCLUDED.ens_name,
                   ens_avatar_url = EXCLUDED.ens_avatar_url,
                   ens_last_resolved_at = EXCLUDED.ens_last_resolved_at"#,
    )
    .bind(chain_id)
    .bind(address)
    .bind(&projection.ens_name)
    .bind(&projection.ens_avatar_url)
    .execute(pg)
    .await?;
    cache_orchestrator_avatar(pg, l1, chain_id, address, &projection).await;
    Ok(projection)
}

/// TD-033: resolve the raw ENS avatar record to local image bytes and
/// record the stored extension. Best-effort and side-effecting: never
/// fails the sweep. When resolution yields nothing we leave any
/// previously-cached file and `ens_avatar_stored_ext` untouched (the main
/// upsert above does not write that column), so a transient fetch failure
/// can't blank a working avatar. Disabled entirely when `AVATAR_STORE_DIR`
/// is unset.
async fn cache_orchestrator_avatar(
    pg: &PgPool,
    l1: &Provider,
    chain_id: i64,
    address: &str,
    projection: &EnsProjection,
) {
    let Some(dir) = crate::avatar::store_dir() else {
        return;
    };
    match &projection.ens_avatar_url {
        Some(raw) => {
            match crate::avatar::resolve_and_store(l1, dir, address, raw).await {
                Ok(Some(ext)) => {
                    if let Err(e) =
                        set_orchestrator_avatar_ext(pg, chain_id, address, Some(&ext)).await
                    {
                        warn!(address = %address, error = %e, "failed to record cached avatar extension");
                    }
                }
                // Ok(None): resolution failed/unsupported — keep prior cache.
                Ok(None) => {}
                Err(e) => warn!(address = %address, error = %e, "avatar caching errored"),
            }
        }
        None => {
            // No avatar record anymore: drop any cached file + marker.
            if let Err(e) = crate::avatar::clear(dir, address).await {
                warn!(address = %address, error = %e, "failed to clear cached avatar");
            }
            if let Err(e) = set_orchestrator_avatar_ext(pg, chain_id, address, None).await {
                warn!(address = %address, error = %e, "failed to clear cached avatar extension");
            }
        }
    }
}

async fn set_orchestrator_avatar_ext(
    pg: &PgPool,
    chain_id: i64,
    address: &str,
    ext: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"UPDATE orchestrator_ens
              SET ens_avatar_stored_ext = $3
            WHERE chain_id = $1
              AND address = $2"#,
    )
    .bind(chain_id)
    .bind(address)
    .bind(ext)
    .execute(pg)
    .await?;
    Ok(())
}

async fn resolve_and_upsert_broadcaster(
    pg: &PgPool,
    l1: &Provider,
    chain_id: i64,
    address: &str,
) -> Result<EnsProjection> {
    let projection = resolve_ens_projection(l1, address).await?;
    sqlx::query(
        r#"INSERT INTO broadcaster_ens (
               chain_id, address, ens_name, ens_avatar_url, ens_last_resolved_at
           ) VALUES ($1, $2, $3, $4, now())
           ON CONFLICT (chain_id, address) DO UPDATE
               SET ens_name = EXCLUDED.ens_name,
                   ens_avatar_url = EXCLUDED.ens_avatar_url,
                   ens_last_resolved_at = EXCLUDED.ens_last_resolved_at"#,
    )
    .bind(chain_id)
    .bind(address)
    .bind(&projection.ens_name)
    .bind(&projection.ens_avatar_url)
    .execute(pg)
    .await?;
    Ok(projection)
}

async fn refresh_address(
    pg: &PgPool,
    l1: &Provider,
    chain_id: i64,
    address: &TrackedAddress,
) -> Result<EnsProjection> {
    match address.entity {
        EnsEntity::Orchestrator => {
            resolve_and_upsert_orchestrator(pg, l1, chain_id, &address.address).await
        }
        EnsEntity::Broadcaster => {
            resolve_and_upsert_broadcaster(pg, l1, chain_id, &address.address).await
        }
    }
}

async fn fetch_ens_change_logs(
    l1: &Provider,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<EnsLog>> {
    let params = serde_json::json!([{
        "topics": [[
            NAME_CHANGED_TOPIC0,
            TEXT_CHANGED_V1_TOPIC0,
            TEXT_CHANGED_V2_TOPIC0
        ]],
        "fromBlock": format!("0x{:x}", from_block),
        "toBlock": format!("0x{:x}", to_block),
    }]);
    let value = l1.call("eth_getLogs", &params).await?;
    let raw_logs: Vec<RawEnsLog> =
        serde_json::from_value(value).context("decoding ENS watcher logs")?;
    Ok(raw_logs
        .into_iter()
        .filter_map(|raw| {
            let topic0 = raw.topics.first()?.to_lowercase();
            Some(EnsLog {
                topic0,
                topics: raw.topics.into_iter().map(|t| t.to_lowercase()).collect(),
                data: raw.data,
            })
        })
        .collect())
}

async fn find_affected_addresses(
    pg: &PgPool,
    chain_id: i64,
    logs: &[EnsLog],
) -> Result<Vec<TrackedAddress>> {
    if logs.is_empty() {
        return Ok(Vec::new());
    }

    let reverse_targets = load_tracked_reverse_nodes(pg, chain_id).await?;
    let forward_targets = load_tracked_forward_nodes(pg, chain_id).await?;
    let mut affected = HashSet::new();

    for log in logs {
        match log.topic0.as_str() {
            NAME_CHANGED_TOPIC0 => {
                if let Some(node) = log.topics.get(1) {
                    if let Some(matches) = reverse_targets
                        .iter()
                        .find(|(candidate, _)| candidate == node)
                        .map(|(_, addresses)| addresses)
                    {
                        affected.extend(matches.iter().cloned());
                    }
                }
            }
            TEXT_CHANGED_V1_TOPIC0 => {
                if text_changed_v1_is_avatar(log)? {
                    if let Some(node) = log.topics.get(1) {
                        if let Some(matches) = forward_targets
                            .iter()
                            .find(|(candidate, _)| candidate == node)
                            .map(|(_, addresses)| addresses)
                        {
                            affected.extend(matches.iter().cloned());
                        }
                    }
                }
            }
            TEXT_CHANGED_V2_TOPIC0 => {
                if text_changed_v2_is_avatar(log)? {
                    if let Some(node) = log.topics.get(1) {
                        if let Some(matches) = forward_targets
                            .iter()
                            .find(|(candidate, _)| candidate == node)
                            .map(|(_, addresses)| addresses)
                        {
                            affected.extend(matches.iter().cloned());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let mut affected = affected.into_iter().collect::<Vec<_>>();
    affected.sort_by(|a, b| {
        a.address
            .cmp(&b.address)
            .then_with(|| a.entity_label().cmp(b.entity_label()))
    });
    Ok(affected)
}

async fn load_tracked_reverse_nodes(
    pg: &PgPool,
    chain_id: i64,
) -> Result<Vec<(String, Vec<TrackedAddress>)>> {
    let rows = sqlx::query(
        r#"SELECT 'orchestrator' AS entity, address
             FROM orchestrator_profile
            WHERE chain_id = $1
            UNION ALL
           SELECT 'broadcaster' AS entity, address
             FROM broadcaster_profile
            WHERE chain_id = $1"#,
    )
    .bind(chain_id)
    .fetch_all(pg)
    .await?;

    let mut out: Vec<(String, Vec<TrackedAddress>)> = Vec::new();
    for row in rows {
        let entity = match row.get::<String, _>("entity").as_str() {
            "orchestrator" => EnsEntity::Orchestrator,
            _ => EnsEntity::Broadcaster,
        };
        let address = row.get::<String, _>("address");
        let node = reverse_node_for_address(&address)?
            .to_string()
            .to_lowercase();
        if let Some((_, addresses)) = out.iter_mut().find(|(candidate, _)| candidate == &node) {
            addresses.push(TrackedAddress { entity, address });
        } else {
            out.push((node, vec![TrackedAddress { entity, address }]));
        }
    }
    Ok(out)
}

async fn load_tracked_forward_nodes(
    pg: &PgPool,
    chain_id: i64,
) -> Result<Vec<(String, Vec<TrackedAddress>)>> {
    let rows = sqlx::query(
        r#"SELECT 'orchestrator' AS entity, address, ens_name
             FROM orchestrator_ens
            WHERE chain_id = $1
              AND ens_name IS NOT NULL
            UNION ALL
           SELECT 'broadcaster' AS entity, address, ens_name
             FROM broadcaster_ens
            WHERE chain_id = $1
              AND ens_name IS NOT NULL"#,
    )
    .bind(chain_id)
    .fetch_all(pg)
    .await?;

    let mut out: Vec<(String, Vec<TrackedAddress>)> = Vec::new();
    for row in rows {
        let entity = match row.get::<String, _>("entity").as_str() {
            "orchestrator" => EnsEntity::Orchestrator,
            _ => EnsEntity::Broadcaster,
        };
        let address = row.get::<String, _>("address");
        let ens_name = row.get::<String, _>("ens_name");
        let node = namehash(&ens_name).to_string().to_lowercase();
        if let Some((_, addresses)) = out.iter_mut().find(|(candidate, _)| candidate == &node) {
            addresses.push(TrackedAddress { entity, address });
        } else {
            out.push((node, vec![TrackedAddress { entity, address }]));
        }
    }
    Ok(out)
}

fn text_changed_v1_is_avatar(log: &EnsLog) -> Result<bool> {
    text_changed_mentions_avatar(log)
}

fn text_changed_v2_is_avatar(log: &EnsLog) -> Result<bool> {
    text_changed_mentions_avatar(log)
}

fn text_changed_mentions_avatar(log: &EnsLog) -> Result<bool> {
    let avatar_hash = format!("{:#x}", keccak256(AVATAR_KEY.as_bytes())).to_lowercase();
    if log.topics.get(2).is_some_and(|topic| topic == &avatar_hash) {
        return Ok(true);
    }

    let bytes = decode_data_hex(&log.data)?;
    if let Ok((key,)) = <(alloy::sol_types::sol_data::String,)>::abi_decode_sequence(&bytes, true) {
        return Ok(key == AVATAR_KEY);
    }
    if let Ok((key, _value)) = <(
        alloy::sol_types::sol_data::String,
        alloy::sol_types::sol_data::String,
    )>::abi_decode_sequence(&bytes, true)
    {
        return Ok(key == AVATAR_KEY);
    }
    Ok(false)
}

async fn resolve_ens_projection(l1: &Provider, address: &str) -> Result<EnsProjection> {
    let normalized = normalize_addr(address)?;
    let reverse_name = format!("{}.addr.reverse", normalized.trim_start_matches("0x"));
    let reverse_node = namehash(&reverse_name);
    let Some(reverse_resolver) = registry_resolver(l1, reverse_node).await? else {
        return Ok(EnsProjection {
            ens_name: None,
            ens_avatar_url: None,
        });
    };
    let Some(ens_name) = resolver_name(l1, &reverse_resolver, reverse_node).await? else {
        return Ok(EnsProjection {
            ens_name: None,
            ens_avatar_url: None,
        });
    };

    let forward_node = namehash(&ens_name);
    let Some(forward_resolver) = registry_resolver(l1, forward_node).await? else {
        return Ok(EnsProjection {
            ens_name: None,
            ens_avatar_url: None,
        });
    };
    let resolved_address = resolver_addr(l1, &forward_resolver, forward_node).await?;
    if resolved_address.as_deref() != Some(normalized.as_str()) {
        return Ok(EnsProjection {
            ens_name: None,
            ens_avatar_url: None,
        });
    }

    let avatar = resolver_text(l1, &forward_resolver, forward_node, "avatar").await?;
    Ok(EnsProjection {
        ens_name: Some(ens_name),
        ens_avatar_url: avatar,
    })
}

async fn registry_resolver(l1: &Provider, node: B256) -> Result<Option<String>> {
    let data = format!(
        "0x{}",
        alloy::hex::encode(ENSRegistry::resolverCall { node }.abi_encode())
    );
    let raw = decode_eth_call_result(l1.eth_call(ENS_REGISTRY, &data, BlockTag::Latest).await?)?;
    let resolver = ENSRegistry::resolverCall::abi_decode_returns(&raw, true)?._0;
    let resolver = format!("{:#x}", resolver).to_lowercase();
    if resolver == ZERO_ADDRESS {
        Ok(None)
    } else {
        Ok(Some(resolver))
    }
}

async fn resolver_name(l1: &Provider, resolver: &str, node: B256) -> Result<Option<String>> {
    let data = format!(
        "0x{}",
        alloy::hex::encode(ENSResolver::nameCall { node }.abi_encode())
    );
    let raw = decode_eth_call_result(l1.eth_call(resolver, &data, BlockTag::Latest).await?)?;
    let name = ENSResolver::nameCall::abi_decode_returns(&raw, true)?._0;
    let name = name.trim().to_string();
    if name.is_empty() {
        Ok(None)
    } else {
        Ok(Some(name))
    }
}

async fn resolver_addr(l1: &Provider, resolver: &str, node: B256) -> Result<Option<String>> {
    let data = format!(
        "0x{}",
        alloy::hex::encode(ENSResolver::addrCall { node }.abi_encode())
    );
    let raw = decode_eth_call_result(l1.eth_call(resolver, &data, BlockTag::Latest).await?)?;
    let addr = ENSResolver::addrCall::abi_decode_returns(&raw, true)?._0;
    let addr = format!("{:#x}", addr).to_lowercase();
    if addr == ZERO_ADDRESS {
        Ok(None)
    } else {
        Ok(Some(addr))
    }
}

async fn resolver_text(
    l1: &Provider,
    resolver: &str,
    node: B256,
    key: &str,
) -> Result<Option<String>> {
    let data = format!(
        "0x{}",
        alloy::hex::encode(
            ENSResolver::textCall {
                node,
                key: key.to_string(),
            }
            .abi_encode()
        )
    );
    let raw = decode_eth_call_result(l1.eth_call(resolver, &data, BlockTag::Latest).await?)?;
    let value = ENSResolver::textCall::abi_decode_returns(&raw, true)?._0;
    let value = value.trim().to_string();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn decode_eth_call_result(value: serde_json::Value) -> Result<Vec<u8>> {
    let s = value
        .as_str()
        .ok_or_else(|| anyhow!("eth_call response was not a hex string"))?;
    alloy::hex::decode(s.trim_start_matches("0x")).context("decoding eth_call return hex")
}

fn decode_data_hex(data: &str) -> Result<Vec<u8>> {
    alloy::hex::decode(data.trim_start_matches("0x")).context("decoding log data hex")
}

fn normalize_addr(s: &str) -> Result<String> {
    let lower = s.to_lowercase();
    if lower.starts_with("0x") && lower.len() == 42 {
        Ok(lower)
    } else {
        Err(anyhow!("invalid address: {s}"))
    }
}

fn namehash(name: &str) -> B256 {
    if name.is_empty() {
        return B256::ZERO;
    }
    let mut node = [0u8; 32];
    for label in name.rsplit('.') {
        let label_hash = keccak256(label.as_bytes());
        let mut combined = [0u8; 64];
        combined[..32].copy_from_slice(&node);
        combined[32..].copy_from_slice(label_hash.as_slice());
        node = keccak256(combined).0;
    }
    B256::from(node)
}

fn reverse_node_for_address(address: &str) -> Result<B256> {
    let normalized = normalize_addr(address)?;
    Ok(namehash(&format!(
        "{}.addr.reverse",
        normalized.trim_start_matches("0x")
    )))
}

async fn load_watch_checkpoint(pg: &PgPool) -> Result<Option<u64>> {
    let block = sqlx::query_scalar::<_, i64>(
        "SELECT last_processed_block FROM indexer_checkpoints WHERE name = $1",
    )
    .bind(L1_WATCH_CHECKPOINT)
    .fetch_optional(pg)
    .await?;
    Ok(block.map(|value| value as u64))
}

async fn advance_watch_checkpoint(pg: &PgPool, block: u64) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO indexer_checkpoints (name, chain_id, last_processed_block, updated_at)
           VALUES ($1, $2, $3, now())
           ON CONFLICT (name) DO UPDATE
              SET last_processed_block = GREATEST(indexer_checkpoints.last_processed_block, EXCLUDED.last_processed_block),
                  updated_at = now()"#,
    )
    .bind(L1_WATCH_CHECKPOINT)
    .bind(1_i64)
    .bind(block as i64)
    .execute(pg)
    .await?;
    Ok(())
}

#[allow(dead_code)]
fn parse_address(s: &str) -> Result<Address> {
    Address::from_str(s).context("parsing EVM address")
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolValue;

    #[test]
    fn reverse_node_normalizes_address_casing() {
        let lower = reverse_node_for_address("0x07cf42f48b6c668bdac50fee95ac0bbfe88ac6e1").unwrap();
        let upper = reverse_node_for_address("0x07CF42F48B6C668BDAC50FEE95AC0BBFE88AC6E1").unwrap();
        assert_eq!(lower, upper);
    }

    #[test]
    fn avatar_filters_decode_both_textchanged_variants() {
        let v1 = EnsLog {
            topic0: TEXT_CHANGED_V1_TOPIC0.to_string(),
            topics: vec![
                TEXT_CHANGED_V1_TOPIC0.to_string(),
                format!("{:#x}", B256::ZERO),
            ],
            data: format!(
                "0x{}",
                alloy::hex::encode((String::from(AVATAR_KEY),).abi_encode_sequence())
            ),
        };
        assert!(text_changed_v1_is_avatar(&v1).unwrap());

        let v2 = EnsLog {
            topic0: TEXT_CHANGED_V2_TOPIC0.to_string(),
            topics: vec![
                TEXT_CHANGED_V2_TOPIC0.to_string(),
                format!("{:#x}", B256::ZERO),
                format!("{:#x}", keccak256(AVATAR_KEY.as_bytes())),
            ],
            data: "0x".to_string(),
        };
        assert!(text_changed_v2_is_avatar(&v2).unwrap());
    }
}
