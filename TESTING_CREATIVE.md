# Testing, Sandboxing, and Creative Development System

## Cross-Platform Sandboxing Strategy

### The Nix-First Approach

**Philosophy:** Nix everywhere, with graceful degradation

```rust
pub enum SandboxStrategy {
    /// NixOS native (best experience)
    NixOS {
        use_system_nix: bool,
    },
    
    /// Non-NixOS Linux with Nix installed
    NixOnLinux {
        nix_path: PathBuf,
    },
    
    /// macOS with Nix
    NixOnMacOS {
        nix_path: PathBuf,
    },
    
    /// Windows WSL with Nix
    NixOnWSL {
        wsl_distro: String,
        nix_path: PathBuf,
    },
    
    /// Fallback: Docker/Podman
    Container {
        runtime: ContainerRuntime,
        image: String,
    },
    
    /// Last resort: Native tools
    Native {
        package_manager: NativePackageManager,
    },
}

pub enum ContainerRuntime {
    Docker,
    Podman,
    Nerdctl,
}

pub enum NativePackageManager {
    Apt,      // Debian/Ubuntu
    Dnf,      // Fedora
    Pacman,   // Arch
    Brew,     // macOS
    Chocolatey, // Windows
    Winget,   // Windows
}
```

### Auto-Detection and Setup

```rust
impl SandboxManager {
    /// Detect best available sandboxing strategy
    pub async fn detect_strategy() -> Result<SandboxStrategy> {
        // 1. Check if we're on NixOS
        if Self::is_nixos().await? {
            return Ok(SandboxStrategy::NixOS { use_system_nix: true });
        }
        
        // 2. Check if Nix is installed (any platform)
        if let Some(nix_path) = Self::find_nix().await {
            #[cfg(target_os = "linux")]
            return Ok(SandboxStrategy::NixOnLinux { nix_path });
            
            #[cfg(target_os = "macos")]
            return Ok(SandboxStrategy::NixOnMacOS { nix_path });
            
            #[cfg(target_os = "windows")]
            if let Some(distro) = Self::detect_wsl().await {
                return Ok(SandboxStrategy::NixOnWSL {
                    wsl_distro: distro,
                    nix_path,
                });
            }
        }
        
        // 3. Offer to install Nix
        if Self::should_install_nix().await? {
            info!("Nix not found. Installing Nix for optimal experience...");
            Self::install_nix().await?;
            return Self::detect_strategy().await; // Re-detect
        }
        
        // 4. Try container runtimes
        for runtime in [ContainerRuntime::Docker, ContainerRuntime::Podman, ContainerRuntime::Nerdctl] {
            if Self::has_container_runtime(&runtime).await {
                return Ok(SandboxStrategy::Container {
                    runtime,
                    image: "nixos/nix:latest".to_string(),
                });
            }
        }
        
        // 5. Fall back to native package manager
        warn!("No sandbox available, using native package manager (less safe)");
        let pm = Self::detect_native_package_manager().await?;
        Ok(SandboxStrategy::Native { package_manager: pm })
    }
    
    /// Install Nix on non-NixOS systems
    async fn install_nix() -> Result<()> {
        println!("Installing Nix package manager...");
        println!("This provides isolated environments for testing.");
        
        #[cfg(not(target_os = "windows"))]
        {
            // Determinate Nix Installer (better than official)
            let install_cmd = "curl --proto '=https' --tlsv1.2 -sSf -L \
                https://install.determinate.systems/nix | sh -s -- install";
            
            Command::new("sh")
                .arg("-c")
                .arg(install_cmd)
                .status()
                .await?;
        }
        
        #[cfg(target_os = "windows")]
        {
            // Install in WSL
            println!("Please install WSL first: wsl --install");
            return Err(anyhow!("WSL required on Windows"));
        }
        
        Ok(())
    }
}
```

### Isolated Testing Environments

