use goose::config::ExtensionConfig;
use goose::recipe::{Recipe, RecipeParameterRequirement};
use std::collections::HashSet;

/// Represents a secret that needs to be collected from the user
#[derive(Debug, Clone)]
pub struct SecretRequirement {
    pub key: String,
    pub extension_name: String,
    pub description: String,
    pub required: bool,
}

impl SecretRequirement {
    pub fn description(&self) -> &str {
        &self.description
    }
}

/// Helper function to check if a string looks like it contains a secret
fn is_secret_key(key: &str) -> bool {
    let key_lower = key.to_lowercase();
    key_lower.contains("api_key") || 
    key_lower.contains("secret") ||
    key_lower.contains("password") ||
    key_lower.contains("token") ||
    key_lower.ends_with("_key") ||
    key_lower.ends_with("_secret")
}

/// Helper function to extract strings from ExtensionConfig for secret checking
fn get_extension_strings(config: &ExtensionConfig) -> Vec<String> {
    match config {
        ExtensionConfig::Sse { name, uri, description, .. } => {
            let mut parts = vec![name.clone(), uri.clone()];
            if let Some(desc) = description {
                parts.push(desc.clone());
            }
            parts
        }
        ExtensionConfig::Stdio { name, cmd, args, description, .. } => {
            let mut parts = vec![name.clone(), cmd.clone()];
            parts.extend(args.clone());
            if let Some(desc) = description {
                parts.push(desc.clone());
            }
            parts
        }
        ExtensionConfig::Builtin { name, display_name, description, .. } => {
            let mut parts = vec![name.clone()];
            if let Some(display) = display_name {
                parts.push(display.clone());
            }
            if let Some(desc) = description {
                parts.push(desc.clone());
            }
            parts
        }
        ExtensionConfig::StreamableHttp { name, uri, description, .. } => {
            let mut parts = vec![name.clone(), uri.clone()];
            if let Some(desc) = description {
                parts.push(desc.clone());
            }
            parts
        }
        ExtensionConfig::Frontend { name, tools, instructions, .. } => {
            let mut parts = vec![name.clone()];
            if let Some(instr) = instructions {
                parts.push(instr.clone());
            }
            // Add tool names
            for tool in tools {
                parts.push(tool.name.to_string());
                if let Some(desc) = &tool.description {
                    parts.push(desc.to_string());
                }
            }
            parts
        }
    }
}

/// Discovers secrets in a recipe by analyzing its extensions and parameters
pub fn discover_recipe_secrets(recipe: &Recipe) -> Vec<SecretRequirement> {
    let mut secrets = Vec::new();
    let mut seen = HashSet::new();

    // Check extensions for potential secrets
    if let Some(extensions) = &recipe.extensions {
        for ext in extensions {
            let ext_name = ext.name();
            let strings_to_check = get_extension_strings(ext);
            
            // Check all strings from the extension for potential secrets
            for s in strings_to_check {
                // Check if the string itself looks like a secret
                if is_secret_key(&s) {
                    let key = format!("extension.{}.value", ext_name);
                    if seen.insert(key.clone()) {
                        secrets.push(SecretRequirement {
                            key,
                            extension_name: ext_name.to_string(),
                            description: format!("Potential secret found in {} extension", ext_name),
                            required: true,
                        });
                    }
                }
                
                // Check for key-value pairs in the string
                for part in s.split_whitespace() {
                    if let Some((key, _)) = part.split_once('=') {
                        if is_secret_key(key) {
                            let secret_key = format!("extension.{}.{}", ext_name, key);
                            if seen.insert(secret_key.clone()) {
                                secrets.push(SecretRequirement {
                                    key: secret_key,
                                    extension_name: ext_name.to_string(),
                                    description: format!("Secret configuration '{}' for {} extension", key, ext_name),
                                    required: true,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    // Check for secrets in parameters
    if let Some(parameters) = &recipe.parameters {
        for param in parameters {
            let is_required = matches!(param.requirement, RecipeParameterRequirement::Required);
            
            if is_required && is_secret_key(&param.key) {
                secrets.push(SecretRequirement {
                    key: param.key.clone(),
                    extension_name: "recipe".to_string(),
                    description: param.description.clone(),
                    required: true,
                });
            }
        }
    }

    secrets
}
