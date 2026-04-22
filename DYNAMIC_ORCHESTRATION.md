# Dynamic Orchestration System - Design Document

## Vision: Runtime-Adaptive Multi-Mode Execution

**Core Principle:** The orchestration topology should be a *live data structure* that responds to:
- Provider availability changes (model goes down/up)
- Workload characteristics shifts (suddenly CPU-bound vs IO-bound)
- Resource exhaustion (hit rate limits, need to reorganize)
- Performance feedback (some workers too slow, redistribute)

**Without requiring:**
- Hard stops/restarts
- Lost work
- Manual intervention

## Orchestration Modes

### 1. Linear Mode (Sequential Chain)
```
Task → Step1 → Step2 → Step3 → Result
```

**When to use:**
- Tasks have strict dependencies
- Each step needs output of previous
- Resource-constrained (only 1 model available)
- Deterministic ordering required

**Example:** "Debug this error" → Reproduce → Analyze → Fix → Test

---

### 2. Concurrent Mode (Parallel Fan-out)
```
Task → [Step1, Step2, Step3] → Aggregate → Result
```

**When to use:**
- Independent subtasks
- Multiple providers available
- Time-critical (need results fast)
- Embarrassingly parallel work

**Example:** "Analyze codebase" → [module_a, module_b, module_c] → Summary

---

### 3. Pipeline Mode (Streaming Processing)
```
Input Stream → Stage1 → Stage2 → Stage3 → Output Stream
```

**When to use:**
- Large volume of similar items
- Each stage can process while next stage consumes
- Memory-efficient needed
- Continuous flow

**Example:** "Process 1000 files" → Read → Transform → Validate → Write

---

### 4. Hierarchical Mode (Recursive Teams)
```
Project
  ├─ Team A (Feature X)
  │    ├─ Worker 1
  │    └─ Worker 2
  └─ Team B (Feature Y)
       ├─ Worker 3
       └─ Worker 4
```

**When to use:**
- Complex multi-domain projects
- Need "middle management" coordination
- Different subtasks need different strategies
- Large scale (4+ models)

**Example:** "Build web app" → [Frontend Team, Backend Team, DB Team]

---

### 5. Competitive Mode (Race with Validation)
```
Task → [Provider1, Provider2, Provider3] → Validator → Best Result
```

**When to use:**
- Critical task, need highest quality
- Multiple capable providers
- Can afford redundant computation
- Quality > cost

**Example:** "Review security-critical code" → [Opus, GPT-4o, GLM-5] → Pick best

---

### 6. Consensus Mode (Multi-Agent Agreement)
```
Task → [Agent1, Agent2, Agent3] → Vote/Merge → Consensus Result
```

**When to use:**
- Ambiguous decisions
- Need confidence/agreement
- Multiple perspectives valuable
- Reduce hallucinations

**Example:** "Is this architecture sound?" → 3 models review → Majority vote

---

## Dynamic Mode Switching

### Trigger Conditions

```rust
enum ModeSwitchTrigger {
    // Provider availability changed
    ProviderJoined { provider: ProviderId },
    ProviderLeft { provider: ProviderId, reason: Reason },
    
    // Resource constraints hit
    RateLimitApproaching { provider: ProviderId, percent: f32 },
    RateLimitExhausted { provider: ProviderId },
    
    // Performance feedback
    WorkerStalled { worker: WorkerId, duration: Duration },
    WorkerSlow { worker: WorkerId, relative_speed: f32 },
    
    // Workload characteristics changed
    TaskPatternShift { from: Pattern, to: Pattern },
    LoadIncrease { factor: f32 },
    LoadDecrease { factor: f32 },
    
    // Quality feedback
    OutputQualityLow { worker: WorkerId, score: f32 },
    ValidationFailed { worker: WorkerId },
    
    // User intervention
    UserOverride { new_mode: Mode },
}
```

### Mode Transition Examples

#### Example 1: Concurrent → Linear (Resource Exhaustion)
```rust
// Initial: 3 models, parallel execution
Concurrent {
    workers: [Ollama, GLM-5, GLM-5]
}

// GLM-5 hits rate limit (38/40 requests this minute)
Event: RateLimitApproaching { provider: NIM, percent: 0.95 }

// System responds: Stop spawning new GLM-5 workers, queue tasks
Action: TransitionTo {
    mode: Linear,
    reason: "NIM rate limit, switching to sequential execution"
}

// New topology:
Linear {
    current: Ollama,
    queue: [task1, task2, task3],
    fallback: NIM (after 60s)
}
```

