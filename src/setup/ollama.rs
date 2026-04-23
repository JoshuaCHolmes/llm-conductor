use anyhow::{anyhow, Result};
use std::process::Command;
use tokio::time::Duration;

/// Status of Ollama installation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStatus {
    InstalledAndRunning,
    InstalledNotRunning,
    NotInstalled,
}

/// Manages Ollama installation and server lifecycle
pub struct OllamaInstaller;

impl OllamaInstaller {
    /// Check current Ollama installation status
    pub async fn check_installation() -> InstallStatus {
        // 1. Check if ollama binary exists
        if which::which("ollama").is_ok() {
            // 2. Check if server is running
            if Self::is_server_running().await {
                InstallStatus::InstalledAndRunning
            } else {
                InstallStatus::InstalledNotRunning
            }
        } else {
            InstallStatus::NotInstalled
        }
    }
    
    /// Check if Ollama server is responding
    pub async fn is_server_running() -> bool {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        
        client
            .get("http://localhost:11434/api/tags")
            .send()
            .await
            .is_ok()
    }
    
    /// Install Ollama automatically
    pub async fn install() -> Result<()> {
        use colored::*;
        
        // Skip installation if wrapped by Nix flake
        if Self::is_nix_wrapped() {
            println!("{}", "✓ Ollama is provided by Nix package wrapper".bright_green());
            println!("{}", "  No manual installation needed.".cyan());
            return Ok(());
        }
        
        println!("{}", "Installing Ollama...".bright_yellow());
        
        #[cfg(target_os = "linux")]
        {
            Self::install_linux().await?;
        }
        
        #[cfg(target_os = "macos")]
        {
            Self::install_macos().await?;
        }
        
        #[cfg(target_os = "windows")]
        {
            Self::install_windows().await?;
        }
        
        println!("{}", "✓ Ollama installed successfully".bright_green());
        
        Ok(())
    }
    
    /// Check if running from Nix-wrapped binary
    fn is_nix_wrapped() -> bool {
        // Check if PATH contains /nix/store and ollama is from there
        if let Ok(ollama_path) = which::which("ollama") {
            if let Some(path_str) = ollama_path.to_str() {
                return path_str.contains("/nix/store");
            }
        }
        false
    }
    
    #[cfg(target_os = "linux")]
    async fn install_linux() -> Result<()> {
        // Check if running on NixOS
        if Self::is_nixos() {
            return Self::install_nixos().await;
        }
        
        // Download and run official install script for generic Linux
        let script = reqwest::get("https://ollama.com/install.sh")
            .await?
            .text()
            .await?;
        
        let temp_script = "/tmp/ollama-install.sh";
        tokio::fs::write(temp_script, script).await?;
        
        let output = Command::new("sh")
            .arg(temp_script)
            .output()
            .map_err(|e| anyhow!("Failed to execute install script: {}", e))?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Ollama installation failed: {}", stderr));
        }
        
        // Clean up
        let _ = std::fs::remove_file(temp_script);
        
