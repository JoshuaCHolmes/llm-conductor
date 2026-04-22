# Headless GUI and Game Testing System

## Virtual Display & Rendering Solutions

### The Virtual Framebuffer Approach

**Core Concept:** Run GUI apps in a virtual display that models can "see" and interact with, without showing on user's screen.

```rust
pub enum VirtualDisplayBackend {
    /// X Virtual Framebuffer (Linux)
    Xvfb {
        display: String,      // ":99"
        resolution: String,   // "1920x1080x24"
    },
    
    /// Wayland on top of virtual GPU
    WaylandHeadless {
        compositor: WaylandCompositor,
    },
    
    /// Mesa's software rendering (no GPU needed)
    LLVMPipe {
        display: String,
    },
    
    /// WebGL via headless browser (for web-based GUIs)
    HeadlessBrowser {
        browser: BrowserEngine,
    },
}

pub enum WaylandCompositor {
    Weston,   // Reference compositor with headless backend
    Sway,     // Tiling compositor (can run headless)
}

pub enum BrowserEngine {
    Chromium,  // Via Puppeteer/Playwright
    Firefox,   // Via Playwright
    WebKit,    // Via Playwright
}
```

### Implementation: Xvfb (X Virtual Framebuffer)

Most common solution for Linux GUIs:

```rust
pub struct VirtualDisplay {
    backend: VirtualDisplayBackend,
    process: Option<Child>,
    display_id: String,
    screenshot_dir: PathBuf,
}

impl VirtualDisplay {
    /// Start virtual display
    pub async fn start() -> Result<Self> {
        let display_id = Self::find_free_display()?;
        
        info!("Starting Xvfb on display {}", display_id);
        
        // Start Xvfb process
        let process = Command::new("Xvfb")
            .arg(&display_id)              // :99
            .arg("-screen")
            .arg("0")
            .arg("1920x1080x24")           // Resolution & color depth
            .arg("-ac")                    // Disable access control
            .arg("+extension")
            .arg("GLX")                    // Enable OpenGL
            .arg("+extension")
            .arg("RANDR")                  // Enable resolution changes
            .arg("-nolisten")
            .arg("tcp")                    // Don't listen on network
            .spawn()?;
        
        // Wait for X server to be ready
        tokio::time::sleep(Duration::from_millis(500)).await;
        
        let screenshot_dir = TempDir::new()?;
        
        Ok(VirtualDisplay {
            backend: VirtualDisplayBackend::Xvfb {
                display: display_id.clone(),
                resolution: "1920x1080x24".to_string(),
            },
            process: Some(process),
            display_id,
            screenshot_dir: screenshot_dir.into_path(),
        })
    }
    
    /// Execute GUI app in virtual display
    pub async fn run_gui_app(&self, command: &str) -> Result<GuiAppHandle> {
        // Set DISPLAY environment variable
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
           .arg(command)
           .env("DISPLAY", &self.display_id)
           .env("XAUTHORITY", "/dev/null")  // No X auth needed
           // Force software rendering (no GPU needed)
           .env("LIBGL_ALWAYS_SOFTWARE", "1")
           .env("GALLIUM_DRIVER", "llvmpipe");
        
        let process = cmd.spawn()?;
        
        Ok(GuiAppHandle {
            process,
            display: self.display_id.clone(),
        })
    }
    
    /// Capture screenshot of virtual display
    pub async fn screenshot(&self, name: &str) -> Result<PathBuf> {
        let output_path = self.screenshot_dir.join(format!("{}.png", name));
        
        // Use xwd + convert or import (ImageMagick)
        Command::new("import")
            .arg("-window")
            .arg("root")                    // Capture entire screen
            .arg("-display")
            .arg(&self.display_id)
            .arg(&output_path)
            .output()
            .await?;
        
        info!("Screenshot saved: {:?}", output_path);
        Ok(output_path)
    }
    
    /// Find available display number
    fn find_free_display() -> Result<String> {
        for i in 99..199 {
            let display = format!(":{}",  i);
            let lock_file = format!("/tmp/.X{}-lock", i);
            
            if !Path::new(&lock_file).exists() {
                return Ok(display);
            }
        }
        
        Err(anyhow!("No free X display found"))
    }
}

impl Drop for VirtualDisplay {
    fn drop(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
        }
    }
}
```

