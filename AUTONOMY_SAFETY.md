# Autonomy, Safety, and Permissions System

## Operating Modes

### Production Mode (Default)
**Philosophy:** Autonomous, efficient, complete the task

```bash
# Default invocation
llm-conductor "Build a REST API for user management"
```

**Behavior:**
- ✅ Automatically parallelize across all available models
- ✅ Make architectural decisions autonomously
- ✅ Complete entire task end-to-end
- ✅ Only stop for truly critical decisions or blocking issues
- ✅ Use resources aggressively but efficiently
- ❌ Don't ask for permission for every small change
- ❌ Don't stop unless genuinely stuck

**Example Flow:**
```
User: "Build a REST API for user management"

System: [Silently orchestrates]
  - Ollama: Designs basic structure
  - GLM-5: Implements CRUD endpoints
  - Ollama: Writes tests
  - GLM-5: Reviews for security issues
  - Ollama: Adds documentation

System: "✓ Complete. Created 8 files, 1247 lines. 
         REST API with auth, CRUD, tests, docs.
         Run with: cargo run"
```

---

### Personal Mode (Interactive)
**Philosophy:** Careful, ask before major changes

```bash
# Explicit flag for personal mode
llm-conductor --personal "Refactor my authentication code"
```

**Behavior:**
- ⚠️  Sequential execution (one model at a time)
- ⚠️  Ask before significant changes
- ⚠️  Show reasoning and plans
- ⚠️  User can approve/reject/modify
- ✅ More conversational
- ✅ Explain decisions

**Example Flow:**
```
User: "Refactor my authentication code"

System: "Analyzing current auth implementation...
         Found: JWT tokens, bcrypt hashing, session storage.
         
         Proposed changes:
         1. Extract auth logic into separate module
         2. Add refresh token support
         3. Improve error handling
         4. Add rate limiting
         
         Proceed with all changes? (y/n/select)"

User: "y"

System: [Makes changes with progress updates]
```

---

## Permissions and Trust Hierarchy

### Model Permission Levels

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionLevel {
    /// Read-only: Can read context, cannot modify
    Observer = 0,
    
    /// Can execute assigned tasks, write to shared memory
    Worker = 1,
    
    /// Can coordinate workers, make tactical decisions
    TeamLead = 2,
    
    /// Can modify architecture, make strategic decisions
    Architect = 3,
    
    /// Full system control, can modify orchestration
    Orchestrator = 4,
}

pub struct ModelPermissions {
    level: PermissionLevel,
    
    /// What this model can do
    capabilities: Capabilities,
}

pub struct Capabilities {
    // File operations
    can_read_files: bool,
    can_write_files: bool,
    can_delete_files: bool,
    can_execute_commands: bool,
    
    // Context operations
    can_read_shared_memory: bool,
    can_write_shared_memory: bool,
    can_modify_pinned_items: bool,
    can_reset_context: bool,
    
    // Coordination
    can_spawn_workers: bool,
    can_reassign_tasks: bool,
    can_request_help: bool,
    can_provide_help: bool,
    
    // Architecture
    can_modify_architecture: bool,
    can_make_breaking_changes: bool,
    
    // Meta
    can_change_orchestration_mode: bool,
    can_modify_resource_limits: bool,
}
```

### Automatic Permission Assignment

```rust
impl Orchestrator {
    fn assign_permissions(&self, model: &ModelInfo) -> ModelPermissions {
        // Permission based on model capability tier
        let level = match model.capability_tier {
            // Frontier models: Full trust
            CapabilityTier::Frontier => {
                if model.is_opus() || model.is_gpt4o() || model.is_glm5() {
                    PermissionLevel::Architect
                } else {
                    PermissionLevel::TeamLead
                }
            }
            
            // Mid-tier: Worker level
            CapabilityTier::Advanced => PermissionLevel::Worker,
            
            // Small local models: Observer
            CapabilityTier::Basic => PermissionLevel::Observer,
        };
        
        ModelPermissions {
            level,
            capabilities: Self::default_capabilities_for_level(level),
        }
    }
    
