use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use livepeer_core::{config::Config, tracing_init};
use serde::Deserialize;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tracing::{error, info, warn};

const SERVICE: &str = "livepeer-alert-bot";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Alertmanager -> Telegram bridge for Livepeer ops alerts.")]
struct Cli {
    #[arg(long, env = "STATIC_CONFIG", default_value = "config/arbitrum.yaml")]
    static_config: PathBuf,

    #[arg(long, env = "ENV_CONFIG", default_value = "config/env/dev.yaml")]
    env_config: PathBuf,

    #[arg(long, env = "ALERT_BOT_BIND", default_value = "0.0.0.0:9111")]
    bind: String,
}

#[derive(Clone)]
struct AppState {
    telegram: Option<TelegramClient>,
}

#[derive(Clone)]
struct TelegramClient {
    client: reqwest::Client,
    bot_token: String,
    chat_id: String,
}

#[derive(Debug, Deserialize)]
struct AlertManagerWebhook {
    #[serde(default)]
    status: String,
    #[serde(default)]
    alerts: Vec<AlertPayload>,
    #[serde(default)]
    external_url: String,
}

#[derive(Debug, Deserialize)]
struct AlertPayload {
    #[serde(default)]
    status: String,
    labels: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    annotations: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    starts_at: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_init::init("info");
    let cli = Cli::parse();
    let cfg = Config::load(&cli.static_config, &cli.env_config).context("loading config")?;
    let telegram = if cfg.telegram_alerting_enabled() {
        Some(TelegramClient {
            client: reqwest::Client::new(),
            bot_token: cfg.telegram_bot_token().context("TELEGRAM_BOT_TOKEN")?,
            chat_id: cfg.telegram_chat_id().context("TELEGRAM_CHAT_ID")?,
        })
    } else {
        None
    };
    let state = Arc::new(AppState { telegram });
    let addr: SocketAddr = cli.bind.parse().context("parsing alert-bot bind address")?;
    let app = Router::new()
        .route("/health", get(health))
        .route("/alertmanager", post(alertmanager))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding alert-bot on {addr}"))?;
    info!(bind = %addr, telegram_enabled = cfg.telegram_alerting_enabled(), "alert-bot starting");
    axum::serve(listener, app)
        .await
        .context("serving alert-bot")
}

async fn health() -> &'static str {
    "ok"
}

async fn alertmanager(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AlertManagerWebhook>,
) -> impl IntoResponse {
    if payload.alerts.is_empty() {
        return StatusCode::NO_CONTENT;
    }
    let body = format_alert_message(&payload);
    info!(alerts = payload.alerts.len(), status = %payload.status, "received alertmanager webhook");
    if let Some(telegram) = &state.telegram {
        if let Err(e) = telegram.send_markdown(&body).await {
            error!(error = %e, "telegram send failed");
            return StatusCode::BAD_GATEWAY;
        }
    } else {
        warn!("telegram alerting disabled; dropping webhook after logging");
    }
    StatusCode::NO_CONTENT
}

impl TelegramClient {
    async fn send_markdown(&self, text: &str) -> Result<()> {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);
        let resp = self
            .client
            .post(url)
            .json(&serde_json::json!({
                "chat_id": self.chat_id,
                "text": text,
                "parse_mode": "Markdown",
                "disable_web_page_preview": true,
            }))
            .send()
            .await
            .context("POST sendMessage")?;
        let status = resp.status();
        let text_body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("telegram send failed: status={} body={}", status, text_body);
        }
        Ok(())
    }
}

fn format_alert_message(payload: &AlertManagerWebhook) -> String {
    let mut out = String::new();
    out.push_str("*Livepeer alert batch*\n");
    out.push_str(&format!(
        "- group status: `{}`\n",
        escape_md(&payload.status)
    ));
    if !payload.external_url.is_empty() {
        out.push_str(&format!(
            "- source: `{}`\n",
            escape_md(&payload.external_url)
        ));
    }
    out.push_str(&format!("- alerts: `{}`\n", payload.alerts.len()));
    for alert in &payload.alerts {
        let name = alert
            .labels
            .get("alertname")
            .map(String::as_str)
            .unwrap_or("unknown");
        let severity = alert
            .labels
            .get("severity")
            .map(String::as_str)
            .unwrap_or("unknown");
        out.push('\n');
        out.push_str(&format!(
            "*{}* `{}`\n",
            escape_md(name),
            escape_md(severity)
        ));
        if let Some(summary) = alert.annotations.get("summary") {
            out.push_str(&format!("{}\n", escape_md(summary)));
        }
        if let Some(description) = alert.annotations.get("description") {
            out.push_str(&format!("{}\n", escape_md(description)));
        }
        out.push_str(&format!(
            "- status: `{}`\n- starts_at: `{}`\n",
            escape_md(&alert.status),
            escape_md(&alert.starts_at)
        ));
        if let Some(task) = alert.labels.get("task") {
            out.push_str(&format!("- task: `{}`\n", escape_md(task)));
        }
        if let Some(provider) = alert.labels.get("provider") {
            out.push_str(&format!("- provider: `{}`\n", escape_md(provider)));
        }
    }
    out
}

fn escape_md(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('_', "\\_")
        .replace('*', "\\*")
        .replace('`', "\\`")
        .replace('[', "\\[")
}
