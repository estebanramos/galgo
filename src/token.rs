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
            TokenError::FileNotFound => write!(f, "{}", "❌ token.txt not found".red()),
            TokenError::IoError(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for TokenError {}

pub fn get_token() -> Result<String, TokenError> {
    println!("{}", "🔑 Trying to read token from environment variable".yellow());
    match std::env::var("GITHUB_TOKEN") {
        Ok(token) => {
            let token = token.trim().to_string();
            if !token.is_empty() {
                return Ok(token);
            }
        }
        Err(_) => {
            println!("{}", "⚠️  GITHUB_TOKEN environment variable not found".yellow());
        }
    }
    println!("{}", "🔑 Trying to read token from token.txt".yellow());
    read_token_from_file()
}

pub fn read_token_from_file() -> Result<String, TokenError> {
    // Try multiple possible locations for token.txt
    let possible_paths = [
        "token.txt",
        "./token.txt",
    ];
    
    let mut last_error = None;
    for path in &possible_paths {
        match File::open(path) {
            Ok(file) => {
                let mut reader = BufReader::new(file);
                let mut token = String::new();
                match reader.read_line(&mut token) {
                    Ok(_) => {
                        let token = token.trim().to_string();
                        if !token.is_empty() {
                            return Ok(token);
                        }
                    }
                    Err(e) => {
                        last_error = Some(TokenError::IoError(e));
                        continue;
                    }
                }
            }
            Err(_) => continue,
        }
    }
    
    Err(last_error.unwrap_or(TokenError::FileNotFound))
}