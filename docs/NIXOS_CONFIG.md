# NixOS Configuration Options

## Basic Setup

Add to your `flake.nix`:

```nix
{
  inputs.llm-conductor.url = "github:JoshuaCHolmes/llm-conductor";
  
  outputs = { self, nixpkgs, llm-conductor, ... }: {
    nixosConfigurations.yourhost = nixpkgs.lib.nixosSystem {
      modules = [
        llm-conductor.nixosModules.default
        ./configuration.nix
      ];
    };
  };
}
```

## Configuration Options

### Minimal (Cloud Providers Only)

Perfect for cloud-based usage (GitHub, TAMU, Outlier, NVIDIA NIM):

```nix
services.llm-conductor = {
  enable = true;
  enableOllama = false;  # Default - no local models
};
```

### With Local Models (Ollama)

For systems with good hardware/GPU acceleration:

```nix
services.llm-conductor = {
  enable = true;
  enableOllama = true;        # Install Ollama
  autoStartOllama = true;     # Start service automatically
};
```

### Manual Ollama Control

Install Ollama but don't auto-start (start manually when needed):

```nix
services.llm-conductor = {
  enable = true;
  enableOllama = true;        # Install Ollama
  autoStartOllama = false;    # Don't auto-start service
};
```

## When to Enable Ollama

**Enable Ollama (`enableOllama = true`) if you:**
- Have a discrete GPU with good VRAM (8GB+)
- Want to run models locally without internet
- Need privacy/offline capabilities
- Have x86_64 with AVX2 support

**Don't enable Ollama (`enableOllama = false`) if you:**
- Are on ARM64 without NPU access (like WSL)
- Have limited RAM/CPU resources
- Only plan to use cloud providers
- Want faster startup times
- Experience slow/poor Ollama performance

## Adding Credentials

After installation, add your API keys:

```bash
# GitHub Copilot (50 requests/month free)
llm-conductor config add-key github YOUR_TOKEN

# TAMU AI (if you have access)
llm-conductor config add-key tamu YOUR_API_KEY

# Outlier Playground (free via RLHF contract)
llm-conductor config add-key outlier_cookie 'YOUR_COOKIES'
llm-conductor config add-key outlier_csrf 'YOUR_CSRF_TOKEN'

# NVIDIA NIM (requires signup)
llm-conductor config add-key nvidia YOUR_API_KEY
```

See [docs/OUTLIER_SETUP.md](OUTLIER_SETUP.md) for detailed Outlier setup.

## Example: JCH-NixOS Development Module

For modular configs, you can wrap it in your own options:

```nix
# modules/optional/development.nix
{ config, lib, pkgs, ... }:

{
  options.jch.development = {
    enable = lib.mkEnableOption "development tools";
    
    llmConductor = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Enable llm-conductor";
    };
    
    llmConductorOllama = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Enable Ollama (requires good hardware)";
    };
  };

  config = lib.mkIf config.jch.development.enable {
    services.llm-conductor = lib.mkIf config.jch.development.llmConductor {
      enable = true;
      enableOllama = config.jch.development.llmConductorOllama;
      autoStartOllama = config.jch.development.llmConductorOllama;
    };
  };
}
```

Then in your main config:

```nix
{
  jch.development = {
    enable = true;
    llmConductor = true;
    llmConductorOllama = false;  # Disable on ARM64/WSL
  };
}
```

## Checking Status

After rebuilding:

```bash
# Check configured providers
llm-conductor providers

# Start interactive chat
llm-conductor

# View configuration
llm-conductor config show
```
