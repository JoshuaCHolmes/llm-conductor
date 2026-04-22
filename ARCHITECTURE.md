# LLM Conductor - Architecture Document

## Core Philosophy

**"Intelligent orchestration that scales from 1 to N models while respecting resource constraints"**

The system must work equally well with:
- Single local model (graceful degradation)
- Full suite of providers (optimal performance)
- Anything in between (adaptive behavior)

## Resource Management System

### Token/Request Tracking

Every provider has multiple limit types that must be tracked:

```rust
pub struct ResourceLimits {
    // Hard limits from provider
    tokens_per_minute: Option<u32>,
    tokens_per_day: Option<u32>,
    tokens_per_month: Option<u32>,
    requests_per_minute: Option<u32>,
    requests_per_day: Option<u32>,
    requests_per_month: Option<u32>,
    
    // Soft limits (user-configured)
    max_cost_per_request: Option<f64>,
    max_concurrent_requests: Option<u32>,
}

pub struct ResourceUsage {
    // Current period usage
    tokens_used_minute: u32,
    tokens_used_day: u32,
    tokens_used_month: u32,
    requests_minute: u32,
    requests_day: u32,
    requests_month: u32,
    
    // Window tracking
    minute_window_start: Instant,
    day_start: DateTime<Utc>,
    month_start: DateTime<Utc>,
    
    // Historical
    total_tokens: u64,
    total_requests: u64,
    total_cost: f64,
}
```

### Example Provider Limits

| Provider | Tokens/min | Tokens/day | Tokens/month | Requests/min | Requests/day | Requests/month |
|----------|------------|------------|--------------|--------------|--------------|----------------|
| **Ollama** | ∞ | ∞ | ∞ | ∞ | ∞ | ∞ |
| **NVIDIA NIM** | ? | ? | ? | 40 | ∞ | ∞ |
| **TAMU AI** | ? | ~100k? | ? | ? | ~50? | ? |
| **GitHub Copilot** | ? | ? | ? | ? | ? | 50 |

### Resource-Aware Routing Algorithm

```rust
async fn select_provider(
    task: &Task,
    available_providers: &[Provider],
    resource_tracker: &ResourceTracker
) -> Result<Provider> {
    // 1. Filter by capability (can this provider handle this task?)
    let capable = available_providers
        .iter()
        .filter(|p| p.can_handle(&task))
        .collect::<Vec<_>>();
    
    // 2. Filter by availability (is it up? not rate-limited?)
    let available = capable
        .iter()
        .filter(|p| resource_tracker.is_available(p))
        .collect::<Vec<_>>();
    
    // 3. Check resource constraints for each
    let within_limits = available
        .iter()
        .filter(|p| {
            let estimated_tokens = task.estimated_tokens();
            resource_tracker.can_consume(p, estimated_tokens)
        })
        .collect::<Vec<_>>();
    
    // 4. Score by: quality, cost, current load
    let scored = within_limits
        .iter()
        .map(|p| {
            let quality_score = p.quality_for_task(&task);
            let cost_score = 1.0 - resource_tracker.scarcity_factor(p);
            let load_score = 1.0 - resource_tracker.current_load(p);
            
            (p, quality_score * 0.5 + cost_score * 0.3 + load_score * 0.2)
        })
        .collect::<Vec<_>>();
    
    // 5. Select best, or fallback
    scored
        .into_iter()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .map(|(p, _)| p.clone())
        .ok_or_else(|| anyhow!("No suitable provider available"))
}
```

## Multi-Agent Orchestration Architecture

### Actor-Based Design (Tokio + mpsc channels)

```
                    ┌─────────────────────┐
                    │  Orchestrator       │
                    │  (Main Conductor)   │
                    └──────────┬──────────┘
                               │
                ┌──────────────┼──────────────┐
                │              │              │
        ┌───────▼──────┐ ┌────▼─────┐ ┌─────▼──────┐
        │  Team Lead   │ │ Team Lead│ │ Team Lead  │
        │  (GLM-5)     │ │ (Local)  │ │ (Opus)     │
        └──────┬───────┘ └────┬─────┘ └─────┬──────┘
               │              │              │
        ┌──────┼──────┐       │       ┌─────┼──────┐
        │      │      │       │       │     │      │
    ┌───▼─┐ ┌─▼──┐ ┌─▼──┐  ┌─▼──┐  ┌─▼─┐ ┌─▼──┐ ┌─▼──┐
    │Work1│ │Wrk2│ │Wrk3│  │Wrk4│  │W5 │ │Wrk6│ │Wrk7│
    └─────┘ └────┘ └────┘  └────┘  └───┘ └────┘ └────┘
```

