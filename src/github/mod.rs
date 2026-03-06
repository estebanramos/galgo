// src/github/mod.rs
pub mod github;
pub mod utils;
pub mod secret_detection;

// Re-export main functions for easier use
pub use github::*;
pub use secret_detection::*;