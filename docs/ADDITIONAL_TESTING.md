# Additional Testing Infrastructure

## Network & Web Service Testing

### Mock HTTP Server Framework

For testing APIs, webhooks, and web services without real backends:

```rust
pub struct MockHttpServer {
    server: Server,
    port: u16,
    expectations: Vec<HttpExpectation>,
    requests_received: Vec<HttpRequest>,
}

pub struct HttpExpectation {
    method: Method,
    path: String,
    /// Optional request body matcher
    body_matcher: Option<BodyMatcher>,
    /// Response to return
    response: MockResponse,
    /// How many times this should be called
    times: ExpectedTimes,
}

pub enum ExpectedTimes {
    Once,
    Exactly(usize),
    AtLeast(usize),
    AtMost(usize),
    Between(usize, usize),
    Any,
}

impl MockHttpServer {
    /// Start mock server on random port
    pub async fn start() -> Result<Self> {
        let port = Self::find_free_port()?;
        
        let server = Server::bind(&format!("127.0.0.1:{}", port).parse()?)
            .serve(make_service_fn(|_conn| async {
                Ok::<_, Infallible>(service_fn(Self::handle_request))
            }));
        
        info!("Mock HTTP server started on port {}", port);
        
        Ok(MockHttpServer {
            server,
            port,
            expectations: Vec::new(),
            requests_received: Vec::new(),
        })
    }
    
    /// Set up expected request/response
    pub fn expect(&mut self, expectation: HttpExpectation) {
        self.expectations.push(expectation);
    }
    
    /// Verify all expectations were met
    pub fn verify(&self) -> Result<()> {
        for expectation in &self.expectations {
            let matching_requests = self.requests_received.iter()
                .filter(|r| self.matches_expectation(r, expectation))
                .count();
            
            match expectation.times {
                ExpectedTimes::Once => {
                    if matching_requests != 1 {
                        return Err(anyhow!(
                            "Expected {} {} to be called once, but was called {} times",
                            expectation.method,
                            expectation.path,
                            matching_requests
                        ));
                    }
                }
                ExpectedTimes::Exactly(n) => {
                    if matching_requests != n {
                        return Err(anyhow!(
                            "Expected {} {} to be called exactly {} times, but was called {} times",
                            expectation.method,
                            expectation.path,
                            n,
                            matching_requests
                        ));
                    }
                }
                // ... other cases
                _ => {}
            }
        }
        
        Ok(())
    }
}

// Example usage:
pub async fn test_api_client() -> Result<()> {
    let mut mock = MockHttpServer::start().await?;
    
    // Set up expectations
    mock.expect(HttpExpectation {
        method: Method::GET,
        path: "/users/123".to_string(),
        body_matcher: None,
        response: MockResponse::json(json!({
            "id": 123,
            "name": "Test User"
        })),
        times: ExpectedTimes::Once,
    });
    
    // Run test
    let client = ApiClient::new(&format!("http://localhost:{}", mock.port));
    let user = client.get_user(123).await?;
    
    assert_eq!(user.name, "Test User");
    
    // Verify expectations
    mock.verify()?;
    
    Ok(())
}
```

---

### Network Simulation (Latency, Failures, Bandwidth)

Simulate poor network conditions:

