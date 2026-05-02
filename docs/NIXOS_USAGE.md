# Adding llm-conductor to Your NixOS Configuration

## Quick Start

Add to your `flake.nix` inputs:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    llm-conductor.url = "github:JoshuaCHolmes/llm-conductor";
    # ... other inputs
  };

  outputs = { self, nixpkgs, llm-conductor, ... }: {
    nixosConfigurations.yourhost = nixpkgs.lib.nixosSystem {
      modules = [
        # Import the llm-conductor module
        llm-conductor.nixosModules.default
        
        ./configuration.nix
      ];
    };
  };
}
```

## Option 1: System-Wide Installation (Recommended)

In your `configuration.nix`:

```nix
{ config, pkgs, ... }:

{
  # Enable llm-conductor with Ollama service
  services.llm-conductor = {
    enable = true;
    autoStartOllama = true;  # Start Ollama on boot
  };
}
```

This automatically:
- Installs `llm-conductor` command
- Installs `ollama` as a wrapped dependency
- Starts Ollama service on boot
- Makes both available to all users

After rebuild:
```bash
sudo nixos-rebuild switch
llm-conductor status
```

## Option 2: User-Specific Installation (Development)

If you want it per-user via Home Manager:

In your `flake.nix`:
```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    home-manager.url = "github:nix-community/home-manager";
    llm-conductor.url = "github:JoshuaCHolmes/llm-conductor";
  };

  outputs = { self, nixpkgs, home-manager, llm-conductor, ... }: {
    homeConfigurations.yourusername = home-manager.lib.homeManagerConfiguration {
      modules = [
        llm-conductor.homeManagerModules.default
        ./home.nix
      ];
    };
  };
}
```

In your `home.nix`:
```nix
{ config, pkgs, ... }:

{
  programs.llm-conductor = {
    enable = true;
  };
}
```

Then rebuild:
```bash
home-manager switch
llm-conductor status
```

## Option 3: Just the Package (No Module)

If you just want the binary without the module:

```nix
{ config, pkgs, inputs, ... }:

{
  environment.systemPackages = [
    inputs.llm-conductor.packages.${pkgs.system}.default
    pkgs.ollama  # Still need to manually add Ollama
  ];
}
```

Or in home-manager:
```nix
{ config, pkgs, inputs, ... }:

{
  home.packages = [
    inputs.llm-conductor.packages.${pkgs.system}.default
    pkgs.ollama
  ];
}
```

## Option 4: Development Overlay

If you want to use it as an overlay:

```nix
{ config, pkgs, inputs, ... }:

{
  nixpkgs.overlays = [
    (final: prev: {
      llm-conductor = inputs.llm-conductor.packages.${prev.system}.default;
    })
  ];
  
  environment.systemPackages = [ pkgs.llm-conductor ];
}
```

## Testing Before Committing to Config

### Try it without installing:
```bash
nix run github:JoshuaCHolmes/llm-conductor -- status
```

### Enter a development shell:
```bash
nix develop github:JoshuaCHolmes/llm-conductor
cargo build
cargo run
```

### Build and inspect:
```bash
nix build github:JoshuaCHolmes/llm-conductor
./result/bin/llm-conductor status
```

## Your Development Config Addon

If you have a development-specific config file (e.g., `development.nix`), add:

```nix
# development.nix
{ config, pkgs, inputs, ... }:

{
  # Option 1: Use the module (recommended)
  services.llm-conductor = {
    enable = true;
    autoStartOllama = true;
  };
  
  # OR Option 2: Just add to packages
  environment.systemPackages = [
    inputs.llm-conductor.packages.${pkgs.system}.default
  ];
}
```

Then in your main `flake.nix`, include it conditionally:

```nix
nixosConfigurations.yourhost = nixpkgs.lib.nixosSystem {
  modules = [
    llm-conductor.nixosModules.default
    ./configuration.nix
    ./development.nix  # Your optional dev tools
  ];
};
```

## Updating

To update to the latest version:

```bash
# Update the flake lock
nix flake lock --update-input llm-conductor

# Rebuild
sudo nixos-rebuild switch
```

Or auto-update periodically:
```nix
{
  inputs.llm-conductor.url = "github:JoshuaCHolmes/llm-conductor/main";
  # This always tracks the main branch
}
```

## Pinning to a Specific Version

To pin to a specific commit:
```nix
{
  inputs.llm-conductor.url = "github:JoshuaCHolmes/llm-conductor?rev=228fba7...";
}
```

Or a tag (once you create releases):
```nix
{
  inputs.llm-conductor.url = "github:JoshuaCHolmes/llm-conductor?ref=v0.1.0";
}
```

## Verifying the Installation

After rebuild:
```bash
# Check it's installed
which llm-conductor
# Should show: /run/current-system/sw/bin/llm-conductor

# Check ollama is wrapped
llm-conductor status
# Should show: ✓ Ollama Running

# Try it out
llm-conductor
```

## Troubleshooting

### "Could not find ollama"
Make sure you're using the module or have ollama in systemPackages:
```nix
services.llm-conductor.enable = true;
# OR
environment.systemPackages = [ pkgs.ollama ];
```

### "Setup required"
First run needs setup:
```bash
llm-conductor setup
```

### Flake not found
Make sure the input is added and you've updated:
```bash
nix flake lock
sudo nixos-rebuild switch --show-trace
```

## Example Full Configuration

```nix
# flake.nix
{
  description = "My NixOS configuration";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    llm-conductor.url = "github:JoshuaCHolmes/llm-conductor";
  };

  outputs = { self, nixpkgs, llm-conductor, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      specialArgs = { inherit inputs; };
      modules = [
        llm-conductor.nixosModules.default
        {
          services.llm-conductor = {
            enable = true;
            autoStartOllama = true;
          };
        }
      ];
    };
  };
}
```

That's it! Now `llm-conductor` and `ollama` are both available system-wide with zero additional configuration needed.
