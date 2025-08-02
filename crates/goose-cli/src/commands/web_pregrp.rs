use anyhow::Result;
use bus::{Bus, Envelope};
use uuid;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures::{sink::SinkExt, stream::StreamExt};
use goose::agents::{Agent, AgentEvent}; 
use goose::message::Message as GooseMessage;
use goose::session;
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, error, info, warn};
use tokio::time::{sleep, Duration};

type SessionStore = Arc<RwLock<std::collections::HashMap<String, Arc<RwLock<Vec<GooseMessage>>>>>>;
type CancellationStore = Arc<RwLock<std::collections::HashMap<String, tokio::task::AbortHandle>>>;

#[derive(Clone, Debug)]
struct BusConfig {
    redis_url: String,
    inbox: String,
    agent_name: String,
    timeout_ms: u64,
}

#[derive(Clone)]
struct AppState {
    agent: Arc<Agent>,
    sessions: SessionStore,
    cancellations: CancellationStore,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum WebSocketMessage {
    #[serde(rename = "message")]
    Message {
        content: String,
        session_id: String,
        timestamp: i64,
    },
    #[serde(rename = "cancel")]
    Cancel { session_id: String },
    #[serde(rename = "response")]
    Response {
        content: String,
        role: String,
        timestamp: i64,
    },
    #[serde(rename = "tool_request")]
    ToolRequest {
        id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },
    #[serde(rename = "tool_response")]
    ToolResponse {
        id: String,
        result: serde_json::Value,
        is_error: bool,
    },
    #[serde(rename = "tool_confirmation")]
    ToolConfirmation {
        id: String,
        tool_name: String,
        arguments: serde_json::Value,
        needs_confirmation: bool,
    },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "thinking")]
    Thinking { message: String },
    #[serde(rename = "context_exceeded")]
    ContextExceeded { message: String },
    #[serde(rename = "cancelled")]
    Cancelled { message: String },
    #[serde(rename = "complete")]
    Complete { message: String },
}

pub async fn handle_web(port: u16, host: String, open: bool) -> Result<()> {
    // Setup logging
    crate::logging::setup_logging(Some("goose-web"), None)?;

    // Load config and create agent just like the CLI does
    let config = goose::config::Config::global();

    let provider_name: String = match config.get_param("GOOSE_PROVIDER") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("No provider configured. Run 'goose configure' first");
            std::process::exit(1);
        }
    };

    let model: String = match config.get_param("GOOSE_MODEL") {
        Ok(m) => m,
        Err(_) => {
            eprintln!("No model configured. Run 'goose configure' first");
            std::process::exit(1);
        }
    };

    let model_config = goose::model::ModelConfig::new(model.clone());

    // Create the agent
    let agent = Agent::new();
    let provider = goose::providers::create(&provider_name, model_config)?;
    agent.update_provider(provider).await?;

    // Load and enable extensions from config
    let extensions = goose::config::ExtensionConfigManager::get_all()?;
    for ext_config in extensions {
        if ext_config.enabled {
            if let Err(e) = agent.add_extension(ext_config.config.clone()).await {
                eprintln!(
                    "Warning: Failed to load extension {}: {}",
                    ext_config.config.name(),
                    e
                );
            }
        }
    }

    let state = AppState {
        agent: Arc::new(agent),
        sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        cancellations: Arc::new(RwLock::new(std::collections::HashMap::new())),
    };

    // Start Redis bus listener
    println!("Initializing Redis bus listener...");
    let bus_cfg = BusConfig {
        redis_url: std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://admin:UltraSecretRoot123@forge.agentic1.xyz:8081".into()),
        inbox: std::env::var("AG1_GOOSE_INBOX")
            .unwrap_or_else(|_| "AG1:agent:GooseAgent:inbox".into()),
        agent_name: std::env::var("AG1_AGENT_NAME").unwrap_or_else(|_| "GooseAgent".into()),
        timeout_ms: 1000,
    };
    println!("Bus configuration: {:?}", bus_cfg);
    
    let bus_state = state.clone();
    let bus_cfg_clone = bus_cfg.clone();
    
    // Spawn the bus listener task
    tokio::spawn(async move {
        println!("Spawning Redis bus listener task...");
        match run_bus_listener(bus_state, bus_cfg_clone).await {
            Ok(_) => println!("Bus listener exited successfully"),
            Err(e) => error!("Bus listener exited with error: {}", e),
        }
    });

    // Build router
    let app = Router::new()
        .route("/", get(serve_index))
        .route("/session/{session_name}", get(serve_session))
        .route("/ws", get(websocket_handler))
        .route("/api/health", get(health_check))
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/{session_id}", get(get_session))
        .route("/static/{*path}", get(serve_static))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;

    println!("\n🪿 Starting Goose web server");
    println!("   Provider: {} | Model: {}", provider_name, model);
    println!(
        "   Working directory: {}",
        std::env::current_dir()?.display()
    );
    println!("   Server: http://{}", addr);
    println!("   Press Ctrl+C to stop\n");

    if open {
        // Open browser
        let url = format!("http://{}", addr);
        if let Err(e) = webbrowser::open(&url) {
            eprintln!("Failed to open browser: {}", e);
        }
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn serve_index() -> Html<&'static str> {
    Html(include_str!("../../static/index.html"))
}