### Game Engine Headless Rendering

Many game engines support headless mode:

```rust
pub enum GameEngineBackend {
    /// Godot headless mode
    Godot {
        headless: bool,
        output_frames: PathBuf,
    },
    
    /// Unity headless (requires Unity installed)
    Unity {
        batch_mode: bool,
        no_graphics: bool,
    },
    
    /// Bevy with headless rendering
    Bevy {
        render_to_texture: bool,
    },
    
    /// Custom engine with software rendering
    Custom {
        renderer: SoftwareRenderer,
    },
}

impl GameEngineBackend {
    /// Run game in headless mode, capture frames
    pub async fn run_headless(&self, game_path: &Path) -> Result<GameSession> {
        match self {
            GameEngineBackend::Godot { output_frames, .. } => {
                // Godot can render to images without display
                Command::new("godot")
                    .arg("--headless")
                    .arg("--render-thread")
                    .arg("safe")
                    .arg("--export-frames")
                    .arg(output_frames)
                    .arg(game_path)
                    .spawn()?;
                
                // Monitor output_frames directory for screenshots
                Ok(GameSession {
                    frame_dir: output_frames.clone(),
                    ..Default::default()
                })
            }
            
            GameEngineBackend::Unity { .. } => {
                // Unity batch mode with screenshot capture
                Command::new("unity")
                    .arg("-batchmode")
                    .arg("-nographics")
                    .arg("-executeMethod")
                    .arg("AutomatedTest.RunTests")  // Custom test runner
                    .arg("-projectPath")
                    .arg(game_path)
                    .spawn()?;
                
                todo!("Implement Unity session")
            }
            
            GameEngineBackend::Bevy { .. } => {
                // Bevy can render to texture in code
                // Game needs to be built with headless feature
                Command::new(game_path)
                    .env("BEVY_HEADLESS", "1")
                    .spawn()?;
                
                todo!("Implement Bevy session")
            }
            
            _ => Err(anyhow!("Backend not implemented"))
        }
    }
}
```

---

## Model-Interactive Testing

### Screenshot-Based Visual Feedback

