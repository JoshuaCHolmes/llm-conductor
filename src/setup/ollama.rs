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
    
    #[cfg(target_os = "linux")]
    async fn install_linux() -> Result<()> {
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
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Ollama installation failed: {}", stderr));
        }
        
        // Clean up
        let _ = std::fs::remove_file(temp_script);
        
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
        println!("Automatic installation not available for Windows.");
        println!();
        println!("Please install Ollama manually:");
        println!("  1. Download from: https://ollama.com/download");
        println!("  2. Or use WSL: wsl --install");
        
        Err(anyhow!("Manual installation required"))
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