```rust
pub struct TestEnvironment {
    /// Unique ID for this environment
    id: String,
    
    /// Sandbox strategy being used
    strategy: SandboxStrategy,
    
    /// Directory for this environment
    work_dir: TempDir,
    
    /// Packages needed
    packages: Vec<String>,
    
    /// Environment variables
    env_vars: HashMap<String, String>,
    
    /// Cleanup policy
    cleanup: CleanupPolicy,
}

pub enum CleanupPolicy {
    /// Delete everything after test
    Aggressive,
    
    /// Keep on failure for debugging
    KeepOnFailure,
    
    /// Keep everything until manual cleanup
    Manual,
    
    /// Keep for N hours
    KeepFor(Duration),
}

impl TestEnvironment {
    /// Create isolated environment with specified packages
    pub async fn create(packages: Vec<String>) -> Result<Self> {
        let strategy = SandboxManager::detect_strategy().await?;
        let work_dir = TempDir::new()?;
        let id = Uuid::new_v4().to_string();
        
        info!("Creating test environment {} with strategy {:?}", id, strategy);
        
        let env = TestEnvironment {
            id: id.clone(),
            strategy: strategy.clone(),
            work_dir,
            packages: packages.clone(),
            env_vars: HashMap::new(),
            cleanup: CleanupPolicy::KeepOnFailure,
        };
        
        // Initialize environment based on strategy
        match strategy {
            SandboxStrategy::NixOS { .. }
            | SandboxStrategy::NixOnLinux { .. }
            | SandboxStrategy::NixOnMacOS { .. } => {
                env.setup_nix_shell(packages).await?;
            }
            
            SandboxStrategy::Container { runtime, image } => {
                env.setup_container(runtime, image, packages).await?;
            }
            
            SandboxStrategy::Native { package_manager } => {
                warn!("Using native package manager - NOT isolated!");
                env.setup_native(package_manager, packages).await?;
            }
            
            _ => {}
        }
        
        Ok(env)
    }
    
    /// Setup using nix-shell
    async fn setup_nix_shell(&self, packages: Vec<String>) -> Result<()> {
        // Create shell.nix in work_dir
        let shell_nix = format!(
            r#"{{ pkgs ? import <nixpkgs> {{}} }}:
pkgs.mkShell {{
  buildInputs = with pkgs; [
    {}
  ];
  
  shellHook = ''
    echo "Test environment ready"
    export TEST_ENV_ID="{}"
  '';
}}"#,
            packages.join("\n    "),
            self.id
        );
        
        let shell_file = self.work_dir.path().join("shell.nix");
        tokio::fs::write(&shell_file, shell_nix).await?;
        
        info!("Created nix-shell environment at {:?}", shell_file);
        Ok(())
    }
    
    /// Execute command in isolated environment
    pub async fn execute(&self, cmd: &str) -> Result<ExecutionResult> {
        match &self.strategy {
            SandboxStrategy::NixOS { .. }
            | SandboxStrategy::NixOnLinux { .. }
            | SandboxStrategy::NixOnMacOS { .. } => {
                self.execute_in_nix_shell(cmd).await
            }
            
            SandboxStrategy::Container { runtime, .. } => {
                self.execute_in_container(runtime, cmd).await
            }
            
            SandboxStrategy::Native { .. } => {
                self.execute_native(cmd).await
            }
            
            _ => Err(anyhow!("Strategy not implemented"))
        }
    }
    
    async fn execute_in_nix_shell(&self, cmd: &str) -> Result<ExecutionResult> {
        let output = Command::new("nix-shell")
            .arg("--pure")  // Isolated environment
            .arg("--run")
            .arg(cmd)
            .current_dir(self.work_dir.path())
            .output()
            .await?;
        
        Ok(ExecutionResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
    
    /// Clean up environment based on policy
    pub async fn cleanup(&self, test_passed: bool) -> Result<()> {
        match self.cleanup {
            CleanupPolicy::Aggressive => {
                info!("Cleaning up environment {}", self.id);
                // TempDir will auto-delete on drop
            }
            
            CleanupPolicy::KeepOnFailure => {
                if test_passed {
                    info!("Test passed, cleaning up");
                } else {
                    warn!("Test failed, keeping environment at {:?}", self.work_dir.path());
                    // Prevent auto-deletion
                    std::mem::forget(self.work_dir);
                }
            }
            
            CleanupPolicy::KeepFor(duration) => {
                info!("Scheduling cleanup for {:?} in {:?}", self.work_dir.path(), duration);
                // Schedule cleanup (implementation depends on persistence layer)
            }
            
            CleanupPolicy::Manual => {
                info!("Manual cleanup required: {:?}", self.work_dir.path());
                std::mem::forget(self.work_dir);
            }
        }
        
        Ok(())
    }
}

pub struct ExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
```

