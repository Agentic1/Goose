use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::time::{timeout as tokio_timeout, Instant};
use tokio_stream::StreamExt;
use tokio_util::codec::{FramedRead, LinesCodec};
use tracing::{debug, error, info, warn};

use crate::config::Config;

/// Represents a live Goose CLI session process.
pub struct GooseSession {
    pub sid: String,
    pub process: Child,
    pub stdin: Option<ChildStdin>,
    pub is_ready: Arc<tokio::sync::Notify>,
    pub last_offset: u64,
    jsonl_path: PathBuf,
}

/// Get the path to a session's JSONL log file
fn session_log_path(sid: &str) -> PathBuf {
    // ~/.local/share/goose/sessions/<sid>.jsonl  (Unix)
    // Lowercase filename is typical; we use lowercase for safety.
    let home_dir = std::env::var("HOME")
        .unwrap_or_else(|_| ".".to_string());
    
    let mut p = PathBuf::from(home_dir);
    p.push(".local");
    p.push("share");
    p.push("goose");
    p.push("sessions");
    
    // Create the directory if it doesn't exist
    if !p.exists() {
        if let Err(e) = std::fs::create_dir_all(&p) {
            error!("Failed to create sessions directory at {}: {}", p.display(), e);
        }
    }
    
    p.push(format!("{}.jsonl", sid.to_lowercase()));
    p
}

impl GooseSession {
    /// Send user input to the Goose CLI process as a properly formatted envelope
    /// 
    /// Formats the input text as an envelope according to the AG1 message specification:
    /// {
    ///     "role": "user",
    ///     "content": { "text": "..." },
    ///     "session_code": null,
    ///     "agent_name": "ag1goose",
    ///     "usage": {},
    ///     "billing_hint": null,
    ///     "trace": [],
    ///     "user_id": null,
    ///     "task_id": null,
    ///     "target": null,
    ///     "reply_to": "AG1:agent:GooseAgent:inbox",
    ///     "envelope_type": "message",
    ///     "tools_used": [],
    ///     "auth_signature": null,
    ///     "timestamp": "...",
    ///     "headers": {},
    ///     "meta": { "priority": "normal" }
    /// }
    /// 
    /// The envelope is serialized to JSON and sent to Goose CLI via stdin.
    pub async fn send_user(&mut self, text: &str) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        use serde_json::json;
        
        let text = text.trim_end(); // Remove any trailing newlines
        let envelope = json!({
            "role": "user",
            "content": {"text": text},
            "session_code": null,
            "agent_name": "ag1goose",
            "usage": {},
            "billing_hint": null,
            "trace": [],
            "user_id": null,
            "task_id": null,
            "target": null,
            "reply_to": format!("AG1:agent:GooseAgent:inbox"),
            "envelope_type": "message",
            "tools_used": [],
            "auth_signature": null,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "headers": {},
            "meta": {
                "priority": "normal"
            }
        });
        
        let message = format!("{}\n", envelope.to_string());
        
        info!("[{}] Sending input to Goose ({} chars): {}", 
              self.sid, message.len(), text);
        
        // Get mutable reference to stdin or return error if None
        let stdin = self.stdin.as_mut()
            .ok_or_else(|| anyhow!("No stdin handle available"))?;
        
        // Write the formatted envelope
        stdin.write_all(message.as_bytes()).await
            .map_err(|e| anyhow!("Failed to write to stdin: {}", e))?;
        
        // Flush to ensure the input is sent
        stdin.flush().await
            .map_err(|e| anyhow!("Failed to flush stdin: {}", e))?;
            
