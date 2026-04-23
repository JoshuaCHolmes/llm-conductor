# Bundled Deployment Strategy

## Goal: Single Executable Experience

**Vision:** User downloads one binary, runs it, and everything else is handled automatically.

---

## What Can Be Bundled

### ✅ Can Bundle (Include in Binary)

1. **llm-conductor binary itself** - Our Rust code
2. **Embedded local model** - Small quantized model for delegation/routing
3. **Model downloader** - Auto-fetch larger models on first run
4. **Virtual display tools** - Xvfb, xdotool (via static linking or bundling)
5. **SQLite** - In-memory DB support (already in Rust)
6. **HTTP server** - Mock server for testing (pure Rust)
7. **Configuration templates** - Default configs

### ❌ Cannot Bundle (External Dependencies)

1. **Ollama server** - Separate daemon that must run
2. **Large models** - Too big to embed (Qwen 2.5 3B = 1.9GB, Phi-3 = 2.3GB)
3. **Nix package manager** - System-level installation
4. **GPU drivers** - System-specific
5. **API keys/credentials** - User-specific secrets

### 🔧 Can Auto-Install on First Run

1. **Ollama** - Download and install if missing
2. **Small local models** - Download on demand
3. **Nix** (optional) - Offer installation
4. **Testing tools** - Download as needed

---

## Architecture: Self-Contained Binary

```rust
llm-conductor                    # Single 50-100MB binary
├── Embedded Assets
│   ├── Default config templates
│   ├── System prompts
│   ├── Tiny routing model (optional, ~50MB GGUF)
│   └── Testing infrastructure code
├── Auto-Installer
│   ├── Ollama installer
│   ├── Model downloader
│   └── Optional: Nix installer
└── Credential Manager
    └── Interactive key/login helper
```

---

## Implementation Strategy

### 1. Embedded Small Model for Routing

Use `include_bytes!` to embed a tiny GGUF model:

```rust
// Embed small model for routing decisions
const ROUTING_MODEL: &[u8] = include_bytes!("../models/phi-3-mini-4k-instruct-q4.gguf");

pub struct EmbeddedRouter {
    model: LlamaModel,
}

impl EmbeddedRouter {
    pub fn new() -> Result<Self> {
        // Load model from embedded bytes
        let model = LlamaModel::from_bytes(ROUTING_MODEL)?;
        Ok(Self { model })
    }
    
    pub fn assess_complexity(&self, task: &str) -> Result<ComplexityLevel> {
        // Use embedded model to assess
        let prompt = format!("Rate complexity 1-5: {}", task);
        let response = self.model.generate(&prompt)?;
        // Parse response...
        Ok(complexity)
    }
}
```

**Model Options for Embedding:**
- **Phi-3 Mini 4K Q4** (~2.3GB) - Good quality, small enough
- **Qwen2.5 1.5B Q4** (~900MB) - Smaller, still capable
- **TinyLlama 1.1B Q4** (~600MB) - Fastest, basic capability
- **No embedded model** - Just download on first run

**Decision:** Start without embedding, download on first run. This keeps binary small.

---

### 2. Ollama Auto-Installation

Detect and install Ollama if missing:

