mod config;
mod bridge;
mod session;
mod util;

use anyhow::Result;
use clap::Parser;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{EnvFilter, fmt};
use tracing_subscriber::prelude::*;
use config::Config;
use bridge::Bridge;

#[tokio::main]
async fn main() -> Result<()> {
    info!("Starting ag1goose-bridge...");
    
    // Initialize tracing
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,rmcp=warn"));
    fmt().with_env_filter(filter).with_writer(std::io::stderr).init();
    info!("Tracing initialized");

    // Load config
    let cfg = Config::default();
    debug!(
        inbox = cfg.inbox, 
        redis_url = cfg.redis_url, 
        goose_bin = cfg.goose_bin, 
        "Loaded config"
    );

    // Create and run bridge
    debug!("Creating bridge instance...");
    let bridge = Bridge::new(cfg).await?;
    info!("Starting bridge run loop...");
    
    if let Err(e) = bridge.run().await {
        error!(error = %e, "Bridge error");
        return Err(e);
    }

    info!("Bridge exited cleanly");
    Ok(())
}