```rust
pub struct VisualTester {
    display: VirtualDisplay,
    model: Box<dyn VisionModel>,
    interaction_log: Vec<Interaction>,
}

pub trait VisionModel: Send + Sync {
    /// Analyze screenshot and provide feedback
    async fn analyze_screenshot(
        &self,
        image_path: &Path,
        context: &str,
    ) -> Result<VisualFeedback>;
    
    /// Suggest next interaction
    async fn suggest_interaction(
        &self,
        current_state: &GuiState,
    ) -> Result<Interaction>;
}

pub struct VisualFeedback {
    /// What the model sees
    description: String,
    
    /// Visual issues identified
    issues: Vec<VisualIssue>,
    
    /// Positive observations
    strengths: Vec<String>,
    
    /// Overall assessment
    rating: f32,  // 0.0-1.0
}

pub enum VisualIssue {
    /// Layout problems
    Layout {
        issue: String,
        severity: Severity,
        location: Option<BoundingBox>,
    },
    
    /// Text readability
    Readability {
        issue: String,
        element: String,
    },
    
    /// Color/contrast issues
    ColorContrast {
        foreground: Color,
        background: Color,
        ratio: f32,  // WCAG contrast ratio
    },
    
    /// Missing elements
    MissingElement {
        expected: String,
        context: String,
    },
    
    /// Alignment issues
    Alignment {
        elements: Vec<String>,
        issue: String,
    },
    
    /// Spacing problems
    Spacing {
        elements: Vec<String>,
        issue: String,
    },
}

impl VisualTester {
    /// Run interactive GUI test session
    pub async fn test_gui_interactively(&mut self, app: &GuiApp) -> Result<VisualTestReport> {
        info!("Starting interactive GUI test");
        
        let mut report = VisualTestReport::default();
        
        // 1. Launch app in virtual display
        let handle = self.display.run_gui_app(&app.launch_command).await?;
        
        // Wait for app to load
        tokio::time::sleep(Duration::from_secs(2)).await;
        
        // 2. Initial screenshot and analysis
        let screenshot = self.display.screenshot("initial").await?;
        let feedback = self.model.analyze_screenshot(
            &screenshot,
            "This is the initial state of the application. Analyze the UI."
        ).await?;
        
        report.add_feedback("Initial State", feedback);
        
        // 3. Interactive testing loop
        for step in 1..=20 {
            info!("Test step {}", step);
            
            // Get current state
            let state = self.capture_gui_state().await?;
            
            // Model suggests next interaction
            let interaction = self.model.suggest_interaction(&state).await?;
            
            info!("Performing interaction: {:?}", interaction);
            
            // Perform interaction
            self.perform_interaction(&interaction).await?;
            
            // Wait for UI to update
            tokio::time::sleep(Duration::from_millis(500)).await;
            
            // Capture and analyze result
            let screenshot = self.display.screenshot(&format!("step_{}", step)).await?;
            let feedback = self.model.analyze_screenshot(
                &screenshot,
                &format!("After interaction: {:?}", interaction)
            ).await?;
            
            report.add_step(step, interaction, feedback);
            
            // Check if we've explored enough
            if self.is_testing_complete(&report) {
                break;
            }
        }
        
        // 4. Generate comprehensive report
        report.finalize();
        
        Ok(report)
    }
    
    /// Perform interaction via X11 automation
    async fn perform_interaction(&self, interaction: &Interaction) -> Result<()> {
        match interaction {
            Interaction::Click { x, y } => {
                self.xdotool_click(*x, *y).await?;
            }
            
            Interaction::Type { text } => {
                self.xdotool_type(text).await?;
            }
            
            Interaction::KeyPress { key } => {
                self.xdotool_key(key).await?;
            }
            
            Interaction::Scroll { direction, amount } => {
                self.xdotool_scroll(direction, *amount).await?;
            }
            
            Interaction::Wait { duration } => {
                tokio::time::sleep(*duration).await;
            }
        }
        
        Ok(())
    }
    
    /// Use xdotool to simulate mouse click
    async fn xdotool_click(&self, x: i32, y: i32) -> Result<()> {
        Command::new("xdotool")
            .env("DISPLAY", &self.display.display_id)
            .arg("mousemove")
            .arg(x.to_string())
            .arg(y.to_string())
            .arg("click")
            .arg("1")  // Left click
            .output()
            .await?;
        
        Ok(())
    }
    
    /// Use xdotool to type text
    async fn xdotool_type(&self, text: &str) -> Result<()> {
        Command::new("xdotool")
            .env("DISPLAY", &self.display.display_id)
            .arg("type")
            .arg("--delay")
            .arg("100")  // 100ms between keys
            .arg(text)
            .output()
            .await?;
        
        Ok(())
    }
}

pub enum Interaction {
    Click { x: i32, y: i32 },
    Type { text: String },
    KeyPress { key: String },  // "Return", "Tab", "Escape", etc.
    Scroll { direction: ScrollDirection, amount: i32 },
    Wait { duration: Duration },
}

pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}
```

### Vision Model Integration

Use vision-capable models (Claude, GPT-4V, etc.) to analyze screenshots:

```rust
pub struct ClaudeVisionModel {
    client: AnthropicClient,
}

impl VisionModel for ClaudeVisionModel {
    async fn analyze_screenshot(
        &self,
        image_path: &Path,
        context: &str,
    ) -> Result<VisualFeedback> {
        // Read image and encode as base64
        let image_data = tokio::fs::read(image_path).await?;
        let base64_image = base64::encode(&image_data);
        
        let prompt = format!(
            "{}\n\n\
             Analyze this GUI screenshot and provide:\n\
             1. Detailed description of what you see\n\
             2. Any visual issues (layout, alignment, spacing, contrast)\n\
             3. Positive aspects of the design\n\
             4. Overall rating (0.0-1.0)\n\n\
             Be specific about locations and elements.",
            context
        );
        
        let response = self.client.send_with_image(
            &prompt,
            &base64_image,
            "image/png"
        ).await?;
        
        // Parse structured feedback from response
        let feedback = self.parse_visual_feedback(&response)?;
        
        Ok(feedback)
    }
    
    async fn suggest_interaction(
        &self,
        current_state: &GuiState,
    ) -> Result<Interaction> {
        let prompt = format!(
            "Current GUI state:\n\
             - Visible elements: {:?}\n\
             - Previous interactions: {:?}\n\n\
             Suggest the next interaction to test the UI comprehensively.\n\
             Focus on: buttons, inputs, navigation, edge cases.\n\n\
             Respond with: {{\"type\": \"click|type|key\", \"params\": {{...}}}}",
            current_state.elements,
            current_state.history
        );
        
        let response = self.client.send(&prompt).await?;
        let interaction = self.parse_interaction(&response)?;
        
        Ok(interaction)
    }
}
```

