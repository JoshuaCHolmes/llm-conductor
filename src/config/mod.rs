mod providers;

pub use providers::{ProviderConfigManager, ProviderSettings, ProvidersConfig};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Manages user credentials and API keys
pub struct CredentialManager {
    config_dir: PathBuf,
}

impl CredentialManager {
    pub fn new() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow!("Could not find config directory"))?
            .join("llm-conductor");
        
        std::fs::create_dir_all(&config_dir)?;
        
        Ok(Self { config_dir })
    }
    
    /// Interactive setup for all credentials
    pub async fn interactive_setup(&self) -> Result<()> {
        use colored::*;
        use dialoguer::{Confirm, Input, Password};
        
        println!("\n{}", "=== API Credentials Setup ===".bright_cyan().bold());
        println!("Let's configure your API keys for cloud providers.\n");
        
        // NVIDIA NIM
        if Confirm::new()
            .with_prompt("Do you have an NVIDIA NIM API key?")
            .default(false)
            .interact()?
        {
            let key: String = Password::new()
                .with_prompt("NVIDIA NIM API key")
                .interact()?;
            
            self.save_credential("NVIDIA_NIM_KEY", &key)?;
            println!("✓ NVIDIA NIM key saved\n");
        }
        
        // GitHub Copilot
        if Confirm::new()
            .with_prompt("Do you have GitHub Copilot access?")
            .default(false)
            .interact()?
        {
            let token: String = Password::new()
                .with_prompt("GitHub token")
                .interact()?;
            
            self.save_credential("GITHUB_TOKEN", &token)?;
            println!("✓ GitHub Copilot configured\n");
        }
        
        // TAMU AI
        if Confirm::new()
            .with_prompt("Do you have TAMU AI access?")
            .default(false)
            .interact()?
        {
            let api_key: String = Password::new()
                .with_prompt("TAMU API key")
                .interact()?;
            
            self.save_credential("TAMU_API_KEY", &api_key)?;
            println!("✓ TAMU key saved\n");
        }
        
        println!("{}", "Credential setup complete! 🔑\n".bright_green().bold());
        
        Ok(())
    }
    
    /// Save a single credential
    pub fn save_credential(&self, key: &str, value: &str) -> Result<()> {
        let env_file = self.config_dir.join(".env");
        
        // Read existing .env if it exists
        let mut env_vars = if env_file.exists() {
            std::fs::read_to_string(&env_file)?
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        
        // Remove old value if exists
        env_vars.retain(|line| !line.starts_with(&format!("{}=", key)));
        
        // Add new value
        env_vars.push(format!("{}={}", key, value));
        
        // Write back
        std::fs::write(env_file, env_vars.join("\n") + "\n")?;
        
        Ok(())
    }
    
    /// Add credential via CLI
    pub fn add_credential(&self, provider: &str, key: &str) -> Result<()> {
        let credential_key = match provider.to_lowercase().as_str() {
            "nvidia" | "nim" => "NVIDIA_NIM_KEY",
            "github" | "copilot" => "GITHUB_TOKEN",
            "tamu" => "TAMU_API_KEY",
            "outlier" | "outlier_cookie" => "OUTLIER_COOKIE",
            "outlier_csrf" => "OUTLIER_CSRF",
            _ => return Err(anyhow!("Unknown provider: {}", provider)),
        };
        
        self.save_credential(credential_key, key)?;
        println!("✓ Credential saved for {}", provider);
        
        Ok(())
    }
    
    /// Load all credentials from .env
    pub fn load_credentials(&self) -> Result<HashMap<String, String>> {
        let env_file = self.config_dir.join(".env");
        
        if !env_file.exists() {
            return Ok(HashMap::new());
        }
        
        let content = std::fs::read_to_string(env_file)?;
        let mut creds = HashMap::new();
        
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            
            if let Some((key, value)) = line.split_once('=') {
                creds.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
        
        Ok(creds)
    }
    
    /// Get specific credential
    pub fn get_credential(&self, key: &str) -> Result<Option<String>> {
        let creds = self.load_credentials()?;
        Ok(creds.get(key).cloned())
    }
    
    /// List configured providers
    pub fn list_configured(&self) -> Result<Vec<String>> {
        let creds = self.load_credentials()?;
        let mut providers = Vec::new();
        
        if creds.contains_key("NVIDIA_NIM_KEY") {
            providers.push("NVIDIA NIM".to_string());
        }
        if creds.contains_key("GITHUB_TOKEN") {
            providers.push("GitHub Copilot".to_string());
        }
        if creds.contains_key("TAMU_API_KEY") {
            providers.push("TAMU AI".to_string());
        }
        if creds.contains_key("OUTLIER_COOKIE") && creds.contains_key("OUTLIER_CSRF") {
            providers.push("Outlier Playground".to_string());
        }
        
        Ok(providers)
    }
}

/// Manages user information and preferences
pub struct UserInfoManager {
    config_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub name: String,
    pub institution: Option<String>,
    pub role: Option<String>,  // "student", "developer", "researcher", etc.
    pub preferences: UserPreferences,
    pub additional_context: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserPreferences {
    pub preferred_language: Option<String>,
    pub coding_style: Option<String>,
    pub verbosity: VerbosityLevel,
    pub auto_approve: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum VerbosityLevel {
    Minimal,
    Normal,
    Verbose,
}

impl Default for VerbosityLevel {
    fn default() -> Self {
        VerbosityLevel::Normal
    }
}

impl UserInfoManager {
    pub fn new() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow!("Could not find config directory"))?
            .join("llm-conductor");
        
        std::fs::create_dir_all(&config_dir)?;
        
        Ok(Self { config_dir })
    }
    
    /// Interactive setup for user information
    pub fn interactive_setup(&self) -> Result<UserInfo> {
        use colored::*;
        use dialoguer::{Confirm, Input, Select};
        
        println!("\n{}", "=== User Information Setup ===".bright_cyan().bold());
        println!("This helps provide better context to AI models.\n");
        
        // Name
        let name: String = Input::new()
            .with_prompt("Your name")
            .interact()?;
        
        // Institution (optional)
        let has_institution = Confirm::new()
            .with_prompt("Are you affiliated with an institution?")
            .default(false)
            .interact()?;
        
        let institution = if has_institution {
            Some(Input::new()
                .with_prompt("Institution name")
                .interact()?)
        } else {
            None
        };
        
        // Role
        let roles = vec!["Student", "Developer", "Researcher", "Other"];
        let role_idx = Select::new()
            .with_prompt("Your primary role")
            .items(&roles)
            .default(0)
            .interact()?;
        
        let role = Some(roles[role_idx].to_lowercase());
        
        // Preferences
        let verbosity = if Confirm::new()
            .with_prompt("Prefer detailed explanations?")
            .default(false)
            .interact()?
        {
            VerbosityLevel::Verbose
        } else {
            VerbosityLevel::Normal
        };
        
        let auto_approve = Confirm::new()
            .with_prompt("Auto-approve low-impact changes? (production mode)")
            .default(true)
            .interact()?;
        
        // Additional context
        println!("\n{}", "Additional Context (optional)".dimmed());
        println!("{}", "You can add any additional information that might be helpful.".dimmed());
        println!("{}", "Examples: 'Working on game development', 'Learning Rust', 'Prefer functional programming'".dimmed());
        
        let mut additional_context = Vec::new();
        
        loop {
            let context: String = Input::new()
                .with_prompt("Add context (or press Enter to skip)")
                .allow_empty(true)
                .interact()?;
            
            if context.is_empty() {
                break;
            }
            
            additional_context.push(context);
        }
        
        let user_info = UserInfo {
            name,
            institution,
            role,
            preferences: UserPreferences {
                preferred_language: None,
                coding_style: None,
                verbosity,
                auto_approve,
            },
            additional_context,
        };
        
        self.save_user_info(&user_info)?;
        
        println!("\n{}", "✓ User information saved!".bright_green().bold());
        
        Ok(user_info)
    }
    
    /// Save user information
    pub fn save_user_info(&self, info: &UserInfo) -> Result<()> {
        let user_file = self.config_dir.join("user.json");
        let json = serde_json::to_string_pretty(info)?;
        std::fs::write(user_file, json)?;
        Ok(())
    }
    
    /// Load user information
    pub fn load_user_info(&self) -> Result<Option<UserInfo>> {
        let user_file = self.config_dir.join("user.json");
        
        if !user_file.exists() {
            return Ok(None);
        }
        
        let json = std::fs::read_to_string(user_file)?;
        let info: UserInfo = serde_json::from_str(&json)?;
        Ok(Some(info))
    }
    
    /// Add additional context
    pub fn add_context(&self, context: String) -> Result<()> {
        let mut info = self.load_user_info()?
            .ok_or_else(|| anyhow!("User info not configured. Run: llm-conductor config setup"))?;
        
        info.additional_context.push(context);
        self.save_user_info(&info)?;
        
        println!("✓ Context added");
        
        Ok(())
    }
    
    /// Update user information field
    pub fn update_field(&self, field: &str, value: &str) -> Result<()> {
        let mut info = self.load_user_info()?
            .ok_or_else(|| anyhow!("User info not configured. Run: llm-conductor config setup"))?;
        
        match field.to_lowercase().as_str() {
            "name" => info.name = value.to_string(),
            "institution" => info.institution = Some(value.to_string()),
            "role" => info.role = Some(value.to_string()),
            _ => return Err(anyhow!("Unknown field: {}", field)),
        }
        
        self.save_user_info(&info)?;
        println!("✓ Updated {} to: {}", field, value);
        
        Ok(())
    }
    
    /// Generate system instructions from user info
    pub fn generate_system_instructions(&self) -> Result<String> {
        let info = self.load_user_info()?;
        
        if let Some(info) = info {
            let mut instructions = vec![
                "You are a helpful AI assistant.".to_string(),
                format!("You are assisting {}", info.name),
            ];
            
            if let Some(ref institution) = info.institution {
                instructions.push(format!("at {}", institution));
            }
            
            if let Some(ref role) = info.role {
                instructions.push(format!("who is a {}", role));
            }
            
            if !info.additional_context.is_empty() {
                instructions.push("\nAdditional context:".to_string());
                for context in &info.additional_context {
                    instructions.push(format!("- {}", context));
                }
            }
            
            match info.preferences.verbosity {
                VerbosityLevel::Minimal => {
                    instructions.push("\nBe concise and to the point.".to_string());
                }
                VerbosityLevel::Verbose => {
                    instructions.push("\nProvide detailed explanations and examples.".to_string());
                }
                VerbosityLevel::Normal => {}
            }
            
            Ok(instructions.join(" "))
        } else {
            Ok("You are a helpful AI assistant.".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_credential_manager() {
        // Basic credential operations
        let temp_dir = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", temp_dir.path());
        
        // Would test save/load here
    }
}
