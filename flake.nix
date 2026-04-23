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
    let
      # Modules that don't depend on system
      modules = {
        nixosModules.default = { config, lib, pkgs, ... }:
          with lib;
          let
            cfg = config.services.llm-conductor;
          in
          {
            options.services.llm-conductor = {
              enable = mkEnableOption "LLM Conductor";
              
              enableOllama = mkOption {
                type = types.bool;
                default = false;
                description = "Install Ollama and optionally auto-start service (for local model support)";
              };
              
              autoStartOllama = mkOption {
                type = types.bool;
                default = true;
                description = "Automatically start Ollama service (requires enableOllama = true)";
              };
            };
            
            config = mkIf cfg.enable {
              environment.systemPackages = [
                self.packages.${pkgs.system}.default
              ] ++ optional cfg.enableOllama pkgs.ollama;
              
              services.ollama = mkIf (cfg.enableOllama && cfg.autoStartOllama) {
                enable = true;
              };
            };
          };
        
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
                default = self.packages.${pkgs.system}.default;
                description = "The llm-conductor package to use";
              };
            };
            
            config = mkIf cfg.enable {
              home.packages = [
                cfg.package
                pkgs.ollama
              ];
            };
          };
      };
    in
    modules // flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" ];
        };
        
        # Note: Ollama is now optional and installed via NixOS module if enableOllama = true
        # The binary will detect and use Ollama if available in PATH
        
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
          ];
          
          buildInputs = with pkgs; [
            openssl
          ] ++ lib.optionals stdenv.isDarwin [
            darwin.apple_sdk.frameworks.Security
            darwin.apple_sdk.frameworks.SystemConfiguration
          ];
          
          meta = with pkgs.lib; {
            description = "Multi-model AI orchestration CLI with intelligent routing";
            homepage = "https://github.com/JoshuaCHolmes/llm-conductor";
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
      }
    );
}
