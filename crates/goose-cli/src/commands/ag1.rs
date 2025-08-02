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
        #[arg(long, default_value_t = 30000)]
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
            let start_time = std::time::Instant::now();
            println!("\n[AG1_DELEGATE] Starting delegation to agent: {}", name);
            println!("[AG1_DELEGATE] Redis: {}", args.redis);
            println!("[AG1_DELEGATE] Role: {}, Envelope Type: {}", role, envelope_type);
            println!("[AG1_DELEGATE] Timeout: {}ms", timeout_ms);
            
            // Parse content JSON
            let content_json: serde_json::Value = serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Failed to parse content as JSON: {}", e))?;
            println!("[AG1_DELEGATE] Content JSON parsed successfully ({} bytes)", content.len());
            
            // Parse meta JSON if provided
            let meta_json: serde_json::Value = match meta {
                Some(ref s) => {
                    let json = serde_json::from_str(s)
                        .map_err(|e| anyhow::anyhow!("Failed to parse meta as JSON: {}", e))?;
                    println!("[AG1_DELEGATE] Meta JSON parsed successfully ({} bytes)", s.len());
                    json
                },
                None => {
                    println!("[AG1_DELEGATE] No meta provided, using empty object");
                    serde_json::json!({})
                },
            };
            
            // Log registry state
            let agents: Vec<_> = reg.list().iter().map(|a| &a.name).collect();
            println!("[AG1_DELEGATE] Registry contains {} agents: {:?}", agents.len(), agents);
            if !agents.iter().any(|a| a == &&name) {
                println!("[AG1_DELEGATE] WARNING: Agent '{}' not found in registry", name);
            }
            
            // Make the delegation call
            println!("[AG1_DELEGATE] Calling delegate_to_name_with_opts...");
            let delegate_start = std::time::Instant::now();
            
            let reply = match ag1_meta::delegate_to_name_with_opts(
                &args.redis, 
                &reg, 
                &name,
                content_json, 
                meta_json,
                &role, 
                &envelope_type,
                timeout_ms
            ).await {
                Ok(reply) => reply,
                Err(e) => {
                    println!("[AG1_DELEGATE] ERROR in delegate_to_name_with_opts: {}", e);
                    return Err(e);
                }
            };
            
            let delegate_duration = delegate_start.elapsed();
            println!("[AG1_DELEGATE] delegate_to_name_with_opts completed in {:?}", delegate_duration);
            
            // Format and print the reply
            let reply_str = serde_json::to_string_pretty(&reply)
                .unwrap_or_else(|_| "[Failed to format reply]".to_string());
            println!("\n[AG1_DELEGATE] === DELEGATION RESULT ({} bytes) ===", reply_str.len());
            println!("--->>> {}", reply_str);
            println!("[AG1_DELEGATE] ====================================\n");
            
            let total_duration = start_time.elapsed();
            println!("[AG1_DELEGATE] Total delegation time: {:?}", total_duration);
        }
    }
    Ok(())
}
