use super::Repository;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use colored::Colorize;
use super::utils::{create_box_header, create_box_footer, create_box_separator, create_box_row, clean_url, extract_file_path, filter_files_by_extension};
use regex::Regex;
use once_cell::sync::Lazy;
use serde::Serialize;

// Pre-compiled regex patterns for variable name extraction (compiled once, reused many times)
static VAR_NAME_REGEX_WITH_KEYWORDS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)\b(?:private|public|readonly|const|let|var|final|static|protected|internal)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*[:=]"#).unwrap()
});

static VAR_NAME_REGEX_SIMPLE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?:^|\s)([a-zA-Z_][a-zA-Z0-9_]*)\s*[:=]\s*["']?"#).unwrap()
});

// Only skip these extensions (e.g. docs). .config, .xml, and other config files are always analyzed.
const FILES_EXTENSIONS: &[&str] = &[".html", ".csv", ".md"];

// Maximum number of files to analyze per repository (GitHub API limit is 100,000, but we use a conservative limit)
const MAX_FILES_PER_REPO: usize = 50000;

// Patterns to exclude (false positives) - these indicate the secret comes from env vars or processes
static EXCLUSION_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // Environment variables
        Regex::new(r#"(?i)(process\.env|os\.getenv|os\.environ|getenv|environ\[|System\.getenv|ENV\[|env\.get)\s*[\[\(]?["']?\w+["']?[\]\)]?"#).unwrap(),
        // Configuration placeholders
        Regex::new(r#"(?i)(placeholder|example|your_|test_|demo_|sample_|xxx|yyyy|changeme|replaceme)"#).unwrap(),
        // Empty or very short values
        Regex::new(r#"(?i)["']\s*["']|["']\s*$|=\s*["']\s*["']|=\s*$"#).unwrap(),
        // Comments indicating it's not a real secret
        Regex::new(r#"(?i)(#|//|/\*).*(example|placeholder|test|demo|sample|changeme)"#).unwrap(),
    ]
});

const PATTERNS_JSON: &str = include_str!("../../patterns.json");

// Secret detection patterns with improved accuracy - captures variable name and secret type
// Pattern format: (Regex, SecretType) where Regex captures groups: (variable_name, secret_value)
static PATTERNS: Lazy<Vec<(Regex, String)>> = Lazy::new(|| {
    let value: serde_json::Value =
        serde_json::from_str(PATTERNS_JSON).expect("Failed to parse patterns.json");

    let arr = value
        .as_array()
        .expect("patterns.json must be a JSON array");

    arr.iter()
        .filter_map(|item| {
            // Check if pattern is disabled (defaults to enabled if field is missing)
            let disabled = item["disabled"]
                .as_bool()
                .unwrap_or(false);
            
            if disabled {
                return None; // Skip disabled patterns
            }
            
            let pattern = item["pattern"]
                .as_str()
                .expect("Each pattern must have a string 'pattern' field");
            let secret_type = item["secret_type"]
                .as_str()
                .expect("Each pattern must have a string 'secret_type' field")
                .to_string();

            let re = Regex::new(pattern).unwrap_or_else(|e| {
                panic!("Invalid regex in patterns.json: {} ({})", pattern, e)
            });

            Some((re, secret_type))
        })
        .collect()
});



fn repo_sort_date(repo: &Repository, commit_date_kind: &str) -> chrono::DateTime<chrono::Utc> {
    if commit_date_kind == "repo" {
        repo.last_commit_date
    } else {
        repo.last_commit_date_file
    }
}

fn is_interesting_config_file(path: &str) -> bool {
    let lower = path.to_lowercase();

    // Skip obvious noise we already exclude elsewhere
    if FILES_EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
        return false;
    }

    // Extensions that typically hold configuration or secrets
    const INTERESTING_EXTS: &[&str] = &[
        ".env",
        ".env.local",
        ".env.production",
        ".config",
        ".cfg",
        ".ini",
        ".yaml",
        ".yml",
        ".json",
        ".xml",
        ".properties",
        ".toml",
        ".tf",
        ".tfvars",
    ];

    let ext = lower.rfind('.').map(|i| &lower[i..]).unwrap_or("");
    if INTERESTING_EXTS.contains(&ext) {
        return true;
    }

    // Filenames that look like they might contain secrets/config
    const NAME_KEYWORDS: &[&str] = &[
        "config",
        "settings",
        "secrets",
        "secret",
        "password",
        "passwd",
        "credential",
        "credentials",
        "token",
        "apikey",
        "api_key",
        "key",
        "keys",
    ];

    NAME_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

pub async fn single_file_detection_with_regex(result: HashMap<String, Repository>, token: &str, error_counter: super::github::HttpErrorCounter, sort_order: &str, commit_date_kind: &str, quiet: bool) {
    const BOX_WIDTH: usize = 80;
    
    // Convert HashMap to Vec and sort by selected commit date (file or repo)
    let mut repos: Vec<(String, Repository)> = result.into_iter().collect();
    let kind = if commit_date_kind == "repo" { "repo" } else { "file" };
    match sort_order {
        "asc" => repos.sort_by(|a, b| repo_sort_date(&a.1, commit_date_kind).cmp(&repo_sort_date(&b.1, commit_date_kind))),
        "desc" => repos.sort_by(|a, b| repo_sort_date(&b.1, commit_date_kind).cmp(&repo_sort_date(&a.1, commit_date_kind))),
        _ => {}
    }
    
    let total_repos = repos.len();
    let mut index = 1;
    
    println!("\n{}", "─".repeat(BOX_WIDTH).cyan());
    println!("{}", format!("  🔍 Analyzing {} repositories for secrets", total_repos).bold().cyan());
    println!("{}", format!("  📊 Sorted by {} commit date: {} ({} first)", 
        kind,
        if sort_order == "asc" { "Ascending" } else { "Descending" },
        if sort_order == "asc" { "oldest" } else { "newest" }
    ).bold().cyan());
    println!("{}\n", "─".repeat(BOX_WIDTH).cyan());
    
    for (_, repo) in repos {
        let repo_url = repo.repository_url.clone();
        let mut skipped_files = 0;
        let mut skipped_files_extensions: Vec<String> = Vec::new();
        let mut secrets_found: Vec<(String, String, String)> = Vec::new(); // (file_path, variable_name, secret_type)
        
        // Analyze files for secrets in parallel FIRST (before printing anything)
        use super::github::get_raw_file_content_from_api;
        use std::sync::Arc;
        use tokio::sync::Semaphore;
        use futures::future::join_all;
        
        const MAX_CONCURRENT_FILE_REQUESTS: usize = 30; // Increased for better parallel processing
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_FILE_REQUESTS));
        let error_counter_arc = Arc::new(error_counter.clone());
        let token_arc = Arc::new(token.to_string());
        
        let file_futures: Vec<_> = repo.files_urls_api.iter().map(|url| {
            let url_clone = url.clone();
            let url_clean = clean_url(url);
            let semaphore_clone = semaphore.clone();
            let error_counter_clone = error_counter_arc.clone();
            let token_clone = token_arc.clone();
            
            async move {
                if FILES_EXTENSIONS.iter().any(|ext| url_clean.ends_with(ext)) {
                    return (None, true, url_clean.clone());
                }
                let _permit = semaphore_clone.acquire().await.unwrap();
                let api_result = get_raw_file_content_from_api(url_clone.clone(), &token_clone).await;
                match api_result {
                    Ok(content) => {
                        let mut found_patterns = Vec::new();
                        
                        // Check for secrets and capture variable names
                        // IMPORTANT: Check ALL patterns to find ALL secrets in the file
                        for (pattern, secret_type) in PATTERNS.iter() {
                            // Use find_iter to find ALL matches of this pattern in the content
                            let content_str = content.as_str();
                            for captures in pattern.captures_iter(content_str) {
                                    // Check exclusion patterns per-match line, not per-file
                                    let match_start_check = captures.get(0).map(|m| m.start()).unwrap_or(0);
                                    let line_start_check = content_str[..match_start_check].rfind('\n').map(|i| i + 1).unwrap_or(0);
                                    let line_end_check = content_str[match_start_check..].find('\n').map(|i| match_start_check + i).unwrap_or(content_str.len());
                                    let match_line = &content_str[line_start_check..line_end_check];
                                    // Never exclude lines that look like XML config (<add key="..." value="...">)
                                    let looks_like_xml_config = match_line.contains("<add")
                                        && match_line.contains("key=")
                                        && match_line.contains("value=");
                                    let is_excluded = !looks_like_xml_config
                                        && EXCLUSION_PATTERNS.iter().any(|exclusion| {
                                            exclusion.is_match(match_line)
                                        });
                                    if is_excluded {
                                        continue;
                                    }
                                    // Extract the line containing the match
                                    let match_start = captures.get(0).map(|m| m.start()).unwrap_or(0);
                                    let line_start = content_str[..match_start].rfind('\n').map(|i| i + 1).unwrap_or(0);
                                    let line_end = content_str[match_start..].find('\n').map(|i| match_start + i).unwrap_or(content_str.len());
                                    let line = &content_str[line_start..line_end];
                                    
                                    // Try to get variable name from first capture group (if pattern captures it)
                                    let var_name = captures.get(1)
                                        .map(|m| {
                                            let captured = m.as_str().trim_matches(|c| c == '"' || c == '\'').to_string();
                                            // Check if this is an XML pattern (contains "<add" in the pattern)
                                            let is_xml_pattern = secret_type.contains("XML");
                                            
                                            if is_xml_pattern {
                                                // For XML patterns, the key name is what we want to show
                                                // Allow it even if it's a keyword (like "passwd", "usuario")
                                                if captured.chars().any(|c| c.is_alphanumeric() || c == '_')
                                                    && captured.len() < 100 {
                                                    captured
                                                } else {
                                                    "unknown".to_string()
                                                }
                                            } else {
                                                // For non-XML patterns, filter out keywords
                                                let keywords = ["api", "key", "apikey", "password", "passwd", "pwd", "token", "secret"];
                                                let captured_lower = captured.to_lowercase();
                                                
                                                if !keywords.contains(&captured_lower.as_str())
                                                    && captured.chars().any(|c| c.is_alphanumeric() || c == '_')
                                                    && captured.len() < 100
                                                    && captured.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false) {
                                                    captured
                                                } else {
                                                    "unknown".to_string()
                                                }
                                            }
                                        })
                                        .unwrap_or_else(|| {
                                            // Fallback: use regex to extract variable name from the line
                                            // Use pre-compiled regex to extract variable name: match pattern like "variable_name = value" or "variable_name: value"
                                            // This regex looks for: (optional keywords) variable_name (optional type) = or :
                                            // Match: optional keywords (with word boundaries), then variable name (camelCase, snake_case, UPPER_CASE), then = or :
                                            if let Some(var_captures) = VAR_NAME_REGEX_WITH_KEYWORDS.captures(line) {
                                                if let Some(var_match) = var_captures.get(1) {
                                                    let var_name = var_match.as_str().to_string();
                                                    if var_name.len() < 100 && var_name.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false) {
                                                        return var_name;
                                                    }
                                                }
                                            }
                                            
                                            // Alternative: try to match variable name directly before = or : without keywords
                                            // This regex finds ALL variable names before the assignment operator and takes the LAST one
                                            // It handles cases like: private readonly publicApiKey = "value"
                                            // Find all matches and take the last one (closest to the assignment)
                                            let mut last_match: Option<String> = None;
                                            for cap in VAR_NAME_REGEX_SIMPLE.captures_iter(line) {
                                                if let Some(var_match) = cap.get(1) {
                                                    let var_name = var_match.as_str().to_string();
                                                    // Exclude common keywords
                                                    let keywords = ["private", "public", "readonly", "const", "let", "var", "final", "static", "protected", "internal", "api", "key", "apikey"];
                                                    if !keywords.contains(&var_name.to_lowercase().as_str())
                                                        && var_name.len() < 100 
                                                        && var_name.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false) {
                                                        last_match = Some(var_name);
                                                    }
                                                }
                                            }
                                            
                                            if let Some(name) = last_match {
                                                return name;
                                            }
                                            
                                            // If regex didn't work, try manual extraction
                                            if match_start >= line_start {
                                                let before_match = &line[..match_start - line_start];
                                                
                                                // Find the position of the assignment operator (= or :)
                                                let assign_pos = before_match.rfind(|c| c == '=' || c == ':');
                                                
                                                if let Some(pos) = assign_pos {
                                                    // Get the part before the assignment operator
                                                    let var_part = &before_match[..pos].trim();
                                                    
                                                    // Common keywords to remove (only as whole words, case-insensitive)
                                                    let keywords = ["private", "public", "readonly", "const", "let", "var", "final", "static", "protected", "internal"];
                                                    
                                                    // Split into words and filter out keywords
                                                    let words: Vec<&str> = var_part.split_whitespace().collect();
                                                    
                                                    // Find the last word that is NOT a keyword (case-insensitive comparison)
                                                    words.iter()
                                                        .rev()
                                                        .find(|word| {
                                                            let cleaned = word.trim_matches(|c| c == '"' || c == '\'' || c == ',' || c == ';' || c == '{' || c == '[' || c == '(' || c == ')' || c == ']' || c == '}');
                                                            let cleaned_lower = cleaned.to_lowercase();
                                                            !keywords.iter().any(|kw| cleaned_lower == *kw)
                                                                && cleaned.chars().any(|c| c.is_alphanumeric() || c == '_')
                                                                && cleaned.len() < 100
                                                                && cleaned.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false)
                                                        })
                                                        .map(|word| {
                                                            word.trim_matches(|c| c == '"' || c == '\'' || c == ',' || c == ';' || c == '{' || c == '[' || c == '(' || c == ')' || c == ']' || c == '}')
                                                                .to_string()
                                                        })
                                                        .unwrap_or_else(|| {
                                                            // Fallback: take the last word
                                                            words.last()
                                                                .map(|w| w.trim_matches(|c| c == '"' || c == '\'' || c == '=' || c == ':' || c == ',' || c == ';' || c == '{' || c == '[' || c == '(' || c == ')' || c == ']' || c == '}').to_string())
                                                                .unwrap_or_else(|| "unknown".to_string())
                                                        })
                                                } else {
                                                    // No assignment operator, try last word
                                                    let words: Vec<&str> = before_match.split_whitespace().collect();
                                                    words.last()
                                                        .map(|w| w.trim_matches(|c| c == '"' || c == '\'' || c == '=' || c == ':' || c == ',' || c == ';' || c == '{' || c == '[' || c == '(' || c == ')' || c == ']' || c == '}').to_string())
                                                        .unwrap_or_else(|| "unknown".to_string())
                                                }
                                            } else {
                                                "unknown".to_string()
                                            }
                                        });
                                    
                                    // Add this match to found_patterns (check for duplicates first)
                                    let pattern_entry = (var_name.clone(), secret_type.to_string());
                                    if !found_patterns.iter().any(|(v, s)| v == &pattern_entry.0 && s == &pattern_entry.1) {
                                        found_patterns.push(pattern_entry);
                                    }
                            }
                        }
                        (Some((url_clean.clone(), found_patterns)), false, url_clean.clone())
                    }
                    Err(e) => {
                        // Record error if it's an HTTP error
                        if let super::github::GithubError::InvalidResponse(msg) = &e {
                            if msg.contains("status") {
                                // Try to extract status code from error message
                                let status_code = msg
                                    .split("status ")
                                    .nth(1)
                                    .and_then(|s| s.split_whitespace().next())
                                    .and_then(|s| s.parse::<u16>().ok())
                                    .unwrap_or(404);
                                error_counter_clone.record_error(status_code);
                            }
                        }
                        (None, false, url_clean.clone())
                    }
                }
            }
        }
        ).collect();
        
        let file_results = join_all(file_futures).await;
        
        for result in file_results {
            match result {
                (Some((url_clean, patterns)), false, _) => {
                    for (var_name, secret_type) in patterns {
                        secrets_found.push((url_clean.clone(), var_name, secret_type));
                    }
                }
                (Some((url_clean, _)), true, _) => {
                    skipped_files += 1;
                    if let Some(ext) = url_clean.split('.').last() {
                        skipped_files_extensions.push(ext.to_string());
                    }
                }
                (None, true, url_clean) => {
                    skipped_files += 1;
                    if let Some(ext) = url_clean.split('.').last() {
                        skipped_files_extensions.push(ext.to_string());
                    }
                }
                _ => {}
            }
        }
        
        // Update progress bar BEFORE processing (so user sees progress immediately)
        // Skip repository if quiet mode and no secrets found (BEFORE printing anything)
        if quiet && secrets_found.is_empty() {
            index += 1;
            continue;
        }
        
        // Now print repository info (only if we have secrets or quiet is false)
        // Box header - use current index, then increment for next iteration
        let current_index = index;
        index += 1;
        println!("{}", create_box_header("Repository", current_index, total_repos, BOX_WIDTH).cyan().bold());
        
        // Repository URL
        println!("{}", create_box_row("🔗 URL", &repo_url, BOX_WIDTH - 2));
        println!("{}", create_box_separator(BOX_WIDTH));
        
        // Get repository metadata to display branch (if API returns it)
        use super::github::{extract_repo_info, get_repo_metadata, get_repo_languages};
        if let Some((owner, repo_name)) = extract_repo_info(&repo_url) {
            if let Ok(metadata) = get_repo_metadata(token, &owner, &repo_name).await {
                let branch = metadata.default_branch.clone();
                println!("{}", create_box_row("🌿 Branch", &branch, BOX_WIDTH - 2));
                println!("{}", create_box_separator(BOX_WIDTH));
            }
        }
        if let Some((owner, repo_name)) = extract_repo_info(&repo_url) {
        let languages = match get_repo_languages(token, &owner, &repo_name).await {
            Ok(languages) => {
                let langs: HashMap<String, u64> =
                serde_json::from_str(&languages).expect("Failed to parse languages JSON");
                let total: u64 = langs.values().sum();
                let mut parts = Vec::new();
                let mut other_langs = 0;
                for (lang, bytes) in &langs {
                    let pct = (*bytes as f64 / total as f64) * 100.0;
                    if pct > 1.0 {
                        parts.push(format!(
                            "• {}: {}",
                            lang.bold().cyan(),
                            format!("{pct:.2}%").yellow()
                        ));
                    }
                    else {
                        other_langs += *bytes;
                    }
                }
                if other_langs > 0 {
                    let other_pct = (other_langs as f64 / total as f64) * 100.0;
                    parts.push(format!(
                        "{}: {}",
                        "Other".bold().cyan(),
                        format!("{other_pct:.2}%").yellow()
                    ));
                }
                println!("{}", create_box_row("🌐 Languages", &parts.join("\n"), BOX_WIDTH - 2));
                println!("{}", create_box_separator(BOX_WIDTH));
            }
            Err(e) => {
                println!("{}", create_box_row("❌ Error", &format!("Failed to get repository languages: {}", e), BOX_WIDTH - 2));
                println!("{}\n", create_box_footer(BOX_WIDTH));
                continue;
            }
            };
        }

        
        // Files detected - show full paths
        let files_list: Vec<String> = repo.files_urls.iter()
            .map(|x| extract_file_path(x))
            .collect();
        let files_str = if files_list.is_empty() {
            "None".to_string()
        } else {
            files_list.join(", ")
        };
        println!("{}", create_box_row("📝 Files", &files_str, BOX_WIDTH - 2));
        println!("{}", create_box_separator(BOX_WIDTH));
        
        // Secrets found
        if !secrets_found.is_empty() {
            println!("{}", create_box_row("🚨 Secrets Found", &format!("{} potential secret(s)", secrets_found.len()), BOX_WIDTH - 2));
            for (file_url, var_name, secret_type) in &secrets_found {
                let file_path = extract_file_path(file_url);
                let display_info = if var_name != "unknown" {
                    format!("{}: {}", var_name.bold().cyan(), secret_type)
                } else {
                    secret_type.to_string()
                };
                println!("│   {} {} {}", "⚠️".red(), file_path.bold().yellow(), format!("({})", display_info).dimmed());
            }
            println!("{}", create_box_separator(BOX_WIDTH));
        } else {
            println!("{}", create_box_row("✅ Status", "No secrets detected", BOX_WIDTH - 2));
            println!("{}", create_box_separator(BOX_WIDTH));
        }
        
        // Last commit dates (file and repo)
        let file_date = repo.last_commit_date_file.format("%d/%m/%Y %H:%M:%S UTC").to_string();
        let repo_date = repo.last_commit_date.format("%d/%m/%Y %H:%M:%S UTC").to_string();
        println!("{}", create_box_row("📆 Last commit (file)", &file_date, BOX_WIDTH - 2));
        println!("{}", create_box_row("📆 Last commit (repo)", &repo_date, BOX_WIDTH - 2));
        
        // Skipped files info
        if skipped_files > 0 {
            println!("{}", create_box_separator(BOX_WIDTH));
            let skipped_info = format!("{} file(s) skipped ({})", skipped_files, skipped_files_extensions.join(", "));
            println!("{}", create_box_row("⏭️  Skipped", &skipped_info, BOX_WIDTH - 2));
        }
        
        // Box footer
        println!("{}\n", create_box_footer(BOX_WIDTH));
    }
    
    println!("{}", "═".repeat(BOX_WIDTH).cyan());
    println!("{}", "  ✨ Analysis Complete".bold().green());
    println!("{}\n", "═".repeat(BOX_WIDTH).cyan());
    
    // Print error summary
    error_counter.print_summary();
}