```rust
pub struct NetworkSimulator {
    /// Artificial latency to add
    latency: Duration,
    
    /// Packet loss rate (0.0-1.0)
    packet_loss: f32,
    
    /// Bandwidth limit (bytes/sec)
    bandwidth_limit: Option<usize>,
    
    /// Simulate intermittent failures
    failure_rate: f32,
}

impl NetworkSimulator {
    /// Wrap HTTP client with network simulation
    pub fn wrap_client(&self, client: HttpClient) -> SimulatedHttpClient {
        SimulatedHttpClient {
            inner: client,
            simulator: self.clone(),
        }
    }
    
    /// Simulate request with network conditions
    async fn simulate_request<F, Fut>(&self, request_fn: F) -> Result<Response>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Response>>,
    {
        // Add latency
        tokio::time::sleep(self.latency).await;
        
        // Simulate packet loss
        if rand::random::<f32>() < self.packet_loss {
            return Err(anyhow!("Simulated packet loss"));
        }
        
        // Simulate failures
        if rand::random::<f32>() < self.failure_rate {
            return Err(anyhow!("Simulated network failure"));
        }
        
        // Make actual request
        let start = Instant::now();
        let response = request_fn().await?;
        
        // Simulate bandwidth limit
        if let Some(limit) = self.bandwidth_limit {
            let content_length = response.content_length().unwrap_or(0);
            let min_duration = Duration::from_secs_f64(content_length as f64 / limit as f64);
            let elapsed = start.elapsed();
            
            if elapsed < min_duration {
                tokio::time::sleep(min_duration - elapsed).await;
            }
        }
        
        Ok(response)
    }
}

// Presets for common scenarios
impl NetworkSimulator {
    pub fn mobile_3g() -> Self {
        Self {
            latency: Duration::from_millis(200),
            packet_loss: 0.01,
            bandwidth_limit: Some(1_000_000), // 1 MB/s
            failure_rate: 0.02,
        }
    }
    
    pub fn mobile_4g() -> Self {
        Self {
            latency: Duration::from_millis(50),
            packet_loss: 0.005,
            bandwidth_limit: Some(10_000_000), // 10 MB/s
            failure_rate: 0.01,
        }
    }
    
    pub fn satellite() -> Self {
        Self {
            latency: Duration::from_millis(600),
            packet_loss: 0.02,
            bandwidth_limit: Some(2_000_000), // 2 MB/s
            failure_rate: 0.05,
        }
    }
    
    pub fn terrible_wifi() -> Self {
        Self {
            latency: Duration::from_millis(300),
            packet_loss: 0.1,
            bandwidth_limit: Some(500_000), // 500 KB/s
            failure_rate: 0.15,
        }
    }
}
```

---

## Database Testing

### In-Memory Database for Fast Tests

```rust
pub enum TestDatabase {
    /// SQLite in-memory (fast, isolated)
    SQLiteMemory {
        connection: SqliteConnection,
    },
    
    /// PostgreSQL in Docker container
    PostgresContainer {
        container: Container,
        port: u16,
    },
    
    /// Mock database (no real DB, just stores/returns data)
    Mock {
        storage: HashMap<String, Value>,
    },
}

impl TestDatabase {
    /// Create test database with schema
    pub async fn create_with_schema(schema: &str) -> Result<Self> {
        // Use SQLite in-memory for speed
        let conn = SqliteConnection::connect(":memory:").await?;
        
        // Apply schema
        sqlx::query(schema).execute(&conn).await?;
        
        Ok(TestDatabase::SQLiteMemory { connection: conn })
    }
    
    /// Seed with test data
    pub async fn seed(&mut self, data: TestData) -> Result<()> {
        match self {
            TestDatabase::SQLiteMemory { connection } => {
                for (table, rows) in data.tables {
                    for row in rows {
                        let query = Self::build_insert_query(&table, &row);
                        sqlx::query(&query).execute(connection).await?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    
    /// Snapshot state for comparison
    pub async fn snapshot(&self) -> Result<DatabaseSnapshot> {
        // Capture all table contents
        todo!("Implement snapshot")
    }
    
    /// Compare with expected state
    pub async fn assert_state(&self, expected: DatabaseSnapshot) -> Result<()> {
        let actual = self.snapshot().await?;
        
        if actual != expected {
            return Err(anyhow!(
                "Database state mismatch:\nExpected: {:?}\nActual: {:?}",
                expected,
                actual
            ));
        }
        
        Ok(())
    }
}
```

---

## Concurrent/Parallel Testing

### Race Condition Detection

```rust
pub struct RaceDetector {
    /// Run operations in different orders to find races
    permutations: usize,
}

impl RaceDetector {
    /// Test for race conditions by running concurrent operations
    pub async fn test_concurrent_operations<F, Fut>(
        &self,
        operations: Vec<F>,
    ) -> Result<RaceTestReport>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send,
    {
        let mut report = RaceTestReport::default();
        
        // Try different execution orders
        for _ in 0..self.permutations {
            // Shuffle operation order
            let mut order: Vec<usize> = (0..operations.len()).collect();
            order.shuffle(&mut rand::thread_rng());
            
            // Run operations concurrently
            let mut tasks = Vec::new();
            for &idx in &order {
                let op = &operations[idx];
                tasks.push(tokio::spawn(op()));
            }
            
            // Wait for all to complete
            let results = futures::future::join_all(tasks).await;
            
            // Check for failures or inconsistencies
            let failures: Vec<_> = results.iter()
                .enumerate()
                .filter_map(|(i, r)| r.as_ref().err().map(|e| (i, e)))
                .collect();
            
            if !failures.is_empty() {
                report.races_detected += 1;
                report.failures.push(RaceFailure {
                    order,
                    errors: failures,
                });
            }
        }
        
        Ok(report)
    }
}
```