#### Example 2: Linear → Hierarchical (Model Comes Online)
```rust
// Initial: Only Ollama available
Linear {
    worker: Ollama,
    queue: [many tasks]
}

// TAMU Opus becomes available
Event: ProviderJoined { provider: TAMU }

// System responds: Reorganize into teams
Action: TransitionTo {
    mode: Hierarchical,
    reason: "High-quality model available, enable task delegation"
}

// New topology:
Hierarchical {
    orchestrator: This,
    teams: [
        Team { lead: Ollama, workers: [Ollama, Ollama], domain: Simple },
        Team { lead: Opus, workers: [Opus], domain: Critical }
    ]
}
```

#### Example 3: Hierarchical → Concurrent (Workload Shift)
```rust
// Initial: Complex project, multiple teams
Hierarchical {
    teams: [TeamA, TeamB, TeamC]
}

// Workload analysis: All tasks are now similar & independent
Event: TaskPatternShift {
    from: Pattern::Heterogeneous,
    to: Pattern::Homogeneous
}

// System responds: Flatten hierarchy
Action: TransitionTo {
    mode: Concurrent,
    reason: "All tasks independent, removing coordination overhead"
}

// New topology:
Concurrent {
    workers: [all workers from all teams],
    coordinator: SimpleAggregator
}
```

## Hot-Swap Actor System

### Actor Lifecycle States

```rust
enum ActorState {
    Starting,           // Initializing
    Ready,              // Waiting for work
    Working { task_id: TaskId },  // Executing
    Draining,           // Finishing current task, won't accept new
    Migrating { to: ActorId },    // Transferring state
    Stopped,            // Clean shutdown
    Crashed { error: Error },     // Unexpected termination
}
```

### Graceful Actor Replacement

```rust
impl Orchestrator {
    /// Replace a worker without losing in-flight work
    async fn hot_swap_worker(
        &mut self,
        old_worker: WorkerId,
        new_provider: ProviderId
    ) -> Result<WorkerId> {
        // 1. Set old worker to Draining state
        self.send_to_worker(old_worker, WorkerMsg::Drain).await?;
        
        // 2. Spawn new worker
        let new_worker = self.spawn_worker(new_provider).await?;
        
        // 3. Redirect new tasks to new worker
        self.routing_table.replace(old_worker, new_worker);
        
        // 4. Wait for old worker to finish current task
        let final_state = self.wait_for_completion(old_worker).await?;
        
        // 5. Transfer any buffered state
        if let Some(state) = final_state.portable_state() {
            self.send_to_worker(new_worker, WorkerMsg::ImportState(state)).await?;
        }
        
        // 6. Gracefully stop old worker
        self.send_to_worker(old_worker, WorkerMsg::Shutdown).await?;
        
        Ok(new_worker)
    }
}
```

### Topology Reconfiguration

```rust
/// Live topology that can be modified at runtime
pub struct Topology {
    /// Current execution mode
    mode: Arc<RwLock<Mode>>,
    
    /// Active actors (thread-safe)
    actors: DashMap<ActorId, ActorHandle>,
    
    /// Current task graph (can be modified)
    graph: Arc<RwLock<TaskGraph>>,
    
    /// Message router (dynamically routes based on topology)
    router: DynamicRouter,
}

impl Topology {
    /// Restructure entire topology without stopping work
    async fn restructure(&mut self, new_mode: Mode) -> Result<()> {
        info!("Restructuring from {:?} to {:?}", self.mode, new_mode);
        
        // 1. Create transition plan
        let plan = TransitionPlan::compute(&self.current_state(), &new_mode)?;
        
        // 2. Spawn new actors for new topology
        for actor_spec in plan.actors_to_spawn {
            let actor = self.spawn_actor(actor_spec).await?;
            self.actors.insert(actor.id, actor);
        }
        
        // 3. Drain actors that won't exist in new topology
        for actor_id in plan.actors_to_remove {
            self.drain_actor(actor_id).await?;
        }
        
        // 4. Rewire message routing (atomic swap)
        self.router.apply_new_routes(plan.new_routes)?;
        
        // 5. Wait for drained actors to finish
        for actor_id in plan.actors_to_remove {
            self.wait_and_stop(actor_id).await?;
        }
        
        // 6. Update mode (atomic)
        *self.mode.write().await = new_mode;
        
        info!("Restructuring complete");
        Ok(())
    }
}
```

## Dynamic Task Graph (DAG)