---

## Testing Strategy for Various Project Types

### Comprehensive Testing Framework

```rust
pub enum ProjectType {
    /// Can run tests directly
    Testable {
        test_framework: TestFramework,
    },
    
    /// Need to compile/build first
    Buildable {
        build_system: BuildSystem,
    },
    
    /// GUI app - can't fully test headless
    GUI {
        framework: GUIFramework,
        can_headless: bool,
    },
    
    /// Web app - can test with headless browser
    WebApp {
        backend: Option<String>,
        frontend: Option<String>,
    },
    
    /// CLI tool - can test with inputs/outputs
    CLI,
    
    /// Library - test with examples
    Library {
        language: String,
    },
    
    /// Game - special testing needs
    Game {
        engine: Option<String>,
    },
}

pub enum TestFramework {
    Cargo,      // Rust
    PyTest,     // Python
    Jest,       // JavaScript/TypeScript
    JUnit,      // Java
    Go,         // Go
    RSpec,      // Ruby
}

pub enum BuildSystem {
    Cargo,
    Make,
    CMake,
    Gradle,
    Maven,
    Npm,
    Poetry,
}

impl TestRunner {
    /// Determine what can be tested
    pub async fn assess_testability(&self, project: &Project) -> TestabilityReport {
        let mut report = TestabilityReport::default();
        
        // Detect project type
        let project_type = self.detect_project_type(project).await;
        
        match project_type {
            ProjectType::Testable { test_framework } => {
                report.can_run_tests = true;
                report.test_command = self.get_test_command(&test_framework);
            }
            
            ProjectType::Buildable { build_system } => {
                report.can_build = true;
                report.build_command = self.get_build_command(&build_system);
                report.can_run_tests = true;  // Usually testable after build
            }
            
            ProjectType::GUI { can_headless, .. } => {
                report.can_build = true;
                report.can_run_tests = can_headless;  // Only if supports headless
                
                if !can_headless {
                    report.limitations.push(
                        "GUI requires display - will test compilation only".to_string()
                    );
                    report.alternative_tests.push(AlternativeTest::StaticAnalysis);
                    report.alternative_tests.push(AlternativeTest::UnitTestsOnly);
                }
            }
            
            ProjectType::WebApp { .. } => {
                report.can_build = true;
                report.can_run_tests = true;  // Use headless browser
                report.test_command = Some("playwright test --browser chromium".to_string());
            }
            
            ProjectType::CLI => {
                report.can_run_tests = true;
                report.can_run_e2e = true;
                report.alternative_tests.push(AlternativeTest::InteractionTests);
            }
            
            ProjectType::Game { .. } => {
                report.can_build = true;
                report.can_run_tests = false;  // Usually needs human testing
                
                report.limitations.push(
                    "Game requires manual play-testing".to_string()
                );
                report.alternative_tests.push(AlternativeTest::UnitTestsOnly);
                report.alternative_tests.push(AlternativeTest::LogicTests);
            }
            
            _ => {}
        }
        
        report
    }
    
    /// Run all possible tests
    pub async fn run_comprehensive_tests(
        &self,
        project: &Project,
        env: &TestEnvironment
    ) -> Result<TestResults> {
        let report = self.assess_testability(project).await;
        let mut results = TestResults::default();
        
        // 1. Static analysis (always possible)
        results.static_analysis = self.run_static_analysis(project, env).await?;
        
        // 2. Compilation/build
        if report.can_build {
            results.build = self.run_build(project, env).await.ok();
        }
        
        // 3. Unit tests
        if report.can_run_tests {
            results.unit_tests = self.run_unit_tests(project, env).await.ok();
        }
        
        // 4. Integration tests
        if report.can_run_tests {
            results.integration_tests = self.run_integration_tests(project, env).await.ok();
        }
        
        // 5. E2E tests (if applicable)
        if report.can_run_e2e {
            results.e2e_tests = self.run_e2e_tests(project, env).await.ok();
        }
        
        // 6. Alternative tests for untestable aspects
        for alt_test in &report.alternative_tests {
            match alt_test {
                AlternativeTest::StaticAnalysis => {
                    // Already done
                }
                AlternativeTest::UnitTestsOnly => {
                    // Focus on logic, not rendering
                    results.alternative.push(
                        self.run_logic_tests(project, env).await?
                    );
                }
                AlternativeTest::InteractionTests => {
                    // Test CLI interactions
                    results.alternative.push(
                        self.run_cli_tests(project, env).await?
                    );
                }
                AlternativeTest::LogicTests => {
                    // Test game logic without rendering
                    results.alternative.push(
                        self.run_game_logic_tests(project, env).await?
                    );
                }
            }
        }
        
        // 7. Manual checks
        results.manual_checks = self.perform_manual_checks(project).await?;
        
        Ok(results)
    }
    
    /// Manual checks for things we can't execute
    async fn perform_manual_checks(&self, project: &Project) -> Result<Vec<ManualCheck>> {
        let mut checks = Vec::new();
        
        // Check 1: File structure makes sense
        checks.push(ManualCheck {
            name: "File Structure".to_string(),
            passed: self.check_file_structure(project).await?,
            details: "All files in logical locations".to_string(),
        });
        
        // Check 2: Documentation exists
        checks.push(ManualCheck {
            name: "Documentation".to_string(),
            passed: self.check_documentation(project).await?,
            details: "README, comments, API docs present".to_string(),
        });
        
        // Check 3: Dependencies reasonable
        checks.push(ManualCheck {
            name: "Dependencies".to_string(),
            passed: self.check_dependencies(project).await?,
            details: "No suspicious or excessive dependencies".to_string(),
        });
        
        // Check 4: Error handling present
        checks.push(ManualCheck {
            name: "Error Handling".to_string(),
            passed: self.check_error_handling(project).await?,
            details: "Errors handled gracefully".to_string(),
        });
        
        // Check 5: Security basics
        checks.push(ManualCheck {
            name: "Security Basics".to_string(),
            passed: self.check_security(project).await?,
            details: "No obvious security issues".to_string(),
        });
        
        Ok(checks)
    }
}

pub enum AlternativeTest {
    StaticAnalysis,
    UnitTestsOnly,
    InteractionTests,
    LogicTests,
}

pub struct TestResults {
    pub static_analysis: StaticAnalysisResult,
    pub build: Option<BuildResult>,
    pub unit_tests: Option<TestResult>,
    pub integration_tests: Option<TestResult>,
    pub e2e_tests: Option<TestResult>,
    pub alternative: Vec<TestResult>,
    pub manual_checks: Vec<ManualCheck>,
}

impl TestResults {
    /// Overall success determination
    pub fn is_success(&self) -> bool {
        // Must pass static analysis
        if !self.static_analysis.passed {
            return false;
        }
        
        // Must build (if buildable)
        if let Some(ref build) = self.build {
            if !build.success {
                return false;
            }
        }
        
        // Must pass all tests that were run
        let all_tests = [
            &self.unit_tests,
            &self.integration_tests,
            &self.e2e_tests,
        ];
        
        for test in all_tests.iter().filter_map(|t| t.as_ref()) {
            if !test.passed {
                return false;
            }
        }
        
        // Must pass all manual checks
        if !self.manual_checks.iter().all(|c| c.passed) {
            return false;
        }
        
        true
    }
    
    /// Generate comprehensive report
    pub fn report(&self) -> String {
        let mut report = String::new();
        
        report.push_str("=== TEST RESULTS ===\n\n");
        
        // Static analysis
        report.push_str(&format!(
            "Static Analysis: {}\n",
            if self.static_analysis.passed { "✓ PASS" } else { "✗ FAIL" }
        ));
        
        // Build
        if let Some(ref build) = self.build {
            report.push_str(&format!(
                "Build: {}\n",
                if build.success { "✓ PASS" } else { "✗ FAIL" }
            ));
        }
        
        // Tests
        for (name, test) in [
            ("Unit Tests", &self.unit_tests),
            ("Integration Tests", &self.integration_tests),
            ("E2E Tests", &self.e2e_tests),
        ] {
            if let Some(ref t) = test {
                report.push_str(&format!(
                    "{}: {} ({}/{})\n",
                    name,
                    if t.passed { "✓ PASS" } else { "✗ FAIL" },
                    t.passed_count,
                    t.total_count
                ));
            }
        }
        
        // Manual checks
        report.push_str("\nManual Checks:\n");
        for check in &self.manual_checks {
            report.push_str(&format!(
                "  {} {}: {}\n",
                if check.passed { "✓" } else { "✗" },
                check.name,
                check.details
            ));
        }
        
        // Overall
        report.push_str(&format!(
            "\nOverall: {}\n",
            if self.is_success() { "✓ ALL TESTS PASSED" } else { "✗ SOME TESTS FAILED" }
        ));
        
        report
    }
}
```

