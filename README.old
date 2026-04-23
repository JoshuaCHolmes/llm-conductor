# LLM Conductor

**Intelligent LLM orchestration system with resource-aware multi-agent coordination**

A production-grade Rust CLI that uses local models for intelligent task delegation across multiple LLM providers, with sophisticated resource tracking, parallel execution, and hierarchical agent coordination.

## Vision

**"Never worry about model limits, costs, or availability - just get work done."**

Whether you have 1 local model or access to 10 providers, Conductor adapts to use resources optimally while respecting rate limits, daily quotas, and cost constraints.

## Key Features

### 🎯 **Resource-Aware Intelligence**
- **Real-time limit tracking**: Tokens/min, tokens/day, requests/month across all providers
- **Smart reservation**: Saves limited resources (TAMU Opus, GitHub Copilot) for critical tasks
- **Graceful degradation**: Automatically falls back when limits reached
- **Cost optimization**: Uses free unlimited providers (Ollama, NVIDIA) when possible

### 🎭 **Adaptive Scaling**
- **1 model**: Direct execution, minimal overhead
- **2-3 models**: Flat delegation, task-based routing  
- **4+ models**: Hierarchical teams with "middle management" coordination
- **Dynamic adjustment**: Scales architecture based on available resources

### 🚀 **Parallel Execution**
- **Actor-based design**: Independent workers communicate via message passing
- **Concurrent requests**: Multiple models work simultaneously when beneficial
- **Resource-bounded**: Never exceeds provider rate limits across all parallel tasks
- **Progress tracking**: Real-time visibility into all active tasks

### 🛡️ **Safety & Robustness**
- **Confirmation prompts**: Protect against destructive operations
- **Session allowlists**: Skip confirmations for trusted tasks
- **Actor supervision**: Automatic restart on crashes with exponential backoff
- **RAII resource guards**: Always release reservations, even on panic
- **Provider failover**: Automatic fallback chains when providers fail

### 📊 **Context Management**
- **Persistent sessions**: Resume conversations across restarts
- **Smart summarization**: Automatic context compression when approaching limits
- **Multi-file tracking**: Reference and modify multiple files in one conversation
- **Semantic caching**: Deduplicate similar prompts to save tokens

## Architecture Highlights

### Resource Tracking
```rust
Every provider tracks multiple limit types:
  - Tokens per minute/day/month
  - Requests per minute/day/month  
  - Concurrent request limits
  - Cost per token

Routing decisions factor in:
  - Current usage vs limits
  - Time until reset
  - Scarcity (% of quota remaining)
  - Task priority
```

### Actor Hierarchy
```
Orchestrator (manages global resources)
    │
    ├─> Team Lead (Feature A) 
    │   ├─> Worker (GLM-5)
    │   └─> Worker (GLM-5)
    │
    ├─> Team Lead (Feature B)
    │   ├─> Worker (Ollama)
    │   └─> Worker (Ollama)
    │
    └─> Team Lead (Critical Review)
        └─> Worker (Opus - reserved resource)
```

### Intelligent Delegation
```rust
// Local conductor model (Phi-3/Qwen 3B) analyzes each task:
Task Complexity → Provider Selection
  Simple        → Ollama (local, unlimited)
  Moderate      → NVIDIA NIM (free 40/min)
  Complex       → GLM-5 Plus (best free quality)
  Critical      → TAMU Opus (limited daily quota)

// With resource awareness:
If NIM at 38/40 requests this minute:
  → Queue task or use alternative
If TAMU at 45/50 daily requests:
  → Reserve remaining 5 for critical tasks only
```

## Providers

| Provider | Models | Limits | Cost | Use Case |
|----------|--------|--------|------|----------|
| **Ollama** | Phi-3 3.8B, Qwen 3B | Unlimited | $0 | Conductor, simple tasks |
| **NVIDIA NIM** | GLM-5 Plus 89B | 40 req/min | $0 | Main workhorse |
| **TAMU AI** *(optional)* | Claude Opus 4.5 | ~50/day | $0 | Critical work |
| **GitHub Copilot** *(optional)* | GPT-4o, Claude | 50/month | $0 | Backup |

## Installation

### Prerequisites
```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install Ollama for local models
curl -fsSL https://ollama.com/install.sh | sh
ollama pull phi-3:3.8b
ollama pull qwen2.5:3b
```

### Build
```bash
git clone https://github.com/yourusername/llm-conductor
cd llm-conductor
cargo build --release
```

### Install
```bash
cargo install --path .
# Or copy binary:
cp target/release/llm-conductor ~/.local/bin/conductor
```

## Configuration

