//! Docker adapter — reads container state + logs through the read-only
//! `docker-socket-proxy` (GET-only) via its Engine HTTP API. We use reqwest
//! directly rather than a Docker client crate to avoid a heavy dependency (and
//! a tokio version conflict), and because we only need two GET endpoints:
//! `/containers/json` and `/containers/{id}/logs`.
//!
//! The proxy refuses POST/exec, so this adapter is structurally incapable of
//! mutating anything regardless of what it sends.

use crate::output;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone)]
pub struct DockerClient {
    http: reqwest::Client,
    /// Base HTTP URL of the Engine API, e.g. `http://docker-proxy:2375`.
    base: String,
}

/// Compact container summary from `GET /containers/json`.
#[derive(Debug, Clone, Serialize)]
pub struct ContainerInfo {
    pub name: String,
    pub image: String,
    /// e.g. "running", "exited", "restarting".
    pub state: String,
    /// e.g. "Up 3 hours (healthy)" / "Exited (1) 2 minutes ago".
    pub status: String,
    pub running: bool,
}

/// Raw shape of a `/containers/json` element (subset we read).
#[derive(Debug, Deserialize)]
struct RawContainer {
    #[serde(rename = "Names")]
    names: Vec<String>,
    #[serde(rename = "Image")]
    image: String,
    #[serde(rename = "State")]
    state: String,
    #[serde(rename = "Status")]
    status: String,
}

impl DockerClient {
    pub fn new(docker_host: &str) -> Result<Self> {
        let base = normalize_host(docker_host)?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("building docker http client")?;
        Ok(Self { http, base })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// List all containers (running + stopped), so a silently-crashed worker
    /// still appears with `running:false`.
    pub async fn list_containers(&self) -> Result<Vec<ContainerInfo>> {
        let url = format!("{}/containers/json?all=1", self.base);
        let raw: Vec<RawContainer> = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .context("docker /containers/json returned an error status")?
            .json()
            .await
            .context("decoding /containers/json")?;

        Ok(raw
            .into_iter()
            .map(|c| ContainerInfo {
                name: c
                    .names
                    .first()
                    .map(|n| n.trim_start_matches('/').to_string())
                    .unwrap_or_default(),
                image: c.image,
                running: c.state.eq_ignore_ascii_case("running"),
                state: c.state,
                status: c.status,
            })
            .collect())
    }

    /// Fetch and de-frame recent logs for a container (by name or id). Returns
    /// the most recent `lines` lines (clamped) and whether the tail was
    /// truncated for the byte/line budget.
    pub async fn container_logs(
        &self,
        container: &str,
        lines: usize,
        since_secs: Option<i64>,
    ) -> Result<(Vec<String>, bool)> {
        let want = lines.clamp(1, output::MAX_LOG_LINES);
        let mut url = format!(
            "{}/containers/{}/logs?stdout=1&stderr=1&tail={}",
            self.base, container, want
        );
        if let Some(secs) = since_secs {
            let since = chrono::Utc::now().timestamp() - secs;
            url.push_str(&format!("&since={since}"));
        }

        let bytes = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET logs for {container}"))?
            .error_for_status()
            .with_context(|| format!("docker logs for {container} returned an error status"))?
            .bytes()
            .await
            .context("reading log bytes")?;

        let text = deframe_logs(&bytes);
        let all_lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        Ok(output::cap_log_lines(all_lines, want))
    }
}

/// Convert a `DOCKER_HOST` value into an HTTP base URL. Supports `tcp://` and
/// `http(s)://`; unix sockets are unsupported (we deliberately go through the
/// tcp socket proxy).
fn normalize_host(docker_host: &str) -> Result<String> {
    let h = docker_host.trim();
    if let Some(rest) = h.strip_prefix("tcp://") {
        Ok(format!("http://{}", rest.trim_end_matches('/')))
    } else if h.starts_with("http://") || h.starts_with("https://") {
        Ok(h.trim_end_matches('/').to_string())
    } else if h.starts_with("unix://") {
        anyhow::bail!(
            "unix-socket DOCKER_HOST is unsupported; point DOCKER_HOST at the tcp socket proxy \
             (e.g. tcp://docker-proxy:2375)"
        )
    } else {
        // Bare host:port — assume http.
        Ok(format!("http://{}", h.trim_end_matches('/')))
    }
}

/// De-multiplex a Docker log stream. Non-TTY containers frame each chunk with
/// an 8-byte header: `[stream(1)][0][0][0][size: u32 BE]` + payload. TTY
/// containers emit raw bytes. We parse frames and fall back to raw UTF-8 if the
/// framing doesn't validate.
fn deframe_logs(buf: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0usize;
    let mut valid = true;
    let mut any = false;

    while i + 8 <= buf.len() {
        let stream = buf[i];
        if stream > 2 || buf[i + 1] != 0 || buf[i + 2] != 0 || buf[i + 3] != 0 {
            valid = false;
            break;
        }
        let size = u32::from_be_bytes([buf[i + 4], buf[i + 5], buf[i + 6], buf[i + 7]]) as usize;
        let start = i + 8;
        let end = start.saturating_add(size);
        if end > buf.len() {
            // Partial trailing frame — take what we have and stop.
            out.push_str(&String::from_utf8_lossy(&buf[start..]));
            any = true;
            break;
        }
        out.push_str(&String::from_utf8_lossy(&buf[start..end]));
        any = true;
        i = end;
    }

    if valid && any {
        out
    } else {
        String::from_utf8_lossy(buf).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::deframe_logs;

    #[test]
    fn deframes_multiplexed() {
        // One stdout frame containing "hi\n".
        let mut buf = vec![1u8, 0, 0, 0, 0, 0, 0, 3];
        buf.extend_from_slice(b"hi\n");
        assert_eq!(deframe_logs(&buf), "hi\n");
    }

    #[test]
    fn falls_back_to_raw() {
        assert_eq!(deframe_logs(b"plain tty line\n"), "plain tty line\n");
    }
}
