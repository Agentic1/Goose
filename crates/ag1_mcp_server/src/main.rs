use anyhow::Result;
use std::future::Future;
use std::sync::Arc;

fn empty_obj() -> serde_json::Value { serde_json::json!({}) }
use ag1_meta::{Registry, delegate_to_name_with_opts};

use rmcp::{
    ErrorData as McpError,
    ServiceExt, ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::tool::Parameters,
    model::*,
    tool, tool_router, tool_handler,
    transport::stdio,
};

use schemars::JsonSchema;
use serde::Deserialize;
use tracing_subscriber as _;

// ---------- Params ----------

#[derive(Debug, Deserialize, JsonSchema)]
struct DescribeParams {
    name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DelegateParams {
    target: String,
    #[serde(default)] content: serde_json::Value,
    #[serde(default = "empty_obj")] meta: serde_json::Value,
    #[serde(default = "default_role")] role: String,
    #[serde(default = "default_envelope_type")] envelope_type: String,
    #[serde(default = "default_timeout")] timeout_ms: u64,
}

fn default_role() -> String { "user".into() }
fn default_envelope_type() -> String { "message".into() }
fn default_timeout() -> u64 { 30000 }

// ---------- Server ----------

#[derive(Clone)]
struct Ag1Server {
    redis_url: String,
    registry: Arc<Registry>,
    tool_router: ToolRouter<Self>,
}

impl Ag1Server {
    fn from_env() -> anyhow::Result<Self> {
        let redis_url = std::env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://admin:UltraSecretRoot123@forge.agentic1.xyz:8081".into());
        let goose_inbox = std::env::var("AG1_GOOSE_INBOX")
            .unwrap_or_else(|_| "AG1:agent:GooseAgent:inbox".into());
        let reg_path = std::env::var("AG1_REGISTRY_PATH")
            .or_else(|_| std::env::var("AG1_REGISTRY"))
            .unwrap_or_else(|_| "config/orchestrator_registry.json".into());

        let reg = Registry::load_map(reg_path, goose_inbox)?;
        Ok(Self {
            redis_url,
            registry: Arc::new(reg),
            tool_router: Self::tool_router(),
        })
    }
}

#[tool_router]
impl Ag1Server {
    #[tool(name = "ag1_list", description = "List agents known to the AG1 registry.")]
    async fn ag1_list(&self) -> Result<CallToolResult, McpError> {
        let vals: Vec<_> = self.registry.list().into_iter().map(|a| {
            serde_json::json!({
                "name": a.name,
                "inbox": a.inbox,
                "capabilities": a.capabilities_keywords,
            })
        }).collect();

        Ok(CallToolResult::success(vec![Content::json(vals)?]))
    }

    #[tool(name = "ag1_describe", description = "Describe an agent by name.")]
    async fn ag1_describe(&self, p: Parameters<DescribeParams>)
        -> Result<CallToolResult, McpError>
    {
        let name = &p.0.name;
        if let Some(a) = self.registry.get(name) {
            Ok(CallToolResult::success(vec![Content::json(a)?]))
        } else {
            Ok(CallToolResult::error(vec![Content::text(format!("Unknown agent: {}", name))]))
        }
    }

    #[tool(name = "ag1_delegate", description = "Delegate a request to an AG1 agent.")]
    async fn ag1_delegate(&self, p: Parameters<DelegateParams>)
        -> Result<CallToolResult, McpError>
    {
        let args = p.0;
        let reply = delegate_to_name_with_opts(
            &self.redis_url,
            &self.registry,
            &args.target,
            args.content,
            args.meta,
            &args.role,
            &args.envelope_type,
            args.timeout_ms,
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::json(reply)?]))
    }
}

#[tool_handler]
impl ServerHandler for Ag1Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("AG1Goose bridge to AetherBus agents.".into()),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,rmcp=warn")
        .init();

    let service = Ag1Server::from_env()?
        .serve(stdio())
        .await?;

    service.waiting().await?;
    Ok(())
}