### Mutable DAG Implementation

```rust
pub struct TaskGraph {
    /// Nodes (tasks) - can add/remove at runtime
    nodes: DashMap<TaskId, TaskNode>,
    
    /// Edges (dependencies) - can add/remove at runtime
    edges: DashMap<TaskId, Vec<TaskId>>,
    
    /// Execution state per node
    states: DashMap<TaskId, ExecutionState>,
}

impl TaskGraph {
    /// Add a new task while execution is ongoing
    pub fn insert_task(&self, task: Task, dependencies: Vec<TaskId>) -> TaskId {
        let id = TaskId::new();
        
        // Add node
        self.nodes.insert(id, TaskNode::new(task));
        
        // Add dependencies
        self.edges.insert(id, dependencies);
        
        // Mark as pending
        self.states.insert(id, ExecutionState::Pending);
        
        // Notify scheduler
        self.notify_scheduler(GraphEvent::TaskAdded { id });
        
        id
    }
    
    /// Dynamically add dependency during execution
    pub fn add_dependency(&self, task: TaskId, depends_on: TaskId) -> Result<()> {
        // Check for cycles
        if self.would_create_cycle(task, depends_on)? {
            return Err(anyhow!("Would create cycle in DAG"));
        }
        
        // Add edge
        self.edges.entry(task).or_default().push(depends_on);
        
        // Update execution state if needed
        self.recompute_ready_tasks();
        
        Ok(())
    }
    
    /// Remove a task (only if not started)
    pub fn remove_task(&self, task: TaskId) -> Result<()> {
        let state = self.states.get(&task)
            .ok_or_else(|| anyhow!("Task not found"))?;
        
        match *state {
            ExecutionState::Pending => {
                self.nodes.remove(&task);
                self.edges.remove(&task);
                self.states.remove(&task);
                Ok(())
            }
            _ => Err(anyhow!("Cannot remove task in state {:?}", state))
        }
    }
}
```

### Adaptive Subgraph Generation

```rust
impl Orchestrator {
    /// Generate new subgraph based on runtime observations
    async fn adapt_graph(&mut self, observations: &WorkloadObservations) {
        // Analyze current execution
        let analysis = self.analyze_performance(observations);
        
        match analysis {
            // Tasks are taking longer than expected - parallelize
            Analysis::Bottleneck { slow_task, .. } => {
                info!("Detected bottleneck at {:?}, parallelizing", slow_task);
                
                // Break task into subtasks
                let subtasks = self.decompose_task(slow_task).await;
                
                // Insert parallel subgraph
                self.graph.replace_task_with_subgraph(
                    slow_task,
                    ParallelSubgraph { tasks: subtasks }
                );
            }
            
            // Many similar tasks - create pipeline
            Analysis::RepetitivePattern { tasks, .. } => {
                info!("Detected repetitive pattern, creating pipeline");
                
                // Create streaming pipeline
                let pipeline = StreamingPipeline::from_tasks(tasks);
                
                // Replace with pipeline subgraph
                self.graph.replace_with_pipeline(tasks, pipeline);
            }
            
            // Tasks have unexpected dependencies - reorder
            Analysis::SuboptimalOrdering { .. } => {
                info!("Reordering tasks for better parallelism");
                
                // Topological re-sort considering actual dependencies
                self.graph.reorder_for_parallelism();
            }
            
            _ => {}
        }
    }
}
```

## Resource-Aware Mode Selection

### Continuous Optimization Loop

```rust
impl Orchestrator {
    /// Background task that monitors and adapts
    async fn adaptive_loop(&mut self) {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        
        loop {
            interval.tick().await;
            
            // 1. Gather current state
            let state = OrchestrationState {
                mode: self.topology.mode().await,
                active_workers: self.count_active_workers(),
                queue_depth: self.task_queue.len(),
                resource_usage: self.resources.snapshot(),
                performance: self.performance_tracker.snapshot(),
            };
            
            // 2. Evaluate if current mode is optimal
            let eval = self.evaluator.evaluate(&state);
            
            // 3. If suboptimal, compute better mode
            if let Evaluation::Suboptimal { suggested_mode, confidence } = eval {
                if confidence > 0.8 {
                    info!(
                        "Mode {} is suboptimal (confidence: {:.2}), switching to {}",
                        state.mode, confidence, suggested_mode
                    );
                    
                    // 4. Perform live transition
                    if let Err(e) = self.topology.restructure(suggested_mode).await {
                        error!("Failed to restructure: {}", e);
                    }
                }
            }
            
            // 5. Adapt task graph based on execution so far
            self.adapt_graph(&self.observations).await;
        }
    }
}
```

