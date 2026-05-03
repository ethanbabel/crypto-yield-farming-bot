use eyre::Result;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::ConnectOptions;
use std::str::FromStr;
use std::time::Duration;
use tracing::log::LevelFilter;

use crate::config::Config;

pub async fn create_pool(config: &Config) -> Result<PgPool, sqlx::Error> {
    let connect_options = PgConnectOptions::from_str(&config.database_url)?
        .log_slow_statements(LevelFilter::Warn, Duration::from_secs(60));

    PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(30))
        .idle_timeout(Duration::from_secs(300))
        .max_lifetime(Duration::from_secs(1800))
        .test_before_acquire(false)
        .connect_with(connect_options)
        .await
}