    fn default_capabilities_for_level(level: PermissionLevel) -> Capabilities {
        match level {
            PermissionLevel::Observer => Capabilities {
                can_read_files: true,
                can_read_shared_memory: true,
                can_request_help: true,
                ..Default::default()  // Everything else false
            },
            
            PermissionLevel::Worker => Capabilities {
                can_read_files: true,
                can_write_files: true,  // In assigned scope only
                can_execute_commands: false,  // No shell access
                can_read_shared_memory: true,
                can_write_shared_memory: true,  // With review
                can_request_help: true,
                ..Default::default()
            },
            
            PermissionLevel::TeamLead => Capabilities {
                can_read_files: true,
                can_write_files: true,
                can_execute_commands: true,  // Can run tests
                can_read_shared_memory: true,
                can_write_shared_memory: true,
                can_modify_pinned_items: false,  // Still can't modify critical items
                can_spawn_workers: true,
                can_reassign_tasks: true,
                can_provide_help: true,
                ..Default::default()
            },
            
            PermissionLevel::Architect => Capabilities {
                // Can do almost everything
                can_delete_files: true,
                can_modify_architecture: true,
                can_make_breaking_changes: true,  // With safety checks
                can_modify_pinned_items: true,
                can_reset_context: true,
                ..Default::default()
            },
            
            PermissionLevel::Orchestrator => Capabilities {
                // Full control (only the main orchestrator)
                ..Capabilities::all()
            },
        }
    }
}
```

### Scoped Permissions

Even within a permission level, actions are scoped:

```rust
pub struct PermissionScope {
    /// Which files this model can modify
    file_allowlist: Option<Vec<PathBuf>>,
    
    /// Which directories are off-limits
    file_denylist: Vec<PathBuf>,
    
    /// Max number of files can create/modify
    max_files_modified: Option<usize>,
    
    /// Max lines of code can write
    max_lines_written: Option<usize>,
    
    /// Which shared memory topics can write to
    memory_topics: Vec<String>,
}

impl Worker {
    async fn write_file(&mut self, path: &Path, content: &str) -> Result<()> {
        // Check permission level
        if !self.permissions.capabilities.can_write_files {
            return Err(anyhow!("Insufficient permissions to write files"));
        }
        
        // Check scope allowlist
        if let Some(ref allowlist) = self.scope.file_allowlist {
            if !allowlist.iter().any(|p| path.starts_with(p)) {
                return Err(anyhow!("Path {:?} not in allowlist", path));
            }
        }
        
        // Check scope denylist
        if self.scope.file_denylist.iter().any(|p| path.starts_with(p)) {
            return Err(anyhow!("Path {:?} is denied", path));
        }
        
        // Check if approaching limits
        if let Some(max) = self.scope.max_files_modified {
            if self.stats.files_modified >= max {
                warn!("Approaching file modification limit, requesting approval");
                return self.request_permission_extension().await;
            }
        }
        
        // Proceed with write
        self.fs.write(path, content).await?;
        self.stats.files_modified += 1;
        
        Ok(())
    }
}
```

### Shared Memory Access Control

```rust
pub struct MemoryAccessControl {
    /// Who can write to pinned items
    pinned_writers: HashSet<WorkerId>,
    
    /// Per-topic access control
    topic_acls: HashMap<String, TopicACL>,
}

pub struct TopicACL {
    /// Anyone can read
    readers: AccessRule,
    
    /// Who can write
    writers: AccessRule,
    
    /// Who can delete/modify existing items
    moderators: AccessRule,
}

pub enum AccessRule {
    Anyone,
    MinimumLevel(PermissionLevel),
    SpecificWorkers(HashSet<WorkerId>),
    None,
}

