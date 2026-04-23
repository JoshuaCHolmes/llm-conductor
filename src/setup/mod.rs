pub mod ollama;
pub mod models;
pub mod first_run;

pub use ollama::{OllamaInstaller, InstallStatus};
pub use models::ModelManager;
pub use first_run::FirstRunSetup;