```rust
pub struct OllamaInstaller;

impl OllamaInstaller {
    /// Check if Ollama is installed and running
    pub async fn check_installation() -> InstallStatus {
        // 1. Check if ollama binary exists
        if which::which("ollama").is_ok() {
            // 2. Check if server is running
            if Self::is_server_running().await {
                return InstallStatus::InstalledAndRunning;
            } else {
                return InstallStatus::InstalledNotRunning;
            }
        }
        
        InstallStatus::NotInstalled
    }
    
    async fn is_server_running() -> bool {
        let client = reqwest::Client::new();
        client.get("http://localhost:11434/api/tags")
            .send()
            .await
            .is_ok()
    }
    
    /// Install Ollama automatically
    pub async fn install() -> Result<()> {
        println!("Ollama not found. Installing...");
        
        #[cfg(target_os = "linux")]
        {
            // Download and run install script
            let script = reqwest::get("https://ollama.com/install.sh")
                .await?
                .text()
                .await?;
            
            // Execute install script
            tokio::fs::write("/tmp/ollama-install.sh", script).await?;
            
            let output = Command::new("sh")
                .arg("/tmp/ollama-install.sh")
                .output()
                .await?;
            
            if !output.status.success() {
                return Err(anyhow!("Ollama installation failed"));
            }
            
            println!("✓ Ollama installed successfully");
        }
        
        #[cfg(target_os = "macos")]
        {
            // Use homebrew or direct download
            if which::which("brew").is_ok() {
                Command::new("brew")
                    .args(&["install", "ollama"])
                    .status()
                    .await?;
            } else {
                // Download .dmg and guide user
                println!("Please download Ollama from: https://ollama.com/download");
                return Err(anyhow!("Manual installation required"));
            }
        }
        
        #[cfg(target_os = "windows")]
        {
            println!("Please download Ollama from: https://ollama.com/download");
            println!("Or use WSL: wsl --install");
            return Err(anyhow!("Manual installation required"));
        }
        
        Ok(())
    }
    
    /// Start Ollama server
    pub async fn start_server() -> Result<()> {
        println!("Starting Ollama server...");
        
        // Start as background daemon
        Command::new("ollama")
            .arg("serve")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        
        // Wait for server to be ready
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if Self::is_server_running().await {
                println!("✓ Ollama server started");
                return Ok(());
            }
        }
        
        Err(anyhow!("Ollama server failed to start"))
    }
}

pub enum InstallStatus {
    InstalledAndRunning,
    InstalledNotRunning,
    NotInstalled,
}
```

---

### 3. Model Management

Download and manage models on demand:

```rust
pub struct ModelManager {
    cache_dir: PathBuf,
}

impl ModelManager {
    pub fn new() -> Result<Self> {
        // Use ~/.cache/llm-conductor/models
        let cache_dir = dirs::cache_dir()
            .ok_or_else(|| anyhow!("Could not find cache directory"))?
            .join("llm-conductor")
            .join("models");
        
        std::fs::create_dir_all(&cache_dir)?;
        
        Ok(Self { cache_dir })
    }
    
    /// Ensure required models are available
    pub async fn ensure_models(&self) -> Result<()> {
        println!("Checking required models...");
        
        // Check if models are already pulled in Ollama
        let ollama_models = self.list_ollama_models().await?;
        
        // Required models for basic operation
        let required = vec![
            "qwen2.5:3b",    // For routing and delegation
        ];
        
        for model in required {
            if !ollama_models.contains(&model.to_string()) {
                println!("Downloading model: {}", model);
                self.pull_model(model).await?;
            } else {
                println!("✓ Model available: {}", model);
            }
        }
        
        Ok(())
    }
    
    async fn list_ollama_models(&self) -> Result<Vec<String>> {
        let client = reqwest::Client::new();
        let response: serde_json::Value = client
            .get("http://localhost:11434/api/tags")
            .send()
            .await?
            .json()
            .await?;
        
        let models = response["models"]
            .as_array()
            .ok_or_else(|| anyhow!("Invalid response"))?
            .iter()
            .filter_map(|m| m["name"].as_str().map(|s| s.to_string()))
            .collect();
        
        Ok(models)
    }
    
    async fn pull_model(&self, model_name: &str) -> Result<()> {
        // Use ollama pull command
        let output = Command::new("ollama")
            .args(&["pull", model_name])
            .status()
            .await?;
        
        if !output.success() {
            return Err(anyhow!("Failed to pull model: {}", model_name));
        }
        
        Ok(())
    }
    
    /// Download model with progress bar
    pub async fn download_with_progress(
        &self,
        model_name: &str,
    ) -> Result<()> {
        use indicatif::{ProgressBar, ProgressStyle};
        
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap()
        );
        
        pb.set_message(format!("Downloading {}", model_name));
        
        // Start pull in background, monitor progress
        let mut child = Command::new("ollama")
            .args(&["pull", model_name])
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        
        // Read output for progress updates
        if let Some(stdout) = child.stdout.take() {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut reader = BufReader::new(stdout).lines();
            
            while let Some(line) = reader.next_line().await? {
                pb.set_message(line);
            }
        }
        
        child.wait().await?;
        pb.finish_with_message(format!("✓ Downloaded {}", model_name));
        
        Ok(())
    }
}
```