impl SharedMemory {
    pub fn post(&mut self, author: WorkerId, item: MemoryItem) -> Result<()> {
        // Check write permission for topic
        let acl = self.acl.topic_acls.get(&item.topic)
            .unwrap_or(&TopicACL::default());
        
        let author_perms = self.get_permissions(author);
        
        if !acl.writers.allows(author, author_perms) {
            return Err(anyhow!("No write permission for topic {}", item.topic));
        }
        
        // Special check for pinned items
        if item.importance > 0.8 {
            // High-importance items require higher permission
            if author_perms.level < PermissionLevel::TeamLead {
                // Downgrade importance instead of rejecting
                item.importance = 0.7;
                warn!("Worker {} tried to pin item, downgraded to 0.7", author);
            }
        }
        
        // Proceed with post
        self.recent.push(item);
        Ok(())
    }
}
```

---

## Early Task Complexity Detection

### Immediate Complexity Assessment

```rust
impl Router {
    /// Assess complexity BEFORE assigning to cheapest model
    async fn smart_route(&self, task: &Task) -> Result<ProviderId> {
        // Quick complexity check (no model needed, heuristics)
        let initial_complexity = self.quick_assess(task);
        
        if initial_complexity.confidence > 0.9 {
            // Highly confident about complexity
            return self.route_by_complexity(initial_complexity.level);
        }
        
        // Use lightweight model to assess
        let assessed = self.assess_with_detector(task).await?;
        
        match assessed.level {
            ComplexityLevel::Trivial => {
                // Ollama can handle
                info!("Trivial task, assigning to Ollama");
                self.select_provider(CapabilityTier::Basic)
            }
            
            ComplexityLevel::Simple => {
                // Ollama or GLM-5
                info!("Simple task, assigning to mid-tier model");
                self.select_provider(CapabilityTier::Advanced)
            }
            
            ComplexityLevel::Moderate => {
                // GLM-5 or Sonnet
                info!("Moderate task, assigning to capable model");
                self.select_provider(CapabilityTier::Frontier)
            }
            
            ComplexityLevel::Complex => {
                // Opus or GPT-4o
                info!("Complex task, assigning to frontier model");
                self.select_best_frontier_model()
            }
            
            ComplexityLevel::Expert => {
                // Definitely Opus if available
                warn!("Expert-level task, requires best model");
                self.select_opus_or_best_available()
            }
        }
    }
    
    /// Fast heuristic-based assessment (no model needed)
    fn quick_assess(&self, task: &Task) -> ComplexityAssessment {
        let mut indicators = ComplexityIndicators::default();
        
        // Keyword analysis
        let keywords_expert = ["security", "crypto", "algorithm", "optimize", "architecture"];
        let keywords_complex = ["refactor", "debug", "performance", "concurrent", "async"];
        let keywords_simple = ["add", "list", "display", "format", "print"];
        
        let desc_lower = task.description.to_lowercase();
        
        if keywords_expert.iter().any(|k| desc_lower.contains(k)) {
            indicators.expert_keywords += 1;
        }
        if keywords_complex.iter().any(|k| desc_lower.contains(k)) {
            indicators.complex_keywords += 1;
        }
        if keywords_simple.iter().any(|k| desc_lower.contains(k)) {
            indicators.simple_keywords += 1;
        }
        
        // Scope analysis
        if task.affects_multiple_modules() {
            indicators.scope_multiplier = 1.5;
        }
        
        // Past performance
        if let Some(similar) = self.history.find_similar_tasks(task) {
            if similar.required_capability > CapabilityTier::Advanced {
                indicators.historical_evidence = 1.2;
            }
        }
        
        // Compute confidence
        let confidence = if indicators.strong_signals() {
            0.95  // Very confident
        } else if indicators.mixed_signals() {
            0.5   // Uncertain, need model assessment
        } else {
            0.8   // Fairly confident
        };
        
        ComplexityAssessment {
            level: indicators.compute_level(),
            confidence,
            reasoning: indicators.explain(),
        }
    }
    
