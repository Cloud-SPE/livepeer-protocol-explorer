//! `container_state` — `docker ps`-equivalent through the read-only proxy.
//! The key signal: a standalone worker (rollups, enricher, tx-receipts) that
//! has silently crashed appears here with `running:false` — something no
//! metric or checkpoint surfaces directly.

use crate::adapters::docker::ContainerInfo;
use crate::context::DiagContext;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ContainerState {
    pub docker_endpoint: String,
    pub total: usize,
    pub running: usize,
    /// Names of containers that exist but are not running.
    pub not_running: Vec<String>,
    pub containers: Vec<ContainerInfo>,
}

pub async fn run(ctx: &DiagContext) -> anyhow::Result<ContainerState> {
    let containers = ctx.docker.list_containers().await?;
    let running = containers.iter().filter(|c| c.running).count();
    let not_running: Vec<String> = containers
        .iter()
        .filter(|c| !c.running)
        .map(|c| c.name.clone())
        .collect();
    Ok(ContainerState {
        docker_endpoint: ctx.docker.base().to_string(),
        total: containers.len(),
        running,
        not_running,
        containers,
    })
}