---

### 4. Credential Manager

Interactive helper for adding API keys:

```rust
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
    
    /// Interactive setup for credentials
    pub async fn interactive_setup(&self) -> Result<()> {
        use dialoguer::{Input, Password, Confirm};
        
        println!("\n{}", "=== Credential Setup ===".bright_cyan().bold());
        println!("Let's set up your API keys and credentials.\n");
        
        // NVIDIA NIM
        if Confirm::new()
            .with_prompt("Do you have an NVIDIA NIM API key?")
            .default(false)
            .interact()?
        {
            let key: String = Password::new()
                .with_prompt("NVIDIA NIM API key")
                .interact()?;
            
            self.save_credential("nvidia_nim_key", &key)?;
            println!("✓ NVIDIA NIM key saved\n");
        }
        
        // GitHub Copilot
        if Confirm::new()
            .with_prompt("Do you have GitHub Copilot access?")
            .default(false)
            .interact()?
        {
            println!("GitHub Copilot uses OAuth. Opening browser for authentication...");
            // TODO: Implement OAuth flow
            println!("✓ GitHub Copilot configured\n");
        }
        
        // TAMU
        if Confirm::new()
            .with_prompt("Do you have TAMU AI access?")
            .default(false)
            .interact()?
        {
            let api_key: String = Password::new()
                .with_prompt("TAMU API key")
                .interact()?;
            
            self.save_credential("tamu_api_key", &api_key)?;
            println!("✓ TAMU key saved\n");
        }
        
        // User info (optional)
        if Confirm::new()
            .with_prompt("Would you like to add personal info for better context?")
            .default(false)
            .interact()?
        {
            let name: String = Input::new()
                .with_prompt("Your name")
                .interact()?;
            
            let institution: String = Input::new()
                .with_prompt("Institution (optional)")
                .allow_empty(true)
                .interact()?;
            
            let user_info = json!({
                "name": name,
                "institution": institution,
            });
            
            let config_file = self.config_dir.join("user.json");
            std::fs::write(config_file, serde_json::to_string_pretty(&user_info)?)?;
            
            println!("✓ User info saved\n");
        }
        
        println!("{}", "Setup complete! 🎉\n".bright_green().bold());
        
        Ok(())
    }
    
    fn save_credential(&self, key: &str, value: &str) -> Result<()> {
        let env_file = self.config_dir.join(".env");
        
        // Read existing .env if it exists
        let mut env_vars = if env_file.exists() {
            std::fs::read_to_string(&env_file)?
                .lines()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        
        // Remove old value if exists
        env_vars.retain(|line| !line.starts_with(&format!("{}=", key.to_uppercase())));
        
        // Add new value
        env_vars.push(format!("{}={}", key.to_uppercase(), value));
        
        // Write back
        std::fs::write(env_file, env_vars.join("\n"))?;
        
        Ok(())
    }
    
    /// Add single credential via CLI
    pub fn add_credential(&self, provider: &str, key: &str) -> Result<()> {
        let credential_key = match provider.to_lowercase().as_str() {
            "nvidia" | "nim" => "nvidia_nim_key",
            "github" | "copilot" => "github_token",
            "tamu" => "tamu_api_key",
            _ => return Err(anyhow!("Unknown provider: {}", provider)),
        };
        
        self.save_credential(credential_key, key)?;
        println!("✓ Credential saved for {}", provider);
        
        Ok(())
    }
    
    /// Load credentials from .env
    pub fn load_credentials(&self) -> Result<HashMap<String, String>> {
        let env_file = self.config_dir.join(".env");
        
        if !env_file.exists() {
            return Ok(HashMap::new());
        }
        
        let content = std::fs::read_to_string(env_file)?;
        let mut creds = HashMap::new();
        
        for line in content.lines() {
            if let Some((key, value)) = line.split_once('=') {
                creds.insert(key.to_string(), value.to_string());
            }
        }
        
        Ok(creds)
    }
}
```

