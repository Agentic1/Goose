use std::future::Future;
use std::sync::Arc;
use anyhow::Result;

use ag1_meta::{Registry, delegate_to_name_with_opts};
use rmcp::{
    ErrorData as McpError,
    ServiceExt, 
    ServerHandler,
    handler::server::router::tool::{ToolRouter, CallToolResult},
    model::content::Content,
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
    #[serde(default)] meta: serde_json::Value,
    #[serde(default = "default_role")] role: String,
    #[serde(default = "default_envelope_type")] envelope_type: String,
    #[serde(default = "default_timeout")] timeout_ms: u64,
}

fn default_role() -> String { "user".into() }
fn default_envelope_type() -> String { "message".into() }
fn default_timeout() -> u64 { 8000 }

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
            .unwrap_or_else(|_| "redis://127.0.0.1/".into());
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
    /// List agents known to the AG1 registry.
    #[tool(name = "ag1_list", description = "List agents known to the AG1 registry.")]
    async fn ag1_list(&self, _args: serde_json::Value) -> Result<CallToolResult, McpError> {
        let vals: Vec<_> = self.registry.list().into_iter().map(|a| {
            serde_json::json!({
                "name": a.name,
                "inbox": a.inbox,
                "capabilities": a.capabilities_keywords,
            })
        }).collect();
        Ok(CallToolResult::success(vec![Content::json(vals)?]))
    }

    /// Describe an agent by name.
    #[tool(name = "ag1_describe", description = "Describe an agent by name.")]
    async fn ag1_describe(&self, args: serde_json::Value) -> Result<CallToolResult, McpError> {
        let p: DescribeParams = serde_json::from_value(args)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        if let Some(a) = self.registry.get(&p.name) {
            Ok(CallToolResult::success(vec![Content::json(a)?]))
        } else {
            Ok(CallToolResult::error(vec![Content::text(format!("Unknown agent: {}", p.name))]))
        }
    }

    /// Delegate a request to a named agent via the AG1 bus.
    #[tool(name = "ag1_delegate", description = "Delegate a request to an AG1 agent.")]
    async fn ag1_delegate(&self, args: serde_json::Value) -> Result<CallToolResult, McpError> {
        let p: DelegateParams = serde_json::from_value(args)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let reply = delegate_to_name_with_opts(
            &self.redis_url,
            &self.registry,
            &p.target,
            p.content,
            p.meta,
            &p.role,
            &p.envelope_type,
            p.timeout_ms,
        ).await.map_err(|e| McpError::internal_error(e.to_string(), None))?;

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

    let service = Ag1Server::from_env()?.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