        info!("[{}] Input sent successfully", self.sid);
        Ok(())
    }
    pub async fn start(cfg: &Config, sid: String) -> Result<Self> {
        debug!(session_id = %sid, "Starting new Goose session");
        let start_time = Instant::now();
        
        // Ensure goose binary is available
        debug!(goose_bin = %cfg.goose_bin, "Looking for goose binary");
        let goose_bin = which::which(&cfg.goose_bin)
            .map_err(|_| {
                let err = anyhow!("goose binary not found on PATH: {}", cfg.goose_bin);
                error!(error = %err, "Failed to find goose binary");
                err
            })?;
            
        debug!(path = %goose_bin.display(), "Found goose binary");

        let mut cmd = Command::new(&goose_bin);
        
        // Start an interactive session with the given session ID
        cmd.arg("session")
           .arg("--name").arg(&sid);
           
        // Enable developer builtins by default
        cmd.arg("--with-builtin").arg("developer");
        
        // Set environment variables needed by the MCP server
        cmd.env("AG1_GOOSE_INBOX", "AG1:agent:GooseAgent:inbox")
           .env("REDIS_URL", "redis://admin:UltraSecretRoot123@forge.agentic1.xyz:8081");
        
        // Log the command being executed
        debug!("Command prepared with explicit extension path");
        
        // Configure process I/O with proper error handling
        cmd.kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        info!(sid = %sid, "starting goose session");
        debug!("Spawning Goose process...");
        
        // Log the full command being executed with all arguments
        let program = cmd.as_std().get_program().to_string_lossy().to_string();
        let args: Vec<String> = cmd.as_std().get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
            
        let command_line = format!(
            "{} {}",
            program,
            args.join(" ")
        );
        
        // Log the full command with all arguments
        debug!(command = %command_line, "Executing command");
        
        // Also log the current working directory
        if let Some(cwd) = cmd.as_std().get_current_dir() {
            debug!(cwd = %cwd.display(), "Current working directory");
        }
        
        // Log environment variables that start with GOOSE_ or AG1_
        for (key, value) in cmd.as_std().get_envs() {
            if let (Some(k), Some(v)) = (key.to_str(), value.and_then(|v| v.to_str())) {
                if k.starts_with("GOOSE_") || k.starts_with("AG1_") {
                    debug!(env = %k, value = %v, "Environment variable");
                }
            }
        }
        
        // Log environment variables for debugging
        if tracing::enabled!(tracing::Level::DEBUG) {
            for (key, value) in cmd.as_std().get_envs() {
                let key_str = key.to_string_lossy();
                let value_str = value.map(|v| v.to_string_lossy().to_string())
                    .unwrap_or_else(|| "<not set>".to_string());
                debug!(env = %key_str, value = %value_str, "Environment variable");
            }
        }
        
        // Spawn the child process with enhanced error handling
        let mut child = match cmd.spawn() {
            Ok(child) => {
                if let Some(pid) = child.id() {
                    debug!(pid, "Successfully spawned Goose process");
                }
                child
            }
            Err(e) => {
                let error_msg = format!("Failed to spawn goose process: {}", e);
                error!(
                    error = %error_msg,
                    program = %cmd.as_std().get_program().to_string_lossy(),
                    args = ?cmd.as_std().get_args().collect::<Vec<_>>(),
                    "Failed to spawn process"
                );
                
                // Provide more specific error messages for common issues
                let detailed_error = if e.kind() == std::io::ErrorKind::NotFound {
                    format!("Command not found: {}", cmd.as_std().get_program().to_string_lossy())
                } else if e.kind() == std::io::ErrorKind::PermissionDenied {
                    format!("Permission denied when executing: {}", cmd.as_std().get_program().to_string_lossy())
                } else {
                    error_msg
                };
                
                return Err(anyhow!(detailed_error));
            }
        };
        
        // Check if the process is still running
        if let Some(exit_status) = child.try_wait()? {
            let err = anyhow!("goose process exited immediately with status: {:?}", exit_status);
            error!(%err, "Process exited immediately");
            return Err(err);
        }
        
        // Get handles to stdin/stdout/stderr
        let stdin = child.stdin.take()
            .ok_or_else(|| anyhow!("Failed to get stdin handle from goose process"))?;
            
        let stdout = child.stdout.take()
            .ok_or_else(|| anyhow!("Failed to get stdout handle from goose process"))?;
            
        let stderr = child.stderr.take()
            .ok_or_else(|| anyhow!("Failed to get stderr handle from goose process"))?;
        
        // Create readiness notifier
        let is_ready = Arc::new(tokio::sync::Notify::new());
        
        // Spawn stderr reader task
        let stderr_sid = sid.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            
            while let Ok(Some(line)) = lines.next_line().await {
                // Filter out non-critical extension errors
                if line.contains("failed to load extension") && line.contains("goose_agent") {
                    debug!(session_id = %stderr_sid, "Non-critical extension error (suppressed): {}", line);
                    continue;
                }
                // Log other stderr lines as warnings
                warn!(session_id = %stderr_sid, "{}", line);
            }
        });
        
        // Spawn stdout reader task
        let stdout_sid = sid.clone();
        let ready_notifier = is_ready.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            
            while let Ok(Some(line)) = lines.next_line().await {
                // Check for session ready signal
                if line.contains("Session ready") {
                    info!(session_id = %stdout_sid, "Goose session is ready");
                    ready_notifier.notify_one();
                }
                
                // Log other stdout lines as debug
                debug!(session_id = %stdout_sid, "{}", line);
                // Check for readiness signal (Goose prints "logging to <path>" when ready)
                if line.contains("logging to") {
                    info!("[{}] Session is ready", stdout_sid);
                    ready_notifier.notify_one();
                } else if line.contains(" WARN ") || line.contains(" ERROR ") {
                    // Redirect warnings and errors to stderr
                    eprintln!("[{}] {}", stdout_sid, line);
                }
            }
        });
        
        // Wait for the JSONL file to be created
        let jsonl_path = session_log_path(&sid);
        let timeout = std::time::Duration::from_secs(10);
        let start = std::time::Instant::now();
        
        while !jsonl_path.exists() {
            if start.elapsed() > timeout {
                return Err(anyhow!("Timeout waiting for JSONL file to be created"));
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        
        info!("[{}] Session created and JSONL file found at {:?}", sid, jsonl_path);
        
        // Create the session
        let mut session = Self {
            sid: sid.clone(),
            process: child,
            stdin: Some(stdin),
            is_ready,
            last_offset: 0,
            jsonl_path: session_log_path(&sid),
        };
        
        // Start monitoring the child process
        if let Err(e) = session.monitor().await {
            error!("[{}] Failed to start process monitor: {}", session.sid, e);
            return Err(anyhow!("Failed to start process monitor: {}", e));
        }
        
        Ok(session)
    }
    /// Wait for a reply from the Goose CLI by reading the JSONL file
    /// using efficient async I/O with proper line handling and timeouts.
    /// 
    /// Returns the assistant's message and the new file offset.
    /// Get the current file offset for this session
    pub fn get_last_offset(&self) -> u64 {
        self.last_offset
    }
    
    pub fn update_offset(&mut self, offset: u64) {
        self.last_offset = offset;
    }

    /// Wait until the Goose CLI session signals readiness.
    ///
    /// This waits for the internal `is_ready` notifier to fire with the provided
    /// timeout. Returns an error if the timeout expires before the session is
    /// ready.
    pub async fn wait_ready(&self, timeout: Duration) -> Result<()> {
        tokio::select! {
            _ = self.is_ready.notified() => Ok(()),
            _ = tokio::time::sleep(timeout) => {
                Err(anyhow!("Timeout waiting for session to become ready"))
            }
        }
    }
    
    /// Wait for an assistant response from the JSONL log file
    /// Returns the assistant's message and the new file offset
    pub async fn wait_assistant_jsonl(
        &self,
        timeout_ms: u64,
        start_offset: u64,
    ) -> Result<(String, u64)> {
        let path = &self.jsonl_path;
        let start_time = Instant::now();
        let timeout_duration = Duration::from_millis(timeout_ms);
        let mut current_offset = start_offset;
        let mut consecutive_errors = 0;
        const MAX_CONSECUTIVE_ERRORS: u32 = 5;
        
        debug!(
            session_id = %self.sid,
            path = %path.display(),
            start_offset,
            timeout_ms,
            "Waiting for assistant response in JSONL file"
        );

        // Wait for the file to exist with a timeout
        while !path.exists() {
            if start_time.elapsed() > timeout_duration {
                return Err(anyhow!("Timeout waiting for session log file to appear: {}", path.display()));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Open the file with retry logic
        let mut file = match File::open(&path).await {
            Ok(file) => file,
            Err(e) => {
                error!(
                    session_id = %self.sid,
                    path = %path.display(),
                    error = %e,
                    "Failed to open JSONL file"
                );
                return Err(anyhow!("Failed to open JSONL file: {}", e));
            }
        };

        // Seek to the start offset
        if let Err(e) = file.seek(std::io::SeekFrom::Start(start_offset)).await {
            error!(
                session_id = %self.sid,
                offset = start_offset,
                error = %e,
                "Failed to seek in JSONL file"
            );
            return Err(anyhow!("Failed to seek in JSONL file: {}", e));
        }

        let mut reader = FramedRead::new(file, LinesCodec::new());
        let mut last_file_size = start_offset;

        // Buffer to hold partial JSON objects read from the log
        let mut buffer = String::new();

        // Read lines until we find an assistant message or timeout
        loop {
            // Check for timeout
            let elapsed = start_time.elapsed();
            if elapsed > timeout_duration {
                debug!(
                    session_id = %self.sid,
                    elapsed_ms = elapsed.as_millis(),
                    "Timeout waiting for assistant response"
                );
                break;
            }

            match tokio_timeout(
                timeout_duration.saturating_sub(elapsed),
                reader.next()
            ).await {
                Ok(Some(Ok(line))) => {
                    consecutive_errors = 0; // Reset error counter on successful read
                    current_offset += line.len() as u64 + 1; // +1 for newline
                    
                    debug!(
                        session_id = %self.sid,
                        line_content = line,
                        "Read line from JSONL"
                    );
                    
                    // Filter out MCP client warnings
                    if line.contains("mcp_client::transport::stdio") {
                        debug!(
                            session_id = %self.sid,
                            "Skipping MCP client warning message"
                        );
                        continue;
                    }

                    
                    buffer.push_str(&line);
                    
                    // Try to parse the buffer
                    match serde_json::from_str::<serde_json::Value>(&buffer) {
                        Ok(json) => {
                            // Clear buffer if we got a complete JSON object
                            buffer.clear();
                            
                            // Handle tool responses specially
                            if let Some(content) = json.get("content").and_then(|c| c.as_array()) {
                                for item in content {
                                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                        debug!(
                                            session_id = %self.sid,
                                            text = text,
                                            "Processing tool response text"
                                        );
                                    }
                                }
                            }
                            
                            // Handle regular assistant responses
                            if let (Some("assistant"), Some(content)) = (
                                json.get("role").and_then(|r| r.as_str()),
                                json.get("content").and_then(|c| c.as_array()).and_then(|a| a.get(0))
                            ) {
                                if let Some(text) = content.get("text").and_then(|t| t.as_str()) {
                                    debug!(
                                        session_id = %self.sid,
                                        content_length = text.len(),
                                        "Found assistant response"
                                    );
                                    return Ok((text.to_string(), current_offset));
                                }
                            }
                        },
                        Err(e) => {
                            // If parsing fails, check if it's a MCP client warning
                            if line.contains("mcp_client::transport::stdio") {
                                debug!(
                                    session_id = %self.sid,
                                    "Skipping MCP client warning message"
                                );
                                continue;
                            }
                            
                            // Otherwise, keep buffering
                            // Continue reading if JSON appears incomplete
                            if e.is_eof() {
                                debug!(session_id = %self.sid, "Waiting for rest of JSON" );
                                continue;
                            }

                            // Log and clear buffer on other errors
                            debug!(session_id = %self.sid, error = %e, "Discarding invalid JSON line");
                            buffer.clear();
                            continue;
                        }
                    }
                }
                
                Ok(Some(Err(e))) => {
                    consecutive_errors += 1;
                    error!(
                        session_id = %self.sid,
                        error = %e,
                        consecutive_errors,
                        "Failed to read line from JSONL"
                    );
                    
                    if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                        return Err(anyhow!("Too many consecutive read errors: {}", e));
                    }
                    
                    // Wait a bit before retrying after an error
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                
                Ok(None) => {
                    // No more lines available, check if file has grown
                    let metadata = match tokio::fs::metadata(&path).await {
                        Ok(m) => m,
                        Err(e) => {
                            error!(
                                session_id = %self.sid,
                                error = %e,
                                "Failed to get file metadata"
                            );
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                    };
                    
                    let current_size = metadata.len();
                    
                    // If file hasn't grown, wait a bit before checking again
                    if current_size <= last_file_size {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                    
                    // File has grown, reopen it and seek to the last position
                    match File::open(&path).await {
                        Ok(mut new_file) => {
                            if let Err(e) = new_file.seek(std::io::SeekFrom::Start(current_offset)).await {
                                error!(
                                    session_id = %self.sid,
                                    error = %e,
                                    offset = current_offset,
                                    "Failed to seek in reopened file"
                                );
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                continue;
                            }
                            
                            reader = FramedRead::new(new_file, LinesCodec::new());
                            last_file_size = current_size;
                        }
                        Err(e) => {
                            error!(
                                session_id = %self.sid,
                                error = %e,
                                "Failed to reopen file"
                            );
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
                
                Err(_) => {
                    // Timeout occurred
                    break;
                }
            }
        }
        
        Err(anyhow!(
            "Timeout waiting for assistant response after {}ms",
            timeout_ms
        ))
    }

    /// Wait for a reply from the Goose CLI by monitoring the JSONL session file
    /// Check if the child process is still running
    pub async fn is_running(&mut self) -> bool {
        match self.process.try_wait() {
            Ok(Some(_)) => false, // Process has exited
            Ok(None) => true,     // Process is still running
            Err(e) => {
                error!("[{}] Error checking process status: {}", self.sid, e);
                false
            }
        }
    }
    
    /// Monitor the child process and clean up when it exits
    pub async fn monitor(&mut self) -> Result<()> {
        let sid = self.sid.clone();
        
        // We'll just log that monitoring is not implemented yet
        // since we can't move the child process out of self
        info!("[{}] Process monitoring is not fully implemented yet", sid);
        
        Ok(())
    }
    
    pub async fn wait_reply_raw(&mut self, timeout_ms: u64) -> Result<String> {
        info!("[{}] Waiting for JSONL response (timeout: {}ms)", self.sid, timeout_ms);
        let start_time = Instant::now();
        
        // First, check if we already have a response in the log file
        if let Ok((response, new_offset)) = self.wait_assistant_jsonl(timeout_ms, self.last_offset).await {
            info!("[{}] Found response in JSONL log", self.sid);
            self.update_offset(new_offset);
            return Ok(response);
        }
        
        // If no response found in existing log, wait for a new one
        info!("[{}] No existing response found in log, waiting for new one...", self.sid);
        
        // Use the current offset to only read new content
        let (response, new_offset) = self.wait_assistant_jsonl(timeout_ms, self.last_offset).await?;
        let elapsed = start_time.elapsed();
        
        // Update the offset for the next read
        self.update_offset(new_offset);
        
        // Log the response (truncate if too long)
        let response_preview = if response.len() > 100 { 
            format!("{}... (truncated)", &response[..100])
        } else {
            response.clone()
        };
        
        info!(
            "[{}] Received response after {:.2?} ({} chars): {}",
            self.sid, elapsed, response.len(), response_preview
        );
        
        if response.is_empty() {
            error!("[{}] Empty response from Goose CLI", self.sid);
            return Err(anyhow!("Empty response from Goose CLI"));
        }
        
        Ok(response)
    }
}