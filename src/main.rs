use std::fmt;
use std::io::IsTerminal;
use clap::{CommandFactory, Parser};
use colored::Colorize;
use github::search;
use token::{describe_token, get_token};
use crate::github::{list_repositories, run_scan, ScanMode, ScanOptions};

mod github;
mod token;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(flatten)]
    search: SearchConfig,

    #[command(flatten)]
    analysis: AnalysisModes,

    #[command(flatten)]
    output: OutputConfig,
}

#[derive(Parser, Debug)]
#[command(next_help_heading = "OUTPUT")]
struct OutputConfig {
    #[arg(long, num_args = 0..=1, value_name = "FILE", help = "Output results as JSON (stdout by default, or to FILE). Works in every mode")]
    json: Option<Option<std::path::PathBuf>>,

    #[arg(short, long, help = "Hide repositories with no secrets found (single-file and whole-repo modes)")]
    quiet: bool,

    #[arg(short, long, help = "Print a full box for every repository (default: only repositories with findings; clean ones take one line)")]
    verbose: bool,

    #[arg(long, help = "Disable colored output (also disabled automatically when stdout is not a terminal)")]
    no_color: bool,

    #[arg(long, help = "Print full secret values instead of masked ones (e.g. AK****LE)")]
    show_secrets: bool,
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
async fn main() {
    let args = Args::parse();

    if args.output.no_color || !std::io::stdout().is_terminal() {
        colored::control::set_override(false);
    }

    let mode = if args.analysis.single_file {
        Some(ScanMode::SingleFile)
    } else if args.analysis.whole_repo {
        Some(ScanMode::WholeRepo)
    } else if args.analysis.list {
        None
    } else {
        eprintln!("❌ {}\n", GalgoError::NoModeSelected);
        Args::command().print_help().unwrap();
        std::process::exit(2);
    };

    let (token, source) = get_token().unwrap_or_else(|e| {
        eprintln!("{}", e);
        std::process::exit(2);
    });
    // stdout when it is free for humans; stderr when stdout carries JSON.
    let json_on_stdout = matches!(&args.output.json, Some(None)) || matches!(&args.output.json, Some(Some(p)) if p.as_os_str() == "-");
    let say = |msg: String| if json_on_stdout { eprintln!("{}", msg) } else { println!("{}", msg) };
    say(format!("{} Token loaded from {} ({})", "🔑".green(), source.bold(), describe_token(&token)));
    match github::check_token(&token).await {
        Ok(quota) => say(format!("{} Token OK, remaining quota: {}", "✅".green(), quota)),
        Err(e) => {
            eprintln!("{} {}", "❌ Token check failed:".red(), e);
            std::process::exit(2);
        }
    }
    eprintln!("{} {}", "⏳ Processing:".green(), args.search.domain);
    let (repos, error_counter) = match search(&token, &args.search.domain, args.search.max_repos, args.analysis.include_ext.clone()).await {
        Ok(r) => r,
        Err(github::GithubError::RateLimitExceeded(msg)) => {
            eprintln!("\n{}\n{}", "⛔ Execution stopped due to rate limit.".bold().red(), msg);
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(2);
        }
    };

    let Some(mode) = mode else {
        list_repositories(
            repos,
            error_counter,
            &args.search.sort,
            &args.search.commit_date,
            args.analysis.include_ext,
            args.analysis.exclude_ext,
            args.output.json,
        ).await;
        return;
    };

    let findings = run_scan(repos, &token, error_counter, ScanOptions {
        mode,
        domain: args.search.domain,
        sort_order: args.search.sort,
        commit_date_kind: args.search.commit_date,
        max_files: args.analysis.max_files,
        quiet: args.output.quiet,
        verbose: args.output.verbose,
        show_secrets: args.output.show_secrets,
        json: args.output.json,
    }).await;

    // Exit codes: 0 = clean, 1 = secrets found, 2 = usage/auth/API error.
    if findings > 0 {
        std::process::exit(1);
    }
}