### Actor Types

#### 1. **Orchestrator** (Singleton)
- Receives user tasks
- Owns ResourceTracker
- Decides: single model or parallel delegation
- Spawns/manages Team Leads
- Aggregates final results

**Messages:**
```rust
enum OrchestratorMsg {
    UserTask(Task, oneshot::Sender<Response>),
    TeamComplete(TeamId, Result<TeamOutput>),
    ResourceUpdate(ProviderId, ResourceUsage),
    Shutdown,
}
```

#### 2. **Team Lead** (Per Feature/Subsystem)
- Manages a group of workers
- Delegates subtasks within team
- Reports progress to Orchestrator
- Handles failures/retries within team

**Messages:**
```rust
enum TeamLeadMsg {
    Subtask(Subtask, oneshot::Sender<SubtaskResult>),
    WorkerComplete(WorkerId, Result<Output>),
    ResourceConstraint(ProviderId),
    StatusRequest(oneshot::Sender<TeamStatus>),
}
```

#### 3. **Worker** (Per Model Instance)
- Executes single task using assigned model
- Streams tokens back to Team Lead
- Reports resource usage
- Self-contained (can't delegate further)

**Messages:**
```rust
enum WorkerMsg {
    Execute(Task, oneshot::Sender<WorkerResult>),
    Cancel,
    Pause,
    Resume,
}
```

### Scaling Behaviors

#### Single Model Available
```rust
Orchestrator
    └─> Worker (Ollama)
    
// No hierarchy, direct execution
```

#### Two Models (Local + Cloud)
```rust
Orchestrator
    ├─> Worker (Ollama) - simple tasks
    └─> Worker (GLM-5)  - complex tasks
    
// Flat delegation, no Team Leads needed
```

#### Full Multi-Model Setup
```rust
Orchestrator
    ├─> Team Lead (Feature A)
    │   ├─> Worker (GLM-5)
    │   └─> Worker (GLM-5)
    ├─> Team Lead (Feature B)
    │   ├─> Worker (Ollama)
    │   ├─> Worker (Ollama)
    │   └─> Worker (GLM-5)
    └─> Team Lead (Critical Review)
        └─> Worker (Opus)
        
// Full hierarchy for complex parallel work
```

## Resource-Aware Parallel Execution

### Scenario: Large Project with Mixed Limits

**User:** "Refactor this codebase to use async/await"

**Resources:**
- Ollama: unlimited
- NVIDIA NIM: 40 req/min, used 20 this minute
- TAMU Opus: 5 requests left today

**Orchestrator Decision:**
```rust
1. Decompose into subtasks:
   - Analyze codebase (cheap)
   - Plan refactoring (medium)
   - Implement changes (expensive)
   - Review & test (critical)

2. Resource-aware assignment:
   Analyze    → Ollama (3 parallel workers, unlimited)
   Plan       → GLM-5 (1 worker, stay under 40/min)
   Implement  → GLM-5 (2 workers, monitor rate limit)
   Review     → Opus (1 worker, save limited daily quota)

3. Execution:
   - Start Analyze immediately (no limits)
   - Queue Plan for GLM-5 (respect 40/min)
   - Wait for Plan before Implement
   - Reserve Opus for final Review
```

### Resource Exhaustion Handling

```rust
impl ResourceTracker {
    fn on_limit_reached(&mut self, provider: ProviderId) -> FallbackStrategy {
        match self.limits.get(&provider) {
            Some(limit) if limit.requests_per_minute.is_some() => {
                // Minute limit - wait and retry
                FallbackStrategy::WaitAndRetry {
                    duration: Duration::from_secs(60 - elapsed.as_secs())
                }
            }
            Some(limit) if limit.requests_per_day.is_some() => {
                // Daily limit - fallback to next provider
                FallbackStrategy::UseAlternative {
                    alternatives: self.get_alternatives(provider)
                }
            }
            Some(limit) if limit.requests_per_month.is_some() => {
                // Monthly limit - serious, alert user
                FallbackStrategy::AlertUser {
                    message: "Monthly limit reached, falling back to free tier",
                    alternatives: self.get_free_alternatives()
                }
            }
            _ => FallbackStrategy::Fail
        }
    }
}
```

## Communication Patterns

### Inter-Actor Message Passing

```rust
// Worker -> Team Lead (progress update)
WorkerProgress {
    worker_id: WorkerId,
    tokens_generated: u32,
    tokens_remaining: u32,
    partial_output: Option<String>,
}

// Team Lead -> Orchestrator (resource request)
ResourceRequest {
    team_id: TeamId,
    provider_needed: ProviderId,
    estimated_tokens: u32,
    priority: Priority,
}

// Orchestrator -> Team Lead (resource grant/deny)
ResourceResponse {
    granted: bool,
    alternative: Option<ProviderId>,
    retry_after: Option<Duration>,
}
```

### Broadcast Updates

```rust
// Orchestrator broadcasts to all actors
enum BroadcastMsg {
    ResourceLimitApproaching { provider: ProviderId, percent: f32 },
    ProviderUnavailable { provider: ProviderId, until: Option<Instant> },
    PriorityOverride { task_id: TaskId, new_priority: Priority },
}
```

## Project State Management

### Shared State (DashMap for concurrent access)

```rust
pub struct ProjectState {
    // Thread-safe concurrent maps
    tasks: DashMap<TaskId, TaskState>,
    resources: DashMap<ProviderId, ResourceUsage>,
    actors: DashMap<ActorId, ActorHandle>,
    
    // Persistent storage
    storage: Arc<dyn StateStore>,
}

pub trait StateStore: Send + Sync {
    async fn save_task(&self, task: &TaskState) -> Result<()>;
    async fn load_tasks(&self) -> Result<Vec<TaskState>>;
    async fn save_conversation(&self, conv: &Conversation) -> Result<()>;
}
```

## Safety & Robustness

### Actor Supervision

```rust
// Each actor has a supervisor
impl Supervisor {
    async fn supervise(&self, actor: ActorHandle) {
        loop {
            select! {
                result = actor.wait() => {
                    match result {
                        Ok(_) => break, // Clean shutdown
                        Err(e) => {
                            error!("Actor crashed: {}", e);
                            // Restart with exponential backoff
                            self.restart_with_backoff(actor).await;
                        }
                    }
                }
                _ = self.shutdown_rx.recv() => {
                    actor.stop().await;
                    break;
                }
            }
        }
    }
}
```

### Resource Safety

```rust
// RAII guard for resource reservation
pub struct ResourceGuard<'a> {
    tracker: &'a ResourceTracker,
    provider: ProviderId,
    tokens_reserved: u32,
}

impl Drop for ResourceGuard<'_> {
    fn drop(&mut self) {
        // Always return reserved tokens, even on panic
        self.tracker.release(self.provider, self.tokens_reserved);
    }
}
```

## Configuration

```toml
[orchestration]
# Max parallel workers across all teams
max_total_workers = 10

# Max workers per team
max_team_workers = 3

# Enable hierarchical mode (auto if >2 providers)
hierarchical_mode = "auto"

[resource_management]
# Buffer before hitting hard limits
safety_margin = 0.9  # Use only 90% of limits

# How to handle limit exhaustion
on_limit_reached = "fallback"  # "fallback", "queue", "fail"

# Alert thresholds
alert_at_percent = 0.8  # Alert at 80% usage

[providers.tamu]
enabled = true
limits.requests_per_day = 50  # User-configured estimate
priority_reserve = 5  # Keep 5 requests for critical tasks

[providers.nvidia]
enabled = true
limits.requests_per_minute = 40
retry_after_limit = "60s"

[providers.ollama]
enabled = true
max_concurrent = 3  # Don't overload local machine
```

## Module Structure

```
src/
├── main.rs                 # CLI entry point
├── orchestrator/
│   ├── mod.rs             # Main orchestrator actor
│   ├── team_lead.rs       # Team lead actor
│   ├── worker.rs          # Worker actor
│   └── supervisor.rs      # Actor supervision
├── resources/
│   ├── mod.rs             # Resource management
│   ├── tracker.rs         # ResourceTracker implementation
│   ├── limits.rs          # Limit definitions
│   └── guard.rs           # RAII resource guards
├── providers/
│   ├── mod.rs             # Provider trait
│   ├── ollama.rs
│   ├── nvidia.rs
│   ├── tamu.rs
│   └── github.rs
├── router/
│   ├── mod.rs             # Routing logic
│   ├── scoring.rs         # Provider scoring
│   └── fallback.rs        # Fallback strategies
├── state/
│   ├── mod.rs             # Project state management
│   ├── storage.rs         # Persistent storage
│   └── memory.rs          # In-memory state
├── conductor.rs           # Local delegation model
├── safety.rs              # Confirmation prompts
├── context.rs             # Context window management
└── ui.rs                  # REPL interface
```

This architecture ensures:
- ✅ Resource limits always respected
- ✅ Graceful degradation (N models → 1 model)
- ✅ Parallel execution when beneficial
- ✅ Single model streaming when optimal
- ✅ Robust failure handling
- ✅ Real-time resource tracking
- ✅ User visibility into decisions
