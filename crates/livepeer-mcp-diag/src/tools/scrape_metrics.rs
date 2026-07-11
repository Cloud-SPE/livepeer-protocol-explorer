//! `scrape_metrics` — raw escape hatch over the three real `/metrics`
//! endpoints (daemon/enricher/api). Each target is scraped concurrently with
//! an independent timeout; a dead target yields `reachable:false`, never an
//! aborted tool.

use crate::adapters::metrics::EndpointScrape;
use crate::context::DiagContext;
use serde::Serialize;

/// Cap on samples returned per endpoint (token budget).
const SAMPLE_CAP: usize = 200;

#[derive(Debug, Serialize)]
pub struct ScrapeResult {
    pub name_filter: Option<String>,
    pub endpoints: Vec<EndpointScrape>,
}

pub async fn run(ctx: &DiagContext, name_filter: Option<String>) -> anyhow::Result<ScrapeResult> {
    let endpoints = ctx
        .metrics
        .scrape_all(name_filter.as_deref(), SAMPLE_CAP)
        .await;
    Ok(ScrapeResult {
        name_filter,
        endpoints,
    })
}