---

### 5. First-Run Setup Flow

```rust
pub struct FirstRunSetup {
    ollama: OllamaInstaller,
    models: ModelManager,
    credentials: CredentialManager,
}

impl FirstRunSetup {
    pub fn new() -> Result<Self> {
        Ok(Self {
            ollama: OllamaInstaller,
            models: ModelManager::new()?,
            credentials: CredentialManager::new()?,
        })
    }
    
    /// Run complete first-time setup
    pub async fn run(&mut self) -> Result<()> {
        println!("\n{}", "=== Welcome to llm-conductor! ===".bright_cyan().bold());
        println!("Let's get you set up.\n");
        
        // Step 1: Check/install Ollama
        println!("{}", "Step 1: Checking Ollama...".bright_white());
        match OllamaInstaller::check_installation().await {
            InstallStatus::InstalledAndRunning => {
                println!("✓ Ollama is installed and running\n");
            }
            InstallStatus::InstalledNotRunning => {
                println!("Ollama installed but not running. Starting...");
                OllamaInstaller::start_server().await?;
            }
            InstallStatus::NotInstalled => {
                use dialoguer::Confirm;
                
                if Confirm::new()
                    .with_prompt("Ollama not found. Install it now?")
                    .default(true)
                    .interact()?
                {
                    OllamaInstaller::install().await?;
                    OllamaInstaller::start_server().await?;
                } else {
                    println!("{}", "Ollama is required for local models.".yellow());
                    println!("Install manually from: https://ollama.com");
                    return Err(anyhow!("Ollama installation declined"));
                }
            }
        }
        
        // Step 2: Download required models
        println!("{}", "Step 2: Setting up models...".bright_white());
        self.models.ensure_models().await?;
        println!();
        
        // Step 3: Configure credentials (optional)
        println!("{}", "Step 3: API credentials (optional)...".bright_white());
        use dialoguer::Confirm;
        
        if Confirm::new()
            .with_prompt("Would you like to add API keys for cloud providers?")
            .default(false)
            .interact()?
        {
            self.credentials.interactive_setup().await?;
        } else {
            println!("Skipping. You can add credentials later with: llm-conductor config add-key\n");
        }
        
        // Done!
        println!("{}", "✓ Setup complete!".bright_green().bold());
        println!("You're ready to use llm-conductor.\n");
        println!("Try: llm-conductor chat\n");
        
        Ok(())
    }
    
    /// Check if setup has been completed
    pub fn is_setup_complete() -> bool {
        // Check for marker file
        let config_dir = dirs::config_dir()
            .map(|d| d.join("llm-conductor"))
            .expect("Could not find config directory");
        
        config_dir.join(".setup_complete").exists()
    }
    
    /// Mark setup as complete
    pub fn mark_complete(&self) -> Result<()> {
        let marker = self.credentials.config_dir.join(".setup_complete");
        std::fs::write(marker, "")?;
        Ok(())
    }
}
```

---

### 6. Updated main.rs