---

## Creative Development Mode

### Extended, Iterative Development

```rust
pub enum DevelopmentMode {
    /// Standard: Complete task, done
    Standard {
        max_iterations: usize,  // 3-5
    },
    
    /// Creative: Keep iterating until feature-complete
    Creative {
        goal: CreativeGoal,
        session: CreativeSession,
    },
}

pub struct CreativeGoal {
    /// High-level vision
    vision: String,  // "A fun platformer game with physics"
    
    /// Feature list (can grow)
    features: Vec<Feature>,
    
    /// Quality bar
    quality_threshold: f32,  // 0.0-1.0
    
    /// Completion criteria
    completion: CompletionCriteria,
}

pub struct Feature {
    name: String,
    description: String,
    priority: Priority,
    status: FeatureStatus,
    
    /// Sub-features (can be added as we design)
    sub_features: Vec<Feature>,
}

pub enum FeatureStatus {
    Planned,
    Designing,
    Implementing,
    Testing,
    Refining,
    Complete,
}

pub enum CompletionCriteria {
    /// All planned features done
    AllFeatures,
    
    /// User says it's done
    UserApproval,
    
    /// Reaches quality threshold
    QualityThreshold(f32),
    
    /// Time/session limit
    TimeLimit { sessions: usize, hours: usize },
    
    /// Combined criteria
    Combined(Vec<CompletionCriteria>),
}

pub struct CreativeSession {
    /// Session history (can span days/weeks)
    sessions: Vec<SessionRecord>,
    
    /// Current session
    current_session: usize,
    
    /// Total time spent
    total_time: Duration,
    
    /// Persistent state
    state: CreativeState,
}

pub struct CreativeState {
    /// What's been done so far
    completed_features: Vec<Feature>,
    
    /// What's in progress
    active_work: Vec<WorkItem>,
    
    /// Ideas for future features
    ideas: Vec<Idea>,
    
    /// Lessons learned
    learnings: Vec<Learning>,
    
    /// Design decisions made
    decisions: Vec<DesignDecision>,
}
```