pub async fn whole_repo_detection(result: HashMap<String, Repository>, token: &str, _domain: &str, error_counter: super::github::HttpErrorCounter, sort_order: &str, commit_date_kind: &str, max_files: Option<usize>, quiet: bool) {
    const BOX_WIDTH: usize = 80;
    
    // Convert HashMap to Vec and sort by selected commit date (file or repo)
    let mut repos: Vec<(String, Repository)> = result.into_iter().collect();
    let kind = if commit_date_kind == "repo" { "repo" } else { "file" };
    match sort_order {
        "asc" => repos.sort_by(|a, b| repo_sort_date(&a.1, commit_date_kind).cmp(&repo_sort_date(&b.1, commit_date_kind))),
        "desc" => repos.sort_by(|a, b| repo_sort_date(&b.1, commit_date_kind).cmp(&repo_sort_date(&a.1, commit_date_kind))),
        _ => {}
    }
    
    let total_repos = repos.len();
    let mut index = 1;
    
    println!("\n{}", "═".repeat(BOX_WIDTH).cyan());
    println!("{}", format!("  🔍 Whole Repository Analysis - Found {} Repository(ies)", total_repos).bold().cyan());
    println!("{}", "  🔐 Scanning likely config/secret files (not filtered by domain)".bold().cyan());
    println!("{}", format!("  📊 Sorted by {} commit date: {} ({} first)", 
        kind,
        if sort_order == "asc" { "Ascending" } else { "Descending" },
        if sort_order == "asc" { "oldest" } else { "newest" }
    ).bold().cyan());
    if let Some(n) = max_files {
        println!("{}", format!("  📁 Max files per repo: {} (--max-files)", n).bold().cyan());
    }
    println!("{}\n", "═".repeat(BOX_WIDTH).cyan());
    
    use super::github::{extract_repo_info, get_repo_files_recursive, get_file_content_by_sha};
    use indicatif::{ProgressBar, ProgressStyle};
    
    for (repo_url, repo) in repos {
        let mut secrets_found: Vec<(String, String)> = Vec::new(); // (path, pattern)
        let mut files_analyzed = 0;
        let mut skipped_files = 0;
        let mut skipped_extensions: Vec<String> = Vec::new();
        let mut hidden_secrets: Vec<(String, String)> = Vec::new(); // (file_path, match_line)
        let mut skip_entire_file_due_to_hidden = false;
        
        // Add a newline to separate from progress bar
        println!();
        


        // Box header - use current index, then increment for next iteration
        let current_index = index;
        index += 1;
        println!("{}", create_box_header("Repository", current_index, total_repos, BOX_WIDTH).cyan().bold());
        println!("{}", create_box_row("🔗 URL", &repo_url, BOX_WIDTH - 2));
        println!("{}", create_box_separator(BOX_WIDTH));
        
        // Extract owner and repo name
        let (owner, repo_name) = match extract_repo_info(&repo_url) {
            Some(info) => info,
            None => {
                println!("{}", create_box_row("❌ Error", "Failed to parse repository URL", BOX_WIDTH - 2));
                println!("{}\n", create_box_footer(BOX_WIDTH));
                continue;
            }
        };
        
        println!("{}", create_box_row("📦 Repository", &format!("{}/{}", owner, repo_name), BOX_WIDTH - 2));
        println!("{}", create_box_separator(BOX_WIDTH));
        
        // Get repository metadata and check if it should be scanned
        use super::github::{get_repo_metadata, should_scan_repository};
        let metadata = match get_repo_metadata(token, &owner, &repo_name).await {
            Ok(m) => m,
            Err(e) => {
                println!("{}", create_box_row("❌ Error", &format!("Failed to get repository metadata: {}", e), BOX_WIDTH - 2));
                println!("{}\n", create_box_footer(BOX_WIDTH));
                continue;
            }
        };
        
        // Check if repository should be scanned
        let (should_scan, skip_reason) = should_scan_repository(&metadata, _domain);
        if !should_scan {
            let reason = skip_reason.unwrap_or_else(|| "Repository filtered out".to_string());
            println!("{}", create_box_row("⏭️  Skipped", &reason, BOX_WIDTH - 2));
            println!("{}", create_box_separator(BOX_WIDTH));
            println!("{}", create_box_row("💡 Reason", "This repository appears to be documentation, reports, analysis, or owned by the same company as the domain", BOX_WIDTH - 2));
            println!("{}\n", create_box_footer(BOX_WIDTH));
            continue;
        }
        
        let branch = metadata.default_branch.clone();
        println!("{}", create_box_row("🌿 Branch", &branch, BOX_WIDTH - 2));
        if let Some(ref desc) = metadata.description {
            if !desc.is_empty() {
                println!("{}", create_box_row("📝 Description", desc, BOX_WIDTH - 2));
            }
        }
        if !metadata.topics.is_empty() {
            println!("{}", create_box_row("🏷️  Topics", &metadata.topics.join(", "), BOX_WIDTH - 2));
        }
        println!("{}", create_box_separator(BOX_WIDTH));
        // Last commit dates (file that matched search, and repo)
        let file_date = repo.last_commit_date_file.format("%d/%m/%Y %H:%M:%S UTC").to_string();
        let repo_date = repo.last_commit_date.format("%d/%m/%Y %H:%M:%S UTC").to_string();
        println!("{}", create_box_row("📆 Last commit (file)", &file_date, BOX_WIDTH - 2));
        println!("{}", create_box_row("📆 Last commit (repo)", &repo_date, BOX_WIDTH - 2));
        println!("{}", create_box_separator(BOX_WIDTH));
        
        // Get all files in repository
        println!("{}", create_box_row("⏳ Status", "Scanning likely config/secret files for secrets...", BOX_WIDTH - 2));
        
        let all_files = match get_repo_files_recursive(token, &owner, &repo_name, &branch).await {
            Ok(files) => files,
            Err(e) => {
                println!("{}", create_box_row("❌ Error", &format!("Failed to get repository files: {}", e), BOX_WIDTH - 2));
                println!("{}\n", create_box_footer(BOX_WIDTH));
                continue;
            }
        };
        
        // Filter to interesting config/secret-like files to reduce API calls
        let mut interesting_files: Vec<(String, String)> = all_files
            .into_iter()
            .filter(|(path, _)| is_interesting_config_file(path))
            .collect();

        let total_interesting = interesting_files.len();

        // Apply per-repo file limit if set (--max-files)
        if let Some(limit) = max_files {
            if interesting_files.len() > limit {
                interesting_files.truncate(limit);
            }
        }

        // Check if repository has too many files to analyze (even after filtering)
        if interesting_files.len() > MAX_FILES_PER_REPO {
            println!("{}", create_box_row("⚠️  Warning", &format!("Repository has {} interesting files (limit: {}). Skipping analysis to avoid API limits.", total_interesting, MAX_FILES_PER_REPO), BOX_WIDTH - 2));
            println!("{}", create_box_separator(BOX_WIDTH));
            println!("{}", create_box_row("💡 Tip", "This repository is too large for full analysis. Consider using single-file mode or --max-files.", BOX_WIDTH - 2));
            println!("{}\n", create_box_footer(BOX_WIDTH));
            continue;
        }

        let files_msg = if max_files.is_some() && total_interesting > interesting_files.len() {
            format!("{} file(s) to analyze ({} interesting, limit applied)", interesting_files.len(), total_interesting)
        } else {
            format!("{} file(s) total", interesting_files.len())
        };
        println!("{}", create_box_row("📊 Total Files", &files_msg, BOX_WIDTH - 2));
        println!("{}", create_box_separator(BOX_WIDTH));
        
        // Progress bar for file scanning with custom style
        // Limit bar width to fit within 80 char box width
        let pb = ProgressBar::new(interesting_files.len() as u64);
        pb.set_style(ProgressStyle::default_bar()
            .template("{spinner:.magenta} [{elapsed_precise}] [{bar:20.magenta/blue}] {pos:>5}/{len:5} ({percent:>3}%) {msg:.dim}")
            .unwrap()
            .progress_chars("█▉▊▋▌▍▎▏░"));
        pb.set_message("Scanning files...");
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        
        // Analyze ALL files for secrets in parallel (no domain filtering)
        use std::sync::Arc;
        use tokio::sync::Semaphore;
        use futures::future::join_all;
        use std::sync::Mutex;
        
        const MAX_CONCURRENT_FILE_REQUESTS: usize = 40; // Increased for whole repo analysis
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_FILE_REQUESTS));
        let error_counter_arc = Arc::new(error_counter.clone());
        let token_arc = Arc::new(token.to_string());
        let owner_arc = Arc::new(owner.clone());
        let repo_name_arc = Arc::new(repo_name.clone());
        
        // Wrap progress bar in Arc<Mutex> to share between futures
        let pb_arc = Arc::new(Mutex::new(pb));
        
        let file_futures: Vec<_> = interesting_files.iter().map(|(file_path, file_sha)| {
            let file_path_clone = file_path.clone();
            let file_sha_clone = file_sha.clone();
            let semaphore_clone = semaphore.clone();
            let error_counter_clone = error_counter_arc.clone();
            let token_clone = token_arc.clone();
            let owner_clone = owner_arc.clone();
            let repo_name_clone = repo_name_arc.clone();
            let pb_clone = pb_arc.clone();
            let hidden_secrets_clone = hidden_secrets.clone();

            async move {
                // Skip certain file extensions
                if FILES_EXTENSIONS.iter().any(|ext| file_path_clone.ends_with(ext)) {
                    // Update progress bar for skipped files too
                    if let Ok(pb_guard) = pb_clone.lock() {
                        pb_guard.inc(1);
                    }
                    return (None, true, file_path_clone, Vec::new()); // (secrets, skipped, path)
                }
                
                let _permit = semaphore_clone.acquire().await.unwrap();
                let result = match get_file_content_by_sha(&token_clone, &owner_clone, &repo_name_clone, &file_sha_clone).await {
                        Ok(content) => {
                        let mut found_patterns = Vec::new();
                        let mut hidden_matches: Vec<(String, String)> = Vec::new(); // (file_path, match_line)
                        // Check for secrets - apply exclusion per-match-line, not per-file
                        for (pattern, secret_type) in PATTERNS.iter() {
                            for m in pattern.find_iter(&content) {
                                let line_start = content[..m.start()].rfind('\n').map(|i| i + 1).unwrap_or(0);
                                let line_end = content[m.start()..].find('\n').map(|i| m.start() + i).unwrap_or(content.len());
                                let match_line = &content[line_start..line_end];
                                let looks_like_xml_config = match_line.contains("<add")
                                    && match_line.contains("key=")
                                    && match_line.contains("value=");

                                static ENV_PLACEHOLDER_REGEX: Lazy<Regex> = Lazy::new(|| {
                                    Regex::new(r"\$\{\w+\}").unwrap()
                                    });
                                let has_hidden_secrets = match_line.contains("os.environ")
                                    || match_line.contains("process.env")
                                    || match_line.contains("getenv()")
                                    //|| !match_line.contains(">> .env")
                                    || match_line.contains("os.getenv")
                                    || ENV_PLACEHOLDER_REGEX.is_match(match_line);

                                if has_hidden_secrets {
                                    hidden_matches.push((file_path_clone.clone(), match_line.trim().to_string()));
                                    skip_entire_file_due_to_hidden = true;
                                }
                                let is_excluded = !looks_like_xml_config
                                    && EXCLUSION_PATTERNS.iter().any(|exclusion| {
                                        exclusion.is_match(match_line)
                                    });
                                if !is_excluded {
                                    found_patterns.push(secret_type.to_string());
                                    break; // One match per pattern per file is enough
                                }
                            }
                        }
                        if skip_entire_file_due_to_hidden {
                            return (None, true, file_path_clone, hidden_matches);
                        }
                        (Some(found_patterns), false, file_path_clone, hidden_matches)
                    }
                    Err(e) => {
                        // Record error if it's an HTTP error
                        if let super::github::GithubError::InvalidResponse(msg) = &e {
                            if msg.contains("status") {
                                // Try to extract status code from error message
                                let status_code = msg
                                    .split("status ")
                                    .nth(1)
                                    .and_then(|s| s.split_whitespace().next())
                                    .and_then(|s| s.parse::<u16>().ok())
                                    .unwrap_or(404);
                                error_counter_clone.record_error(status_code);
                            }
                        }
                        (None, false, file_path_clone, Vec::new())
                    }
                };
                
                // Update progress bar when this file completes
                if let Ok(pb_guard) = pb_clone.lock() {
                    pb_guard.inc(1);
                }
                
                result
            }
        }).collect();
        
        // Wait for all files to process (progress bar updates as each completes)
        let file_results = join_all(file_futures).await;
        
        // Get the progress bar back to finish it
        let pb = Arc::try_unwrap(pb_arc).unwrap().into_inner().unwrap();
        
        // Process results
        for result in file_results {
            match result {
                (Some(patterns), false, file_path, hidden_matches) => {
                    files_analyzed += 1;
                    for pattern in patterns {
                        secrets_found.push((file_path.clone(), pattern));
                    }
                    for hidden_match in hidden_matches {
                        hidden_secrets.push(hidden_match);
                    }
                }
                (_, true, file_path, hidden_matches) => {
                    skipped_files += 1;
                    if let Some(ext) = file_path.split('.').last() {
                        if !skipped_extensions.contains(&ext.to_string()) {
                            skipped_extensions.push(ext.to_string());
                        }
                    }
                    for hidden_match in hidden_matches {
                        hidden_secrets.push(hidden_match);
                    }
                }
                _ => {
                skipped_files += 1;
                }
            }
        }
        
        if quiet && secrets_found.is_empty() {
            index += 1;
            continue;
        }
        // Finish and clear the progress bar to avoid leftover output
        pb.finish_and_clear();
        
        // Display results
        println!("{}", create_box_row("📝 Files Analyzed", &format!("{} file(s) scanned for secrets", files_analyzed), BOX_WIDTH - 2));
        println!("{}", create_box_separator(BOX_WIDTH));
        
        // Hidden secrets found
        if !hidden_secrets.is_empty() {
            println!("{}", create_box_separator(BOX_WIDTH));
            println!(
                "{}",
                create_box_row(
                    "🚨 Hidden/Env-based",
                    &format!("{} match(es) skipped (env-based)", hidden_secrets.len()),
                    BOX_WIDTH - 2
                )
            );
            for (file_path, line_info) in &hidden_secrets {
                println!(
                    "│   {} {} {}",
                    "ℹ️".yellow(),
                    file_path.bold().yellow(),
                    format!("({})", line_info).dimmed()
                );
            }
            println!("{}", create_box_separator(BOX_WIDTH));
        }

        // Secrets found
        if !secrets_found.is_empty() {
            println!("{}", create_box_row("🚨 Secrets Found", &format!("{} potential secret(s) detected", secrets_found.len()), BOX_WIDTH - 2));
            for (file_path, pattern) in &secrets_found {
                println!("│   {} {} {}", "⚠️".red(), file_path.bold().yellow(), format!("({})", pattern).dimmed());
            }
            println!("{}", create_box_separator(BOX_WIDTH));
        } else {
            println!("{}", create_box_row("✅ Secrets Status", "No secrets detected in repository", BOX_WIDTH - 2));
            println!("{}", create_box_separator(BOX_WIDTH));
        }
        
        // Skipped files info
        if skipped_files > 0 {
            println!("{}", create_box_separator(BOX_WIDTH));
            let skipped_info = format!("{} file(s) skipped ({})", skipped_files, skipped_extensions.join(", "));
            println!("{}", create_box_row("⏭️  Skipped", &skipped_info, BOX_WIDTH - 2));
        }
        
        // Box footer
        println!("{}\n", create_box_footer(BOX_WIDTH));
    }
    
    println!("{}", "═".repeat(BOX_WIDTH).cyan());
    println!("{}", "  ✨ Whole Repository Analysis Complete".bold().green());
    println!("{}\n", "═".repeat(BOX_WIDTH).cyan());
    
    // Print error summary
    error_counter.print_summary();
}