---

## Load/Stress Testing

### Load Generator

```rust
pub struct LoadGenerator {
    /// Target requests per second
    target_rps: f64,
    
    /// Duration of test
    duration: Duration,
    
    /// Number of concurrent clients
    concurrency: usize,
}

impl LoadGenerator {
    /// Run load test against service
    pub async fn run_load_test<F, Fut>(
        &self,
        request_fn: F,
    ) -> Result<LoadTestReport>
    where
        F: Fn() -> Fut + Send + Sync + Clone + 'static,
        Fut: Future<Output = Result<()>> + Send,
    {
        let mut report = LoadTestReport::default();
        let start = Instant::now();
        
        // Calculate delay between requests
        let delay_between_requests = Duration::from_secs_f64(1.0 / self.target_rps);
        
        // Spawn concurrent workers
        let mut workers = Vec::new();
        for _ in 0..self.concurrency {
            let request_fn = request_fn.clone();
            let duration = self.duration;
            
            let worker = tokio::spawn(async move {
                let mut worker_report = WorkerReport::default();
                let worker_start = Instant::now();
                
                while worker_start.elapsed() < duration {
                    let req_start = Instant::now();
                    
                    match request_fn().await {
                        Ok(_) => {
                            worker_report.successful_requests += 1;
                            worker_report.latencies.push(req_start.elapsed());
                        }
                        Err(e) => {
                            worker_report.failed_requests += 1;
                            worker_report.errors.push(e.to_string());
                        }
                    }
                    
                    tokio::time::sleep(delay_between_requests).await;
                }
                
                worker_report
            });
            
            workers.push(worker);
        }
        
        // Collect results
        for worker in workers {
            let worker_report = worker.await?;
            report.merge(worker_report);
        }
        
        report.total_duration = start.elapsed();
        report.calculate_statistics();
        
        Ok(report)
    }
}

pub struct LoadTestReport {
    pub successful_requests: usize,
    pub failed_requests: usize,
    pub total_duration: Duration,
    
    // Latency statistics
    pub min_latency: Duration,
    pub max_latency: Duration,
    pub mean_latency: Duration,
    pub p50_latency: Duration,
    pub p95_latency: Duration,
    pub p99_latency: Duration,
    
    // Throughput
    pub actual_rps: f64,
    
    pub errors: Vec<String>,
}
```

---

## File System Testing

### Virtual/Mock File System

```rust
pub struct VirtualFileSystem {
    /// In-memory file contents
    files: HashMap<PathBuf, Vec<u8>>,
    
    /// Directory structure
    directories: HashSet<PathBuf>,
    
    /// Simulate file system errors
    error_simulator: Option<FsErrorSimulator>,
}

pub struct FsErrorSimulator {
    /// Paths that should fail to read
    unreadable_paths: HashSet<PathBuf>,
    
    /// Paths that should fail to write
    unwritable_paths: HashSet<PathBuf>,
    
    /// Simulate disk full
    disk_full: bool,
    disk_space_remaining: u64,
}

impl VirtualFileSystem {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            directories: HashSet::new(),
            error_simulator: None,
        }
    }
    
    /// Simulate reading file
    pub fn read(&self, path: &Path) -> Result<Vec<u8>> {
        // Check error simulator
        if let Some(ref sim) = self.error_simulator {
            if sim.unreadable_paths.contains(path) {
                return Err(anyhow!("Permission denied"));
            }
        }
        
        // Return file contents
        self.files.get(path)
            .cloned()
            .ok_or_else(|| anyhow!("File not found: {:?}", path))
    }
    
    /// Simulate writing file
    pub fn write(&mut self, path: &Path, contents: Vec<u8>) -> Result<()> {
        // Check error simulator
        if let Some(ref sim) = self.error_simulator {
            if sim.unwritable_paths.contains(path) {
                return Err(anyhow!("Permission denied"));
            }
            
            if sim.disk_full {
                return Err(anyhow!("No space left on device"));
            }
            
            if contents.len() as u64 > sim.disk_space_remaining {
                return Err(anyhow!("No space left on device"));
            }
        }
        
        self.files.insert(path.to_path_buf(), contents);
        Ok(())
    }
    
    /// Assert file exists with expected contents
    pub fn assert_file_contents(&self, path: &Path, expected: &[u8]) -> Result<()> {
        let actual = self.read(path)?;
        
        if actual != expected {
            return Err(anyhow!(
                "File contents mismatch for {:?}\nExpected: {:?}\nActual: {:?}",
                path,
                String::from_utf8_lossy(expected),
                String::from_utf8_lossy(&actual)
            ));
        }
        
        Ok(())
    }
}
```

