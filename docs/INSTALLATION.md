# Installation Guide

## Quick Start (All Platforms)

### Download Binary
```bash
# Coming soon: Download pre-built binary
curl -L https://github.com/yourusername/llm-conductor/releases/latest/download/llm-conductor-$(uname -s)-$(uname -m) -o llm-conductor
chmod +x llm-conductor
./llm-conductor setup
```

## NixOS / Nix Package Manager

### Option 1: System-wide Installation (NixOS)

Add to `/etc/nixos/configuration.nix`:

```nix
{
  # Add the flake input
  inputs.llm-conductor.url = "github:yourusername/llm-conductor";
  
  # In your configuration
  services.llm-conductor = {
    enable = true;
    autoStartOllama = true;  # Automatically start Ollama service
  };
}
```

Then rebuild:
```bash
sudo nixos-rebuild switch
```

**This automatically:**
- ✅ Installs `llm-conductor` command
- ✅ Installs and wraps `ollama` as a dependency
- ✅ Starts Ollama service on boot (if autoStartOllama = true)
- ✅ Makes both available in your PATH

### Option 2: User Installation (Home Manager)

Add to `~/.config/home-manager/home.nix`:

```nix
{
  # Add the flake input
  inputs.llm-conductor.url = "github:yourusername/llm-conductor";
  
  # In your home configuration
  programs.llm-conductor = {
    enable = true;
  };
}
```

Then rebuild:
```bash
home-manager switch
```

### Option 3: Direct Flake Usage (No Config Changes)

Run without installing:
```bash
nix run github:yourusername/llm-conductor
```

Install to user profile:
```bash
nix profile install github:yourusername/llm-conductor
```

### Option 4: Development Shell

Clone and enter dev environment:
```bash
git clone https://github.com/yourusername/llm-conductor
cd llm-conductor
nix develop  # Enters shell with Rust, Ollama, and all dependencies
```

### Why Nix is Great for This

When you install via Nix:
1. **Ollama is handled automatically** - No separate installation needed
2. **Reproducible** - Exact same versions everywhere
3. **Isolated** - Won't conflict with other installations
4. **PATH wrapping** - `ollama` is automatically available when you run `llm-conductor`
5. **Garbage collection** - Clean removal with `nix-collect-garbage`

The `flake.nix` uses `wrapProgram` to ensure `ollama` is always in PATH when running `llm-conductor`, even though they're separate packages.

## Linux (Non-Nix)

### Ubuntu / Debian
```bash
curl -fsSL https://raw.githubusercontent.com/yourusername/llm-conductor/main/install.sh | bash
```

Or manually:
```bash
# Download binary
wget https://github.com/yourusername/llm-conductor/releases/latest/download/llm-conductor-linux-x64
chmod +x llm-conductor-linux-x64
sudo mv llm-conductor-linux-x64 /usr/local/bin/llm-conductor

# Run setup (will auto-install Ollama)
llm-conductor setup
```

## macOS

### Via Homebrew (Coming Soon)
```bash
brew install yourusername/tap/llm-conductor
llm-conductor setup
```

### Manual
```bash
# Download binary
curl -L https://github.com/yourusername/llm-conductor/releases/latest/download/llm-conductor-darwin-arm64 -o llm-conductor
chmod +x llm-conductor
sudo mv llm-conductor /usr/local/bin/

# Run setup (will use Homebrew to install Ollama if available)
llm-conductor setup
```

## Windows

### Native Windows
```bash
# Download installer
curl -L https://github.com/yourusername/llm-conductor/releases/latest/download/llm-conductor-setup.exe -o llm-conductor-setup.exe

# Run installer (will auto-install Ollama)
.\llm-conductor-setup.exe

# Or run setup after
llm-conductor setup
```

### WSL2 (Recommended for Development)
```bash
# Inside WSL
curl -fsSL https://raw.githubusercontent.com/yourusername/llm-conductor/main/install.sh | bash
llm-conductor setup
```

## Docker

```bash
docker run -it --rm \
  -v ~/.config/llm-conductor:/root/.config/llm-conductor \
  ghcr.io/yourusername/llm-conductor:latest
```

## Build from Source

### Requirements
- Rust 1.75+ (or use Nix dev shell)
- OpenSSL development libraries

### All Platforms
```bash
# Clone repository
git clone https://github.com/yourusername/llm-conductor
cd llm-conductor

# Build
cargo build --release

# Install
cargo install --path .

# Or copy binary
cp target/release/llm-conductor ~/.local/bin/
```

### Using Nix (Easiest for Development)
```bash
git clone https://github.com/yourusername/llm-conductor
cd llm-conductor

# Enter dev shell (includes all dependencies)
nix develop

# Build and run
cargo build
cargo run -- setup
```

## Post-Installation

After installation via any method, run the setup wizard:

```bash
llm-conductor setup
```

This will:
1. Collect user information (optional)
2. Install Ollama (if not present, **skipped on NixOS module installation**)
3. Download recommended local models
4. Configure API keys for cloud providers (optional)

## Verification

Check installation:
```bash
llm-conductor status
```

Should show:
- ✓ Ollama installed and running
- ✓ Local models available
- ✓ User configuration present

## Next Steps

- See [USAGE.md](USAGE.md) for how to use the CLI
- See [PLATFORM_NOTES.md](PLATFORM_NOTES.md) for platform-specific details
- See [ARCHITECTURE.md](ARCHITECTURE.md) for system design

## Troubleshooting

### NixOS: Command not found after installation

Rebuild your system:
```bash
sudo nixos-rebuild switch
```

Or for home-manager:
```bash
home-manager switch
```

### Other Linux: Ollama not starting

Start manually:
```bash
ollama serve &
```

Or add to systemd:
```bash
sudo systemctl enable ollama
sudo systemctl start ollama
```

### Windows: Installer blocked

Right-click → Properties → Unblock → Apply

### macOS: "Unidentified developer"

```bash
xattr -d com.apple.quarantine /usr/local/bin/llm-conductor
```
