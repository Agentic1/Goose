use async_trait::async_trait;
use rmcp::{Tool, ToolMetadata, ToolRegistry};
use serde_json::Value;
use anyhow::Result;
use bus::{Bus, Envelope};

pub struct DelegateTool;

#[async_trait]
impl Tool for DelegateTool {
    fn metadata(&self) -> ToolMetadata {
        ToolMetadata::builder("delegate")
            .description("Send an envelope over AetherBus and await one reply")
            .build()
    }

    async fn call(&self, params: Value) -> Result<Value> {
        let redis = params["redis"].as_str().unwrap();
        let out   = params["out_stream"].as_str().unwrap();
        let inn   = params["in_stream"].as_str().unwrap();
        let tgt   = params["target"].as_str().unwrap();
        let content = params["content"].clone();

        let env = crate::delegate(redis, out, inn, tgt, content, 5000).await?;
        Ok(serde_json::to_value(env)?)
    }
}

/// Helper to register into Goose's registry
pub fn register_tools(reg: &mut ToolRegistry) {
    reg.register(Box::new(DelegateTool));
}
