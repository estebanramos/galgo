// src/github/mod.rs
pub mod github;
pub mod utils;
pub mod secret_detection;

// Re-exportar las funciones principales para facilitar el uso
pub use github::*;
pub use utils::*;
pub use secret_detection::*;