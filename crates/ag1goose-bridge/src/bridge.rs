use anyhow::{Result, anyhow};
use tracing::{info, error, warn, debug};
use std::collections::HashMap;
use tokio::sync::Mutex;
use serde_json::json;
use uuid;
use uuid::Uuid;
use crate::{config::Config, session::GooseSession};
use bus::{Bus, Envelope};
use std::time::Instant;

pub struct Bridge {
    cfg: Config,
    bus: Bus,
    sessions: Mutex<HashMap<String, GooseSession>>, // key: session_code
    reply_to_session: Mutex<HashMap<String, String>>, // key: reply_to, value: session_id
}

impl Bridge {
    pub async fn new(cfg: Config) -> Result<Self> {
        println!("[DEBUG] Creating new Bridge instance");
        println!("[DEBUG] Connecting to Redis at: {}", cfg.redis_url);
        
        let start = Instant::now();
        let bus = Bus::new(&cfg.redis_url).map_err(|e| {
            println!("[ERROR] Failed to connect to Redis: {}", e);
            e
        })?;
        
        println!("[DEBUG] Successfully connected to Redis in {:?}", start.elapsed());
        println!("[DEBUG] Bridge instance created successfully");
        
        Ok(Self { 
            cfg, 
            bus, 
            sessions: Mutex::new(HashMap::new()),
            reply_to_session: Mutex::new(HashMap::new()),
        })
    }

    async fn get_or_start_session(&self, sid: &str) -> Result<()> {
        println!("[DEBUG] Getting or starting session for ID: {}", sid);
        let start = Instant::now();
        
        let mut map = self.sessions.lock().await;
        if !map.contains_key(sid) {
            println!("[DEBUG] Creating new session for ID: {}", sid);
            match GooseSession::start(&self.cfg, sid.to_string()).await {
                Ok(sess) => {
                    println!("[DEBUG] Successfully created new session for ID: {}", sid);
                    map.insert(sid.to_string(), sess);
                }
                Err(e) => {
                    println!("[ERROR] Failed to create session for ID {}: {}", sid, e);
                    return Err(e);
                }
            }
        } else {
            println!("[DEBUG] Using existing session for ID: {}", sid);
        }
        
        println!("[DEBUG] Session operation completed in {:?}", start.elapsed());
        Ok(())
    }

