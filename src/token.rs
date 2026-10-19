use std::fs::File;
use std::io::{BufRead, BufReader};
use std::fmt;
use colored::Colorize;

#[derive(Debug)]
pub enum TokenError {
    FileNotFound,
    IoError(std::io::Error)
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenError::FileNotFound => write!(f, "{}", "❌ No GitHub token found: set GITHUB_TOKEN or put it in ./token.txt".red()),
            TokenError::IoError(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for TokenError {}

/// Returns the token and a description of where it came from (never the token itself).
pub fn get_token() -> Result<(String, &'static str), TokenError> {
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Ok((token, "GITHUB_TOKEN environment variable"));
        }
    }
    read_token_from_file().map(|t| (t, "./token.txt"))
}

pub fn read_token_from_file() -> Result<String, TokenError> {
    let file = File::open("token.txt").map_err(|_| TokenError::FileNotFound)?;
    let mut token = String::new();
    BufReader::new(file).read_line(&mut token).map_err(TokenError::IoError)?;
    let token = token.trim().to_string();
    if token.is_empty() { Err(TokenError::FileNotFound) } else { Ok(token) }
}

/// Safe-to-print hint of a token: its kind and last 4 characters.
pub fn describe_token(token: &str) -> String {
    let kind = if token.starts_with("github_pat_") { "fine-grained PAT" }
        else if token.starts_with("ghp_") { "classic PAT" }
        else if token.starts_with("gho_") || token.starts_with("ghu_") || token.starts_with("ghs_") { "app/OAuth token" }
        else { "token" };
    let tail: String = token.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{}, ends in …{}", kind, tail)
}