---

## Time-Based Testing

### Virtual Clock for Time Travel

```rust
pub struct VirtualClock {
    /// Current virtual time
    current_time: Mutex<Instant>,
    
    /// Real start time
    real_start: Instant,
    
    /// Time multiplier (2.0 = 2x speed, 0.5 = half speed)
    multiplier: f64,
}

impl VirtualClock {
    pub fn new() -> Self {
        Self {
            current_time: Mutex::new(Instant::now()),
            real_start: Instant::now(),
            multiplier: 1.0,
        }
    }
    
    /// Get current virtual time
    pub fn now(&self) -> Instant {
        *self.current_time.lock().unwrap()
    }
    
    /// Advance time by duration
    pub fn advance(&self, duration: Duration) {
        let mut current = self.current_time.lock().unwrap();
        *current += duration;
    }
    
    /// Jump to specific point in time
    pub fn set_time(&self, time: Instant) {
        let mut current = self.current_time.lock().unwrap();
        *current = time;
    }
    
    /// Sleep in virtual time
    pub async fn sleep(&self, duration: Duration) {
        // In real time, this might be instant or scaled
        let real_duration = Duration::from_secs_f64(
            duration.as_secs_f64() / self.multiplier
        );
        
        tokio::time::sleep(real_duration).await;
        self.advance(duration);
    }
}

// Example: Test time-based features
pub async fn test_session_timeout() -> Result<()> {
    let clock = VirtualClock::new();
    
    let session = Session::new(&clock);
    
    // Session should be valid initially
    assert!(session.is_valid(&clock));
    
    // Advance time by 1 hour
    clock.advance(Duration::from_secs(3600));
    
    // Session should now be expired
    assert!(!session.is_valid(&clock));
    
    Ok(())
}
```

---

## Security Testing

### Input Fuzzing

```rust
pub struct InputFuzzer {
    /// Types of malicious inputs to try
    attack_vectors: Vec<AttackVector>,
}

pub enum AttackVector {
    /// SQL injection attempts
    SqlInjection,
    
    /// XSS attempts
    CrossSiteScripting,
    
    /// Path traversal
    PathTraversal,
    
    /// Buffer overflow
    BufferOverflow,
    
    /// Null bytes
    NullBytes,
    
    /// Unicode exploits
    UnicodeExploits,
    
    /// Format string attacks
    FormatString,
}

impl InputFuzzer {
    /// Generate malicious test inputs
    pub fn generate_inputs(&self) -> Vec<String> {
        let mut inputs = Vec::new();
        
        for vector in &self.attack_vectors {
            inputs.extend(self.generate_for_vector(vector));
        }
        
        inputs
    }
    
    fn generate_for_vector(&self, vector: &AttackVector) -> Vec<String> {
        match vector {
            AttackVector::SqlInjection => vec![
                "'; DROP TABLE users; --".to_string(),
                "1' OR '1'='1".to_string(),
                "admin'--".to_string(),
                "' UNION SELECT * FROM users--".to_string(),
            ],
            
            AttackVector::CrossSiteScripting => vec![
                "<script>alert('XSS')</script>".to_string(),
                "<img src=x onerror=alert('XSS')>".to_string(),
                "javascript:alert('XSS')".to_string(),
            ],
            
            AttackVector::PathTraversal => vec![
                "../../etc/passwd".to_string(),
                "..\\..\\windows\\system32\\config\\sam".to_string(),
                "....//....//etc/passwd".to_string(),
            ],
            
            AttackVector::BufferOverflow => vec![
                "A".repeat(10000),
                "A".repeat(100000),
                "\x00".repeat(1000),
            ],
            
            // ... more vectors
            _ => Vec::new(),
        }
    }
    
    /// Test function with all malicious inputs
    pub async fn fuzz_function<F, Fut>(&self, test_fn: F) -> Result<FuzzReport>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let mut report = FuzzReport::default();
        let inputs = self.generate_inputs();
        
        for input in inputs {
            match test_fn(input.clone()).await {
                Ok(_) => {
                    report.handled_safely += 1;
                }
                Err(e) => {
                    // Check if error is expected (safe rejection) or crash
                    if Self::is_safe_error(&e) {
                        report.rejected_safely += 1;
                    } else {
                        report.vulnerabilities.push(Vulnerability {
                            input,
                            error: e.to_string(),
                        });
                    }
                }
            }
        }
        
        Ok(report)
    }
}
```

