//! Prometheus `/metrics` scrape adapter.
//!
//! Best-effort *enrichment* only — never a source of truth. Only three prod
//! containers expose a `/metrics` server (daemon `:9107`, enricher `:9112`,
//! api `:8080`); the rollup + tx-receipts workers expose none, so their health
//! is derived from the DB. Each endpoint is scraped concurrently with an
//! independent short timeout: one dead target (often the very symptom being
//! debugged) must never fail the whole tool.

use serde::Serialize;
use std::collections::BTreeMap;
use std::time::Duration;

/// One parsed metric sample.
#[derive(Debug, Clone, Serialize)]
pub struct Sample {
    pub name: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    pub value: f64,
}

/// Result of scraping a single endpoint. Always returned — an unreachable
/// endpoint yields `reachable: false` with the error, not an aborted tool.
#[derive(Debug, Clone, Serialize)]
pub struct EndpointScrape {
    pub url: String,
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub sample_count: usize,
    /// Samples matching the requested name filter (or a curated default set),
    /// capped by the caller for token budget.
    pub samples: Vec<Sample>,
}

#[derive(Clone)]
pub struct MetricsClient {
    http: reqwest::Client,
    endpoints: Vec<String>,
}

impl MetricsClient {
    pub fn new(endpoints: Vec<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_default();
        Self { http, endpoints }
    }

    /// Scrape one endpoint; returns matching samples (all if `name_filter` is
    /// None, else names containing the filter substring), capped at `cap`.
    pub async fn scrape_one(
        &self,
        url: &str,
        name_filter: Option<&str>,
        cap: usize,
    ) -> EndpointScrape {
        match self.fetch_text(url).await {
            Ok(text) => {
                let all = parse_prometheus(&text);
                let sample_count = all.len();
                let samples: Vec<Sample> = all
                    .into_iter()
                    .filter(|s| match name_filter {
                        Some(f) => s.name.contains(f),
                        None => true,
                    })
                    .take(cap)
                    .collect();
                EndpointScrape {
                    url: url.to_string(),
                    reachable: true,
                    error: None,
                    sample_count,
                    samples,
                }
            }
            Err(e) => EndpointScrape {
                url: url.to_string(),
                reachable: false,
                error: Some(e),
                sample_count: 0,
                samples: Vec::new(),
            },
        }
    }

    /// Scrape all configured endpoints concurrently.
    pub async fn scrape_all(&self, name_filter: Option<&str>, cap: usize) -> Vec<EndpointScrape> {
        let futs = self
            .endpoints
            .iter()
            .map(|url| self.scrape_one(url, name_filter, cap));
        futures::future::join_all(futs).await
    }

    /// Chain head block as reported by any reachable endpoint's
    /// `livepeer_chain_head_block` gauge (the daemon publishes it, with the
    /// `livepeer_` prefix). None if unavailable.
    pub async fn chain_head(&self) -> Option<i64> {
        const METRIC: &str = "livepeer_chain_head_block";
        for scrape in self.scrape_all(Some(METRIC), 8).await {
            if let Some(s) = scrape.samples.iter().find(|s| s.name == METRIC) {
                return Some(s.value as i64);
            }
        }
        None
    }

    async fn fetch_text(&self, url: &str) -> Result<String, String> {
        let resp = self.http.get(url).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        resp.text().await.map_err(|e| e.to_string())
    }
}

/// Minimal Prometheus text-exposition parser: `name{labels} value [ts]`.
/// Skips `#` comment/HELP/TYPE lines. Good enough for gauges/counters we read;
/// not a full spec implementation.
pub fn parse_prometheus(text: &str) -> Vec<Sample> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Split metric identifier from the value (+ optional timestamp).
        let (ident, rest) = match line.find(['{', ' ']) {
            Some(_) => split_ident_value(line),
            None => continue,
        };
        let Some(value) = rest else { continue };
        let (name, labels) = parse_ident(&ident);
        if name.is_empty() {
            continue;
        }
        out.push(Sample {
            name,
            labels,
            value,
        });
    }
    out
}

/// Split a full sample line into (identifier-with-labels, value). The value is
/// the first whitespace-separated token after the identifier.
fn split_ident_value(line: &str) -> (String, Option<f64>) {
    // The identifier ends at the first space that is NOT inside `{...}`.
    let mut depth = 0usize;
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b' ' if depth == 0 => {
                let ident = line[..i].to_string();
                let value = line[i..]
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse::<f64>().ok());
                return (ident, value);
            }
            _ => {}
        }
    }
    (line.to_string(), None)
}

/// Parse `name{k="v",k2="v2"}` into (name, labels).
fn parse_ident(ident: &str) -> (String, BTreeMap<String, String>) {
    let mut labels = BTreeMap::new();
    let Some(brace) = ident.find('{') else {
        return (ident.trim().to_string(), labels);
    };
    let name = ident[..brace].trim().to_string();
    let inner = ident[brace + 1..].trim_end_matches('}');
    for pair in split_top_level_commas(inner) {
        if let Some((k, v)) = pair.split_once('=') {
            let v = v.trim().trim_matches('"');
            labels.insert(k.trim().to_string(), v.to_string());
        }
    }
    (name, labels)
}

fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                cur.push(c);
            }
            ',' if !in_quotes => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse_prometheus;

    #[test]
    fn parses_labeled_and_bare() {
        let text = "# HELP x\n# TYPE x gauge\nchain_head_block 12345\ntask_lag_blocks{task=\"indexer\"} 42\n";
        let s = parse_prometheus(text);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "chain_head_block");
        assert_eq!(s[0].value, 12345.0);
        assert_eq!(s[1].name, "task_lag_blocks");
        assert_eq!(s[1].labels.get("task").unwrap(), "indexer");
        assert_eq!(s[1].value, 42.0);
    }
}
