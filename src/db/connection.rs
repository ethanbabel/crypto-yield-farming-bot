use sqlx::ConnectOptions;
use sqlx::postgres::{
    PgConnectOptions,
    PgPool,
    PgPoolOptions,
};
use eyre::Result;
use std::str::FromStr;
use std::time::Duration;
use tracing::log::LevelFilter;

use crate::config::Config;

pub async fn create_pool(config: &Config) -> Result<PgPool, sqlx::Error> {
    let connect_options = PgConnectOptions::from_str(&config.database_url)?
        .log_slow_statements(LevelFilter::Warn, Duration::from_secs(60));

    PgPoolOptions::new()
        .max_connections(5) 
        .connect_with(connect_options)
        .await
}