        Ok(())
    }
    
    /// Check if running on NixOS
    #[cfg(target_os = "linux")]
    fn is_nixos() -> bool {
        std::fs::read_to_string("/etc/os-release")
            .map(|content| content.to_lowercase().contains("nixos"))
            .unwrap_or(false)
    }
    
    /// Install Ollama on NixOS using nix-env
    #[cfg(target_os = "linux")]
    async fn install_nixos() -> Result<()> {
        use colored::*;
        
        println!("{}", "Detected NixOS - installing via nix-env...".bright_cyan());
        println!("{}", "Note: For persistent installation, add 'ollama' to your system packages.".yellow());
        
        // Install ollama to user profile
        let output = Command::new("nix-env")
            .args(&["-iA", "nixpkgs.ollama"])
            .output()
            .map_err(|e| anyhow!("Failed to run nix-env: {}", e))?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "NixOS Ollama installation failed: {}\n\n\
                Alternative installation methods:\n\
                  1. Add to system packages: environment.systemPackages = [ pkgs.ollama ];\n\
                  2. Use nix-shell: nix-shell -p ollama\n\
                  3. Add to home-manager: home.packages = [ pkgs.ollama ];",
                stderr
            ));
        }
        
        println!("{}", "✓ Ollama installed via Nix".bright_green());
        
        Ok(())
    }
    
    #[cfg(target_os = "macos")]
    async fn install_macos() -> Result<()> {
        // Try homebrew first
        if which::which("brew").is_ok() {
            println!("Installing via Homebrew...");
            
            let output = Command::new("brew")
                .args(&["install", "ollama"])
                .output()?;
            
            if output.status.success() {
                return Ok(());
            }
        }
        
        // Fall back to manual download
        println!("Homebrew not available or installation failed.");
        println!("Please download Ollama from: https://ollama.com/download");
        println!("Or install Homebrew and try again.");
        
        Err(anyhow!("Manual installation required"))
    }
    
    #[cfg(target_os = "windows")]
    async fn install_windows() -> Result<()> {
        use colored::*;
        
        println!("{}", "Installing Ollama on Windows...".bright_yellow());
        
        // Check if running in WSL
        if Self::is_wsl() {
            println!("{}", "Detected WSL - using Linux installation method".bright_cyan());
            return Self::install_linux().await;
        }
        
        // Download the Windows installer
        let installer_url = "https://ollama.com/download/OllamaSetup.exe";
        let temp_path = std::env::temp_dir().join("OllamaSetup.exe");
        
        println!("Downloading Ollama installer...");
        let response = reqwest::get(installer_url).await?;
        let bytes = response.bytes().await?;
        tokio::fs::write(&temp_path, bytes).await?;
        
        println!("Running installer...");
        let output = Command::new(&temp_path)
            .arg("/SILENT")  // Silent installation
            .output()
            .map_err(|e| anyhow!("Failed to run installer: {}", e))?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Installation failed: {}", stderr));
        }
        
        // Clean up
        let _ = std::fs::remove_file(temp_path);
        
        println!("{}", "✓ Ollama installed successfully".bright_green());
        println!("{}", "Note: You may need to restart your terminal.".yellow());
        
        Ok(())
    }
    
    /// Check if running in WSL
    #[cfg(target_os = "windows")]
    fn is_wsl() -> bool {
        std::fs::read_to_string("/proc/version")
            .map(|content| content.to_lowercase().contains("microsoft"))
            .unwrap_or(false)
    }
    
    /// Start Ollama server in background
    pub async fn start_server() -> Result<()> {
        use colored::*;
        
        println!("{}", "Starting Ollama server...".bright_yellow());
        
        // Start as background process
        #[cfg(not(target_os = "windows"))]
        {
            Command::new("ollama")
                .arg("serve")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| anyhow!("Failed to start Ollama: {}", e))?;
        }
        
        #[cfg(target_os = "windows")]
        {
            Command::new("ollama")
                .arg("serve")
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .spawn()
                .map_err(|e| anyhow!("Failed to start Ollama: {}", e))?;
        }
        
        // Wait for server to be ready
        for i in 0..20 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            
            if Self::is_server_running().await {
                println!("{}", "✓ Ollama server started".bright_green());
                return Ok(());
            }
            
            if i == 10 {
                println!("{}", "Still waiting for Ollama to start...".dimmed());
            }
        }
        
        Err(anyhow!("Ollama server failed to start within 10 seconds"))
    }
    
    /// Stop Ollama server
    pub async fn stop_server() -> Result<()> {
        #[cfg(not(target_os = "windows"))]
        {
            Command::new("pkill")
                .arg("ollama")
                .status()?;
        }
        
        #[cfg(target_os = "windows")]
        {
            Command::new("taskkill")
                .args(&["/IM", "ollama.exe", "/F"])
                .status()?;
        }
        
        Ok(())
    }
    
    /// Get Ollama version
    pub fn get_version() -> Result<String> {
        let output = Command::new("ollama")
            .arg("--version")
            .output()?;
        
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(anyhow!("Failed to get Ollama version"))
        }
    }
}