    /// Use small detector model to assess
    async fn assess_with_detector(&self, task: &Task) -> Result<ComplexityAssessment> {
        // Use Ollama (fast, free, local) to assess
        let detector = self.get_detector_model();
        
        let prompt = format!(
            "Assess the complexity of this task on a scale of 1-5:
            1 = Trivial (basic text operations)
            2 = Simple (straightforward logic)
            3 = Moderate (requires some reasoning)
            4 = Complex (multi-step, nuanced)
            5 = Expert (requires deep expertise)
            
            Task: {}
            
            Respond with just the number and brief reason.",
            task.description
        );
        
        let response = detector.generate(&prompt).await?;
        
        // Parse response
        let (level, reasoning) = self.parse_complexity_response(&response)?;
        
        Ok(ComplexityAssessment {
            level,
            confidence: 0.85,  // Detector model assessment
            reasoning,
        })
    }
}

pub struct ComplexityAssessment {
    level: ComplexityLevel,
    confidence: f32,  // 0.0-1.0
    reasoning: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComplexityLevel {
    Trivial = 1,    // "List files in directory"
    Simple = 2,     // "Add logging to function"
    Moderate = 3,   // "Implement user authentication"
    Complex = 4,    // "Refactor architecture for scalability"
    Expert = 5,     // "Design distributed consensus algorithm"
}
```

---

## Escalation and Help System

### Upstream Communication Protocol

```rust
pub enum EscalationReason {
    /// Model is genuinely stuck
    Stuck {
        attempts: usize,
        stuck_reason: StuckReason,
        context_size: usize,
    },
    
    /// Context overload
    ContextOverload {
        current_tokens: usize,
        max_tokens: usize,
        pruning_ineffective: bool,
    },
    
    /// Missing information
    MissingInformation {
        what: String,
        tried: Vec<String>,  // What sources already checked
    },
    
    /// Task beyond capability
    OutOfScope {
        reason: String,
        suggest_model: Option<ModelId>,
    },
    
    /// Conflicting requirements
    Ambiguous {
        interpretations: Vec<String>,
        needs_clarification: String,
    },
    
    /// Resource constraints
    ResourceExhausted {
        resource: Resource,
        needed: usize,
        available: usize,
    },
}

impl Worker {
    /// Request help from upstream (team lead or orchestrator)
    async fn escalate(&mut self, reason: EscalationReason) -> Result<EscalationResponse> {
        // Log escalation (for learning)
        self.telemetry.log_escalation(&reason);
        
        // Determine if this is a real issue or premature escalation
        let validity = self.validate_escalation(&reason)?;
        
        match validity {
            EscalationValidity::Genuine => {
                info!("Genuine escalation: {:?}", reason);
                
                // Send to supervisor
                self.send_upstream(WorkerMsg::Escalate {
                    worker_id: self.id,
                    reason: reason.clone(),
                    current_state: self.export_state(),
                }).await?;
                
                // Wait for response
                self.recv_from_upstream().await
            }
            
            EscalationValidity::Premature { try_first } => {
                warn!("Premature escalation detected, trying: {}", try_first);
                // System suggests what to try first
                Err(anyhow!("Try: {}", try_first))
            }
            
            EscalationValidity::FalsePositive { reason: r } => {
                warn!("False positive escalation: {}", r);
                // Don't escalate, continue working
                Err(anyhow!("Not a genuine issue: {}", r))
            }
        }
    }
    
    /// Validate that escalation is warranted
    fn validate_escalation(&self, reason: &EscalationReason) -> Result<EscalationValidity> {
        match reason {
            EscalationReason::Stuck { attempts, .. } => {
                // Must have tried at least 3 times
                if *attempts < 3 {
                    return Ok(EscalationValidity::Premature {
                        try_first: "Try at least 3 different approaches".to_string()
                    });
                }
                
                // Must have tried context reset
                if !self.stats.context_resets > 0 {
                    return Ok(EscalationValidity::Premature {
                        try_first: "Try resetting context first".to_string()
                    });
                }
                
                Ok(EscalationValidity::Genuine)
            }
            
            EscalationReason::ContextOverload { pruning_ineffective, .. } => {
                // Must have tried pruning first
                if !pruning_ineffective {
                    return Ok(EscalationValidity::Premature {
                        try_first: "Try aggressive context pruning".to_string()
                    });
                }
                
                Ok(EscalationValidity::Genuine)
            }
            
            EscalationReason::MissingInformation { tried, .. } => {
                // Must have checked shared memory
                if !tried.iter().any(|s| s.contains("shared_memory")) {
                    return Ok(EscalationValidity::Premature {
                        try_first: "Check shared memory for the information".to_string()
                    });
                }
                
                // Must have tried querying orchestrator
                if !tried.iter().any(|s| s.contains("query")) {
                    return Ok(EscalationValidity::Premature {
                        try_first: "Query orchestrator for specific information".to_string()
                    });
                }
                
                Ok(EscalationValidity::Genuine)
            }
            
            EscalationReason::OutOfScope { suggest_model, .. } => {
                // Always valid - model knows its limits
                Ok(EscalationValidity::Genuine)
            }
            
            _ => Ok(EscalationValidity::Genuine)
        }
    }
}

pub enum EscalationValidity {
    Genuine,
    Premature { try_first: String },
    FalsePositive { reason: String },
}

pub enum EscalationResponse {
    /// Task reassigned to more capable model
    Reassigned {
        new_worker: WorkerId,
        transfer_context: bool,
    },
    
