use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub inbox: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub connector_type: Option<String>,
    #[serde(default)]
    pub connector_details: serde_json::Value,
    #[serde(default)]
    pub capabilities_keywords: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Registry {
    by_name: HashMap<String, AgentInfo>,
    pub goose_inbox: String,
}

impl Registry {
    /// Load your **map-shaped** JSON and derive AgentInfo rows.
    pub fn load_map<P: AsRef<Path>>(path: P, goose_inbox: impl Into<String>) -> anyhow::Result<Self> {
        let text = fs::read_to_string(path)?;
        let raw: HashMap<String, serde_json::Value> = serde_json::from_str(&text)?;

        let mut by_name = HashMap::new();
        for (name, v) in raw {
            let inbox = v.get("target_inbox")
                .and_then(|s| s.as_str())
                .ok_or_else(|| anyhow::anyhow!("agent {name} missing target_inbox"))?
                .to_string();

            let description = v.get("description").and_then(|s| s.as_str()).map(|s| s.to_string());
            let connector_type = v.get("connector_type").and_then(|s| s.as_str()).map(|s| s.to_string());
            let connector_details = v.get("connector_details").cloned().unwrap_or_default();
            let capabilities_keywords = v.get("capabilities_keywords")
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();

            let info = AgentInfo {
                name: name.clone(),
                inbox,
                description,
                connector_type,
                connector_details,
                capabilities_keywords,
            };
            by_name.insert(name, info);
        }

        Ok(Self {
            by_name,
            goose_inbox: goose_inbox.into(),
        })
    }

    pub fn list(&self) -> Vec<&AgentInfo> {
        let mut v: Vec<_> = self.by_name.values().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn get(&self, name: &str) -> Option<&AgentInfo> {
        self.by_name.get(name)
    }
}
