//! crates/bus/src/lib.rs
#![allow(clippy::unused_io_amount)]

use std::collections::HashMap;


use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BusError {
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Envelope {
    pub role: String,
    #[serde(default)]
    pub content: serde_json::Value,

    #[serde(default)] pub session_code:   Option<String>,
    #[serde(default)] pub agent_name:     Option<String>,
    #[serde(default)] pub usage:          serde_json::Value,
    #[serde(default)] pub billing_hint:   Option<String>,
    #[serde(default)] pub trace:          Vec<String>,
    #[serde(default)] pub user_id:        Option<String>,
    #[serde(default)] pub task_id:        Option<String>,
    #[serde(default)] pub target:         Option<String>,
    #[serde(default)] pub reply_to:       Option<String>,
    #[serde(default, rename = "envelope_type")]
    pub envelope_type: Option<String>,
    #[serde(default)] pub tools_used:     Vec<String>,
    #[serde(default)] pub auth_signature: Option<String>,
    #[serde(default)] pub timestamp:      Option<String>,
    #[serde(default)] pub headers:        HashMap<String, String>,
    #[serde(default)] pub meta:           serde_json::Value,
    #[serde(default)] pub envelope_id:    Option<String>,
    #[serde(default)] pub correlation_id: Option<String>,
    #[serde(default)] pub consumer_group: Option<String>,
    #[serde(default)] pub consumer_id:    Option<String>,
    #[serde(default)] pub delivery_count: Option<u32>,
}

pub struct Bus {
    client: redis::Client,
}

impl Bus {
    pub fn new(redis_url: &str) -> Result<Self, BusError> {
        Ok(Self {
            client: redis::Client::open(redis_url)?,
        })
    }

    /// Return the latest entry id in the stream, or "0-0" if empty.
    pub async fn tail_id(&self, stream: &str) -> Result<String, BusError> {
        let mut conn = self.client.get_async_connection().await?;
        let reply: redis::Value = redis::cmd("XREVRANGE")
            .arg(stream)
            .arg("+").arg("-")
            .arg("COUNT").arg(1)
            .query_async(&mut conn)
            .await?;
        use redis::Value::*;
        if let Bulk(b) = reply {
            if let Some(Bulk(entry)) = b.first() {
                if let Some(Data(idb)) = entry.first() {
                    return Ok(String::from_utf8_lossy(idb).into_owned());
                }
            }
        }
        Ok("0-0".to_string())
    }

    /// XADD <stream> * env <json>
    pub async fn send(&self, stream: &str, env: &Envelope) -> Result<String, BusError> {
        let timestamp = chrono::Utc::now().to_rfc3339();
        println!("\n[BUS_DEBUG][{}] SENDING MESSAGE", timestamp);
        println!("[BUS_DEBUG] Stream: {}", stream);
        println!("[BUS_DEBUG] Envelope ID: {:?}", env.envelope_id);
        println!("[BUS_DEBUG] Correlation ID: {:?}", env.correlation_id);
        println!("[BUS_DEBUG] Role: {}", env.role);
        println!("[BUS_DEBUG] Agent: {:?}", env.agent_name);
        println!("[BUS_DEBUG] Target: {:?}", env.target);
        println!("[BUS_DEBUG] Reply To: {:?}", env.reply_to);
        println!("[BUS_DEBUG] Envelope Type: {:?}", env.envelope_type);
        
        // Log the full envelope for debugging
        if let Ok(env_json) = serde_json::to_string_pretty(&env) {
            println!("[BUS_DEBUG] Full envelope: {}", env_json);
        }
        println!("[BUS_DEBUG] Content: {}", env.content);
        
        let mut conn = match self.client.get_async_connection().await {
            Ok(conn) => {
                println!("[BUS_DEBUG] ✅ Connected to Redis");
                conn
            }
            Err(e) => {
                println!("[BUS_ERROR] ❌ Redis connection failed: {}", e);
                return Err(BusError::Redis(e));
            }
        };
        
        let json = match serde_json::to_string(env) {
            Ok(json) => {
                println!("[BUS_DEBUG] ✅ Envelope serialized to JSON ({} bytes)", json.len());
                json
            }
            Err(e) => {
                println!("[BUS_ERROR] ❌ Failed to serialize envelope: {}", e);
                return Err(BusError::Json(e));
            }
        };
        
        println!("[BUS_DEBUG] Executing Redis XADD command");
        println!("[BUS_DEBUG] Redis command: XADD {} * data {}", stream, json);
        
        // Chain the command directly to avoid ownership issues
        match redis::cmd("XADD")
            .arg(stream)
            .arg("*")
            .arg("data")
            .arg(&json)
            .query_async(&mut conn)
            .await {
            Ok(id) => {
                println!("[BUS_DEBUG] Successfully sent message to Redis. Message ID: {}", id);
                Ok(id)
            }
            Err(e) => {
                println!("[BUS_ERROR] Failed to execute XADD command: {}", e);
                Err(BusError::Redis(e))
            }
        }
    }

    /// Blocking read after `last_id`. Use "$" for new-only.
    pub async fn recv_block(
        &self,
        stream: &str,
        last_id: &str,
        block_ms: u64,
    ) -> Result<Option<Envelope>, BusError> {
        let mut conn = self.client.get_async_connection().await?;

        let reply: redis::Value = redis::cmd("XREAD")
            .arg("BLOCK")
            .arg(block_ms)
            .arg("STREAMS")
            .arg(stream)
            .arg(last_id)
            .query_async(&mut conn)
            .await?;

        if let Some((id, env_json)) = extract_env(&reply) {
            let mut env: Envelope = serde_json::from_str(&env_json)?;
            //env.envelope_id.get_or_insert(id);
            env.envelope_id = Some(id); 
            return Ok(Some(env));
        }
        Ok(None)
    }

    /// Create a consumer group for a stream. Succeeds if the group already exists.
    pub async fn create_consumer_group(&self, stream: &str, group: &str) -> Result<(), BusError> {
        let timestamp = chrono::Utc::now().to_rfc3339();
        println!("\n[BUS_DEBUG][{}] CREATING CONSUMER GROUP", timestamp);
        println!("[BUS_DEBUG] Stream: {}", stream);
        println!("[BUS_DEBUG] Group: {}", group);
        
        let mut conn = match self.client.get_async_connection().await {
            Ok(conn) => {
                println!("[BUS_DEBUG] ✅ Connected to Redis");
                conn
            }
            Err(e) => {
                println!("[BUS_DEBUG] ❌ Failed to connect to Redis: {}", e);
                return Err(BusError::Redis(e));
            }
        };
        
        println!("[BUS_DEBUG] Executing XGROUP CREATE: XGROUP CREATE {} {} 0 MKSTREAM", stream, group);
        
        let result: Result<(), redis::RedisError> = redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(stream)
            .arg(group)
            .arg("0")
            .arg("MKSTREAM")
            .query_async(&mut conn)
            .await;
            
        match result {
            Ok(_) => {
                println!("[BUS_DEBUG] ✅ Successfully created consumer group");
                Ok(())
            }
            Err(e) => {
                if e.to_string().contains("BUSYGROUP") {
                    println!("[BUS_DEBUG] ℹ️ Consumer group already exists");
                    // Group already exists, which is fine
                    Ok(())
                } else {
                    println!("[BUS_DEBUG] ❌ Failed to create consumer group: {}", e);
                    Err(BusError::Redis(e))
                }
            }
        }
    }

    /// Blocking read from a consumer group
    pub async fn recv_block_group(
        &self,
        stream: &str,
        group: &str,
        consumer: &str,
        block_ms: u64,
    ) -> Result<Option<Envelope>, BusError> {
        let timestamp = chrono::Utc::now().to_rfc3339();
        println!("\n[BUS_DEBUG][{}] WAITING FOR MESSAGE", timestamp);
        println!("[BUS_DEBUG] Stream: {}", stream);
        println!("[BUS_DEBUG] Consumer Group: {}", group);
        println!("[BUS_DEBUG] Consumer ID: {}", consumer);
        println!("[BUS_DEBUG] Block Timeout: {}ms", block_ms);
        println!("\n[BUS_DEBUG] WAITING FOR MESSAGE");
        println!("[BUS_DEBUG] Stream: {}", stream);
        println!("[BUS_DEBUG] Consumer Group: {}", group);
        println!("[BUS_DEBUG] Consumer ID: {}", consumer);
        println!("[BUS_DEBUG] Block Timeout: {}ms", block_ms);
        let timestamp = chrono::Utc::now().to_rfc3339();
        println!("\n[BUS_DEBUG][{}] WAITING FOR MESSAGE", timestamp);
        println!("[BUS_DEBUG] Stream: {}", stream);
        println!("[BUS_DEBUG] Consumer Group: {}", group);
        println!("[BUS_DEBUG] Consumer ID: {}", consumer);
        println!("[BUS_DEBUG] Block Timeout: {}ms", block_ms);

        let start = std::time::Instant::now();
        let mut conn = match self.client.get_async_connection().await {
            Ok(conn) => {
                println!("[BUS_DEBUG] ✅ Connected to Redis");
                conn
            }
            Err(e) => {
                println!("[BUS_ERROR] ❌ Redis connection failed: {}", e);
                return Err(BusError::Redis(e));
            }
        };

        println!("[BUS_DEBUG] Executing XREADGROUP on stream: {}", stream);
        
        let reply = match redis::cmd("XREADGROUP")
            .arg("GROUP").arg(group).arg(consumer)
            .arg("COUNT").arg(1)
            .arg("BLOCK").arg(block_ms)
            .arg("STREAMS")
            .arg(stream)
            .arg(">")
            .query_async::<_, redis::Value>(&mut conn).await {
            Ok(reply) => {
                println!("[BUS_DEBUG] ✅ Received reply from Redis (took: {:?})", start.elapsed());
                reply
            }
            Err(e) => {
                println!("[BUS_ERROR] ❌ Redis command failed: {}", e);
                return Err(BusError::Redis(e));
            }
        };

        if let Some((id, json)) = extract_env(&reply) {
            println!("[BUS_DEBUG] 📨 Received message with ID: {}", id);
            println!("[BUS_DEBUG] Raw message: {}", json);
            
            let mut env: Envelope = match serde_json::from_str(&json) {
                Ok(env) => {
                    println!("[BUS_DEBUG] ✅ Successfully parsed envelope");
                    env
                }
                Err(e) => {
                    println!("[BUS_ERROR] ❌ Failed to parse envelope: {}", e);
                    return Err(BusError::Json(e));
                }
            };
            
            env.envelope_id = Some(id.clone());
            env.consumer_group = Some(group.to_string());
            env.consumer_id = Some(consumer.to_string());
            
            println!("[BUS_DEBUG] Envelope ID: {:?}", env.envelope_id);
            println!("[BUS_DEBUG] Correlation ID: {:?}", env.correlation_id);
            println!("[BUS_DEBUG] Role: {}", env.role);
            println!("[BUS_DEBUG] Agent: {:?}", env.agent_name);
            println!("[BUS_DEBUG] Target: {:?}", env.target);
            println!("[BUS_DEBUG] Reply To: {:?}", env.reply_to);
            println!("[BUS_DEBUG] Envelope Type: {:?}", env.envelope_type);
            println!("[BUS_DEBUG] Content: {}", env.content);
            
            return Ok(Some(env));
        } else {
            println!("[BUS_DEBUG] ⏳ No messages received (timeout or empty stream)");
        }
        
        Ok(None)
    }

    /// Acknowledge that a message has been processed
    pub async fn ack_message(
        &self,
        stream: &str,
        group: &str,
        message_id: &str,
    ) -> Result<(), BusError> {
        let mut conn = self.client.get_async_connection().await?;
        redis::cmd("XACK")
            .arg(stream)
            .arg(group)
            .arg(message_id)
            .query_async::<_, ()>(&mut conn)
            .await?;
        Ok(())
    }
}

/// Return (id, env_json) for first message in XREAD reply
fn extract_env(v: &redis::Value) -> Option<(String, String)> {
    use redis::Value::*;
    let outer = match v { Bulk(v) => v, _ => return None };
    let stream_bulk = match outer.first()? { Bulk(v) => v, _ => return None };
    let msgs = match stream_bulk.get(1)? { Bulk(v) => v, _ => return None }; // second elem is fine
    let first_msg = match msgs.first()? { Bulk(v) => v, _ => return None };
    let id = match first_msg.first()? { Data(b) => String::from_utf8_lossy(b).into_owned(), _ => return None };
    let fields = match first_msg.get(1)? { Bulk(v) => v, _ => return None };

    let mut it = fields.iter();
    let mut found_env: Option<String> = None;
    let mut found_data: Option<String> = None;

    while let (Some(k), Some(v)) = (it.next(), it.next()) {
        if let (Data(kb), Data(vb)) = (k, v) {
            let key = std::str::from_utf8(kb).ok()?;
            let val = String::from_utf8_lossy(vb).into_owned();
            match key {
                "env"  => found_env  = Some(val),
                "data" => found_data = Some(val),
                _ => {}
            }
        }
    }

    // Prefer "env", fall back to "data"
    if let Some(json) = found_env.or(found_data) {
        return Some((id, json));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    

    #[tokio::test]
    async fn round_trip() {
        let bus = Bus::new("redis://admin:UltraSecretRoot123@forge.agentic1.xyz:8081").unwrap();
        let env = Envelope {
            role: "user_request".into(),
            content: json!({"text": "ping"}),
            session_code: None,
            agent_name: Some("tester".into()),
            usage: json!({}),
            billing_hint: None,
            trace: vec![],
            user_id: None,
            task_id: None,
            target: None,
            reply_to: Some("tester_inbox".into()),
            envelope_type: Some("message".into()),
            tools_used: vec![],
            auth_signature: None,
            timestamp: None,
            headers: HashMap::new(),
            meta: json!({}),
            envelope_id: None,
            correlation_id: Some("test-cid".into()),
        };

        let stream = "ag1:bus:test";
        let id = bus.send(stream, &env).await.unwrap();
        assert!(!id.is_empty());

        let got = bus.recv_block(stream, "0-0", 50).await.unwrap();
        assert!(got.is_some());
        let got = got.unwrap();
        assert_eq!(got.role, "user_request");
        assert_eq!(got.content["text"], "ping");
    }
}
