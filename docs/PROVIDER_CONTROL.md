# Provider Control Configuration

This document describes how to control which providers llm-conductor uses.

## Problem: Multiple Ollama Instances

On systems with both Windows and WSL/Linux, you might have:
- Windows Ollama running on localhost:11434
- NixOS Ollama service that tries to start on the same port
- Need to choose which one to use (or disable both)

## Configuration File

Create `~/.config/llm-conductor/providers.toml`:

```toml
# Provider Control Configuration

[providers]
# Enable/disable entire providers
# Credentials must still be configured for enabled providers
ollama = true          # Use Ollama if available
github = true          # Use GitHub Copilot
tamu = true            # Use TAMU AI
nvidia = true          # Use NVIDIA NIM
outlier = true         # Use Outlier Playground

[ollama]
# Ollama-specific settings
enabled = true                    # Master switch for Ollama
url = "http://localhost:11434"    # Default Ollama endpoint
prefer_system = true              # Prefer system Ollama over NixOS service
fallback_urls = [                 # Try these if primary fails
    "http://localhost:11435",
    "http://localhost:11436",
]

# Model preferences
default_model = "qwen2.5:3b"
auto_pull = false                 # Don't auto-download models

[github]
enabled = true
# Uses credentials from ~/.config/llm-conductor/.env

[tamu]
enabled = true
# Uses credentials from ~/.config/llm-conductor/.env

[nvidia]
enabled = true
# Uses credentials from ~/.config/llm-conductor/.env

[outlier]
enabled = true
# Uses credentials from ~/.config/llm-conductor/.env

[routing]
# Model selection preferences
prefer_local = false              # Prefer cloud models over local
prefer_free = true                # Prefer free tiers (GitHub, Outlier)
prefer_unlimited = true           # Prefer unlimited sources (Outlier, local)

# Fallback order when primary provider fails
fallback_order = [
    "outlier",  # Try Outlier first (unlimited Opus!)
    "tamu",     # Then TAMU (daily limits)
    "github",   # Then GitHub (monthly limits)
    "nvidia",   # Then NVIDIA NIM (rate limits)
    "ollama",   # Finally local (always available if running)
]
```

## Implementation Plan

### Phase 1: Basic Provider Toggle (Immediate)

Add to `src/config/mod.rs`:

```rust
#[derive(Debug, Deserialize, Serialize)]
pub struct ProviderConfig {
    pub enabled: bool,
    #[serde(default)]
    pub priority: u8,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProvidersConfig {
    #[serde(default = "default_enabled")]
    pub ollama: ProviderConfig,
    #[serde(default = "default_enabled")]
    pub github: ProviderConfig,
    #[serde(default = "default_enabled")]
    pub tamu: ProviderConfig,
    #[serde(default = "default_enabled")]
    pub nvidia: ProviderConfig,
    #[serde(default = "default_enabled")]
    pub outlier: ProviderConfig,
}

fn default_enabled() -> ProviderConfig {
    ProviderConfig { enabled: true, priority: 50 }
}
```

### Phase 2: Ollama Source Selection

Add to `src/providers/ollama.rs`:

```rust
pub enum OllamaSource {
    System,      // System-installed (Windows, Linux)
    NixOS,       // NixOS service
    Docker,      // Docker container
    Custom(String), // Custom URL
}

impl OllamaProvider {
    pub fn with_source(source: OllamaSource) -> Result<Self> {
        let url = match source {
            OllamaSource::System => "http://localhost:11434",
            OllamaSource::NixOS => "http://localhost:11434",
            OllamaSource::Docker => "http://localhost:11434",
            OllamaSource::Custom(url) => &url,
        };
        // Try connection...
    }
    
    pub async fn detect_source() -> Option<OllamaSource> {
        // Check Windows process list
        // Check systemd services
        // Check Docker containers
        // Return first found
    }
}
```

### Phase 3: CLI Commands

```bash
# View provider status
llm-conductor providers --verbose
# Shows:
# - Which providers are enabled
# - Which have credentials
# - Which are actually reachable
# - For Ollama: which source is being used

# Enable/disable providers
llm-conductor config set-provider ollama --enabled=false
llm-conductor config set-provider outlier --enabled=true --priority=1

# Ollama-specific
llm-conductor config set-ollama-url http://localhost:11435
llm-conductor config detect-ollama  # Auto-detect and configure
```

## Current Workaround

Until full implementation:

### Disable Ollama Entirely

Create `~/.config/llm-conductor/.env` with:
```bash
OLLAMA_DISABLED=true
```

### Use Specific Ollama Port

Create `~/.config/llm-conductor/.env` with:
```bash
OLLAMA_URL=http://localhost:11435
```

Or set environment variable:
```bash
export OLLAMA_HOST=localhost:11435
llm-conductor
```

## NixOS Module Options

In your `configuration.nix` or modular config:

```nix
services.llm-conductor = {
  enable = true;
  
  # Don't install/start NixOS Ollama (use Windows/system instead)
  enableOllama = false;
  
  # Or: Install but use different port
  # enableOllama = true;
  # ollamaPort = 11435;
};
```

## Migration Path

1. **Today**: Use NixOS `enableOllama = false` to avoid conflicts
2. **Next PR**: Add basic provider enable/disable via config file
3. **Later PR**: Add Ollama source detection and selection
4. **Future**: Add smart fallback and load balancing

## Discussion Points

- Should provider control be in `.env`, `providers.toml`, or both?
- Should Ollama auto-detect and use any available instance?
- Should we show warnings when providers are configured but disabled?
- How to handle the case where credentials exist but provider is disabled?
