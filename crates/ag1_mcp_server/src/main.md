use std::future::Future; 
use anyhow::Result;
use rmcp::{
    ErrorData as McpError,
    ServiceExt, ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::*,
    tool, tool_router, tool_handler,
    transport::stdio,
};

#[derive(Clone)]
struct Ag1Server {
    tool_router: ToolRouter<Self>,
}

impl Ag1Server {
    fn from_env() -> anyhow::Result<Self> {
        Ok(Self { tool_router: Self::tool_router() })
    }
}

#[tool_router]
impl Ag1Server {
    /// Simple health check.
    #[tool(name = "ag1_ping", description = "Health check")]
    async fn ag1_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text("pong")]))
    }
}

#[tool_handler]
impl ServerHandler for Ag1Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("AG1Goose minimal MCP server.".into()),
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