### Design Phase (Extended Discussion)

```rust
impl CreativeConductor {
    /// Enter deep design mode before implementation
    pub async fn design_phase(&mut self, goal: &CreativeGoal) -> Result<DesignDocument> {
        info!("Entering design phase for: {}", goal.vision);
        
        let mut design = DesignDocument::new(goal.clone());
        let mut iteration = 1;
        
        loop {
            info!("Design iteration {}", iteration);
            
            // 1. Brainstorm with creative models
            let ideas = self.brainstorm_session(&design).await?;
            design.add_ideas(ideas);
            
            // 2. Analyze feasibility
            let analysis = self.analyze_feasibility(&design).await?;
            design.add_analysis(analysis);
            
            // 3. Identify gaps/issues
            let gaps = self.identify_gaps(&design).await?;
            
            if gaps.is_empty() {
                info!("No gaps identified in design");
            } else {
                warn!("Gaps found: {:?}", gaps);
                
                // 4. Address gaps
                for gap in gaps {
                    let solution = self.address_gap(&design, &gap).await?;
                    design.add_solution(gap, solution);
                }
                
                iteration += 1;
                continue;  // Another iteration to verify
            }
            
            // 5. Review with critical eye
            let review = self.critical_review(&design).await?;
            
            match review {
                Review::Approved => {
                    info!("Design approved, ready to implement");
                    break;
                }
                
                Review::NeedsWork { issues } => {
                    warn!("Design needs work: {:?}", issues);
                    
                    // Address issues
                    for issue in issues {
                        let fix = self.address_issue(&design, &issue).await?;
                        design.apply_fix(issue, fix);
                    }
                    
                    iteration += 1;
                    continue;
                }
                
                Review::FundamentalFlaws => {
                    error!("Fundamental flaws in design, starting over");
                    design = DesignDocument::new(goal.clone());
                    iteration = 1;
                    continue;
                }
            }
            
            // Safety: Don't iterate forever
            if iteration > 20 {
                warn!("Design iterations exceeded 20, proceeding with current design");
                break;
            }
        }
        
        // Final design document
        design.finalize();
        info!("Design phase complete after {} iterations", iteration);
        
        Ok(design)
    }
    
    /// Brainstorm with multiple creative models
    async fn brainstorm_session(&self, design: &DesignDocument) -> Result<Vec<Idea>> {
        info!("Starting brainstorm session");
        
        // Use multiple models for diverse perspectives
        let creative_models = vec![
            ModelId::Opus,  // Creative & capable
            ModelId::GLM5,  // Different perspective
            ModelId::Sonnet, // Practical creativity
        ];
        
        let mut all_ideas = Vec::new();
        
        // Parallel brainstorming
        let mut tasks = Vec::new();
        
        for model in creative_models {
            if !self.is_available(&model).await {
                continue;
            }
            
            let design_clone = design.clone();
            let task = tokio::spawn(async move {
                Self::brainstorm_with_model(model, &design_clone).await
            });
            
            tasks.push(task);
        }
        
        // Collect all ideas
        for task in tasks {
            if let Ok(Ok(ideas)) = task.await {
                all_ideas.extend(ideas);
            }
        }
        
        // Deduplicate and rank
        let unique_ideas = self.deduplicate_ideas(all_ideas);
        let ranked_ideas = self.rank_ideas(unique_ideas).await?;
        
        info!("Brainstorm produced {} unique ideas", ranked_ideas.len());
        
        Ok(ranked_ideas)
    }
    
    /// Identify gaps in design (what's missing?)
    async fn identify_gaps(&self, design: &DesignDocument) -> Result<Vec<DesignGap>> {
        let prompt = format!(
            "Review this design and identify what's missing:\n\n\
             {}\n\n\
             Look for:\n\
             - Missing features for complete user experience\n\
             - Technical considerations not addressed\n\
             - Edge cases not covered\n\
             - Scalability concerns\n\
             - User experience gaps\n\
             - Security/safety issues\n\
             - Performance bottlenecks\n\n\
             Be thorough and critical.",
            design.summary()
        );
        
        // Use best available model for critical review
        let reviewer = self.get_best_available_model().await?;
        let response = reviewer.generate(&prompt).await?;
        
        // Parse gaps from response
        let gaps = self.parse_gaps(&response)?;
        
        Ok(gaps)
    }
    
    /// Critical review before finalizing design
    async fn critical_review(&self, design: &DesignDocument) -> Result<Review> {
        info!("Performing critical design review");
        
        let prompt = format!(
            "Perform a critical review of this design:\n\n\
             {}\n\n\
             Consider:\n\
             1. Is the architecture sound?\n\
             2. Are all features well-specified?\n\
             3. Are there any fundamental flaws?\n\
             4. Is this actually buildable?\n\
             5. Will this meet the stated goal?\n\n\
             Be brutally honest. If there are issues, list them.\n\
             If it's fundamentally flawed, say so.\n\
             Only approve if you're confident this can work.",
            design.full_document()
        );
        
        // Use multiple reviewers for consensus
        let reviewers = vec![
            self.get_model(ModelId::Opus).await?,
            self.get_model(ModelId::GLM5).await?,
        ];
        
        let mut reviews = Vec::new();
        
        for reviewer in reviewers {
            let response = reviewer.generate(&prompt).await?;
            let review = self.parse_review(&response)?;
            reviews.push(review);
        }
        
        // Consensus: all must approve
        if reviews.iter().all(|r| matches!(r, Review::Approved)) {
            Ok(Review::Approved)
        } else {
            // Collect all issues
            let mut all_issues = Vec::new();
            for review in reviews {
                if let Review::NeedsWork { issues } = review {
                    all_issues.extend(issues);
                }
            }
            
            if all_issues.is_empty() {
                Ok(Review::Approved)
            } else {
                Ok(Review::NeedsWork { issues: all_issues })
            }
        }
    }
}

pub enum Review {
    Approved,
    NeedsWork { issues: Vec<DesignIssue> },
    FundamentalFlaws,
}
```

