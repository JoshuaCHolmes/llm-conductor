# Platform-Specific Notes

## NixOS

### Ollama Installation

On NixOS, dynamically linked binaries from generic Linux distributions won't work due to NixOS's unique filesystem structure. The installer detects NixOS and uses `nix-env` to install Ollama.

**Automatic Installation (via setup):**
```bash
llm-conductor setup
```

**Manual Installation Options:**

1. **System-wide (recommended for permanent use):**
   ```nix
   # /etc/nixos/configuration.nix
   environment.systemPackages = with pkgs; [
     ollama
   ];
   ```

2. **Home Manager:**
   ```nix
   # ~/.config/home-manager/home.nix
   home.packages = with pkgs; [
     ollama
   ];
   ```

3. **User Profile:**
   ```bash
   nix-env -iA nixpkgs.ollama
   ```

4. **Temporary Shell (for testing):**
   ```bash
   nix-shell -p ollama
   ```

### Running Ollama Server

On NixOS, start the Ollama server with:
```bash
# If installed system-wide or via nix-env
ollama serve

# Or via nix-shell
nix-shell -p ollama --run "ollama serve"
```

## macOS

### Ollama Installation

The installer attempts to use Homebrew if available:
```bash
brew install ollama
```

If Homebrew is not available, manual installation is required:
1. Download from https://ollama.com/download
2. Install the .dmg package

## Windows

### Ollama Installation

Automatic installation is not available for Windows. Options:

1. **WSL2 (Recommended):**
   - Install WSL2: `wsl --install`
   - Install llm-conductor inside WSL
   - Follow Linux instructions

2. **Native Windows:**
   - Download installer from https://ollama.com/download
   - Run the Windows installer

### WSL2 Considerations

- WSL2 has access to GPU via CUDA (if configured)
- File paths use Linux conventions inside WSL
- Can access Windows filesystem via `/mnt/c/`

## General Linux

For non-NixOS Linux distributions, the installer uses the official Ollama install script:
```bash
curl -fsSL https://ollama.com/install.sh | sh
```

This works on:
- Ubuntu / Debian
- Fedora / RHEL
- Arch Linux
- Other systemd-based distributions

## Platform Detection

The installer automatically detects your platform and uses the appropriate installation method:

- **NixOS**: Checks `/etc/os-release` for "nixos" string
- **macOS**: Uses `target_os = "macos"` compile-time flag
- **Windows**: Uses `target_os = "windows"` compile-time flag
- **Generic Linux**: Default for other Linux systems

## Troubleshooting

### NixOS: "ollama: command not found"

If ollama is installed but not found, ensure it's in your PATH:
```bash
# Check if installed
nix-env -q ollama

# Add to current session
export PATH="$HOME/.nix-profile/bin:$PATH"
```

### macOS: Permission Denied

```bash
# Fix Homebrew permissions
sudo chown -R $(whoami) /usr/local/bin /usr/local/lib /usr/local/share
```

### Windows WSL: Can't connect to Ollama

Ensure the Ollama server is running:
```bash
# Inside WSL
ollama serve

# In another terminal
llm-conductor
```

## Future Improvements

- [ ] Add systemd service for automatic Ollama startup (Linux)
- [ ] Add launchd service for automatic startup (macOS)
- [ ] Improve Windows native support
- [ ] Add Docker-based option for maximum portability
- [ ] Auto-detect and use existing Ollama installations
