fn normalize_content(mut content: Value) -> Value {
    let mut obj = if content.is_object() {
        content.as_object_mut().unwrap().clone()
    } else if content.is_null() {
        let mut map = serde_json::Map::new();
        map.insert("text".to_string(), json!(""));
        map
    } else {
        let mut map = serde_json::Map::new();
        let text = if content.is_string() { 
            content.as_str().unwrap_or("").to_string()
        } else if content.is_number() || content.is_boolean() {
            content.to_string()
        } else {
            serde_json::to_string(&content).unwrap_or_default()
        };
        map.insert("text".to_string(), json!(text));
        map
    };

    if !obj.contains_key("text") {
        obj.insert("text".to_string(), json!(""));
    }

    Value::Object(obj)
}

pub fn create_envelope(content: serde_json::Value, role: &str, meta: Option<serde_json::Value>) -> Envelope {
    let content = normalize_content(content);
    Envelope {
        content,
        role: role.to_string(),
        meta: meta.unwrap_or_else(|| serde_json::json!({})),
        session_code: None,
        agent_name: None,
        usage: serde_json::json!({}),
        billing_hint: None,
        trace: vec![],
        user_id: None,
        task_id: None,
        target: None,
        reply_to: None,
        envelope_type: None,
        tools_used: vec![],
        auth_signature: None,
        timestamp: None,
        headers: std::collections::HashMap::new(),
        envelope_id: None,
        correlation_id: None,
        consumer_group: None,
        consumer_id: None,
        delivery_count: None,
    }
}
mod registry;
pub use registry::{Registry, AgentInfo};

use anyhow::{bail, Result};
use bus::{Bus, Envelope};
use serde_json::{json, Value};
use uuid::Uuid;
use chrono::Utc;


pub async fn delegate_to_name_with_opts(
    redis_url: &str,
    reg: &Registry,
    target_name: &str,
    content: serde_json::Value,
    meta: serde_json::Value,
    role: &str,
    envelope_type: &str,
    timeout_ms: u64,
) -> Result<Envelope> {
    let info = reg.get(target_name)
        .ok_or_else(|| anyhow::anyhow!("unknown agent: {target_name}"))?;
    delegate_with_opts(
        redis_url, &info.inbox, &reg.goose_inbox, target_name,
        content, meta, role, envelope_type, timeout_ms
    ).await
}


pub async fn delegate_to_name(
    redis_url: &str,
    reg: &Registry,
    target_name: &str,
    content: serde_json::Value,
    meta: serde_json::Value,
    timeout_ms: u64,
) -> Result<Envelope> {
    let info = reg.get(target_name)
        .ok_or_else(|| anyhow::anyhow!("unknown agent: {target_name}"))?;
    delegate(redis_url, &info.inbox, &reg.goose_inbox, target_name, content, meta, timeout_ms).await
}