    /// Hint/guidance provided, try again
    Guidance {
        hint: String,
        additional_context: Option<Context>,
    },
    
    /// Information provided
    InformationProvided {
        answer: String,
    },
    
    /// User intervention needed
    UserInterventionRequired {
        question: String,
    },
    
    /// Permission granted
    PermissionGranted {
        for_action: String,
        scope: PermissionScope,
    },
}
```

### TeamLead Response to Escalations

```rust
impl TeamLead {
    async fn handle_escalation(&mut self, msg: EscalationMsg) -> Result<()> {
        let worker_id = msg.worker_id;
        let reason = msg.reason;
        
        info!("Handling escalation from {:?}: {:?}", worker_id, reason);
        
        match reason {
            EscalationReason::OutOfScope { suggest_model, .. } => {
                // Worker correctly identified it needs help
                if let Some(better_model) = suggest_model {
                    // Reassign to suggested model
                    self.reassign_task(worker_id, better_model).await?;
                } else {
                    // Find best available model
                    let best = self.find_best_available_model(&msg.task)?;
                    self.reassign_task(worker_id, best).await?;
                }
            }
            
            EscalationReason::Stuck { .. } => {
                // Provide fresh perspective or reassign
                if self.can_provide_hint(&msg) {
                    let hint = self.generate_hint(&msg).await?;
                    self.send_to_worker(worker_id, WorkerMsg::Guidance { hint }).await?;
                } else {
                    // Reassign to different model for fresh approach
                    let alternate = self.find_alternate_model(worker_id)?;
                    self.reassign_task(worker_id, alternate).await?;
                }
            }
            
            EscalationReason::MissingInformation { what, .. } => {
                // Try to provide the information
                if let Some(info) = self.lookup_information(&what).await {
                    self.send_to_worker(
                        worker_id,
                        WorkerMsg::InformationProvided { answer: info }
                    ).await?;
                } else {
                    // Escalate to orchestrator or user
                    self.escalate_to_orchestrator(msg).await?;
                }
            }
            
            EscalationReason::Ambiguous { interpretations, needs_clarification } => {
                // This requires user input in personal mode, but in production mode,
                // make best judgment
                if self.mode == Mode::Personal {
                    // Ask user
                    self.request_user_clarification(needs_clarification).await?;
                } else {
                    // Make autonomous decision
                    let choice = self.choose_best_interpretation(&interpretations).await?;
                    self.send_to_worker(
                        worker_id,
                        WorkerMsg::Guidance {
                            hint: format!("Proceeding with interpretation: {}", choice)
                        }
                    ).await?;
                }
            }
            
            _ => {
                // Other escalations go to orchestrator
                self.escalate_to_orchestrator(msg).await?;
            }
        }
        
        Ok(())
    }
}
```

---

## Balancing Autonomy and Safety

### The "Try → Validate → Escalate" Pattern

```rust
impl Worker {
    async fn execute_with_safety(&mut self, task: Task) -> Result<TaskResult> {
        loop {
            // 1. TRY: Attempt the task
            let attempt = self.attempt_task(&task).await;
            
            match attempt {
                Ok(result) => {
                    // 2. VALIDATE: Check if result is good
                    let validation = self.validate_result(&task, &result).await?;
                    
                    match validation {
                        Validation::Pass => {
                            // Success! Return result
                            return Ok(result);
                        }
                        
                        Validation::Warning(w) => {
                            // Acceptable but with caveats
                            warn!("Task completed with warnings: {}", w);
                            return Ok(result.with_warnings(w));
                        }
                        
                        Validation::Fail(reason) => {
                            // Failed validation, try again
                            warn!("Validation failed: {}", reason);
                            
                            // Check if we should keep trying
                            if self.should_retry(&task, &reason) {
                                task.add_constraint(format!("Previous attempt failed: {}", reason));
                                continue;  // Try again
                            } else {
                                // 3. ESCALATE: Can't complete, need help
                                return self.escalate_and_wait(
                                    EscalationReason::Stuck {
                                        attempts: task.attempts,
                                        stuck_reason: StuckReason::FailedValidation,
                                        context_size: self.current_context.token_count(),
                                    }
                                ).await;
                            }
                        }
                    }
                }
                
                Err(e) => {
                    // Error during execution
                    error!("Task execution error: {}", e);
                    
                    // Classify error
                    match self.classify_error(&e) {
                        ErrorType::Recoverable => {
                            // Can retry
                            warn!("Recoverable error, retrying");
                            continue;
                        }
                        
                        ErrorType::OutOfScope => {
                            // Need better model
                            return self.escalate_and_wait(
                                EscalationReason::OutOfScope {
                                    reason: e.to_string(),
                                    suggest_model: None,
                                }
                            ).await;
                        }
                        
                        ErrorType::Fatal => {
                            // Can't recover
                            return Err(e);
                        }
                    }
                }
            }
        }
    }
}
```

### Autonomous Decision Making with Guardrails

```rust
pub struct SafetyGuardrails {
    /// Auto-approve if impact score below threshold
    auto_approve_threshold: f32,  // 0.3 = low impact
    