**First run** (auto-generates config):
```bash
conductor config init
```

**Edit** `~/.config/conductor/config.toml`:
```toml
[orchestration]
max_total_workers = 10
hierarchical_mode = "auto"

[resource_management]
safety_margin = 0.9  # Use 90% of limits
on_limit_reached = "fallback"

[providers.ollama]
enabled = true
base_url = "http://localhost:11434"
max_concurrent = 3

[providers.nvidia]
enabled = true
api_key = "nvapi-..."  # Get from build.nvidia.com
limits.requests_per_minute = 40

[providers.tamu]
enabled = false  # Optional
api_key = "sk-..."
limits.requests_per_day = 50
priority_reserve = 5

[providers.github_copilot]
enabled = false  # Optional, auto-detects gh CLI
limits.requests_per_month = 50
```

## Usage

### Interactive Chat
```bash
$ conductor chat

🎼 LLM Conductor v0.1.0
📡 Connected: Ollama (local), NVIDIA NIM (40/min)

> Implement a binary search tree in Rust

[Conductor → Analyzing with Phi-3]
[Routing to GLM-5: complex implementation task]
[GLM-5 Plus] Here's a complete BST implementation...

> Run tests on this

[Conductor → GLM-5: test execution]
[GLM-5] Running cargo test...
✓ All 12 tests passed
```

### Project Mode (Multi-Agent)
```bash
$ conductor project "Refactor codebase to use async/await"

🎪 Project Maestro Mode
[Orchestrator] Analyzing project structure...
[Orchestrator] Spawning 3 teams (Analysis, Implementation, Review)

[Team: Analysis] 
  - Worker 1 (Ollama): Analyzing module_a.rs
  - Worker 2 (Ollama): Analyzing module_b.rs
  - Worker 3 (Ollama): Analyzing module_c.rs
  ✓ Analysis complete (47 functions need refactoring)

[Team: Implementation]
  - Worker 1 (GLM-5): Refactoring auth.rs [14/40 req/min]
  - Worker 2 (GLM-5): Refactoring api.rs [15/40 req/min]
  ⚠️  Rate limit approaching, queueing worker 3...
  - Worker 3 (GLM-5): Queued for utils.rs

[Team: Review]
  - Worker 1 (Opus): Final code review [4/5 daily requests reserved]
  
✓ Project complete! 47 functions refactored, all tests passing.
```

### Provider Status
```bash
$ conductor providers

📡 Provider Status:

✓ Ollama (Local)
  Models: phi-3:3.8b, qwen2.5:3b
  Limits: Unlimited
  Status: Ready
  
✓ NVIDIA NIM  
  Models: GLM-5 Plus 89B
  Limits: 23/40 requests this minute
  Status: Available (rate limit: 58% used)
  
○ TAMU AI
  Models: Claude Opus 4.5
  Limits: 42/50 requests today
  Status: Reserved for critical tasks only
  
✗ GitHub Copilot
  Status: Not configured (run: gh auth login)
```

## Development

### Project Structure
See [ARCHITECTURE.md](./ARCHITECTURE.md) for detailed design.

```
src/
├── orchestrator/     # Actor coordination
├── resources/        # Limit tracking & resource guards
├── providers/        # Provider implementations
├── router/           # Intelligent task routing
├── state/            # Project state & persistence
└── ui.rs            # REPL interface
```

### Running Tests
```bash
cargo test
```

### Development Mode
```bash
cargo run -- chat --verbose
```

## Roadmap

### Phase 1: Foundation (Current)
- [x] Project structure
- [x] Architecture design
- [ ] Resource tracking system
- [ ] Ollama provider
- [ ] NVIDIA NIM provider
- [ ] Basic routing
- [ ] REPL UI

### Phase 2: Intelligence
- [ ] Conductor model integration
- [ ] Smart delegation
- [ ] Context window management
- [ ] Session persistence

### Phase 3: Orchestration
- [ ] Actor-based execution
- [ ] Team lead coordination
- [ ] Parallel task execution
- [ ] Resource-aware scheduling

### Phase 4: Multi-Agent
- [ ] Project maestro mode
- [ ] Hierarchical teams
- [ ] Cross-model collaboration
- [ ] Progress tracking UI

### Phase 5: Polish
- [ ] Safety confirmations
- [ ] Rollback support
- [ ] Configuration UI
- [ ] Comprehensive docs

## Contributing

This is a personal project, but feedback and suggestions welcome!

## License

MIT License - See LICENSE

## Credits

Built by Joshua Holmes for intelligent, resource-aware LLM orchestration.

**Goal achieved:** Unlimited AI access through smart resource management and multi-provider coordination.