/// One repository entry for JSON export in list mode.
#[derive(Debug, Serialize)]
struct ListRepoJson {
    repository_url: String,
    last_commit_file: chrono::DateTime<chrono::Utc>,
    last_commit_repo: chrono::DateTime<chrono::Utc>,
    files: Vec<String>,
}

/// List repositories with basic information only (no secret analysis).
/// If `include_ext` or `exclude_ext` are set, the file list per repo is filtered by extension (e.g. `.rs`, `.toml`).
/// If `export_json` is `Some(None)` output is written as JSON to stdout; if `Some(Some(path))` to that file.
pub async fn list_repositories(
    result: HashMap<String, Repository>,
    error_counter: super::github::HttpErrorCounter,
    sort_order: &str,
    commit_date_kind: &str,
    include_ext: Option<Vec<String>>,
    exclude_ext: Option<Vec<String>>,
    export_json: Option<Option<std::path::PathBuf>>,
) {
    // Convert HashMap to Vec and sort by selected commit date (file or repo)
    let mut repos: Vec<(String, Repository)> = result.into_iter().collect();
    match sort_order {
        "asc" => repos.sort_by(|a, b| repo_sort_date(&a.1, commit_date_kind).cmp(&repo_sort_date(&b.1, commit_date_kind))),
        "desc" => repos.sort_by(|a, b| repo_sort_date(&b.1, commit_date_kind).cmp(&repo_sort_date(&a.1, commit_date_kind))),
        _ => {}
    }

    let total_repos = repos.len();
    let kind = if commit_date_kind == "repo" { "repo" } else { "file" };

    if let Some(json_dest) = export_json {
        // Build JSON list (same filtering as human output)
        let mut list: Vec<ListRepoJson> = Vec::new();
        for (_, repo) in &repos {
            let urls = filter_files_by_extension(
                &repo.files_urls,
                include_ext.as_deref(),
                exclude_ext.as_deref(),
                extract_file_path,
            );
            if urls.is_empty() && (include_ext.is_some() || exclude_ext.is_some()) {
                continue;
            }
            let files: Vec<String> = urls.iter().map(|url| extract_file_path(url)).collect();
            list.push(ListRepoJson {
                repository_url: repo.repository_url.clone(),
                last_commit_file: repo.last_commit_date_file,
                last_commit_repo: repo.last_commit_date,
                files,
            });
        }
        let payload = serde_json::to_string_pretty(&list).expect("serialize list to JSON");
        match json_dest {
            None => {
                println!("{}", payload);
            }
            Some(path) => {
                let path = path.as_path();
                if path == Path::new("-") {
                    println!("{}", payload);
                } else {
                    let mut f = std::fs::File::create(path).expect("create JSON output file");
                    f.write_all(payload.as_bytes()).expect("write JSON to file");
                }
            }
        }
    } else {
        // Human-readable output
        println!("\n{} Found {} repository(ies) (sorted by {} commit date)\n", "📋".green(), total_repos.to_string().bold(), kind);

        for (_, repo) in repos {
            let urls = filter_files_by_extension(
                &repo.files_urls,
                include_ext.as_deref(),
                exclude_ext.as_deref(),
                extract_file_path,
            );
            if urls.is_empty() && (include_ext.is_some() || exclude_ext.is_some()) {
                continue;
            }
            let files_list: Vec<String> = urls.iter().map(|url| extract_file_path(url)).collect();
            let files_str = if files_list.is_empty() {
                "None".to_string()
            } else {
                files_list.join(", ")
            };

            let file_commit_date = repo.last_commit_date_file.format("%d/%m/%Y %H:%M:%S UTC").to_string();
            let repo_commit_date = repo.last_commit_date.format("%d/%m/%Y %H:%M:%S UTC").to_string();

            println!("• {}", repo.repository_url.cyan().bold());
            println!("  • Last commit (file): {}", file_commit_date.yellow());
            println!("  • Last commit (repo): {}", repo_commit_date.yellow());
            println!("  • Files: {}", files_str.green());
            println!();
        }

        println!();
    }

    error_counter.print_summary();
}