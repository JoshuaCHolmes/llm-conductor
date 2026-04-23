# Getting Started with llm-conductor

## 🎯 For NixOS Users (Quickest Path)

### Step 1: Test Without Installing

Try it out first:
```bash
nix run github:JoshuaCHolmes/llm-conductor -- status
```

### Step 2: Add to Your Configuration

Edit your `flake.nix`:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    llm-conductor.url = "github:JoshuaCHolmes/llm-conductor";
    # ... your other inputs
  };

  outputs = { self, nixpkgs, llm-conductor, ... }: {
    nixosConfigurations.yourhost = nixpkgs.lib.nixosSystem {
      modules = [
        # Import the module
        llm-conductor.nixosModules.default
        
        # Configure it
        {
          services.llm-conductor = {
            enable = true;
            autoStartOllama = true;
          };
        }
        
        # Your other modules
        ./configuration.nix
      ];
    };
  };
}
```

### Step 3: Rebuild

```bash
sudo nixos-rebuild switch
```

### Step 4: Start Using

```bash
llm-conductor status    # Check everything is working
llm-conductor           # Start chatting!
```

That's it! Ollama is automatically included as a wrapped dependency.

## 📋 For Development Add-On Config

If you have a separate `development.nix` file for optional dev tools:

```nix
# development.nix
{ config, pkgs, inputs, ... }:

{
  services.llm-conductor = {
    enable = true;
    autoStartOllama = true;
  };
}
```

Then in your `flake.nix`, include it:

```nix
nixosConfigurations.yourhost = nixpkgs.lib.nixosSystem {
  modules = [
    llm-conductor.nixosModules.default
    ./configuration.nix
    ./development.nix  # Your optional dev tools
  ];
};
```

## 🔄 Updating

Update to latest version:
```bash
nix flake lock --update-input llm-conductor
sudo nixos-rebuild switch
```

## ✅ Verify Installation

After rebuild, check:
```bash
# Binary should be in PATH
which llm-conductor
# Output: /run/current-system/sw/bin/llm-conductor

# Ollama should be wrapped
llm-conductor status
# Should show: ✓ Ollama Running

# Check version
llm-conductor --version
```

## 🎨 First Chat Session

```bash
# Start interactive mode
llm-conductor

# You'll see:
# ╔═══════════════════════════════════════╗
# ║  llm-conductor 🎭                    ║
# ╚═══════════════════════════════════════╝
#
# Connected to: qwen2.5:3b (Ollama)
#
# Type your message, or /help for commands
# ❯ _

# Try some commands:
❯ Hello! Can you explain what you are?
❯ /help
❯ /models
❯ /status
❯ /exit
```

## 🛠️ Common Commands

```bash
# Interactive chat (default)
llm-conductor
llm-conductor chat

# System status
llm-conductor status

# Available providers/models
llm-conductor providers

# Configuration
llm-conductor config show           # View config
llm-conductor config user           # Update user info
llm-conductor config add-key        # Add API key

# First-time setup (if needed)
llm-conductor setup
```

## 📚 Next Steps

1. **Read** [NIXOS_USAGE.md](NIXOS_USAGE.md) for all configuration options
2. **Explore** [ARCHITECTURE.md](ARCHITECTURE.md) to understand the system
3. **Check** [ROADMAP.md](ROADMAP.md) for upcoming features
4. **Test** The interactive chat and experiment!

## 💡 Tips

- **Ollama models** are downloaded on first use (~1.9GB for qwen2.5:3b)
- **Setup runs once** - creates config in `~/.config/llm-conductor/`
- **Nix handles everything** - Updates, dependencies, cleanup
- **Rollback works** - If something breaks, `nixos-rebuild switch --rollback`

## 🆘 Troubleshooting

### "Ollama not found"
The module should handle this. If not:
```bash
# Check if ollama is in PATH
which ollama

# Try starting manually
ollama serve &
```

### "Setup required" on first run
This is normal! Run:
```bash
llm-conductor setup
```

### Build takes forever
First build downloads and compiles Rust dependencies. Subsequent builds are fast due to Nix caching.

## 🎓 What Makes This Special?

Unlike typical installations:
- ✅ **No manual Ollama install** - Wrapped as dependency
- ✅ **One config line** - `services.llm-conductor.enable = true;`
- ✅ **Reproducible** - Same result on any NixOS system
- ✅ **Rollback-able** - Previous generation always available
- ✅ **Clean removal** - `nix-collect-garbage` removes everything

---

**Ready?** Add it to your config and start orchestrating models! 🚀
