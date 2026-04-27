use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub fn init(default_filter: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().json())
        .init();
}
