use std::fmt;
use clap::{CommandFactory, Parser};
use colored::Colorize;
use github::search;
use token::get_token;
use crate::github::single_file_detection_with_regex;

mod github;
mod token;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(flatten)]
    search: SearchConfig,

    #[command(flatten)]
    analysis: AnalysisModes,
}

#[derive(Parser, Debug)]
#[command(next_help_heading = "CONFIGURATION")]
struct SearchConfig {
    #[arg(short, long, value_parser = validate_domain, help = "Domain to search for secrets")]
    domain: String,
}

#[derive(Parser, Debug)]
#[command(next_help_heading = "MODES")]
struct AnalysisModes {
    #[arg(short, long, help = "Analyze single file match", group = "mode")]
    single_file: bool,

    #[arg(short, long, help = "Analyze whole repository", group = "mode")]
    whole_repo: bool,

}

fn validate_domain(s: &str) -> Result<String, String> {
    if s.contains(".") {
        Ok(s.to_string())
    } else {
        Err("Invalid domain".to_string())
    }
}                  

#[derive(Debug)]
pub enum GalgoError {
    NoModeSelected,
}

impl std::error::Error for GalgoError {}

impl fmt::Display for GalgoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GalgoError::NoModeSelected => write!(f, "No mode selected"),
        }
    }
}

#[tokio::main]
async fn main() -> () {
    // Parseo los argumentos
    let args = Args::parse();
    
    if args.analysis.single_file {
        match get_token() {
            Ok(token) => {
                println!("{} {}", "⏳ Processing:".green(), args.search.domain);
                let result = search(&token, &args.search.domain).await;
                match result {
                    Ok(result) => {
                        single_file_detection_with_regex(result, &token).await;
                    }
                    Err(e) => println!("Error: {}", e),
                }
            }
            Err(e) => println!("Error: {}", e),
        }
    }
    else if args.analysis.whole_repo {
        match get_token() {
            Ok(token) => {
                println!("{} {}", "⏳ Processing:".green(), args.search.domain);
                let result = search(&token, &args.search.domain).await;
                match result {
                    Ok(result) => {
                        println!("{}", "⚠️  Whole repository analysis mode is not yet implemented".yellow());
                        println!("{}", "Found repositories:".green());
                        for (url, repo) in result {
                            println!("  - {} ({})", url, repo.repository_name);
                        }
                    }
                    Err(e) => println!("Error: {}", e),
                }
            }
            Err(e) => println!("Error: {}", e),
        }
    }
    else {
        let mut cmd = Args::command();
        eprintln!("❌ {}", GalgoError::NoModeSelected);
        eprintln!("\n{}", "For more information:".yellow());
        cmd.print_help().unwrap();
        std::process::exit(1);
    }
    
}






