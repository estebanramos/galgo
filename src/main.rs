use std::fmt;
use clap::{CommandFactory, Parser};
use colored::Colorize;
use github::search;
use token::get_token;
use crate::github::{single_file_detection_with_regex, whole_repo_detection, list_repositories};

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
    
    #[arg(long, value_parser = validate_max_repos, help = "Maximum number of repositories to analyze (default: unlimited, up to API limits)")]
    max_repos: Option<usize>,
    
    #[arg(long, value_parser = validate_sort_order, default_value = "desc", help = "Sort results by commit date: 'asc' (oldest first) or 'desc' (newest first)")]
    sort: String,

    #[arg(long, value_parser = validate_commit_date_kind, default_value = "file", help = "Which date to use for sorting and display: 'file' (last commit of the found file, default) or 'repo' (last commit on the repo)")]
    commit_date: String,
}

#[derive(Parser, Debug)]
#[command(next_help_heading = "MODES")]
struct AnalysisModes {
    #[arg(short, long, help = "Analyze single file match", group = "mode")]
    single_file: bool,

    #[arg(short, long, help = "Analyze whole repository", group = "mode")]
    whole_repo: bool,
    
    #[arg(short, long, help = "List repositories only (no secret analysis)", group = "mode")]
    list: bool,

    #[arg(long, value_delimiter(','), help = "Only show files with these extensions in list mode (e.g. .rs,.toml)")]
    include_ext: Option<Vec<String>>,

    #[arg(long, value_delimiter(','), help = "Hide files with these extensions in list mode (e.g. .md,.html)")]
    exclude_ext: Option<Vec<String>>,

    #[arg(long, num_args = 0..=1, value_name = "FILE", help = "In list mode: output as JSON (default: stdout)")]
    json: Option<Option<std::path::PathBuf>>,

    #[arg(short, long, help = "Hide repositories with no secrets found (single-file mode only)")]
    quiet: bool,

    #[arg(long, value_parser = validate_max_files, help = "In whole-repo mode: maximum number of files to analyze per repository (default: unlimited)")]
    max_files: Option<usize>,
}

fn validate_domain(s: &str) -> Result<String, String> {
    if s.contains(".") {
        Ok(s.to_string())
    } else {
        Err("Invalid domain".to_string())
    }
}

fn validate_max_repos(s: &str) -> Result<usize, String> {
    match s.parse::<usize>() {
        Ok(n) if n > 0 => Ok(n),
        Ok(_) => Err("Maximum repositories must be greater than 0".to_string()),
        Err(_) => Err("Invalid number for maximum repositories".to_string()),
    }
}

fn validate_sort_order(s: &str) -> Result<String, String> {
    match s.to_lowercase().as_str() {
        "asc" | "desc" => Ok(s.to_lowercase()),
        _ => Err("Sort order must be 'asc' (ascending) or 'desc' (descending)".to_string()),
    }
}

fn validate_commit_date_kind(s: &str) -> Result<String, String> {
    match s.to_lowercase().as_str() {
        "file" | "repo" => Ok(s.to_lowercase()),
        _ => Err("Commit date must be 'file' (last commit of the file) or 'repo' (last commit on the repo)".to_string()),
    }
}

fn validate_max_files(s: &str) -> Result<usize, String> {
    match s.parse::<usize>() {
        Ok(n) if n > 0 => Ok(n),
        Ok(_) => Err("Maximum files per repo must be greater than 0".to_string()),
        Err(_) => Err("Invalid number for maximum files".to_string()),
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
    // Parse command-line arguments
    let args = Args::parse();
    
    if args.analysis.single_file {
        match get_token() {
            Ok(token) => {
                println!("{} {}", "⏳ Processing:".green(), args.search.domain);
                let result = search(&token, &args.search.domain, args.search.max_repos, args.analysis.include_ext.clone()).await;
                match result {
                    Ok((repos, error_counter)) => {
                        single_file_detection_with_regex(repos, &token, error_counter, &args.search.sort, &args.search.commit_date, args.analysis.quiet).await;
                    }
                    Err(e) => {
                        if let github::GithubError::RateLimitExceeded(_) = e {
                            eprintln!("\n{}", "Execution stopped due to rate limit.".bold().red());
                            std::process::exit(1);
                        } else {
                            eprintln!("Error: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
            }
            Err(e) => println!("Error: {}", e),
        }
    }
    else if args.analysis.whole_repo {
        match get_token() {
            Ok(token) => {
                println!("{} {}", "⏳ Processing:".green(), args.search.domain);
                let result = search(&token, &args.search.domain, args.search.max_repos, args.analysis.include_ext.clone()).await;
                match result {
                    Ok((repos, error_counter)) => {
                        whole_repo_detection(repos, &token, &args.search.domain, error_counter, &args.search.sort, &args.search.commit_date, args.analysis.max_files, args.analysis.quiet).await;
                    }
                    Err(e) => {
                        if let github::GithubError::RateLimitExceeded(_) = e {
                            eprintln!("\n{}", "Execution stopped due to rate limit.".bold().red());
                            std::process::exit(1);
                        } else {
                            eprintln!("Error: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
            }
            Err(e) => println!("Error: {}", e),
        }
    }
    else if args.analysis.list {
        match get_token() {
            Ok(token) => {
                println!("{} {}", "⏳ Processing:".green(), args.search.domain);
                let result = search(&token, &args.search.domain, args.search.max_repos, args.analysis.include_ext.clone()).await;
                match result {
                    Ok((repos, error_counter)) => {
                        list_repositories(
                            repos,
                            error_counter,
                            &args.search.sort,
                            &args.search.commit_date,
                            args.analysis.include_ext.clone(),
                            args.analysis.exclude_ext.clone(),
                            args.analysis.json.clone(),
                        ).await;
                    }
                    Err(e) => {
                        if let github::GithubError::RateLimitExceeded(_) = e {
                            eprintln!("\n{}", "Execution stopped due to rate limit.".bold().red());
                            std::process::exit(1);
                        } else {
                            eprintln!("Error: {}", e);
                            std::process::exit(1);
                        }
                    }
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