    /// Require confirmation if above threshold
    require_confirmation_threshold: f32,  // 0.7 = high impact
    
    /// Hard block if above this
    hard_block_threshold: f32,  // 0.9 = critical
}

impl Worker {
    /// Make decision with safety checks
    async fn make_decision(&self, decision: Decision) -> Result<DecisionOutcome> {
        // Calculate impact score
        let impact = self.assess_impact(&decision);
        
        let guardrails = if self.mode == Mode::Personal {
            SafetyGuardrails {
                auto_approve_threshold: 0.1,  // Very conservative
                require_confirmation_threshold: 0.3,
                hard_block_threshold: 0.8,
            }
        } else {
            SafetyGuardrails {
                auto_approve_threshold: 0.5,  // Aggressive
                require_confirmation_threshold: 0.8,
                hard_block_threshold: 0.95,
            }
        };
        
        if impact.score < guardrails.auto_approve_threshold {
            // Low impact: proceed autonomously
            info!("Auto-approving low-impact decision: {}", decision.description);
            Ok(DecisionOutcome::Approved)
        } else if impact.score < guardrails.require_confirmation_threshold {
            // Medium impact: log but proceed
            warn!("Proceeding with medium-impact decision: {}", decision.description);
            self.log_decision(&decision, &impact).await;
            Ok(DecisionOutcome::Approved)
        } else if impact.score < guardrails.hard_block_threshold {
            // High impact: requires approval in personal mode
            if self.mode == Mode::Personal {
                warn!("High-impact decision requires confirmation");
                self.request_user_approval(&decision, &impact).await
            } else {
                // In production mode, log prominently and proceed
                warn!("HIGH-IMPACT DECISION (auto-approved in production mode):");
                warn!("  {}", decision.description);
                warn!("  Impact: {:.2} - {}", impact.score, impact.reasoning);
                self.log_decision(&decision, &impact).await;
                Ok(DecisionOutcome::Approved)
            }
        } else {
            // Critical impact: always require approval
            error!("CRITICAL DECISION requires user approval:");
            error!("  {}", decision.description);
            error!("  Impact: {:.2} - {}", impact.score, impact.reasoning);
            self.request_user_approval(&decision, &impact).await
        }
    }
    