### Iterative Implementation with Feedback

```rust
impl CreativeConductor {
    /// Build incrementally with constant testing and feedback
    pub async fn iterative_development(&mut self, design: DesignDocument) -> Result<Project> {
        info!("Starting iterative development");
        
        let mut project = Project::from_design(&design);
        let mut iteration = 1;
        
        loop {
            info!("Development iteration {}", iteration);
            
            // 1. Pick next feature to implement
            let feature = self.select_next_feature(&project, &design)?;
            
            if feature.is_none() {
                info!("All features complete!");
                break;
            }
            
            let feature = feature.unwrap();
            info!("Implementing feature: {}", feature.name);
            
            // 2. Implement feature
            let implementation = self.implement_feature(&mut project, &feature).await?;
            
            // 3. Test thoroughly
            let test_results = self.test_implementation(&project, &feature).await?;
            
            // 4. Get feedback
            let feedback = self.evaluate_implementation(
                &project,
                &feature,
                &test_results
            ).await?;
            
            match feedback {
                Feedback::Excellent => {
                    info!("Feature implementation excellent, moving on");
                    project.mark_complete(feature);
                }
                
                Feedback::Good => {
                    info!("Feature implementation good enough");
                    project.mark_complete(feature);
                }
                
                Feedback::NeedsImprovement { suggestions } => {
                    warn!("Feature needs improvement: {:?}", suggestions);
                    
                    // Refine implementation
                    for suggestion in suggestions {
                        self.apply_improvement(&mut project, &feature, &suggestion).await?;
                    }
                    
                    // Test again
                    let retest = self.test_implementation(&project, &feature).await?;
                    
                    if retest.is_success() {
                        project.mark_complete(feature);
                    } else {
                        warn!("Still not passing after improvements");
                        // Continue anyway, will iterate on it later
                    }
                }
                
                Feedback::Failed => {
                    error!("Feature implementation failed");
                    // Try different approach
                    project.rollback_feature(&feature)?;
                    continue;
                }
            }
            
            // 5. Holistic check: Does overall project still make sense?
            let health = self.check_project_health(&project).await?;
            
            if !health.is_healthy() {
                warn!("Project health degraded: {:?}", health.issues);
                
                // Refactor/clean up before continuing
                self.refactor_project(&mut project, &health.issues).await?;
            }
            
            // 6. Check if we should add more features
            let new_ideas = self.get_new_feature_ideas(&project).await?;
            
            for idea in new_ideas {
                if self.should_add_feature(&idea, &project).await? {
                    info!("Adding new feature: {}", idea.name);
                    project.add_feature(idea.into_feature());
                }
            }
            
            iteration += 1;
            
            // Check completion criteria
            if self.is_complete(&project, &design.goal).await? {
                info!("Project complete!");
                break;
            }
            
            // Safety: check if user wants to stop
            if iteration % 10 == 0 {
                if !self.confirm_continue(iteration).await? {
                    info!("User requested stop");
                    break;
                }
            }
        }
        
        // Final polish
        self.polish_project(&mut project).await?;
        
        Ok(project)
    }
    
    /// Check if project meets completion criteria
    async fn is_complete(&self, project: &Project, goal: &CreativeGoal) -> Result<bool> {
        match &goal.completion {
            CompletionCriteria::AllFeatures => {
                Ok(project.all_features_complete())
            }
            
            CompletionCriteria::QualityThreshold(threshold) => {
                let quality = self.assess_quality(project).await?;
                Ok(quality >= *threshold)
            }
            
            CompletionCriteria::UserApproval => {
                self.request_user_approval(project).await
            }
            
            CompletionCriteria::TimeLimit { sessions, hours } => {
                let session = &self.session;
                Ok(session.current_session >= *sessions || 
                   session.total_time.as_secs() >= (*hours as u64 * 3600))
            }
            
            CompletionCriteria::Combined(criteria) => {
                // All must be met
                for criterion in criteria {
                    if !self.is_complete_by_criterion(project, criterion).await? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
        }
    }
    
    /// Prevent premature completion
    fn prevent_premature_completion(&self, project: &Project) -> Result<Vec<MissingElement>> {
        let mut missing = Vec::new();
        
        // Must have tests
        if project.test_count() == 0 {
            missing.push(MissingElement::Tests);
        }
        
        // Must have documentation
        if !project.has_documentation() {
            missing.push(MissingElement::Documentation);
        }
        
        // Must have error handling
        if !project.has_error_handling() {
            missing.push(MissingElement::ErrorHandling);
        }
        
        // Must have all core features
        for feature in project.core_features() {
            if !feature.is_complete() {
                missing.push(MissingElement::CoreFeature(feature.name.clone()));
            }
        }
        
        Ok(missing)
    }
}
```

