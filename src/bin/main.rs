use dotenvy::dotenv;

use crypto_yield_farming_bot::logging;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Load environment variables from .env file
    dotenv()?;

    // Initialize logging
    if let Err(e) = logging::init_logging(env!("CARGO_BIN_NAME").to_string()) {
        eprintln!("Failed to initialize logging: {}", e);
        return Err(e.into());
    }
    
    // Test Loki logging via tracing
    tracing::error!("Hello Loki! (Error)");
    tracing::warn!("Hello Loki! (Warning)");
    tracing::info!("Hello Loki! (Info)");
    tracing::debug!("Hello Loki! (Debug)");
    tracing::trace!("Hello Loki! (Trace)");

    // Give time for tracing-loki to send logs to Alloy
    println!("Waiting 3 seconds for logs to be sent to Loki...");
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    println!("Done waiting. Check Grafana Cloud for the logs!");

    Ok(())
}