    fn assess_impact(&self, decision: &Decision) -> ImpactAssessment {
        let mut score = 0.0;
        let mut reasons = Vec::new();
        
        // Irreversible changes
        if decision.is_destructive() {
            score += 0.4;
            reasons.push("Destructive operation");
        }
        
        // Broad scope
        if decision.affects_multiple_modules() {
            score += 0.2;
            reasons.push("Affects multiple modules");
        }
        
        // Breaking changes
        if decision.is_breaking_change() {
            score += 0.3;
            reasons.push("Breaking API change");
        }
        
        // Security implications
        if decision.affects_security() {
            score += 0.5;
            reasons.push("Security implications");
        }
        
        ImpactAssessment {
            score: score.min(1.0),
            reasoning: reasons.join(", "),
        }
    }
}
```

---

## Anti-Patterns to Avoid

### ❌ Bad: Over-Communication

```rust
// DON'T: Send every tiny update to everyone
for change in small_changes {
    shared_memory.broadcast_to_all(change);  // ❌ Context pollution
}
```

### ✅ Good: Batched Updates

```rust
// DO: Batch small changes, send summary
let batch = small_changes.collect();
if batch.len() > 10 {
    let summary = summarize_changes(&batch);
    shared_memory.post_update(summary);  // ✅ Efficient
}
```

---

### ❌ Bad: Premature Escalation

```rust
// DON'T: Give up immediately
fn solve_problem(&self, problem: Problem) -> Result<Solution> {
    if problem.is_hard() {
        return self.escalate("Too hard");  // ❌ Didn't even try
    }
    // ...
}
```

### ✅ Good: Try First, Then Escalate

```rust
// DO: Exhaust options before escalating
fn solve_problem(&mut self, problem: Problem) -> Result<Solution> {
    // Try multiple approaches
    for attempt in 1..=3 {
        match self.attempt_solution(&problem, attempt) {
            Ok(solution) => return Ok(solution),
            Err(e) => warn!("Attempt {} failed: {}", attempt, e),
        }
    }
    
    // Tried 3 times, now escalate
    self.escalate(EscalationReason::Stuck { attempts: 3, .. })
}
```

---

### ❌ Bad: False Positives on Stuck Detection

```rust
// DON'T: Flag as stuck just because it's taking time
if task.duration > Duration::from_secs(30) {
    return Err("Stuck");  // ❌ Maybe just slow
}
```

### ✅ Good: Multi-Factor Stuck Detection

```rust
// DO: Look for actual stuck patterns
if stuck_detector.is_stuck(output)
    && task.duration > expected_duration * 2
    && !making_progress() {
    return Err("Genuinely stuck");  // ✅ Confident
}
```

---

## Mode Comparison Table

| Feature | Production Mode | Personal Mode |
|---------|----------------|---------------|
| **Execution** | Parallel, all models | Sequential, one model |
| **Autonomy** | High - complete tasks end-to-end | Low - ask for approval |
| **Decision Making** | Auto-approve low/medium impact | Confirm medium/high impact |
| **Escalation** | Only when genuinely blocked | More liberal |
| **Resource Usage** | Aggressive but efficient | Conservative |
| **User Interaction** | Minimal (only critical) | Conversational |
| **Output** | Final result + summary | Step-by-step updates |

---

## Implementation Checklist

- [ ] Permission system with 5 levels
- [ ] Scoped capabilities per model
- [ ] Shared memory ACLs
- [ ] Early complexity detection (heuristic + detector model)
- [ ] Escalation validation system
- [ ] Try→Validate→Escalate pattern
- [ ] Safety guardrails with configurable thresholds
- [ ] Mode-specific behavior (production vs personal)
- [ ] Anti-pattern detection and prevention

This creates a system that's autonomous enough to complete tasks end-to-end, but safe enough to prevent disasters.
