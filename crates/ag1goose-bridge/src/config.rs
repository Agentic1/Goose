use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Stream to receive user requests for Goose
    pub inbox: String, // e.g. "AG1:agent:GooseAgent:inbox"
    /// REDIS URL for the bus
    pub redis_url: String,
    /// Path to goose binary ("goose" if on PATH)
    pub goose_bin: String,
    /// Max per‑turn wait for a reply from Goose (ms)
    pub turn_timeout_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            inbox: std::env::var("AG1_GOOSE_INBOX").unwrap_or_else(|_| "AG1:agent:GooseAgent:inbox".into()),
            redis_url: std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://admin:UltraSecretRoot123@forge.agentic1.xyz:8081".into()),
            // Use the 'goose' binary from the system PATH
            goose_bin: std::env::var("GOOSE_BIN").unwrap_or_else(|_| {
                "/Users/admin/.local/bin/goose".to_string()
            }),
            turn_timeout_ms: std::env::var("GOOSE_TURN_TIMEOUT_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(120_000),
        }
    }
}