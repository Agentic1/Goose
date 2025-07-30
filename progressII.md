# Goose WebSocket to Bus Integration - Progress Update II

## Current Implementation Status (2025-07-29)

### Message Flow Architecture

```
+----------------+     +------------------+     +-----------------+     +------------------+
|                |     |                  |     |                 |     |                  |
|  Web Client    |<--->|  WebSocket       |<--->|  Bus Bridge     |<--->|  Redis Bus       |
|  (Browser)     | WS  |  Server (Axum)   |     |  (ag1_meta)     |     |  (Message Queue) |
|                |     |                  |     |                 |     |                  |
+----------------+     +------------------+     +--------+--------+     +------------------+
                                                       |
                                                       | CLI
                                                       v
                                                +------------------+
                                                |  Goose CLI       |
                                                |  (Agent)         |
                                                +------------------+
```

### Key Components & Improvements

1. **Message Format Standardization**
   - Implemented content normalization in `ag1_meta` crate
   - All messages now follow `{ "text": "message content" }` format
   - Fixed null content issues in message delegation

2. **Delegation Improvements**
   - Enhanced `delegate_with_opts` to enforce proper message formatting
   - Added robust error handling for message processing
   - Improved logging for debugging message flow

3. **Session Management**
   - Implemented JSONL-based session logging
   - Added offset tracking for efficient log reading
   - Fixed session initialization and monitoring

## Recent Fixes & Changes

### Fixed Issues
1. **Message Formatting**
   - Resolved issue where delegated messages were sent as raw strings
   - Ensured consistent `{ "text": ... }` format across all message types
   - Added content normalization at message receive/send boundaries

2. **Error Handling**
   - Added proper error handling for Redis operations
   - Improved error messages for debugging
   - Fixed session cleanup on disconnection

3. **Performance**
   - Optimized message serialization/deserialization
   - Reduced unnecessary logging in production
   - Improved connection handling

## Current Focus
- Ensuring reliable message delivery through Redis
- Handling session timeouts and reconnections
- Improving error recovery and logging

## Next Steps
1. Test message filtering to prevent self-reply loops
2. Add more robust error handling for edge cases
3. Implement proper cleanup of resources
4. Add metrics collection for monitoring

## Known Issues
- Occasional message format inconsistencies in delegated messages (under investigation)
- Session cleanup on application shutdown needs improvement
- Memory usage optimization needed for high-load scenarios
- [ ] Handle session timeouts and reconnections
- [ ] Add session-specific message routing

### Phase 3: Advanced Features
- [ ] Implement tool request/response handling
- [ ] Add message queuing for offline clients
- [ ] Implement message acknowledgments
- [ ] Add rate limiting and throttling

### Phase 4: Testing & Optimization
- [ ] Unit tests for core components
- [ ] Integration tests with WebSocket clients
- [ ] Load testing and performance optimization
- [ ] Security audit and hardening

## Next Steps

1. Implement the core bridge components
2. Set up basic WebSocket server with Bus integration
3. Test basic message passing between web client and Goose CLI
4. Add session management
5. Implement error handling and reconnection logic



----


diff --git a/crates/ag1_meta/src/lib.rs b/crates/ag1_meta/src/lib.rs
index fe7eda9226aad4acdb258d9bbc8f2aac7e412ab5..5a7c8f240263632e3bc3f77e3c96f5e5bf0651ee 100644
--- a/crates/ag1_meta/src/lib.rs
+++ b/crates/ag1_meta/src/lib.rs
@@ -1,54 +1,82 @@
+fn normalize_content(mut content: Value) -> Value {
+    let mut obj = if content.is_object() {
+        content.as_object_mut().unwrap().clone()
+    } else if content.is_null() {
+        let mut map = serde_json::Map::new();
+        map.insert("text".to_string(), json!(""));
+        map
+    } else {
+        let mut map = serde_json::Map::new();
+        let text = if content.is_string() {
+            content.as_str().unwrap_or("").to_string()
+        } else if content.is_number() || content.is_boolean() {
+            content.to_string()
+        } else {
+            serde_json::to_string(&content).unwrap_or_default()
+        };
+        map.insert("text".to_string(), json!(text));
+        map
+    };
+
+    if !obj.contains_key("text") {
+        obj.insert("text".to_string(), json!(""));
+    }
+
+    Value::Object(obj)
+}
+
 pub fn create_envelope(content: serde_json::Value, role: &str, meta: Option<serde_json::Value>) -> Envelope {
+    let content = normalize_content(content);
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
     }
 }
 mod registry;
 pub use registry::{Registry, AgentInfo};
 
 use anyhow::{bail, Result};
 use bus::{Bus, Envelope};
-use serde_json::json;
+use serde_json::{json, Value};
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
diff --git a/crates/ag1_meta/src/lib.rs b/crates/ag1_meta/src/lib.rs
index fe7eda9226aad4acdb258d9bbc8f2aac7e412ab5..5a7c8f240263632e3bc3f77e3c96f5e5bf0651ee 100644
--- a/crates/ag1_meta/src/lib.rs
+++ b/crates/ag1_meta/src/lib.rs
@@ -85,79 +113,51 @@ pub async fn delegate_with_opts(
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
     let cid = Uuid::new_v4().to_string();
     let now = Utc::now().to_rfc3339();
 
     println!("[AG1_meta] Getting tail_id for stream: {}", in_stream);
     let mut last_id = bus.tail_id(in_stream).await.unwrap_or_else(|e| {
         println!("[WARN] Failed to get tail_id: {}", e);
         "0-0".to_string()
     });
     println!("[AG1_meta] Starting to listen from id: {}", last_id);
 
     println!("[AG1_meta] Creating envelope");
-    // Ensure content is always an object with at least a text field
-    let mut content_obj = if content.is_object() {
-        content.as_object_mut().unwrap().clone()
-    } else if content.is_null() {
-        // If content is null, create a new object with empty text
-        let mut map = serde_json::Map::new();
-        map.insert("text".to_string(), json!(""));
-        map
-    } else {
-        // For non-object, non-null values, create an object with the value as text
-        let mut map = serde_json::Map::new();
-        let text = if content.is_string() {
-            content.as_str().unwrap_or("").to_string()
-        } else if content.is_number() || content.is_boolean() {
-            content.to_string()
-        } else {
-            // For arrays or other types, convert to JSON string
-            serde_json::to_string(&content).unwrap_or_default()
-        };
-        map.insert("text".to_string(), json!(text));
-        map
-    };
-    
-    // Ensure there's always a text field (double-check in case the object was empty)
-    if !content_obj.contains_key("text") {
-        content_obj.insert("text".to_string(), json!(""));
-    }
-    
-    let content = serde_json::Value::Object(content_obj);
+    let content = normalize_content(content);
     println!("[AG1_meta] Envelope content: {}", content);
 
     let env = Envelope {
         role: role.to_string(),
         content,
         session_code: None,
         agent_name: Some("ag1goose".into()),
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
     };
 
     println!("[AG1_meta] Sending envelope to stream: {}", out_stream);