---

## Storage Management

```rust
pub struct StorageManager {
    /// Track disk usage
    usage: DiskUsage,
    
    /// Cleanup policies
    policies: CleanupPolicies,
}

pub struct DiskUsage {
    /// Temporary build artifacts
    temp_size: u64,
    
    /// Test environments
    test_env_size: u64,
    
    /// Project outputs
    project_size: u64,
    
    /// Total limit (optional)
    limit: Option<u64>,
}

impl StorageManager {
    /// Monitor and enforce storage limits
    pub async fn monitor_usage(&mut self) -> Result<()> {
        // Update usage stats
        self.usage.update().await?;
        
        if let Some(limit) = self.usage.limit {
            let total = self.usage.total();
            
            if total > limit {
                warn!("Storage limit exceeded: {} > {}", total, limit);
                self.aggressive_cleanup().await?;
            } else if total > (limit as f64 * 0.8) as u64 {
                warn!("Approaching storage limit: {} / {}", total, limit);
                self.routine_cleanup().await?;
            }
        }
        
        Ok(())
    }
    
    /// Clean up after project completion
    pub async fn cleanup_project(&mut self, project: &Project) -> Result<()> {
        info!("Cleaning up project: {}", project.name);
        
        // 1. Remove temporary files
        project.remove_temp_files().await?;
        
        // 2. Remove build artifacts (keep final binary)
        project.clean_build_artifacts().await?;
        
        // 3. Remove test environments
        project.cleanup_test_envs().await?;
        
        // 4. Compress logs
        project.compress_logs().await?;
        
        // 5. Archive if needed
        if self.policies.archive_completed {
            project.archive().await?;
        }
        
        info!("Cleanup complete");
        Ok(())
    }
}
```

---

## Summary: Testing & Creative Development

**Key Features:**

1. **Cross-Platform Sandboxing:**
   - Nix everywhere (best option)
   - Auto-install on non-NixOS systems
   - Fallback to Docker/Podman
   - Last resort: native package managers

2. **Comprehensive Testing:**
   - Detect what's testable
   - Run all possible tests
   - Manual checks for untestable aspects
   - Clear reports on limitations

3. **Creative Development Mode:**
   - Extended design phase with iteration
   - Multi-model brainstorming
   - Critical review before implementation
   - Iterative feature development
   - Feedback-driven refinement
   - No premature completion

4. **Storage Management:**
   - Monitor disk usage
   - Automatic cleanup
   - Keep only what's needed

Ready to implement? We have a solid foundation for a production-ready system.