### Mode Scoring Algorithm

```rust
fn score_mode(mode: &Mode, state: &OrchestrationState) -> f32 {
    let mut score = 0.0;
    
    // Factor 1: Resource efficiency
    score += match mode {
        Mode::Linear => {
            // Efficient if low on resources
            if state.resource_usage.is_constrained() { 0.8 } else { 0.2 }
        }
        Mode::Concurrent => {
            // Efficient if resources abundant
            if state.resource_usage.is_abundant() { 0.9 } else { 0.3 }
        }
        Mode::Hierarchical => {
            // Efficient at large scale
            if state.active_workers > 4 { 0.85 } else { 0.4 }
        }
        _ => 0.5
    };
    
    // Factor 2: Workload match
    score += match (mode, &state.workload_pattern) {
        (Mode::Linear, Pattern::Sequential) => 0.9,
        (Mode::Concurrent, Pattern::Parallel) => 0.9,
        (Mode::Pipeline, Pattern::Streaming) => 0.9,
        _ => 0.5
    };
    
    // Factor 3: Current performance
    if state.performance.throughput > state.performance.target_throughput {
        score += 0.8; // Current mode is performing well
    } else {
        score += 0.2; // Current mode underperforming
    }
    
    // Factor 4: Transition cost
    let transition_cost = estimate_transition_cost(state.mode, mode);
    score -= transition_cost * 0.3; // Penalize expensive transitions
    
    score / 4.0 // Normalize
}
```

## Message Protocol for Dynamic Coordination

```rust
/// Messages that enable runtime reconfiguration
enum ControlMsg {
    // Topology changes
    SwitchMode { new_mode: Mode, reason: String },
    RestructureNow,
    
    // Actor lifecycle
    DrainWorker { worker_id: WorkerId },
    MigrateWorker { from: WorkerId, to: WorkerId },
    ReplaceWorker { old: WorkerId, new: WorkerId },
    
    // Graph modifications
    AddTask { task: Task, dependencies: Vec<TaskId> },
    RemoveTask { task_id: TaskId },
    AddDependency { task: TaskId, depends_on: TaskId },
    
    // Resource adjustments
    IncreaseParallelism { factor: f32 },
    DecreaseParallelism { factor: f32 },
    ReserveResources { provider: ProviderId, amount: u32 },
    
    // Performance tuning
    AdjustBatchSize { new_size: usize },
    ChangeQueueStrategy { strategy: QueueStrategy },
}
```

## Monitoring & Observability

### Real-time Metrics

```rust
pub struct OrchestrationMetrics {
    // Mode tracking
    current_mode: Mode,
    mode_changes: Counter,
    time_in_each_mode: HashMap<Mode, Duration>,
    
    // Performance
    tasks_completed: Counter,
    tasks_failed: Counter,
    avg_task_duration: Gauge,
    throughput: Gauge, // tasks/sec
    
    // Resource utilization
    workers_active: Gauge,
    workers_idle: Gauge,
    workers_draining: Gauge,
    queue_depth: Gauge,
    
    // Transitions
    hot_swaps_performed: Counter,
    restructures_performed: Counter,
    failed_transitions: Counter,
}
```

### Decision Logging

```rust
/// Every mode change logged for analysis
struct ModeTransition {
    timestamp: Instant,
    from_mode: Mode,
    to_mode: Mode,
    trigger: ModeSwitchTrigger,
    state_before: OrchestrationState,
    state_after: OrchestrationState,
    transition_duration: Duration,
    success: bool,
}
```

## Implementation Plan

### Phase 1: Mutable Topology
1. `Topology` struct with `Arc<RwLock<Mode>>`
2. Actor spawning/draining/hot-swap
3. Dynamic routing table
4. Basic mode switching (Linear ↔ Concurrent)

### Phase 2: Dynamic DAG
1. `TaskGraph` with concurrent insertions
2. Cycle detection
3. Runtime dependency addition
4. Subgraph replacement

### Phase 3: Adaptive Loop
1. Performance monitoring
2. Mode scoring algorithm
3. Automatic mode selection
4. Continuous optimization

### Phase 4: Advanced Modes
1. Pipeline mode implementation
2. Competitive mode
3. Consensus mode
4. Custom mode plugins

This design enables true "living system" orchestration that adapts to reality in real-time.