```rust
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(false)
        .init();
    
    // Check if first run
    if !FirstRunSetup::is_setup_complete() {
        let mut setup = FirstRunSetup::new()?;
        setup.run().await?;
        setup.mark_complete()?;
    }
    
    // Ensure Ollama is running
    match OllamaInstaller::check_installation().await {
        InstallStatus::InstalledNotRunning => {
            println!("Starting Ollama server...");
            OllamaInstaller::start_server().await?;
        }
        InstallStatus::NotInstalled => {
            eprintln!("Ollama not found. Please run: llm-conductor setup");
            return Err(anyhow!("Ollama not installed"));
        }
        _ => {}
    }
    
    // Create router
    let mut router = Router::new();
    
    // Add providers
    router.add_provider(Box::new(OllamaProvider::new(None)));
    
    // Load credentials and add cloud providers if configured
    let creds = CredentialManager::new()?.load_credentials()?;
    
    if let Some(nim_key) = creds.get("NVIDIA_NIM_KEY") {
        router.add_provider(Box::new(NvidiaNimProvider::new(nim_key.clone())));
    }
    
    // Run REPL
    let mut repl = Repl::new(router);
    repl.run().await?;
    
    Ok(())
}
```

---

## CLI Commands

```bash
# First run - automatic setup
$ llm-conductor
# Runs interactive setup, installs Ollama, downloads models

# Manual setup
$ llm-conductor setup

# Add credentials
$ llm-conductor config add-key nvidia <API_KEY>
$ llm-conductor config add-key github <TOKEN>
$ llm-conductor config add-key tamu <API_KEY>

# Interactive credential setup
$ llm-conductor config setup-keys

# List configured providers
$ llm-conductor providers

# Chat
$ llm-conductor chat
$ llm-conductor  # Default to chat

# Check status
$ llm-conductor status
# Shows: Ollama status, models available, providers configured
```

---

## Binary Distribution Strategy

### Release Artifacts

```
llm-conductor-v0.1.0-linux-x86_64          # Linux binary
llm-conductor-v0.1.0-linux-aarch64         # Linux ARM
llm-conductor-v0.1.0-macos-x86_64          # macOS Intel
llm-conductor-v0.1.0-macos-aarch64         # macOS Apple Silicon
llm-conductor-v0.1.0-windows-x86_64.exe    # Windows
```

### Binary Size Targets

- **Without embedded model**: ~15-25MB (compressed)
- **With embedded model**: ~50-100MB (compressed)

### Installation Script

```bash
# install.sh
curl -sSL https://llm-conductor.dev/install.sh | sh

# Downloads appropriate binary for platform
# Puts in ~/.local/bin or /usr/local/bin
# Runs first-time setup
```

---

## Summary

**What User Gets:**
1. **Single binary download** (~20MB without model, ~80MB with)
2. **First-run auto-setup** - Installs Ollama, downloads models
3. **Interactive credential manager** - Easy API key setup
4. **Everything bundled except:**
   - User's API keys (they provide)
   - Large models (auto-downloaded on first run)
   - System dependencies (Ollama, auto-installed)

**User Experience:**
```bash
$ curl -sSL llm-conductor.dev/install.sh | sh
[Downloading llm-conductor...]
✓ Installed to ~/.local/bin/llm-conductor

$ llm-conductor
=== Welcome to llm-conductor! ===
Ollama not found. Install it now? [Y/n] y
✓ Ollama installed
✓ Ollama server started
Downloading model: qwen2.5:3b ...
✓ Downloaded qwen2.5:3b
Would you like to add API keys? [y/N] n
✓ Setup complete!

❯ Hello!
Using qwen2.5:3b ...
❯ Hello! How can I help you today?
```

**Implementation Priority:**
1. ✅ Single binary (already done with Rust)
2. **Ollama auto-installer** (high priority)
3. **Model manager** (high priority)
4. **Credential manager** (high priority)
5. **First-run setup** (medium priority)
6. **Embedded model** (low priority - download instead)

This gives us a polished, professional single-binary experience!
