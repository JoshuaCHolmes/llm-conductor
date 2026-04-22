# Context Management & Coordination System

## Core Philosophy

**Default Behavior:** Fully dynamic resource utilization with minimal necessary usage
- Use cheapest/fastest model that can handle the task
- Scale up only when needed
- Automatically scale down when possible
- Share context efficiently across models

**Override Flags:** User can constrain behavior
```bash
# Force single model (no parallelization)
llm-conductor --single-model

# Cap maximum concurrent models
llm-conductor --max-models 2

# Limit to specific providers
llm-conductor --providers ollama,nim

# Force specific model for all tasks
llm-conductor --force-model claude-opus-4.5

# Cap resource usage per provider
llm-conductor --max-requests-per-min nim:30

# Disable dynamic scaling
llm-conductor --no-auto-scale
```

---

## Context Synchronization Problem

### The Challenge

When working with large codebases or long conversations:
- **Problem 1:** New model joins mid-project → needs project context
- **Problem 2:** Model switches due to rate limit → replacement needs context
- **Problem 3:** Multiple models working → need shared understanding
- **Problem 4:** Context window limits → need intelligent pruning

### Context Levels

```rust
pub struct ContextHierarchy {
    /// Level 0: Always included (tiny, critical)
    core: CoreContext,          // ~1K tokens: Task goal, constraints, user prefs
    
    /// Level 1: Project-level (small, stable)
    project: ProjectContext,    // ~5K tokens: Architecture, key files, conventions
    
    /// Level 2: Session-level (medium, evolving)
    session: SessionContext,    // ~20K tokens: Recent changes, decisions made
    
    /// Level 3: Task-level (large, specific)
    task: TaskContext,          // ~50K tokens: Relevant code, full context for this task
    
    /// Level 4: Extended (huge, rarely needed)
    extended: ExtendedContext,  // ~200K tokens: Full codebase, entire history
}
```

### Adaptive Context Loading

```rust
impl ContextManager {
    /// Smart context loading based on model and task
    async fn prepare_context_for_model(
        &self,
        model: &ModelInfo,
        task: &Task,
        urgency: Urgency
    ) -> Context {
        let budget = self.calculate_token_budget(model, task);
        
        // Always include core
        let mut context = Context::new(self.core.clone());
        let mut used = self.core.token_count();
        
        // Progressive loading based on available budget
        if used + self.project.token_count() < budget {
            context.add_project(self.project.clone());
            used += self.project.token_count();
        } else {
            // Summarize project context if too large
            let summary = self.summarize_project(budget - used).await;
            context.add_project_summary(summary);
        }
        
        // Load recent session history
        let session_budget = budget.saturating_sub(used);
        let session_ctx = self.session.recent_relevant(task, session_budget);
        context.add_session(session_ctx);
        used += session_ctx.token_count();
        
        // Load task-specific context (most important)
        let task_budget = budget.saturating_sub(used);
        let task_ctx = self.task.for_task(task, task_budget);
        context.add_task(task_ctx);
        
        context
    }
    
    /// Calculate how many tokens we can spend on context
    fn calculate_token_budget(&self, model: &ModelInfo, task: &Task) -> usize {
        let max_context = model.context_window;
        let reserved_for_output = task.estimated_output_tokens().unwrap_or(4000);
        let reserved_for_system = 500;
        
        max_context
            .saturating_sub(reserved_for_output)
            .saturating_sub(reserved_for_system)
    }
}
```

---

## Fast Context Catch-Up

### The "Project Brief" Pattern

When a new model joins or needs quick onboarding:

```rust
pub struct ProjectBrief {
    /// 1-paragraph project description
    summary: String,                    // ~100 tokens
    
    /// Current objective
    current_goal: String,               // ~50 tokens
    
    /// Key decisions made (chronological, recent first)
    decisions: Vec<Decision>,           // ~500 tokens
    
    /// Important files/modules
    key_files: Vec<FileReference>,      // ~200 tokens
    
    /// Active constraints
    constraints: Vec<String>,           // ~100 tokens
    
    /// Current state (what's done, what's pending)
    progress: ProgressSnapshot,         // ~200 tokens
    
    // Total: ~1150 tokens for comprehensive onboarding
}

impl ProjectBrief {
    /// Generate brief from full context (for new models)
    async fn from_context(ctx: &Context, summarizer: &dyn Model) -> Self {
        // Use a fast local model to summarize
        let summary = summarizer.summarize(
            &ctx,
            "Create 1-paragraph project summary with current goal"
        ).await;
        
        ProjectBrief {
            summary,
            current_goal: ctx.extract_current_goal(),
            decisions: ctx.recent_decisions().take(5).collect(),
            key_files: ctx.most_referenced_files().take(10).collect(),
            constraints: ctx.active_constraints(),
            progress: ctx.snapshot_progress(),
        }
    }
}
```

