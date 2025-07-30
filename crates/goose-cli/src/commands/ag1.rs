use anyhow::Result;
use clap::{Args, Subcommand};
use ag1_meta::{Registry, delegate_to_name_with_opts};

#[derive(Args, Debug)]
pub struct Ag1Cmd {
    /// Path to your orchestrator_registry.json
    #[arg(long, default_value = "config/orchestrator_registry.json")]
    pub registry: String,

    /// Goose inbox stream to receive replies
    #[arg(long, env = "AG1_GOOSE_INBOX", default_value = "AG1:agent:GooseAgent:inbox")]
    pub goose_inbox: String,

    /// Redis URL
    #[arg(long, env = "REDIS_URL", default_value = "redis://admin:UltraSecretRoot123@forge.agentic1.xyz:8081")]
    pub redis: String,

    #[command(subcommand)]
    pub cmd: Ag1Sub,
}

#[derive(Subcommand, Debug)]
pub enum Ag1Sub {
    /// List agents discovered in the registry
    List,
    /// Show one agent's full record
    Describe { name: String },
    /// Send to agent by name
    Delegate {
        name: String,
        #[arg(long)]
        content: String,
        #[arg(long)]                // optional meta
        meta: Option<String>,
        #[arg(long, default_value = "user")]           // NEW
        role: String,
        #[arg(long, default_value = "message")]        // NEW
        envelope_type: String,
        #[arg(long, default_value_t = 8000)]
        timeout_ms: u64,
    },
}

pub async fn run(args: Ag1Cmd) -> Result<()> {
    let reg = Registry::load_map(&args.registry, &args.goose_inbox)?;

    match args.cmd {
        Ag1Sub::List => {
            for a in reg.list() {
                println!("{:<24}  {}", a.name, a.inbox);
            }
        }
        Ag1Sub::Describe { name } => {
            let a = reg.get(&name).ok_or_else(|| anyhow::anyhow!("not found: {name}"))?;
            println!("{}", serde_json::to_string_pretty(a)?);
        }
        Ag1Sub::Delegate { name, content, meta, role, envelope_type, timeout_ms } => {
            let content_json: serde_json::Value = serde_json::from_str(&content)?;
            let meta_json: serde_json::Value = match meta {
                Some(s) => serde_json::from_str(&s)?,
                None => serde_json::json!({}),
            };
            let reply = ag1_meta::delegate_to_name_with_opts(
                &args.redis, &reg, &name,
                content_json, meta_json,
                &role, &envelope_type,
                timeout_ms
            ).await?;
            println!("xxxx {}", serde_json::to_string_pretty(&reply)?);
        }
    }
    Ok(())
}