---

## Game Testing with Automated Play

### Game Bot Framework

```rust
pub struct GameBot {
    game_handle: GameHandle,
    vision: Box<dyn VisionModel>,
    strategy: GameStrategy,
    telemetry: GameTelemetry,
}

pub enum GameStrategy {
    /// Explore all areas
    Explorer,
    
    /// Try to win/complete objectives
    Objective,
    
    /// Stress test (rapid random inputs)
    StressTest,
    
    /// Follow tutorial/guided path
    Tutorial,
}

impl GameBot {
    /// Play game autonomously and report findings
    pub async fn play_session(&mut self, duration: Duration) -> Result<GameTestReport> {
        info!("Starting automated game session");
        
        let mut report = GameTestReport::default();
        let start = Instant::now();
        
        while start.elapsed() < duration {
            // 1. Capture current game state
            let frame = self.game_handle.capture_frame().await?;
            
            // 2. Analyze with vision model
            let analysis = self.vision.analyze_screenshot(
                &frame,
                "Analyze this game frame. What's happening? What should the player do next?"
            ).await?;
            
            report.add_frame_analysis(analysis.clone());
            
            // 3. Decide next action based on strategy
            let action = self.decide_action(&analysis).await?;
            
            // 4. Execute action
            self.execute_game_action(&action).await?;
            
            // 5. Monitor telemetry
            self.telemetry.record_action(&action);
            
            // Check for crashes, freezes, errors
            if let Some(issue) = self.detect_issues().await? {
                report.add_issue(issue);
            }
            
            // Small delay between actions
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        
        report.telemetry = self.telemetry.clone();
        report.finalize();
        
        Ok(report)
    }
    
    async fn execute_game_action(&self, action: &GameAction) -> Result<()> {
        match action {
            GameAction::Move { direction } => {
                // Send movement keys
                self.send_key_press(&direction.to_key()).await?;
            }
            
            GameAction::Jump => {
                self.send_key_press("space").await?;
            }
            
            GameAction::Interact => {
                self.send_key_press("e").await?;
            }
            
            GameAction::Attack => {
                self.send_mouse_click(MouseButton::Left).await?;
            }
            
            GameAction::Wait { duration } => {
                tokio::time::sleep(*duration).await;
            }
        }
        
        Ok(())
    }
    
    /// Detect crashes, freezes, errors from screenshots
    async fn detect_issues(&self) -> Result<Option<GameIssue>> {
        // Check for error messages in screenshot
        let current_frame = self.game_handle.get_current_frame()?;
        
        // Look for common error indicators
        if self.contains_error_text(&current_frame)? {
            return Ok(Some(GameIssue::ErrorMessage));
        }
        
        // Check if game is frozen (same frame for too long)
        if self.game_handle.is_frozen()? {
            return Ok(Some(GameIssue::Freeze));
        }
        
        // Check frame rate
        let fps = self.telemetry.current_fps();
        if fps < 10.0 {
            return Ok(Some(GameIssue::PerformanceDrop { fps }));
        }
        
        Ok(None)
    }
}

pub enum GameAction {
    Move { direction: Direction },
    Jump,
    Interact,
    Attack,
    Menu,
    Wait { duration: Duration },
}

pub enum GameIssue {
    Crash,
    Freeze,
    ErrorMessage,
    PerformanceDrop { fps: f32 },
    GraphicalGlitch { description: String },
    SoftLock { reason: String },
}

pub struct GameTelemetry {
    actions_performed: Vec<GameAction>,
    fps_history: Vec<f32>,
    frame_times: Vec<Duration>,
    errors_encountered: Vec<String>,
}
```

---

## Tools and Dependencies

### Required Packages (via Nix)

