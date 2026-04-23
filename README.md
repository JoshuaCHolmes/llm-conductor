# llm-conductor 🎭

**Multi-model AI orchestration CLI with intelligent routing and zero-cost operation**

[![NixOS](https://img.shields.io/badge/NixOS-Ready-blue.svg)](https://nixos.org)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

> A production-grade CLI for managing multiple AI models with intelligent task routing, context management, and multi-agent orchestration—all while maintaining zero ongoing costs.

## ✨ Features

- 🎯 **Intelligent Model Routing** - Automatically selects the best model for each task
- 🔄 **Multi-Provider Support** - Ollama (local), NVIDIA NIM, GitHub Copilot, TAMU AI
- 📦 **Zero-Config Nix Packaging** - Ollama included as wrapped dependency
- 🚀 **Auto-Installation** - Works on Windows, macOS, Linux, NixOS with fallbacks
- 💰 **Zero Cost Operation** - Aggregate free tiers and local models
- 🎨 **Beautiful CLI** - Clean interface with streaming responses
- 📊 **Resource Tracking** - Monitor per-minute, daily, and monthly usage
- 🔒 **Safe Execution** - Permission system with impact scoring

## 🚀 Quick Start

### NixOS (Recommended)

Add to your `flake.nix`:
```nix
{
  inputs.llm-conductor.url = "github:JoshuaCHolmes/llm-conductor";
  
  # In your configuration:
  services.llm-conductor = {
    enable = true;
    autoStartOllama = true;
  };
}
```

Then:
```bash
sudo nixos-rebuild switch
llm-conductor  # Start chatting!
```

**That's it!** Both llm-conductor and Ollama are installed and configured automatically.

### Other Platforms

See [INSTALLATION.md](INSTALLATION.md) for platform-specific instructions.

## 📚 Usage

```bash
llm-conductor              # Start interactive chat
llm-conductor status       # Show system status
llm-conductor setup        # Run setup wizard
llm-conductor providers    # List providers
llm-conductor config show  # View configuration
```

## 🎯 Goals

1. **Functionally Unlimited Access** - Aggregate free tiers from multiple providers
2. **Zero Ongoing Costs** - Use local models + free API tiers
3. **Intelligent Routing** - Match task complexity to model capability
4. **Production Grade** - Safe, tested, well-documented

## 📖 Documentation

- **[INSTALLATION.md](INSTALLATION.md)** - Installation guide
- **[NIXOS_USAGE.md](NIXOS_USAGE.md)** - NixOS instructions
- **[ARCHITECTURE.md](ARCHITECTURE.md)** - System design
- **[TESTING.md](TESTING.md)** - Testing guide
- **[ROADMAP.md](ROADMAP.md)** - Development roadmap

## 🗺️ Status

**Phase 1: Complete ✅**
- Core infrastructure
- Ollama integration
- Setup wizard
- Nix packaging

**Phase 2: In Progress**
- Multi-model orchestration
- Additional providers
- Resource tracking

See [ROADMAP.md](ROADMAP.md) for details.

## 📝 License

MIT License - see [LICENSE](LICENSE) for details.

## 🔗 Links

- **Repository**: https://github.com/JoshuaCHolmes/llm-conductor
- **Issues**: https://github.com/JoshuaCHolmes/llm-conductor/issues

---

**Built by** Joshua Holmes | CS @ Texas A&M University
