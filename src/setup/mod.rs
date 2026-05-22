pub mod first_run;
pub mod models;
pub mod ollama;

pub use first_run::FirstRunSetup;
pub use models::ModelManager;
pub use ollama::{InstallStatus, OllamaInstaller};