pub async fn delegate_with_opts(
    redis_url: &str,
    out_stream: &str,
    in_stream: &str,
    target: &str,
    mut content: serde_json::Value,
    meta: serde_json::Value,
    role: &str,
    envelope_type: &str,
    timeout_ms: u64,
) -> Result<Envelope> {
    println!("[AG1_meta] delegate_with_opts called with:");
    println!("  - redis_url: {}", redis_url);
    println!("  - out_stream: {}", out_stream);
    println!("  - in_stream: {}", in_stream);
    println!("  - target: {}", target);
    println!("  - content: {}", content);
    println!("  - meta: {}", meta);
    println!("  - role: {}", role);
    println!("  - envelope_type: {}", envelope_type);
    println!("  - timeout_ms: {}", timeout_ms);
    println!("[AG1_meta] delegate_with_opts - Starting");
    println!("[AG1_meta]   redis_url: {}", redis_url);
    println!("[AG1_meta]   out_stream: {}", out_stream);
    println!("[AG1_meta]   in_stream: {}", in_stream);
    println!("[AG1_meta]   target: {}", target);
    println!("[AG1_meta]   content: {}", content);
    println!("[AG1_meta]   role: {}", role);
    println!("[AG1_meta]   envelope_type: {}", envelope_type);
    println!("[AG1_meta]   timeout_ms: {}", timeout_ms);
    println!("[AG1_meta] Creating new Bus instance");
    let bus = Bus::new(redis_url)?;
    println!("[AG1_meta] Bus instance created");
    let group = "ag1_meta";
    let consumer_id = Uuid::new_v4().to_string();
    if let Err(e) = bus.create_consumer_group(in_stream, group).await {
        println!("[AG1_meta] failed to create consumer group: {}", e);
    }
    let cid = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();

    println!("[AG1_meta] Creating envelope");
    // Ensure content is properly formatted as an object with a text field
    let content = match content {
        Value::String(s) => json!({ "text": s }),
        Value::Object(mut obj) => {
            // If it's already an object but doesn't have a text field, add it
            if !obj.contains_key("text") {
                obj.insert("text".to_string(), json!(""));
            }
            Value::Object(obj)
        },
        _ => json!({ "text": content.to_string() })
    };
    // Double-check the format is correct
    let content = normalize_content(content);
    println!("[AG1_meta] POST Normalisation Envelope content: {}", content);
    
    // Ensure the content is an object with a text field
    let content = if let Value::Object(obj) = content {
        if !obj.contains_key("text") {
            let mut new_obj = obj.clone();
            new_obj.insert("text".to_string(), json!(""));
            Value::Object(new_obj)
        } else {
            Value::Object(obj)
        }
    } else {
        json!({ "text": content.to_string() })
    };

    let env = Envelope {
        role: role.to_string(),
        content,
        session_code: None,
        agent_name: Some("ag1goose".into()),
        //agent_name: Some(target.to_string()),
        //agent_name: Some(env::var("AGENT_NAME").unwrap_or_else(|_| "ag1goose".to_string())),
        usage: json!({}),
        billing_hint: None,
        trace: vec![],
        user_id: None,
        task_id: None,
        target: Some(target.into()),
        reply_to: Some(in_stream.to_string()),
        envelope_type: Some(envelope_type.to_string()),
        tools_used: vec![],
        auth_signature: None,
        timestamp: Some(now),
        headers: Default::default(),
        meta: if meta.is_object() { meta } else { serde_json::json!({}) },
        envelope_id: Some(cid.clone()),
        correlation_id: Some(cid.clone()),
        consumer_group: None,
        consumer_id: None,
        delivery_count: None,
    };

    println!("[AG1_meta] Sending envelope to stream: {}", out_stream);
    println!("[AG1_meta] Envelope content: {:#?}", env);
    match bus.send(out_stream, &env).await {
        Ok(_) => println!("[AG1_meta] Envelope sent successfully"),
        Err(e) => {
            println!("[ERROR] Failed to send envelope: {}", e);
            return Err(e.into());
        }
    }

    let start = std::time::Instant::now();
    let slice_ms: u64 = 800;

    loop {
        let elapsed = start.elapsed().as_millis() as u64;
        if elapsed >= timeout_ms {
            bail!("no reply within {} ms (cid={})", timeout_ms, cid);
        }
        let block = slice_ms.min(timeout_ms - elapsed);

        if let Some(reply) = bus
            .recv_block_group(in_stream, group, &consumer_id, block)
            .await?
        {
            if reply.correlation_id.as_deref() == Some(&cid) {
                if let Some(id) = &reply.envelope_id {
                    let _ = bus.ack_message(in_stream, group, id).await;
                }
                return Ok(reply);
            } else if let Some(id) = &reply.envelope_id {
                let _ = bus.ack_message(in_stream, group, id).await;
            }
        }
    }
}

pub async fn delegate(
    redis_url: &str,
    out_stream: &str,
    in_stream: &str,
    target: &str,
    content: serde_json::Value,
    meta: serde_json::Value,
    timeout_ms: u64,
) -> Result<Envelope> {
    delegate_with_opts(
        redis_url, out_stream, in_stream, target,
        content, meta, "user", "message", timeout_ms
    ).await
}

/*
#[allow(dead_code)]
fn valid_stream(s: &str) -> bool {
    // AG1:<class>:<id...>:inbox
    let parts: Vec<&str> = s.split(':').collect();
    parts.len() >= 4 &&
    parts[0] == "AG1" &&
    matches!(parts[1], "agent" | "service" | "edge") &&
    parts.last() == Some(&"inbox")
}
*/
