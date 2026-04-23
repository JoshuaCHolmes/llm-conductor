# Nix Packaging Explanation

## How Dependencies Work in Nix Packages

### The Problem

When you build a Rust binary that expects `ollama` to be available in PATH, it won't work if Ollama isn't installed. Normally, this would require users to:
1. Install `llm-conductor`
2. Separately install `ollama`
3. Ensure both are in PATH

### The Nix Solution: `wrapProgram`

Nix uses **PATH wrapping** to automatically make dependencies available. Here's how it works:

```nix
# In flake.nix
runtimeDeps = with pkgs; [
  ollama  # This is the dependency
];

# Later in buildRustPackage
postInstall = ''
  wrapProgram $out/bin/llm-conductor \
    --prefix PATH : ${pkgs.lib.makeBinPath runtimeDeps}
'';
```

### What This Does

1. **Builds the Rust binary** normally via `cargo build`
2. **Creates a wrapper script** that sets up the environment
3. **Adds Ollama's bin directory** to PATH before running

The final result looks like:
```
/nix/store/xxx-llm-conductor/bin/llm-conductor
  └─> (wrapper script)
      ├─> Sets PATH="/nix/store/yyy-ollama/bin:$PATH"
      └─> Runs /nix/store/xxx-llm-conductor/bin/.llm-conductor-wrapped
```

### User Experience

When a user installs via Nix:

```nix
# In configuration.nix or home.nix
services.llm-conductor.enable = true;
```

**This automatically:**
- ✅ Installs `llm-conductor` binary
- ✅ Installs `ollama` as a dependency
- ✅ Makes `ollama` available whenever you run `llm-conductor`
- ✅ Both commands work system-wide
- ✅ No manual installation needed

### Detection in Our Code

We detect if Ollama is provided by Nix:

```rust
fn is_nix_wrapped() -> bool {
    if let Ok(ollama_path) = which::which("ollama") {
        if let Some(path_str) = ollama_path.to_str() {
            return path_str.contains("/nix/store");
        }
    }
    false
}
```

If true, we skip the auto-install step since Nix already provides it.

## NixOS Module vs Home Manager Module

### NixOS Module (System-wide)
```nix
# /etc/nixos/configuration.nix
services.llm-conductor = {
  enable = true;
  autoStartOllama = true;
};
```

**Provides:**
- System-wide installation (all users)
- Systemd service for Ollama (optional)
- GPU acceleration configuration
- Root-level management

### Home Manager Module (Per-user)
```nix
# ~/.config/home-manager/home.nix
programs.llm-conductor = {
  enable = true;
};
```

**Provides:**
- User-specific installation
- No root access needed
- User config in ~/.config/llm-conductor
- Per-user Ollama service

## Why This is Better Than Traditional Packaging

### Traditional Package (e.g., .deb, .rpm)
```bash
# Install package
apt install llm-conductor

# Uh oh, missing dependency!
# User must manually:
apt install ollama
```

**Problems:**
- Dependency resolution at runtime
- Version conflicts possible
- Manual intervention required
- Can break with updates

### Nix Package
```bash
# Install via NixOS config
services.llm-conductor.enable = true;
nixos-rebuild switch
```

**Benefits:**
- ✅ All dependencies automatically resolved
- ✅ Exact versions guaranteed (via lockfile)
- ✅ No version conflicts (isolated in /nix/store)
- ✅ Rollback if something breaks
- ✅ Works exactly the same everywhere

## Build Process

### 1. Cargo builds the Rust binary
```bash
cargo build --release
# Produces: target/release/llm-conductor
```

### 2. Nix wraps the binary
```bash
# Original binary moved to .llm-conductor-wrapped
mv $out/bin/llm-conductor $out/bin/.llm-conductor-wrapped

# Create wrapper script
cat > $out/bin/llm-conductor <<EOF
#!/bin/sh
export PATH="/nix/store/yyy-ollama/bin:$PATH"
exec $out/bin/.llm-conductor-wrapped "$@"
EOF

chmod +x $out/bin/llm-conductor
```

### 3. User runs the command
```bash
$ llm-conductor setup

# This actually runs:
# PATH="/nix/store/.../ollama/bin:$PATH" \
#   /nix/store/.../llm-conductor/bin/.llm-conductor-wrapped setup

# So when our code does:
Command::new("ollama").arg("serve").spawn()

# It finds: /nix/store/.../ollama/bin/ollama
# Because the wrapper put it in PATH!
```

## Testing the Flake

### Local Development
```bash
# Build the flake
nix build

# Run directly
./result/bin/llm-conductor

# Check that ollama is in PATH
./result/bin/llm-conductor status
# Should show: ✓ Ollama installed (via Nix wrapper)
```

### Install to Profile
```bash
nix profile install .
llm-conductor status
```

### Development Shell
```bash
nix develop
# Now both cargo and ollama are available
cargo run
```

## Common Patterns

### Runtime Dependencies Only
```nix
# Ollama is only needed at runtime, not build time
nativeBuildInputs = []; # Build-time only
buildInputs = [];       # Link-time only
runtimeDeps = [ ollama ]; # Runtime PATH
```

### Optional Dependencies
```nix
# Make ollama optional
postInstall = ''
  wrapProgram $out/bin/llm-conductor \
    --prefix PATH : ${pkgs.lib.makeBinPath runtimeDeps} \
    --set LLM_CONDUCTOR_OLLAMA_OPTIONAL "1"
'';
```

### Multiple Runtime Dependencies
```nix
runtimeDeps = with pkgs; [
  ollama
  git
  curl
  # Any other tools needed at runtime
];
```

## Advantages for Users

1. **Zero Configuration**: Just add one line to NixOS config
2. **Reproducible**: Same versions everywhere
3. **Safe**: Can't break system packages
4. **Reversible**: Easy to uninstall cleanly
5. **Composable**: Works with other Nix packages
6. **Transparent**: User doesn't need to know about Ollama

## Comparison with Other Languages

### Python (with dependencies)
```nix
# Python packages bundle dependencies differently
python3Packages.buildPythonApplication {
  propagatedBuildInputs = [ other-python-packages ];
  # But for external binaries, still needs wrapProgram
}
```

### Go (static binaries)
```nix
# Go binaries are often static
buildGoModule {
  # No runtime deps needed usually
  # But if calling external commands, same approach
}
```

### Rust (our case)
```nix
# Rust binaries are dynamic but self-contained
# External tool calls need PATH wrapping
rustPlatform.buildRustPackage {
  postInstall = "wrapProgram ...";
}
```

## Future Improvements

- [ ] Make Ollama truly optional (flag to disable)
- [ ] Support alternative local model runners
- [ ] Bundle small models in the Nix package
- [ ] Create overlay for easy customization
- [ ] Add NixOS tests to CI
