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
        use colored::*;
        
        // Check if running on NixOS
        if Self::is_nixos() {
            return Self::install_nixos().await;
        }
        
        println!("{}", "Installing Ollama on Linux...".bright_yellow());
        
        // Try official install script first
        match Self::install_linux_via_script().await {
            Ok(_) => {
                println!("{}", "✓ Installed via official script".bright_green());
                return Ok(());
            }
            Err(e) => {
                println!("{}", format!("⚠ Official script failed: {}", e).yellow());
                println!("{}", "Trying direct binary download...".cyan());
            }
        }
        
        // Fallback: Direct binary download
        Self::install_linux_binary().await
    }
    
    #[cfg(target_os = "linux")]
    async fn install_linux_via_script() -> Result<()> {
        // Download and run official install script
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
        
        // Clean up
        let _ = std::fs::remove_file(temp_script);
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("{}", stderr));
        }
        
        Ok(())
    }
    
    #[cfg(target_os = "linux")]
    async fn install_linux_binary() -> Result<()> {
        use colored::*;
        
        // Detect architecture
        let arch = std::env::consts::ARCH;
        let binary_url = match arch {
            "x86_64" => "https://ollama.com/download/ollama-linux-amd64",
            "aarch64" => "https://ollama.com/download/ollama-linux-arm64",
            _ => return Err(anyhow!("Unsupported architecture: {}", arch)),
        };
        
        println!("Downloading Ollama binary for {}...", arch);
        let response = reqwest::get(binary_url).await?;
        let bytes = response.bytes().await?;
        
        // Try to install to /usr/local/bin (may need sudo)
        let install_path = "/usr/local/bin/ollama";
        
        match tokio::fs::write(install_path, &bytes).await {
            Ok(_) => {
                // Set executable permissions
                let _ = Command::new("chmod")
                    .arg("+x")
                    .arg(install_path)
                    .output();
                
                println!("{}", "✓ Installed to /usr/local/bin/ollama".bright_green());
            }
            Err(_) => {
                // Fallback to user home directory
                let home = std::env::var("HOME")?;
                let user_bin = format!("{}/.local/bin", home);
                std::fs::create_dir_all(&user_bin)?;
                
                let user_path = format!("{}/ollama", user_bin);
                tokio::fs::write(&user_path, &bytes).await?;
                
                let _ = Command::new("chmod")
                    .arg("+x")
                    .arg(&user_path)
                    .output();
                
                println!("{}", format!("✓ Installed to {}", user_path).bright_green());
                println!("{}", format!("Add to PATH: export PATH=\"{}:$PATH\"", user_bin).yellow());
            }
        }
        
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
        use colored::*;
        
        println!("{}", "Installing Ollama on macOS...".bright_yellow());
        
        // Try homebrew first (fastest)
        if which::which("brew").is_ok() {
            println!("Installing via Homebrew...");
            
            let output = Command::new("brew")
                .args(&["install", "ollama"])
                .output()?;
            
            if output.status.success() {
                println!("{}", "✓ Installed via Homebrew".bright_green());
                return Ok(());
            }
        }
        
        // Fall back to direct download and installation
        println!("{}", "Homebrew not available, downloading installer...".yellow());
        
        let dmg_url = "https://ollama.com/download/Ollama-darwin.zip";
        let temp_dir = std::env::temp_dir();
        let zip_path = temp_dir.join("Ollama.zip");
        let app_path = temp_dir.join("Ollama.app");
        
        // Download the zip
        println!("Downloading Ollama...");
        let response = reqwest::get(dmg_url).await?;
        let bytes = response.bytes().await?;
        tokio::fs::write(&zip_path, bytes).await?;
        
        // Unzip
        println!("Extracting...");
        let output = Command::new("unzip")
            .arg("-q")
            .arg(&zip_path)
            .arg("-d")
            .arg(&temp_dir)
            .output()?;
        
        if !output.status.success() {
            return Err(anyhow!("Failed to extract Ollama"));
        }
        
        // Move to Applications
        println!("Installing to /Applications...");
        let output = Command::new("mv")
            .arg(&app_path)
            .arg("/Applications/Ollama.app")
            .output()?;
        
        if !output.status.success() {
            return Err(anyhow!("Failed to move to Applications. You may need sudo."));
        }
        
        // Add CLI to PATH by symlinking
        let cli_source = "/Applications/Ollama.app/Contents/Resources/ollama";
        let cli_target = "/usr/local/bin/ollama";
        
        println!("Creating CLI symlink...");
        let _ = Command::new("ln")
            .arg("-sf")
            .arg(cli_source)
            .arg(cli_target)
            .output();
        
        // Clean up
        let _ = std::fs::remove_file(zip_path);
        
        println!("{}", "✓ Ollama installed successfully".bright_green());
        println!("{}", "Note: You may need to start Ollama from Applications.".yellow());
        
        Ok(())
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