### Incremental Context Updates

Instead of sending full context to every model on every change:

```rust
pub enum ContextDelta {
    /// New file added to project
    FileAdded { path: PathBuf, summary: String },
    
    /// File modified
    FileModified { path: PathBuf, diff: String },
    
    /// Decision made
    DecisionRecorded { decision: Decision },
    
    /// Task completed
    TaskCompleted { task_id: TaskId, result: String },
    
    /// Constraint added
    ConstraintAdded { constraint: String },
    
    /// Important finding
    InsightDiscovered { insight: String },
}

impl ContextManager {
    /// Broadcast small delta instead of full context
    async fn broadcast_delta(&self, delta: ContextDelta) {
        // All active models get notified of change
        for worker in self.active_workers.iter() {
            worker.send(WorkerMsg::ContextUpdate(delta.clone())).await;
        }
        
        // Persist to session history
        self.session.append(delta);
    }
}
```

### Intelligent Context Pruning

When context grows too large:

```rust
impl ContextManager {
    /// Prune context intelligently when approaching limits
    async fn prune_context(&mut self, target_size: usize) {
        let current_size = self.total_token_count();
        
        if current_size <= target_size {
            return; // No pruning needed
        }
        
        let to_remove = current_size - target_size;
        
        // Pruning priority (remove in this order):
        // 1. Old successful task results (keep failures for learning)
        let removed = self.prune_old_successes(to_remove);
        if removed >= to_remove { return; }
        
        // 2. Intermediate reasoning steps (keep conclusions)
        let removed = self.prune_reasoning_chains(to_remove - removed);
        if removed >= to_remove { return; }
        
        // 3. Duplicate information (deduplicate)
        let removed = self.deduplicate_context(to_remove - removed);
        if removed >= to_remove { return; }
        
        // 4. Summarize verbose sections
        self.summarize_verbose_sections(to_remove - removed).await;
    }
}
```

---

## Model-to-Model Communication

### Shared Memory Board

Like a project whiteboard everyone can see:

```rust
pub struct SharedMemory {
    /// Pinned items (always visible to all models)
    pinned: Vec<MemoryItem>,
    
    /// Recent updates (rolling buffer)
    recent: RingBuffer<MemoryItem, 50>,
    
    /// Indexed by topic (for quick lookup)
    topics: HashMap<String, Vec<MemoryItem>>,
}

pub struct MemoryItem {
    timestamp: Instant,
    author: WorkerId,           // Which model wrote this
    category: MemoryCategory,
    content: String,
    importance: f32,            // 0.0-1.0
    references: Vec<ItemId>,    // Links to other items
}

pub enum MemoryCategory {
    Decision,        // "We decided to use REST API"
    Finding,         // "Found security issue in auth.rs"
    Question,        // "Should we use async here?"
    Answer,          // Response to a question
    Warning,         // "This approach might hit rate limits"
    Progress,        // "Completed module X"
    Insight,         // "Pattern: All errors lack context"
}

impl SharedMemory {
    /// Model writes to shared memory
    pub fn post(&mut self, item: MemoryItem) {
        // Add to recent buffer
        self.recent.push(item.clone());
        
        // Index by topic
        for topic in item.extract_topics() {
            self.topics.entry(topic).or_default().push(item.clone());
        }
        
        // High-importance items get pinned
        if item.importance > 0.8 {
            self.pinned.push(item);
        }
    }
    
    /// Model reads relevant context
    pub fn read_relevant(&self, task: &Task) -> Vec<MemoryItem> {
        let mut relevant = Vec::new();
        
        // Always include pinned
        relevant.extend(self.pinned.iter().cloned());
        
        // Include recent
        relevant.extend(self.recent.iter().cloned());
        
        // Include items matching task topics
        for topic in task.topics() {
            if let Some(items) = self.topics.get(&topic) {
                relevant.extend(items.iter().cloned());
            }
        }
        
        // Sort by importance, deduplicate
        relevant.sort_by(|a, b| b.importance.partial_cmp(&a.importance).unwrap());
        relevant.dedup_by(|a, b| a.content == b.content);
        
        relevant
    }
}
```

