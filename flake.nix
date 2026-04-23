{
  description = "LLM Conductor - Multi-model AI orchestration CLI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" ];
        };
        
        # Runtime dependencies that will be available in PATH
        runtimeDeps = with pkgs; [
          ollama  # Ollama will be automatically available
        ];
        
        llm-conductor = pkgs.rustPlatform.buildRustPackage {
          pname = "llm-conductor";
          version = "0.1.0";
          
          src = ./.;
          
          cargoLock = {
            lockFile = ./Cargo.lock;
          };
          
          nativeBuildInputs = with pkgs; [
            rustToolchain
            pkg-config
            makeWrapper  # For wrapping the binary
          ];
          
          buildInputs = with pkgs; [
            openssl
          ] ++ lib.optionals stdenv.isDarwin [
            darwin.apple_sdk.frameworks.Security
            darwin.apple_sdk.frameworks.SystemConfiguration
          ];
          
          # Wrap the binary to include ollama in PATH
          postInstall = ''
            wrapProgram $out/bin/llm-conductor \
              --prefix PATH : ${pkgs.lib.makeBinPath runtimeDeps}
          '';
          
          meta = with pkgs.lib; {
            description = "Multi-model AI orchestration CLI with intelligent routing";
            homepage = "https://github.com/yourusername/llm-conductor";
            license = licenses.mit;
            maintainers = [ ];
            platforms = platforms.unix;
          };
        };
        
      in
      {
        # Default package
        packages.default = llm-conductor;
        
        # Named package
        packages.llm-conductor = llm-conductor;
        
        # Development shell
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustToolchain
            rust-analyzer
            pkg-config
            openssl
            ollama  # Available in dev shell
          ];
          
          shellHook = ''
            echo "🎭 llm-conductor development environment"
            echo "Ollama: $(ollama --version 2>/dev/null || echo 'checking...')"
            echo ""
            echo "Commands:"
            echo "  cargo build         - Build the project"
            echo "  cargo run           - Run the CLI"
            echo "  cargo test          - Run tests"
            echo "  ollama serve &      - Start Ollama server"
          '';
        };
        
        # NixOS module for system-wide installation
        nixosModules.default = { config, lib, pkgs, ... }:
          with lib;
          let
            cfg = config.services.llm-conductor;
          in
          {
            options.services.llm-conductor = {
              enable = mkEnableOption "LLM Conductor";
              
              autoStartOllama = mkOption {
                type = types.bool;
                default = true;
                description = "Automatically start Ollama service";
              };
            };
            
            config = mkIf cfg.enable {
              environment.systemPackages = [
                llm-conductor
                pkgs.ollama  # Ensure ollama is installed
              ];
              
              # Optionally enable Ollama service
              services.ollama = mkIf cfg.autoStartOllama {
                enable = true;
                acceleration = "auto";  # Use GPU if available
              };
            };
          };
        
        # Home Manager module for user installation
        homeManagerModules.default = { config, lib, pkgs, ... }:
          with lib;
          let
            cfg = config.programs.llm-conductor;
          in
          {
            options.programs.llm-conductor = {
              enable = mkEnableOption "LLM Conductor";
              
              package = mkOption {
                type = types.package;
                default = llm-conductor;
                description = "The llm-conductor package to use";
              };
            };
            
            config = mkIf cfg.enable {
              home.packages = [
                cfg.package
                pkgs.ollama  # Ensure ollama is available
              ];
            };
          };
      }
    );
}