```nix
{
  # Virtual display
  xvfb = pkgs.xorg.xvfbrun;
  
  # X11 automation
  xdotool = pkgs.xdotool;
  xautomation = pkgs.xautomation;
  
  # Screenshot capture
  imagemagick = pkgs.imagemagick;
  scrot = pkgs.scrot;
  
  # Wayland support
  weston = pkgs.weston;
  sway = pkgs.sway;
  
  # Browser automation (for web GUIs)
  chromium = pkgs.chromium;
  playwright = pkgs.playwright-driver.browsers;
  
  # Game engines (optional)
  godot = pkgs.godot;
  
  # Software rendering (no GPU needed)
  mesa = pkgs.mesa;
  llvmpipe = pkgs.mesa.drivers;
}
```

### Cross-Platform Considerations

```rust
impl VirtualDisplay {
    pub async fn create_for_platform() -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            // Linux: Use Xvfb or Wayland headless
            if Self::has_xvfb().await {
                Self::start_xvfb().await
            } else if Self::has_weston().await {
                Self::start_weston_headless().await
            } else {
                Err(anyhow!("No virtual display backend available"))
            }
        }
        
        #[cfg(target_os = "macos")]
        {
            // macOS: Use instruments or AppleScript automation
            Self::start_macos_virtual_display().await
        }
        
        #[cfg(target_os = "windows")]
        {
            // Windows: Use WSL with Xvfb, or Windows UI automation
            if Self::is_wsl().await {
                Self::start_xvfb().await
            } else {
                Self::start_windows_ui_automation().await
            }
        }
    }
}
```

---

## Practical Testing Flow

### Complete GUI Testing Pipeline

```rust
pub async fn test_gui_application(app: &GuiApp) -> Result<GuiTestReport> {
    // 1. Create virtual display
    let display = VirtualDisplay::start().await?;
    
    // 2. Create vision-enabled tester
    let mut tester = VisualTester::new(
        display,
        Box::new(ClaudeVisionModel::new())
    );
    
    // 3. Run interactive test session
    let report = tester.test_gui_interactively(app).await?;
    
    // 4. Analyze results
    if report.overall_rating() > 0.7 {
        info!("GUI test passed!");
    } else {
        warn!("GUI test found issues: {:?}", report.issues);
    }
    
    // 5. Cleanup
    display.shutdown().await?;
    
    Ok(report)
}
```

### Game Testing Pipeline

```rust
pub async fn test_game(game: &Game) -> Result<GameTestReport> {
    // 1. Create virtual display
    let display = VirtualDisplay::start().await?;
    
    // 2. Create game bot
    let mut bot = GameBot::new(
        game,
        GameStrategy::Explorer,
        Box::new(ClaudeVisionModel::new())
    );
    
    // 3. Play for 10 minutes
    let report = bot.play_session(Duration::from_secs(600)).await?;
    
    // 4. Check for issues
    if report.issues.is_empty() {
        info!("Game test passed - no issues detected!");
    } else {
        warn!("Game test found {} issues", report.issues.len());
        for issue in &report.issues {
            warn!("  - {:?}", issue);
        }
    }
    
    // 5. Cleanup
    display.shutdown().await?;
    
    Ok(report)
}
```

---

## Summary

**Key Capabilities:**

1. **Virtual Display** - Xvfb/Wayland headless, no user desktop interference
2. **Screenshot Capture** - Take snapshots at any point for analysis
3. **Vision Model Analysis** - Use Claude/GPT-4V to analyze UI/game visually
4. **Automated Interaction** - xdotool for clicks, typing, key presses
5. **Game Bot** - Autonomous gameplay with telemetry and issue detection
6. **Cross-Platform** - Works on Linux (native), macOS, Windows (WSL)

**What Models Can Do:**

- ✅ See and analyze GUI layouts
- ✅ Identify visual issues (alignment, contrast, spacing)
- ✅ Interact with GUI (click, type, navigate)
- ✅ Play games autonomously
- ✅ Detect crashes, freezes, errors
- ✅ Provide design feedback

**No User Disruption:**

- Runs in virtual display (`:99`, not `:0`)
- User's desktop unaffected
- Can run in background
- Screenshots saved to temp directory

This gives models true visual feedback and interaction capabilities for comprehensive testing!