### Cross-Model Consultation

When a model gets stuck:

```rust
impl Worker {
    async fn execute_task(&mut self, task: Task) -> Result<TaskResult> {
        let start = Instant::now();
        let mut attempt = 0;
        
        loop {
            attempt += 1;
            
            // Try to complete task
            match self.model.generate(&task.prompt).await {
                Ok(result) => {
                    // Validate result quality
                    let quality = self.assess_quality(&result);
                    
                    if quality.is_acceptable() {
                        return Ok(result);
                    }
                    
                    // Low quality - are we stuck?
                    if quality.is_stuck() {
                        warn!("Model stuck on task, requesting help");
                        return self.request_help(&task, &result).await;
                    }
                }
                Err(e) => {
                    error!("Model error: {}", e);
                    return self.request_help(&task, &format!("Error: {}", e)).await;
                }
            }
            
            // Timeout check
            if start.elapsed() > task.timeout {
                warn!("Task timeout, requesting help");
                return self.request_help(&task, "timeout").await;
            }
            
            // Too many attempts
            if attempt > 3 {
                warn!("Too many attempts, requesting help");
                return self.request_help(&task, "stuck_in_loop").await;
            }
        }
    }
    
    async fn request_help(&self, task: &Task, reason: &str) -> Result<TaskResult> {
        // Post to shared memory
        self.shared_memory.post(MemoryItem {
            author: self.id,
            category: MemoryCategory::Question,
            content: format!("Stuck on: {} (reason: {})", task.description, reason),
            importance: 0.8,
            ..Default::default()
        });
        
        // Ask orchestrator for help
        self.send_to_orchestrator(WorkerMsg::RequestHelp {
            task: task.clone(),
            reason: reason.to_string(),
            context: self.current_context.clone(),
        }).await?;
        
        // Wait for response (orchestrator will assign to different model)
        let response = self.recv_from_orchestrator().await?;
        
        match response {
            OrchestratorMsg::TaskReassigned { new_worker } => {
                info!("Task reassigned to {:?}", new_worker);
                Err(anyhow!("Task reassigned"))
            }
            OrchestratorMsg::HelpProvided { hint } => {
                info!("Received hint: {}", hint);
                // Try again with hint
                task.add_hint(hint);
                self.execute_task(task).await
            }
            _ => Err(anyhow!("Unexpected response"))
        }
    }
}
```

### Detection of "Stuck" States

```rust
pub struct StuckDetector {
    /// Track output patterns
    output_history: RingBuffer<String, 5>,
}

impl StuckDetector {
    /// Detect if model is stuck in a rut
    pub fn is_stuck(&mut self, output: &str) -> StuckReason {
        self.output_history.push(output.to_string());
        
        // Pattern 1: Repeating same output
        if self.all_similar() {
            return StuckReason::RepetitiveOutput;
        }
        
        // Pattern 2: Repeatedly saying "I need to..." without doing it
        if self.excessive_planning() {
            return StuckReason::AnalysisParalysis;
        }
        
        // Pattern 3: Bouncing between same two approaches
        if self.oscillating() {
            return StuckReason::Oscillating;
        }
        
        // Pattern 4: Generic responses without progress
        if self.no_concrete_progress() {
            return StuckReason::Vague;
        }
        
        // Pattern 5: Asking same question repeatedly
        if self.repeated_questions() {
            return StuckReason::MissingInformation;
        }
        
        StuckReason::NotStuck
    }
    
    fn all_similar(&self) -> bool {
        // Check if last N outputs are >80% similar
        let outputs: Vec<_> = self.output_history.iter().collect();
        if outputs.len() < 3 { return false; }
        
        let similarities: Vec<f32> = outputs.windows(2)
            .map(|w| string_similarity(w[0], w[1]))
            .collect();
        
        similarities.iter().all(|&s| s > 0.8)
    }
    
    fn excessive_planning(&self) -> bool {
        // Check if model keeps planning without executing
        let plan_phrases = [
            "I need to", "First, I'll", "The approach is",
            "I should", "Let me think", "The plan is"
        ];
        
        self.output_history.iter()
            .filter(|out| plan_phrases.iter().any(|p| out.contains(p)))
            .count() >= 3
    }
}

pub enum StuckReason {
    NotStuck,
    RepetitiveOutput,       // Saying same thing over and over
    AnalysisParalysis,      // Planning without executing
    Oscillating,            // Bouncing between approaches
    Vague,                  // Generic responses, no specifics
    MissingInformation,     // Needs info it doesn't have
}
```