async fn serve_session(
    axum::extract::Path(session_name): axum::extract::Path<String>,
) -> Html<String> {
    let html = include_str!("../../static/index.html");
    // Inject the session name into the HTML so JavaScript can use it
    let html_with_session = html.replace(
        "<script src=\"/static/script.js\"></script>",
        &format!(
            "<script>window.GOOSE_SESSION_NAME = '{}';</script>\n    <script src=\"/static/script.js\"></script>",
            session_name
        )
    );
    Html(html_with_session)
}

async fn serve_static(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    match path.as_str() {
        "style.css" => (
            [("content-type", "text/css")],
            include_str!("../../static/style.css"),
        )
            .into_response(),
        "script.js" => (
            [("content-type", "application/javascript")],
            include_str!("../../static/script.js"),
        )
            .into_response(),
        "img/logo_dark.png" => (
            [("content-type", "image/png")],
            include_bytes!("../../../../documentation/static/img/logo_dark.png").to_vec(),
        )
            .into_response(),
        "img/logo_light.png" => (
            [("content-type", "image/png")],
            include_bytes!("../../../../documentation/static/img/logo_light.png").to_vec(),
        )
            .into_response(),
        _ => (http::StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

async fn health_check() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "goose-web"
    }))
}

async fn list_sessions() -> Json<serde_json::Value> {
    match session::list_sessions() {
        Ok(sessions) => {
            let session_info: Vec<serde_json::Value> = sessions
                .into_iter()
                .filter_map(|(name, path)| {
                    session::read_metadata(&path).ok().map(|metadata| {
                        serde_json::json!({
                            "name": name,
                            "path": path,
                            "description": metadata.description,
                            "message_count": metadata.message_count,
                            "working_dir": metadata.working_dir
                        })
                    })
                })
                .collect();

            Json(serde_json::json!({
                "sessions": session_info
            }))
        }
        Err(e) => Json(serde_json::json!({
            "error": e.to_string()
        })),
    }
}
async fn get_session(
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let session_file = match session::get_path(session::Identifier::Name(session_id)) {
        Ok(path) => path,
        Err(e) => {
            return Json(serde_json::json!({
                "error": format!("Invalid session ID: {}", e)
            }));
        }
    };

    let error_response = |e: Box<dyn std::error::Error>| {
        Json(serde_json::json!({
            "error": e.to_string()
        }))
    };

    match session::read_messages(&session_file) {
        Ok(messages) => match session::read_metadata(&session_file) {
            Ok(metadata) => Json(serde_json::json!({
                "metadata": metadata,
                "messages": messages
            })),
            Err(e) => error_response(e.into()),
        },
        Err(e) => error_response(e.into()),
    }
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (sender, mut receiver) = socket.split();
    let sender = Arc::new(Mutex::new(sender));

    while let Some(msg) = receiver.next().await {
        if let Ok(msg) = msg {
            match msg {
                Message::Text(text) => {
                    println!("WebSocket message received: {}", text);
                    println!("WebSocket message length: {} bytes", text.len());
                    
                    match serde_json::from_str::<WebSocketMessage>(&text.to_string()) {
                        Ok(WebSocketMessage::Message {
                            content,
                            session_id,
                            ..
                        }) => {
                            println!("WebSocket Message parsed successfully");
                            println!("WebSocket Content: {}", content);
                            println!("WebSocket Session ID: {}", session_id);
                            // Get session file path from session_id
                            let session_file = match session::get_path(session::Identifier::Name(
                                session_id.clone(),
                            )) {
                                Ok(path) => path,
                                Err(e) => {
                                    tracing::error!("Failed to get session path: {}", e);
                                    continue;
                                }
                            };

                            // Get or create session in memory (for fast access during processing)
                            let session_messages = {
                                let sessions = state.sessions.read().await;
                                if let Some(session) = sessions.get(&session_id) {
                                    session.clone()
                                } else {
                                    drop(sessions);
                                    let mut sessions = state.sessions.write().await;

                                    // Load existing messages from JSONL file if it exists
                                    let existing_messages = session::read_messages(&session_file)
                                        .unwrap_or_else(|_| Vec::new());

                                    //let new_session = Arc::new(Mutex::new(existing_messages));
                                    let new_session = Arc::new(RwLock::new(existing_messages));
                                    sessions.insert(session_id.clone(), new_session.clone());
                                    new_session
                                }
                            };

                            // Clone sender for async processing
                            let sender_clone = sender.clone();
                            let agent = state.agent.clone();

                            // Process message in a separate task to allow streaming
                            let task_handle = tokio::spawn(async move {
                                println!("Starting message processing task");
                                println!("Content to process: {}", content);
                                println!("Session file: {}", session_file.display());
                                
                                let result = process_message_streaming(
                                    &agent,
                                    session_messages,
                                    session_file,
                                    content,
                                    sender_clone,
                                )
                                .await;

                                if let Err(e) = result {
                                    error!("Error processing message: {}", e);
                                }
                            });

                            // Store the abort handle
                            {
                                let mut cancellations = state.cancellations.write().await;
                                cancellations
                                    .insert(session_id.clone(), task_handle.abort_handle());
                            }

                            // Wait for task completion and handle abort
                            let sender_for_abort = sender.clone();
                            let session_id_for_cleanup = session_id.clone();
                            let cancellations_for_cleanup = state.cancellations.clone();

                            tokio::spawn(async move {
                                match task_handle.await {
                                    Ok(_) => {
                                        // Task completed normally
                                    }
                                    Err(e) if e.is_cancelled() => {
                                        // Task was aborted
                                        let mut sender = sender_for_abort.lock().await;
                                        let _ = sender
                                            .send(Message::Text(
                                                serde_json::to_string(
                                                    &WebSocketMessage::Cancelled {
                                                        message: "Operation cancelled by user"
                                                            .to_string(),
                                                    },
                                                )
                                                .unwrap()
                                                .into(),
                                            ))
                                            .await;
                                    }
                                    Err(e) => {
                                        error!("Task error: {}", e);
                                    }
                                }

                                // Clean up cancellation token
                                {
                                    let mut cancellations = cancellations_for_cleanup.write().await;
                                    cancellations.remove(&session_id_for_cleanup);
                                }
                            });
                        }
                        Ok(WebSocketMessage::Cancel { session_id }) => {
                            // Cancel the active operation for this session
                            let abort_handle = {
                                let mut cancellations = state.cancellations.write().await;
                                cancellations.remove(&session_id)
                            };

                            if let Some(handle) = abort_handle {
                                handle.abort();

                                // Send cancellation confirmation
                                let mut sender = sender.lock().await;
                                let _ = sender
                                    .send(Message::Text(
                                        serde_json::to_string(&WebSocketMessage::Cancelled {
                                            message: "Operation cancelled".to_string(),
                                        })
                                        .unwrap()
                                        .into(),
                                    ))
                                    .await;
                            }
                        }
                        Ok(_) => {
                            // Ignore other message types
                        }
                        Err(e) => {
                            error!("Failed to parse WebSocket message: {}", e);
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        } else {
            break;
        }
    }
}

async fn process_message_streaming(
    agent: &Agent,
    //session_messages: Arc<Mutex<Vec<GooseMessage>>>,
    session_messages: Arc<RwLock<Vec<GooseMessage>>>,
    session_file: std::path::PathBuf,
    content: String,
    sender: Arc<Mutex<futures::stream::SplitSink<WebSocket, Message>>>,
) -> Result<()> {
    use futures::StreamExt;
    use goose::agents::SessionConfig;
    use goose::message::MessageContent;
    use goose::session;

    println!("[Web] Received content: {}", content);
    println!("[Web] Content length: {} bytes", content.len());
    
    // Create a user message
    let user_message = GooseMessage::user().with_text(content.clone());
    println!("[Web] Created user message with content: {}", content);

    // Get existing messages from session and add the new user message
    let mut messages = {
        println!("[Web] Acquiring session messages write lock");
        let mut session_msgs = session_messages.write().await;
        println!("[Web] Session has {} existing messages", session_msgs.len());
        session_msgs.push(user_message.clone());
        println!("[Web] Added user message to session");
        session_msgs.clone()
    };

    // Persist messages to JSONL file with provider for automatic description generation
    let provider = agent.provider().await;
    if provider.is_err() {
        let error_msg = "I'm not properly configured yet. Please configure a provider through the CLI first using `goose configure`.".to_string();
        let mut sender = sender.lock().await;
        let _ = sender
            .send(Message::Text(
                serde_json::to_string(&WebSocketMessage::Response {
                    content: error_msg,
                    role: "assistant".to_string(),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                })
                .unwrap()
                .into(),
            ))
            .await;
        return Ok(());
    }

    let provider = provider.unwrap();
    let working_dir = Some(std::env::current_dir()?);
    session::persist_messages(
        &session_file,
        &messages,
        Some(provider.clone()),
        working_dir.clone(),
    )
    .await?;

    let session_config = SessionConfig {
        id: session::Identifier::Path(session_file.clone()),
        working_dir: std::env::current_dir()?,
        schedule_id: None,
        execution_mode: None,
        max_turns: None,
        retry_config: None,
    };

    match agent.reply(&messages, Some(session_config), None).await {
        Ok(mut stream) => {
            println!("[Web] Successfully got response stream from agent");
            while let Some(result) = stream.next().await {
                println!("[Web] Got result from stream");
                match result {
                    Ok(AgentEvent::Message(message)) => {
                        println!("[Web] Received agent message with {} content items", message.content.len());
                        // Add message to our session
                        {
                            println!("[Web] Acquiring session messages write lock");
                            let mut session_msgs = session_messages.write().await;
                            println!("[Web] Session has {} messages before adding", session_msgs.len());
                            session_msgs.push(message.clone());
                            println!("[Web] Added message to session, now has {} messages", session_msgs.len());
                        }

                        // Persist messages to JSONL file (no provider needed for assistant messages)
                        let current_messages = {
                            //let session_msgs = session_messages.lock().await;
                            let session_msgs = session_messages.read().await;
                            session_msgs.clone()
                        };
                        session::persist_messages(
                            &session_file,
                            &current_messages,
                            None,
                            working_dir.clone(),
                        )
                        .await?;
                        // Handle different message content types
                        for content in &message.content {
                            println!("[Web] Processing message content: {:?}", content);
                            match content {
                                MessageContent::Text(text) => {
                                    println!("[Web] Found text content: {}", text.text);
                                    // Send the text response
                                    let mut sender = sender.lock().await;
                                    let _ = sender
                                        .send(Message::Text(
                                            serde_json::to_string(&WebSocketMessage::Response {
                                                content: text.text.clone(),
                                                role: "assistant".to_string(),
                                                timestamp: chrono::Utc::now().timestamp_millis(),
                                            })
                                            .unwrap()
                                            .into(),
                                        ))
                                        .await;
                                }
                                MessageContent::ToolRequest(req) => {
                                    // Send tool request notification
                                    let mut sender = sender.lock().await;
                                    if let Ok(tool_call) = &req.tool_call {
                                        let _ = sender
                                            .send(Message::Text(
                                                serde_json::to_string(
                                                    &WebSocketMessage::ToolRequest {
                                                        id: req.id.clone(),
                                                        tool_name: tool_call.name.clone(),
                                                        arguments: tool_call.arguments.clone(),
                                                    },
                                                )
                                                .unwrap()
                                                .into(),
                                            ))
                                            .await;
                                    }
                                }
                                MessageContent::ToolResponse(_resp) => {
                                    // Tool responses are already included in the complete message stream
                                    // and will be persisted to session history. No need to send separate
                                    // WebSocket messages as this would cause duplicates.
                                }
                                MessageContent::ToolConfirmationRequest(confirmation) => {
                                    // Send tool confirmation request
                                    let mut sender = sender.lock().await;
                                    let _ = sender
                                        .send(Message::Text(
                                            serde_json::to_string(
                                                &WebSocketMessage::ToolConfirmation {
                                                    id: confirmation.id.clone(),
                                                    tool_name: confirmation.tool_name.clone(),
                                                    arguments: confirmation.arguments.clone(),
                                                    needs_confirmation: true,
                                                },
                                            )
                                            .unwrap()
                                            .into(),
                                        ))
                                        .await;

                                    // For now, auto-approve in web mode
                                    // TODO: Implement proper confirmation UI
                                    agent.handle_confirmation(
                                        confirmation.id.clone(),
                                        goose::permission::PermissionConfirmation {
                                            principal_type: goose::permission::permission_confirmation::PrincipalType::Tool,
                                            permission: goose::permission::Permission::AllowOnce,
                                        }
                                    ).await;
                                }
                                MessageContent::Thinking(thinking) => {
                                    // Send thinking indicator
                                    let mut sender = sender.lock().await;
                                    let _ = sender
                                        .send(Message::Text(
                                            serde_json::to_string(&WebSocketMessage::Thinking {
                                                message: thinking.thinking.clone(),
                                            })
                                            .unwrap()
                                            .into(),
                                        ))
                                        .await;
                                }
                                MessageContent::ContextLengthExceeded(msg) => {
                                    // Send context exceeded notification
                                    let mut sender = sender.lock().await;
                                    let _ = sender
                                        .send(Message::Text(
                                            serde_json::to_string(
                                                &WebSocketMessage::ContextExceeded {
                                                    message: msg.msg.clone(),
                                                },
                                            )
                                            .unwrap()
                                            .into(),
                                        ))
                                        .await;

                                    // For now, auto-summarize in web mode
                                    // TODO: Implement proper UI for context handling
                                    let (summarized_messages, _) =
                                        agent.summarize_context(&messages).await?;
                                    messages = summarized_messages;
                                }
                                _ => {
                                    // Handle other message types as needed
                                }
                            }
                        }
                    }
                    Ok(AgentEvent::McpNotification(_notification)) => {
                        // Handle MCP notifications if needed
                        // For now, we'll just log them
                        tracing::error!("Received MCP notification in web interface");
                    }
                    Ok(AgentEvent::ModelChange { model, mode }) => {
                        // Log model change
                        tracing::error!("Model changed to {} in {} mode", model, mode);
                    }

                    Err(e) => {
                        error!("Error in message stream: {}", e);
                        let mut sender = sender.lock().await;
                        let _ = sender
                            .send(Message::Text(
                                serde_json::to_string(&WebSocketMessage::Error {
                                    message: format!("Error: {}", e),
                                })
                                .unwrap()
                                .into(),
                            ))
                            .await;
                        break;
                    }
                }
            }
        }
        Err(e) => {
            error!("Error calling agent: {}", e);
            let mut sender = sender.lock().await;
            let _ = sender
                .send(Message::Text(
                    serde_json::to_string(&WebSocketMessage::Error {
                        message: format!("Error: {}", e),
                    })
                    .unwrap()
                    .into(),
                ))
                .await;
        }
    }

    // Send completion message
    let mut sender = sender.lock().await;
    let _ = sender
        .send(Message::Text(
            serde_json::to_string(&WebSocketMessage::Complete {
                message: "Response complete".to_string(),
            })
            .unwrap()
            .into(),
        ))
        .await;

    Ok(())
}

// Add webbrowser dependency for opening browser
use webbrowser;

async fn run_bus_listener(state: AppState, cfg: BusConfig) -> Result<()> {
    use tokio::time::{sleep, Duration};
    let mut backoff = 1u64;
    
    println!("🚀 Starting Redis bus listener with config: {:?}", cfg);
    
    loop {
        println!("Attempting to connect to Redis at {}...", cfg.redis_url);
        let bus = match Bus::new(&cfg.redis_url) {
            Ok(bus) => {
                println!("✅ Successfully connected to Redis at {}", cfg.redis_url);
                bus
            },
            Err(e) => {
                error!("❌ Failed to connect to Redis at {}: {}", cfg.redis_url, e);
                println!("Retrying in {} seconds...", backoff);
                sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(30);
                continue;
            }
        };

        println!("starting bus listener");
        // Start reading from the beginning of the stream (0-0) instead of new messages only ($)
        let mut last_id = "0-0".to_string();
        println!("📡 Listening for messages on stream: {}", cfg.inbox);
        
        // Debug: Print Redis connection details
        println!("🔌 Redis URL: {}", cfg.redis_url);
        println!("🔌 Inbox stream: {}", cfg.inbox);
        println!("🔌 Timeout: {}ms", cfg.timeout_ms);
        
        loop {
            println!("\n--- New Poll Cycle ---");
            println!("⏳ Waiting for message on stream: {}", cfg.inbox);
            println!("🔍 Last message ID: {}", last_id);
            
            let start = std::time::Instant::now();
            let result = bus.recv_block(&cfg.inbox, &last_id, cfg.timeout_ms).await;
            let elapsed = start.elapsed();
            
            println!("⏱️  Redis call took: {:?}", elapsed);
            println!("📦 Received result: {:?}", result);
            
            match result {
                Ok(Some(env)) => {
                    println!("📩 Received message on stream: {}", cfg.inbox);
                    println!("Message envelope: {:?}", env);
                    println!("Envelope content: {:?}", env.content);
                    
                    backoff = 1;
                    if let Some(id) = &env.envelope_id {
                        println!("Updating last message ID: {}", id);
                        last_id = id.clone();
                    }
                    
                    if env.role != "user" {
                        println!("Skipping non-user message with role: {}", env.role);
                        continue;
                    }
                    println!("📝 Processing message from envelope");
                    println!("📦 Envelope content type: {:?}", env.content); // Add content type logging
                    println!("📦 Envelope content raw: {:?}", env.content); // Add raw content logging
                    
                    // Normalize the content to ensure it has a text field
                    use serde_json::{json, Value};
                    let normalized_content = match env.content {
                        Value::String(s) => {
                            println!("📝 Found string content: {}", s);
                            json!({ "text": s })
                        },
                        Value::Object(mut map) => {
                            let keys: Vec<_> = map.keys().collect();
                            println!("📝 Found object content with keys: {:?}", keys);
                            
                            // If there's no text field, add one with the first string value or empty string
                            if !map.contains_key("text") {
                                let first_string = map.values().find(|v| v.is_string())
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                map.insert("text".to_string(), Value::String(first_string.to_string()));
                            }
                            
                            Value::Object(map)
                        },
                        Value::Null => {
                            println!("⚠️  Found null content, using empty text");
                            json!({ "text": "" })
                        },
                        other => {
                            println!("⚠️  Unknown content type, converting to text: {:?}", other);
                            json!({ "text": other.to_string() })
                        }
                    };
                    
                    // Extract the text content for logging
                    let text = normalized_content["text"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    println!("📝 Normalized text content: {}", text);
                    
                    if text.is_empty() {
                        warn!("Received empty message content");
                    }
                    
                    let sid = env.session_code.clone().unwrap_or_else(|| "default".into());
                    let reply_to = env.reply_to.clone().unwrap_or_else(|| cfg.inbox.clone());
                    
                    println!("📋 Session ID: {}, Reply To: {}", sid, reply_to);
                    
                    let session_messages = {
                        println!("🔒 Acquiring write lock on sessions");
                        let mut sessions = state.sessions.write().await;
                        println!("🔍 Looking up or creating session: {}", sid);
                        let session = sessions
                            .entry(sid.clone())
                            .or_insert_with(|| {
                                println!("➕ Created new session: {}", sid);
                                Arc::new(RwLock::new(Vec::new()))
                            })
                            .clone();
                        println!("🔓 Released sessions lock");
                        session
                    };
                    
                    println!("🔄 Processing message through agent");
                    match process_bus_message(&state.agent, session_messages, text).await {
                        Ok(response) => {
                            println!("✅ Successfully processed message");
                            
                            let reply_env = Envelope {
                                role: "assistant".into(),
                                content: serde_json::json!({ "text": response }),
                                session_code: Some(sid),
                                agent_name: Some(cfg.agent_name.clone()),
                                usage: serde_json::json!({}),
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
                                meta: serde_json::json!({}),
                                envelope_id: env.correlation_id.clone(),
                                correlation_id: env.correlation_id.clone(),
                            };
                            
                            println!("📤 Sending response to: {}", reply_to);
                            println!("Response envelope: {:?}", reply_env);
                            
                            match bus.send(&reply_to, &reply_env).await {
                                Ok(_) => println!("✅ Successfully sent response to {}", reply_to),
                                Err(e) => error!("❌ Failed to send response to {}: {}", reply_to, e),
                            };
                        }
                        Err(e) => {
                            error!("bus message error: {}", e);
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    error!("bus recv error: {}", e);
                    break;
                }
            }
        }
        println!("bus listener reconnecting in {}s", backoff);
        sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

async fn process_bus_message(
    agent: &Agent,
    session_messages: Arc<RwLock<Vec<GooseMessage>>>,
    content: String,
) -> Result<String> {
    use futures::StreamExt;
    use goose::agents::SessionConfig;

    println!("📨 Processing message with content: {}", &content[..content.len().min(100)].to_string());

    let user_message = GooseMessage::user().with_text(content.clone());
    
    // Add user message to session
    {
        println!("🔒 Acquiring write lock for session messages");
        let mut msgs = session_messages.write().await;
        println!("✍️  Adding user message to session ({} messages total)", msgs.len() + 1);
        msgs.push(user_message);
        println!("🔓 Released write lock");
    }
    
    // Get a read lock to clone messages
    let messages = { 
        println!("📋 Cloning messages for processing");
        session_messages.read().await.clone() 
    };
    
    println!("⚙️  Creating session configuration");
    let session_config = SessionConfig {
        id: session::Identifier::Name("bus".into()),
        working_dir: std::env::current_dir()?,
        schedule_id: None,
        execution_mode: None,
        max_turns: None,
        retry_config: None,
    };
    
    println!("🤖 Sending message to agent");
    let mut stream = match agent.reply(&messages, Some(session_config), None).await {
        Ok(stream) => {
            println!("✅ Successfully got response stream from agent");
            stream
        },
        Err(e) => {
            error!("❌ Failed to get response from agent: {}", e);
            return Err(e.into());
        }
    };
    
    println!("📥 Processing agent response stream");
    let mut response = String::new();
    let mut message_count = 0;
    
    while let Some(item) = stream.next().await {
        message_count += 1;
        match item {
            Ok(AgentEvent::Message(msg)) => {
                println!("📝 Processing agent message chunk #{}", message_count);
                for c in &msg.content {
                    if let goose::message::MessageContent::Text(t) = c {
                        response.push_str(&t.text);
                        println!("📝 Appended text chunk ({} chars)", t.text.len());
                    }
                }
                
                // Add assistant message to session
                println!("🔒 Acquiring write lock to save assistant message");
                let mut msgs = session_messages.write().await;
                msgs.push(msg);
                println!("💾 Saved assistant message to session ({} messages total)", msgs.len());
                println!("🔓 Released write lock");
            },
            Ok(event) => {
                println!("ℹ️  Received agent event: {:?}", event);
            },
            Err(e) => {
                error!("❌ Error in agent response stream: {}", e);
                return Err(e.into());
            }
        }
    }
    
    println!("✅ Finished processing agent response ({} events, {} response chars)", 
          message_count, response.len());
    
    if response.is_empty() {
        warn!("⚠️  Empty response from agent");
    } else {
        let truncated = if response.len() > 100 {
            format!("{}...", &response[..100])
        } else {
            response.clone()
        };
        println!("📝 Final response (first 100 chars): {}", truncated);
    }
    
    Ok(response)
}