---

## What Models Can Already Handle Well

Models have good built-in capabilities for:

### ✅ **Unit Testing**
- Writing test cases
- Mocking dependencies
- Assertion writing
- Test data generation

### ✅ **Code Analysis**
- Static analysis
- Linting
- Type checking
- Code review

### ✅ **Integration Testing**
- Test orchestration
- Setup/teardown
- Multi-component testing

### ✅ **CLI Testing**
- Input/output validation
- Exit code checking
- Command-line parsing tests

### ✅ **Documentation Testing**
- Example verification
- Doc comment accuracy
- README correctness

---

## What Benefits from Special Infrastructure

### 🔧 **Requires Infrastructure:**

1. **Network/HTTP Testing** ← Mock servers, network simulation
2. **Database Testing** ← In-memory DBs, test data management
3. **GUI/Game Testing** ← Virtual displays (already covered)
4. **Load/Stress Testing** ← Load generators, metrics collection
5. **Security Testing** ← Fuzzing, vulnerability scanning
6. **Time-Based Testing** ← Virtual clocks, time manipulation
7. **File System Testing** ← Virtual FS, error simulation
8. **Concurrent Testing** ← Race detection, ordering permutation

---

## Recommended Built-in Infrastructure

### Priority 1 (High Value):
1. ✅ **Virtual Display** (already designed)
2. **Mock HTTP Server** - Essential for API testing
3. **In-Memory Database** - Fast, isolated DB tests
4. **Network Simulator** - Test under poor conditions

### Priority 2 (Medium Value):
5. **Virtual Clock** - Test timeouts, scheduling
6. **Load Generator** - Performance testing
7. **Input Fuzzer** - Security testing

### Priority 3 (Nice to Have):
8. **Virtual File System** - Test FS errors
9. **Race Detector** - Concurrent testing

---

## Integration with Orchestrator

```rust
pub struct TestingInfrastructure {
    /// Virtual display for GUI testing
    pub virtual_display: Option<VirtualDisplay>,
    
    /// Mock HTTP servers
    pub mock_servers: Vec<MockHttpServer>,
    
    /// Test databases
    pub test_dbs: Vec<TestDatabase>,
    
    /// Network simulator
    pub network_sim: Option<NetworkSimulator>,
    
    /// Virtual clock
    pub clock: Option<VirtualClock>,
}

impl TestingInfrastructure {
    /// Set up all testing infrastructure
    pub async fn setup_for_project(project: &Project) -> Result<Self> {
        let mut infra = TestingInfrastructure::default();
        
        // Detect project needs
        if project.has_gui() {
            infra.virtual_display = Some(VirtualDisplay::start().await?);
        }
        
        if project.has_api_clients() {
            let mock = MockHttpServer::start().await?;
            infra.mock_servers.push(mock);
        }
        
        if project.has_database() {
            let db = TestDatabase::create_with_schema(&project.schema).await?;
            infra.test_dbs.push(db);
        }
        
        Ok(infra)
    }
    
    /// Cleanup all infrastructure
    pub async fn cleanup(&mut self) -> Result<()> {
        if let Some(display) = &mut self.virtual_display {
            display.shutdown().await?;
        }
        
        for server in &mut self.mock_servers {
            server.shutdown().await?;
        }
        
        Ok(())
    }
}
```

---

## Summary

**Models are already good at:**
- Unit tests, integration tests, CLI tests
- Code analysis, linting, type checking
- Test case generation, mocking

**Built-in infrastructure should provide:**
1. ✅ **Virtual Display** (GUI/game testing) - Already designed
2. **Mock HTTP Server** (API testing) - High priority
3. **In-Memory Database** (DB testing) - High priority
4. **Network Simulator** (poor network conditions) - Medium priority
5. **Virtual Clock** (time-based features) - Medium priority
6. **Load Generator** (performance) - Medium priority
7. **Input Fuzzer** (security) - Medium priority

**Bottom line:** Models can handle most testing with their existing tools (bash, file access, etc.). The specialized infrastructure provides the most value for:
- **Visual testing** (GUI/games) ← Already covered
- **Service simulation** (mock servers, DBs) ← Add this
- **Adversarial testing** (network issues, security fuzzing) ← Nice to have

Should we add mock HTTP server and in-memory DB support to the design?