### Context Reset Strategy

When a model needs fresh perspective:

```rust
impl Worker {
    /// Clear model's context and provide minimal reframing
    async fn reset_context(&mut self, task: &Task) -> Result<()> {
        info!("Resetting context for worker {:?}", self.id);
        
        // 1. Save what we learned (don't lose everything)
        let learned = self.extract_useful_insights();
        self.shared_memory.post(MemoryItem {
            author: self.id,
            category: MemoryCategory::Insight,
            content: format!("Before reset: {}", learned),
            importance: 0.6,
            ..Default::default()
        });
        
        // 2. Clear current context
        self.current_context = Context::new(self.context_manager.core.clone());
        
        // 3. Load minimal fresh context
        let fresh = self.context_manager.prepare_minimal_context(task).await;
        self.current_context.merge(fresh);
        
        // 4. Add fresh perspective prompt
        self.current_context.add_instruction(
            "Previous attempt got stuck. Approaching with fresh perspective."
        );
        
        Ok(())
    }
}
```

---

## Optimal Communication Patterns

### The "Need to Know" Principle

Not every model needs every update:

```rust
pub struct CommunicationPolicy {
    /// Minimum importance to broadcast to all
    broadcast_threshold: f32,  // 0.8 = only critical updates
    
    /// Share within team always, across teams only if important
    team_boundary_threshold: f32,  // 0.5
}

impl Orchestrator {
    /// Decide who needs to know about an update
    fn route_update(&self, update: ContextDelta) -> Vec<WorkerId> {
        let importance = update.importance();
        let mut recipients = Vec::new();
        
        // Critical updates: everyone
        if importance >= self.policy.broadcast_threshold {
            recipients.extend(self.all_workers());
            return recipients;
        }
        
        // Medium importance: same team + related teams
        if importance >= self.policy.team_boundary_threshold {
            let author_team = self.find_team(update.author());
            recipients.extend(author_team.members());
            
            // Related teams (working on same feature/module)
            let related = self.find_related_teams(author_team);
            recipients.extend(related.iter().flat_map(|t| t.members()));
            
            return recipients;
        }
        
        // Low importance: same team only
        let author_team = self.find_team(update.author());
        recipients.extend(author_team.members());
        
        recipients
    }
}
```

### Periodic Sync Points

Even with incremental updates, occasionally do full sync:

```rust
impl Orchestrator {
    async fn sync_loop(&mut self) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        
        loop {
            interval.tick().await;
            
            // Every minute: light sync (deltas only)
            self.broadcast_recent_deltas().await;
            
            // Every 5 minutes: medium sync (summaries)
            if self.tick_count % 5 == 0 {
                let summary = self.generate_project_summary().await;
                self.broadcast_summary(summary).await;
            }
            
            // Every 30 minutes: deep sync (full context refresh)
            if self.tick_count % 30 == 0 {
                self.full_context_sync().await;
            }
            
            self.tick_count += 1;
        }
    }
    
    async fn full_context_sync(&mut self) {
        info!("Performing full context sync");
        
        // Generate fresh context snapshot
        let snapshot = self.context_manager.snapshot().await;
        
        // Send to all workers
        for worker in self.workers.iter() {
            worker.send(WorkerMsg::ContextRefresh(snapshot.clone())).await;
        }
    }
}
```

### Question-Answer Protocol

Models can ask specific questions instead of needing full context:

```rust
pub enum WorkerQuery {
    /// "What's the status of task X?"
    TaskStatus(TaskId),
    
    /// "What did we decide about Y?"
    DecisionLookup(String),
    
    /// "What's in file Z?"
    FileContents(PathBuf),
    
    /// "Who's working on feature W?"
    WorkerStatus { feature: String },
    
    /// "What are the current constraints?"
    CurrentConstraints,
}

impl Worker {
    /// Ask a specific question instead of loading all context
    async fn query(&self, query: WorkerQuery) -> Result<QueryResponse> {
        // Send query to orchestrator
        self.send_to_orchestrator(WorkerMsg::Query(query.clone())).await?;
        
        // Wait for specific answer (fast, no extra context)
        let response = self.recv_from_orchestrator().await?;
        
        match response {
            OrchestratorMsg::QueryResponse(resp) => Ok(resp),
            _ => Err(anyhow!("Unexpected response"))
        }
    }
}
```

---

## Resource Minimization Strategies

### Lazy Context Loading

Don't load context until actually needed:

```rust
pub struct LazyContext {
    /// What's loaded in memory
    loaded: Context,
    
    /// What's available but not loaded
    available: Vec<ContextSource>,
}

impl LazyContext {
    /// Only load when model actually requests it
    async fn get(&mut self, key: &str) -> Result<String> {
        // Check if already loaded
        if let Some(value) = self.loaded.get(key) {
            return Ok(value.clone());
        }
        
        // Find in available sources
        for source in &self.available {
            if source.has_key(key) {
                let value = source.load(key).await?;
                
                // Cache for future use
                self.loaded.insert(key, value.clone());
                
                return Ok(value);
            }
        }
        
        Err(anyhow!("Key not found: {}", key))
    }
}
```

### Automatic Downgrading

Use powerful models only when necessary:

```rust
impl Router {
    /// Try with cheaper model first, upgrade if needed
    async fn route_with_fallback(&self, task: Task) -> Result<TaskResult> {
        // Sort models by cost (cheapest first)
        let mut models = self.available_models();
        models.sort_by_key(|m| m.cost_per_token);
        
        for model in models {
            // Skip if model clearly can't handle task
            if task.min_capability > model.capability {
                continue;
            }
            
            info!("Trying {} for task", model.name);
            
            match self.execute_on_model(&model, &task).await {
                Ok(result) => {
                    // Validate quality
                    let quality = self.assess_quality(&result);
                    
                    if quality.is_acceptable() {
                        info!("Task succeeded on {}", model.name);
                        return Ok(result);
                    }
                    
                    warn!("Quality insufficient on {}, trying better model", model.name);
                    continue;
                }
                Err(e) => {
                    warn!("Failed on {}: {}", model.name, e);
                    continue;
                }
            }
        }
        
        Err(anyhow!("All models failed"))
    }
}
```

### Aggressive Caching

Cache everything that might be reused:

```rust
pub struct ContextCache {
    /// Summarized file contents
    file_summaries: LruCache<PathBuf, String>,
    
    /// Expensive computations
    analyses: LruCache<String, AnalysisResult>,
    
    /// Model responses for identical prompts
    responses: LruCache<u64, String>,  // hash -> response
}

impl ContextCache {
    /// Check cache before loading/computing
    async fn get_or_compute<F, Fut>(
        &mut self,
        key: String,
        computer: F
    ) -> Result<String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<String>>,
    {
        let hash = hash_key(&key);
        
        // Check cache
        if let Some(cached) = self.responses.get(&hash) {
            return Ok(cached.clone());
        }
        
        // Compute
        let result = computer().await?;
        
        // Cache for next time
        self.responses.put(hash, result.clone());
        
        Ok(result)
    }
}
```

---

## Configuration Examples

### Minimal Usage Mode (Default)

```toml
[orchestration]
mode = "dynamic"  # Fully adaptive
min_models = 1    # Start with just 1
max_models = 10   # But can scale up to 10

[resource_policy]
strategy = "minimal"  # Use cheapest that works
upgrade_on_failure = true
downgrade_on_success = true
cache_aggressively = true

[context]
lazy_loading = true
incremental_updates = true
compression = "aggressive"
```

### Constrained Mode (User Override)

```toml
[orchestration]
mode = "linear"   # Force single model at a time
max_models = 1    # Hard limit

[resource_policy]
strategy = "conservative"
max_requests_per_min = { nim = 30, tamu = 5 }

[context]
# Even more aggressive minimization
max_context_tokens = 50000
prune_threshold = 0.9  # Keep only top 10% important
```

### Full Throttle Mode (When Resources Available)

```toml
[orchestration]
mode = "dynamic"
min_models = 3     # Always keep 3 active
max_models = 20    # Can go wild

[resource_policy]
strategy = "performance"  # Prefer speed over cost
parallel_by_default = true

[context]
lazy_loading = false  # Preload everything
max_context_tokens = 200000
```

---

## Implementation Priority

1. **Phase 1: Basic Context Management**
   - `ContextHierarchy` with 5 levels
   - `ProjectBrief` for fast onboarding
   - `ContextDelta` for incremental updates

2. **Phase 2: Communication**
   - `SharedMemory` board
   - Question-answer protocol
   - Selective update routing

3. **Phase 3: Stuck Detection**
   - `StuckDetector` with pattern recognition
   - Context reset capability
   - Cross-model help requests

4. **Phase 4: Optimization**
   - Lazy loading
   - Aggressive caching
   - Automatic downgrading

This ensures models work together efficiently without overwhelming each other with information.
