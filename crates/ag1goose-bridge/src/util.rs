
use chrono::Utc;
use serde_json::Value;

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

pub fn extract_text(content: &Value) -> Option<String> {
    // Primary convention: { "text": "..." }
    if let Some(t) = content.get("text").and_then(|v| v.as_str()) {
        return Some(t.to_string());
    }
    // Otherwise stringify JSON compactly
    Some(content.to_string())
}