    pub async fn run(&self) -> Result<()> {
        info!(inbox = %self.cfg.inbox, "bridge started");
        println!("[DEBUG] Bridge starting to listen on inbox: {}", self.cfg.inbox);
        
        let mut last_id = "$".to_string();
        let mut message_count = 0;
        
        loop {
            println!("[DEBUG] Waiting for next message... (last_id: {})", last_id);
            match self.bus.recv_block(&self.cfg.inbox, &last_id, 2000).await {
                Ok(Some(env)) => {
                    message_count += 1;
                    println!("[DEBUG] Received message #{}", message_count);
                    
                    if let Some(id) = &env.envelope_id { 
                        last_id = id.clone();
                        println!("[DEBUG] Updated last_id to: {}", last_id);
                    }
                    
                    let start = Instant::now();
                    match self.handle_envelope(env).await {
                        Ok(_) => {
                            println!("[DEBUG] Successfully processed message #{} in {:?}", 
                                    message_count, start.elapsed());
                        }
                        Err(e) => {
                            error!(error=?e, "failed handling envelope");
                            println!("[ERROR] Failed to handle message #{}: {}", message_count, e);
                        }
                    }
                }
                Ok(None) => {
                    println!("[DEBUG] No messages received, continuing to poll...");
                }
                Err(e) => {
                    error!(error=?e, "error receiving message");
                    println!("[ERROR] Error receiving message: {}", e);
                    // Add a small delay to prevent tight loop on errors
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// Get the reply-to address from an envelope or use a default
    fn get_reply_to(&self, env: &Envelope) -> String {
        match &env.reply_to {
            Some(rt) => {
                debug!(reply_to = %rt, "Using reply-to address from envelope");
                rt.clone()
            }
            None => {
                let default_reply = "AG1:agent:TestClient:inbox".to_string();
                warn!(default_reply = %default_reply, "No reply-to address in envelope, using default");
                default_reply
            }
        }
    }
    
    async fn handle_envelope(&self, env: Envelope) -> Result<()> {
        info!(correlation_id = ?env.correlation_id, "Handling envelope");
        
        // Skip non-user messages
        if env.role != "user" {
            debug!(role = %env.role, "Skipping non-user message");
            return Ok(());
        }
        
        // Get reply-to address
        let reply_to = self.get_reply_to(&env);
        
        // Check if we have an existing session for this reply_to
        let sid = if let Some(session_id) = self.get_session_for_reply_to(&reply_to).await? {
            info!(session_id = %session_id, reply_to = %reply_to, "Reusing existing session");
            session_id
        } else {
            // Generate new session ID if none provided
            let sid = env.session_code.clone().unwrap_or_else(|| {
                let new_sid = format!("sess_{}", Uuid::new_v4().to_string().split('-').next().unwrap_or(""));
                info!(new_session_id = %new_sid, "Generated new session ID");
                new_sid
            });
            
            // Store the mapping from reply_to to session ID
            self.map_reply_to_session(&reply_to, &sid).await?;
            sid
        };
        
        // Get or create the session
        self.get_or_start_session(&sid).await?;
        
        // Get or generate correlation ID
        let cid = env.correlation_id.clone().unwrap_or_else(|| {
            let new_cid = Uuid::new_v4().to_string();
            debug!("Generated new correlation ID: {}", new_cid);
            new_cid
        });

        // Get the message text
        let message = env.content.get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("No text content in message"))?;
            
        info!("[{}] Processing message ({} chars) with CID: {}", 
             sid, message.len(), cid);
        
        // Get session with lock scope
        let response = {
            let mut sessions = self.sessions.lock().await;
            let session = sessions.get_mut(&sid).ok_or_else(|| {
                error!("[{}] Session not found in session map", sid);
                anyhow!("Session not found")
            })?;
            
            // Get the current offset before sending input
            let start_offset = session.get_last_offset();
            debug!("[{}] Starting JSONL read from offset: {}", sid, start_offset);
            
            // Send the input to the session
            if let Err(e) = session.send_user(message).await {
                error!("[{}] Failed to send user input: {}", sid, e);
                return Err(anyhow!("Failed to send input: {}", e));
            }
            
            // Wait for the response with a timeout using JSONL file
            // Using a 30 second timeout for the response
            match session.wait_assistant_jsonl(30000, start_offset).await {
                Ok((response, new_offset)) => {
                    // Update the session's last_offset for the next read
                    session.update_offset(new_offset);
                    debug!("[{}] Updated session offset to: {}", sid, new_offset);
                    response
                },
                Err(e) => {
                    error!("[{}] Error getting response from Goose (JSONL): {}", sid, e);
                    error!("[{}] Session state - is process running? {}", sid, 
                          if session.is_running().await { "yes" } else { "no" });
                    format!("Error getting response from Goose: {}", e)
                }
            }
        };
        
        // Log the response details
        info!("[{}] Sending response ({} chars) to {}", 
             sid, response.len(), reply_to);
        
        // Create and send the response envelope
        let response_env = Envelope {
            role: "assistant".to_string(),
            content: json!({ 
                "text": response,
                "session_id": sid,
                "timestamp": chrono::Utc::now().to_rfc3339()
            }),
            session_code: Some(sid.clone()),
            agent_name: Some("GooseAgent".to_string()),
            usage: json!({}),
            billing_hint: None,
            trace: vec![],
            user_id: None,
            task_id: None,
            target: None,
            reply_to: Some(reply_to.clone()),
            envelope_type: Some("message_reply".into()),
            tools_used: vec![],
            auth_signature: None,
            timestamp: Some(chrono::Utc::now().to_rfc3339()),
            headers: Default::default(),
            meta: json!({ "x_stream_key": self.cfg.inbox }),
            envelope_id: Some(uuid::Uuid::new_v4().to_string()),
            correlation_id: Some(cid),
            consumer_group: None,
            consumer_id: None,
            delivery_count: None,
        };
        
        if let Err(e) = self.bus.send(&reply_to, &response_env).await {
            println!("[ERROR][{}] Failed to send response: {}", sid, e);
            return Err(e.into());
        }
        
        println!("[DEBUG][{}] Successfully sent response to {}", sid, reply_to);
        Ok(())
    }
    
    /// Get the session ID associated with a reply_to address, if any
    async fn get_session_for_reply_to(&self, reply_to: &str) -> Result<Option<String>> {
        let map = self.reply_to_session.lock().await;
        Ok(map.get(reply_to).cloned())
    }
    
    /// Map a reply_to address to a session ID
    async fn map_reply_to_session(&self, reply_to: &str, session_id: &str) -> Result<()> {
        let mut map = self.reply_to_session.lock().await;
        map.insert(reply_to.to_string(), session_id.to_string());
        Ok(())
    }
    
    /// Clean up session mappings when a session ends
    async fn cleanup_session_mapping(&self, session_id: &str) -> Result<()> {
        let mut map = self.reply_to_session.lock().await;
        map.retain(|_, v| v != session_id);
        Ok(())
    }
}