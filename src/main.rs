//! LLM Conductor - Intelligent LLM orchestration
use anyhow::Result;
use tracing_subscriber;

use llm_conductor::cli::Repl;
use llm_conductor::providers::OllamaProvider;
use llm_conductor::router::Router;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(false)
        .init();
    
    // Create router
    let mut router = Router::new();
    
    // Add Ollama provider
    let ollama = OllamaProvider::new(None);
    router.add_provider(Box::new(ollama));
    
    // Create and run REPL
    let mut repl = Repl::new(router);
    repl.run().await?;
    
    Ok(())
}
