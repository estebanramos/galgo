use reqwest::{Client, header::HeaderMap};
use serde_json::Value;
use std::{fmt, time::Duration};
use std::collections::HashMap;
use std::sync::Arc;
use colored::Colorize;
use super::utils::{clean_file_name, clean_url, extract_file_path};
use chrono::{DateTime, Utc};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use tokio::sync::Semaphore;
use std::sync::atomic::{AtomicU32, Ordering};

const USER_AGENT: &str = "Galgo App";
const MAX_CONCURRENT_REQUESTS: usize = 50; // Maximum concurrent API requests (increased for better performance)

/// HTTP error counter for tracking API errors
#[derive(Clone, Default)]
pub struct HttpErrorCounter {
    pub status_401: Arc<AtomicU32>,
    pub status_403: Arc<AtomicU32>,
    pub status_404: Arc<AtomicU32>,
    pub status_422: Arc<AtomicU32>,
    pub status_429: Arc<AtomicU32>,
    pub status_500: Arc<AtomicU32>,
    pub status_503: Arc<AtomicU32>,
    pub other_errors: Arc<AtomicU32>,
}

impl HttpErrorCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_error(&self, status: u16) {
        match status {
            401 => self.status_401.fetch_add(1, Ordering::Relaxed),
            403 => self.status_403.fetch_add(1, Ordering::Relaxed),
            404 => self.status_404.fetch_add(1, Ordering::Relaxed),
            422 => self.status_422.fetch_add(1, Ordering::Relaxed),
            429 => self.status_429.fetch_add(1, Ordering::Relaxed),
            500 => self.status_500.fetch_add(1, Ordering::Relaxed),
            503 => self.status_503.fetch_add(1, Ordering::Relaxed),
            _ => self.other_errors.fetch_add(1, Ordering::Relaxed),
        };
    }

    pub fn total_errors(&self) -> u32 {
        self.status_401.load(Ordering::Relaxed) +
        self.status_403.load(Ordering::Relaxed) +
        self.status_404.load(Ordering::Relaxed) +
        self.status_422.load(Ordering::Relaxed) +
        self.status_429.load(Ordering::Relaxed) +
        self.status_500.load(Ordering::Relaxed) +
        self.status_503.load(Ordering::Relaxed) +
        self.other_errors.load(Ordering::Relaxed)
    }

    pub fn print_summary(&self) {
        let total = self.total_errors();
        if total > 0 {
            println!("\n{}", "─".repeat(80).yellow());
            println!("{}", "  ⚠️  HTTP Error Summary".bold().yellow());
            println!("{}", "─".repeat(80).yellow());
            if self.status_401.load(Ordering::Relaxed) > 0 {
                println!("  401 Unauthorized: {}", self.status_401.load(Ordering::Relaxed));
            }
            if self.status_403.load(Ordering::Relaxed) > 0 {
                println!("  403 Forbidden: {}", self.status_403.load(Ordering::Relaxed));
            }
            if self.status_404.load(Ordering::Relaxed) > 0 {
                println!("  404 Not Found: {}", self.status_404.load(Ordering::Relaxed));
            }
            if self.status_422.load(Ordering::Relaxed) > 0 {
                println!("  422 Unprocessable Entity: {}", self.status_422.load(Ordering::Relaxed));
            }
            if self.status_429.load(Ordering::Relaxed) > 0 {
                println!("  429 Rate Limit Exceeded: {}", self.status_429.load(Ordering::Relaxed));
            }
            if self.status_500.load(Ordering::Relaxed) > 0 {
                println!("  500 Internal Server Error: {}", self.status_500.load(Ordering::Relaxed));
            }
            if self.status_503.load(Ordering::Relaxed) > 0 {
                println!("  503 Service Unavailable: {}", self.status_503.load(Ordering::Relaxed));
            }
            if self.other_errors.load(Ordering::Relaxed) > 0 {
                println!("  Other Errors: {}", self.other_errors.load(Ordering::Relaxed));
            }
            println!("  {} Total HTTP Errors: {}", "⚠️".red(), total.to_string().red().bold());
            println!("{}\n", "─".repeat(80).yellow());
        }
    }
}

pub struct Repository {
    pub repository_url: String,
    pub repository_name: String,
    pub author: String,
    pub files_names: Vec<String>,
    pub files_urls: Vec<String>,
    pub files_urls_api: Vec<String>,
    /// Last commit on the repository (default branch).
    pub last_commit_date: DateTime<Utc>,
    /// Last commit that touched the found file.
    pub last_commit_date_file: DateTime<Utc>,
}

impl fmt::Display for Repository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", "🔎 Repository".blue().bold())?;
        write!(f, "\n    📦 URL: {}", self.repository_url.yellow())?;
        write!(f, "\n    📄 Files: {}", self.files_urls.iter().map(|x| clean_file_name(x.clone())).collect::<Vec<String>>().join(", ").yellow())?;
        write!(f, "\n    👤 Author: {}", self.author.yellow())?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum GithubError {
    RequestError(reqwest::Error),
    InvalidResponse(String),
    RateLimitExceeded(String),
}

impl std::error::Error for GithubError {}

impl fmt::Display for GithubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GithubError::RequestError(e) => write!(f, "Request Error: {}", e),
            GithubError::InvalidResponse(e) => write!(f, "Invalid Response: {}", e),
            GithubError::RateLimitExceeded(e) => write!(f, "Rate Limit Exceeded: {}", e),
        }
    }
}

fn create_github_client() -> Result<Client, GithubError> {
    let mut headers = HeaderMap::new();
    headers.insert("User-Agent", USER_AGENT.parse().map_err(|e| {
        GithubError::InvalidResponse(format!("Invalid User-Agent header: {}", e))
    })?);
    Client::builder()
        .pool_max_idle_per_host(50)  // Increased connection pool for better performance
        .pool_idle_timeout(Duration::from_secs(120))
        .timeout(Duration::from_secs(30))  // Add timeout to prevent hanging requests
        .default_headers(headers)
        .build()
        .map_err(GithubError::RequestError)
}

/// Search GitHub API for a given domain with pagination support
/// Returns a HashMap of unique repositories and error counter
/// max_repos: Optional limit on number of unique repositories to return (None = unlimited, up to API limits)
/// include_ext: When set, the GitHub Code Search API is restricted to these file extensions (e.g. ["rs", "toml"]).
pub async fn search(
    token: &str,
    domain: &str,
    max_repos: Option<usize>,
    include_ext: Option<Vec<String>>,
) -> Result<(HashMap<String, Repository>, HttpErrorCounter), GithubError> {
    const PER_PAGE: usize = 100;
    const MAX_TOTAL_RESULTS: usize = 1000; // GitHub API limit for code search
    
    let client = create_github_client()?;
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS));
    let error_counter = HttpErrorCounter::new();
    let mut repository_list: HashMap<String, Repository> = HashMap::new();
    let mut page = 1;
    let mut total_items_processed = 0;
    
    use indicatif::{ProgressBar, ProgressStyle};
    
    println!("\n{}", "═".repeat(80).cyan());
    println!("{}", format!("  🔍 Step 1/3: Searching GitHub for repositories matching '{}'", domain).bold().cyan());
    println!("{}\n", "═".repeat(80).cyan());
    
    // Create progress bar for search with custom spinner style
    let search_pb = ProgressBar::new_spinner();
    search_pb.set_style(ProgressStyle::default_spinner()
        .template("{spinner:.cyan.bold} {msg:.cyan}")
        .unwrap()
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]));
    search_pb.set_message("Searching GitHub API...");
    search_pb.enable_steady_tick(std::time::Duration::from_millis(80));
    
    // Will be initialized on first page
    let mut commit_pb: Option<ProgressBar> = None;
    // When API returns 422 with extension qualifier, we retry with domain-only and filter client-side
    let mut query_override: Option<String> = None;
    
    loop {
        // Build search query: domain + optional extension qualifiers.
        // Legacy Code Search API does not support parentheses; use "domain extension:py" or "domain extension:rs OR extension:toml".
        let query = if let Some(ref q) = query_override {
            q.clone()
        } else {
            match &include_ext {
                Some(exts) if !exts.is_empty() => {
                    let ext_clause: String = exts
                        .iter()
                        .map(|e| {
                            let e = e.trim().trim_start_matches('.').to_lowercase();
                            if e.is_empty() {
                                String::new()
                            } else {
                                format!("extension:{}", e)
                            }
                        })
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" OR ");
                    if ext_clause.is_empty() {
                        domain.to_string()
                    } else {
                        format!("{} {}", domain, ext_clause)
                    }
                }
                _ => domain.to_string(),
            }
        };
        // GitHub API expects spaces as '+' in the q parameter
        let q_encoded = query.replace(' ', "+");
        let url = format!("https://api.github.com/search/code?per_page={}&s=indexed&type=Code&o=desc&q={}&page={}",
                         PER_PAGE, q_encoded, page);
        
        let response = client.get(&url)
            .header("User-Agent", "Galgo App")
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(GithubError::RequestError)?;
        
        let status = response.status().as_u16();
        if !response.status().is_success() {
            error_counter.record_error(status);
            
            // Read the body once for all error handling
            let body = response.text().await.map_err(GithubError::RequestError)?;
            
            // Check for rate limit errors (403 or 429 with rate limit message)
            if status == 403 || status == 429 {
                let is_rate_limit = body.contains("API rate limit exceeded") 
                    || body.contains("rate limit")
                    || body.contains("X-RateLimit-Remaining")
                    || body.contains("rate limiting")
                    || status == 429;
                
                if is_rate_limit {
                    let error_msg = format!("GitHub API rate limit exceeded. Please wait before trying again.\nResponse: {}", body);
                    eprintln!("\n{}", "═".repeat(80).red());
                    eprintln!("{}", "  ⛔ RATE LIMIT EXCEEDED".bold().red());
                    eprintln!("{}", "═".repeat(80).red());
                    eprintln!("{}", error_msg);
                    eprintln!("{}", "═".repeat(80).red());
                    eprintln!();
                    return Err(GithubError::RateLimitExceeded(error_msg));
                }
                
                // If 403 but not rate limit, handle as forbidden
                if status == 403 {
                    eprintln!("[403] Forbidden: {}", body);
                    return Err(GithubError::InvalidResponse(format!("Forbidden: {}", body)));
                }
            }
            
            match status {
                401 => {
                    eprintln!("[401] Unauthorized => most likely the token is invalid or expired");
                    return Err(GithubError::InvalidResponse("Unauthorized".to_string()));
                }
                422 => {
                    // Invalid query (e.g. extension qualifier not accepted). Retry without extension once.
                    if query_override.is_none()
                        && include_ext.as_ref().map(|e| !e.is_empty()).unwrap_or(false)
                    {
                        eprintln!("[422] Query with extension qualifier not accepted by API, retrying without extension (results will be filtered by --include-ext/--exclude-ext).");
                        query_override = Some(domain.to_string());
                        continue; // retry same page with domain-only
                    }
                    eprintln!("[422] API Error: {}", body);
                    break; // Stop pagination
                }
                _ => {
                    // Continue but record the error
                    eprintln!("[{}] Error fetching page {}, continuing...", status, page);
                    page += 1;
                    continue;
                }
            }
        }

        let body = response.text().await.map_err(GithubError::RequestError)?;
        let json: Value = serde_json::from_str(&body)
            .map_err(|e| GithubError::InvalidResponse(format!("Failed to parse JSON: {}", e)))?;
        
        let items = json["items"].as_array()
            .ok_or_else(|| GithubError::InvalidResponse("Missing 'items' array in response".to_string()))?;
        
        let total_count = json["total_count"].as_u64().unwrap_or(0);
        
        if page == 1 {
            search_pb.finish_with_message("✅ Found matches, processing...");
            // Calculate the actual number of repos we'll process (limited by max_repos if set)
            let repos_to_process = if let Some(limit) = max_repos {
                total_count.min(limit as u64)
            } else {
                total_count.min(MAX_TOTAL_RESULTS as u64)
            };
            let step2_msg = format!("  🔄 Step 2/3: Fetching commit dates for {} repositories...", repos_to_process);
            println!("\n{}", "─".repeat(80).cyan());
            println!("{}", step2_msg.bold().cyan());
            println!("{}\n", "─".repeat(80).cyan());
            
            // Create progress bar for commit dates (Step 2) with gradient style
            // Limit bar width to fit within 80 char box width
            let pb = ProgressBar::new(repos_to_process);
            pb.set_style(ProgressStyle::default_bar()
                .template("{spinner:.yellow} [{elapsed_precise}] [{bar:20.yellow/blue}] {pos:>4}/{len:4} ({percent:>3}%) {msg:.dim}")
                .unwrap()
                .progress_chars("█▉▊▋▌▍▎▏░"));
            pb.set_message("Fetching commit dates...");
            pb.enable_steady_tick(std::time::Duration::from_millis(100));
            commit_pb = Some(pb);
        }
        
        // Update search progress (simplified to avoid lifetime issues)
        if page > 1 {
            search_pb.set_message("Fetching more pages...");
        }
        
        if items.is_empty() {
            // No more results
            break;
        }
        
        // Collect repository info first (without commit dates)
        let mut repo_data: Vec<(String, String, String, String, String, String, String)> = Vec::new();
        for x in items {
            // Check if we've reached the repository limit
            if let Some(limit) = max_repos {
                if repository_list.len() >= limit {
                    println!("   ✅ Reached maximum repository limit ({})", limit);
                    break;
                }
            }
            
            let file_url = x["html_url"].as_str()
                .ok_or_else(|| GithubError::InvalidResponse("Missing html_url".to_string()))?
                .to_string();
            let file_url_api = x["url"].as_str()
                .ok_or_else(|| GithubError::InvalidResponse("Missing url".to_string()))?
                .to_string();
            let clean_file = clean_file_name(file_url.clone());
            let repository_name = x["repository"]["name"].as_str()
                .ok_or_else(|| GithubError::InvalidResponse("Missing repository name".to_string()))?
                .to_string();
            let repository_url = x["repository"]["html_url"].as_str()
                .ok_or_else(|| GithubError::InvalidResponse("Missing repository html_url".to_string()))?
                .to_string();
            
            // Skip if we already have this repository
            if repository_list.contains_key(&repository_url) {
                continue;
            }
            
            let commits_url = x["repository"]["commits_url"].as_str()
                .ok_or_else(|| GithubError::InvalidResponse("Missing commits_url".to_string()))?
                .to_string()
                .replace("{/sha}", "");
            let author = x["repository"]["owner"]["html_url"].as_str()
                .ok_or_else(|| GithubError::InvalidResponse("Missing owner html_url".to_string()))?
                .to_string();
            
            repo_data.push((repository_url.clone(), repository_name.clone(), author.clone(), 
                          file_url.clone(), file_url_api.clone(), clean_file.clone(), commits_url.clone()));
        }
        
        // Fetch commit dates in parallel
        let token_arc = Arc::new(token.to_string());
        let error_counter_arc = Arc::new(error_counter.clone());
        let semaphore_clone = semaphore.clone();
        
        // Use atomic counter to track progress
        use std::sync::atomic::{AtomicU64, Ordering};
        let progress_counter = Arc::new(AtomicU64::new(0));
        let progress_counter_clone = progress_counter.clone();
        let commit_pb_ref = commit_pb.as_ref();
        
        let commit_futures: Vec<_> = repo_data.iter().map(|(repo_url, _repo_name, _author, file_url, _file_url_api, _clean_file, commits_url)| {
            let token_clone = token_arc.clone();
            let error_counter_clone = error_counter_arc.clone();
            let semaphore_clone = semaphore_clone.clone();
            let commits_url_clone = commits_url.clone();
            let repo_url_clone = repo_url.clone();
            let progress_counter_task = progress_counter_clone.clone();
            let file_path = extract_file_path(file_url);
            
            async move {
                let _permit = semaphore_clone.acquire().await.unwrap();
                // Fetch both: last commit on repo (no path) and last commit that touched the file (with path)
                let (repo_result, file_result) = tokio::join!(
                    get_last_commit_date(&token_clone, commits_url_clone.clone(), None),
                    get_last_commit_date(&token_clone, commits_url_clone.clone(), Some(file_path.as_str())),
                );
                let last_commit_date = repo_result.unwrap_or_else(|e| {
                    if let GithubError::InvalidResponse(msg) = &e {
                        if msg.contains("status") {
                            error_counter_clone.record_error(404);
                        }
                    }
                    Utc::now()
                });
                let last_commit_date_file = file_result.unwrap_or_else(|e| {
                    if let GithubError::InvalidResponse(msg) = &e {
                        if msg.contains("status") {
                            error_counter_clone.record_error(404);
                        }
                    }
                    Utc::now()
                });
                
                // Update progress counter when this future completes
                let current = progress_counter_task.fetch_add(1, Ordering::Relaxed) + 1;
                if let Some(pb) = commit_pb_ref {
                    pb.set_position(current);
                }
                
                Ok((repo_url_clone, last_commit_date, last_commit_date_file))
            }
        }).collect();
        
        // Wait for all commit dates in parallel (progress bar updates as each completes)
        let commit_results: Vec<Result<(String, DateTime<Utc>, DateTime<Utc>), GithubError>> = futures::future::join_all(commit_futures).await;
        
        // Process results and build repository list
        for (i, result) in commit_results.into_iter().enumerate() {
            if i < repo_data.len() {
                let (repository_url, repository_name, author, file_url, file_url_api, clean_file, _) = &repo_data[i];
                
                let (last_commit_date, last_commit_date_file) = match result {
                    Ok((_, repo_date, file_date)) => (repo_date, file_date),
                    Err(_) => (Utc::now(), Utc::now()),
                };
                
                let repository = Repository {
                    repository_url: repository_url.clone(),
                    repository_name: repository_name.clone(),
                    author: author.clone(),
                    files_urls: vec![file_url.clone()],
                    files_urls_api: vec![file_url_api.clone()],
                    files_names: vec![clean_file.clone()],
                    last_commit_date,
                    last_commit_date_file,
                };
                
                repository_list.insert(repository_url.clone(), repository);
                total_items_processed += 1;
            }
        }
        
        // Check if we should continue pagination
        if let Some(limit) = max_repos {
            if repository_list.len() >= limit {
                break;
            }
        }
        
        // Check if we got less than PER_PAGE results (last page)
        if items.len() < PER_PAGE {
            break;
        }
        
        // Check if we've reached GitHub's API limit
        if total_items_processed >= MAX_TOTAL_RESULTS {
            println!("   ⚠️  Reached GitHub API limit ({} results)", MAX_TOTAL_RESULTS);
            break;
        }
        
        page += 1;
        search_pb.set_message("Fetching more pages...");
        
        // Minimal delay to respect rate limits (reduced from 100ms to 50ms for better performance)
        // GitHub API allows 5000 requests/hour for authenticated users, so we can be more aggressive
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }
    
    search_pb.finish_with_message("✅ Search complete!");
    
    // Finish commit progress bar if it exists
    if let Some(pb) = commit_pb {
        pb.finish_with_message("✅ Commit dates fetched!");
    }
    
    let repo_count = repository_list.len();
    println!("\n{}", "─".repeat(80).cyan());
    println!("{}", format!("  📊 Step 3/3: Found {} unique repositories", repo_count).bold().cyan());
    println!("{}\n", "─".repeat(80).cyan());
    
    Ok((repository_list, error_counter))
}

#[allow(dead_code)]
pub async fn get_raw_file_content(url: String, token: &str) -> Result<String, GithubError> {
    let content_url = url.clone();
    let raw_url = content_url.replace("/blob", "").replace("https://github.com/", "https://raw.githubusercontent.com/").replace("\"", "");
    let client = create_github_client()?;
    let response = client.get(raw_url)
        .header("User-Agent", "Galgo App")
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github.v3.raw")
        .send()
        .await
        .map_err(GithubError::RequestError)?;
    match response.status().is_success() {
        true => {
            let body = response.text().await.map_err(GithubError::RequestError)?;
            return Ok(body);
        }
        false => {
            return Err(GithubError::InvalidResponse(format!("Failed to get raw file content: {}", response.status())));
        }
    }
}

pub async fn get_raw_file_content_from_api(url: String, token: &str) -> Result<String, GithubError> {
    // Usar la API de GitHub en lugar de raw CDN
    let client = create_github_client()?;
    let cleaned_url = clean_url(&url);
    let response = client.get(&cleaned_url)
        .header("User-Agent", "Galgo App")
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github.raw")
        .send()
        .await
        .map_err(GithubError::RequestError)?;
    
    let status = response.status().as_u16();
    if !response.status().is_success() {
        // Check for rate limit errors
        if status == 403 || status == 429 {
            let body = response.text().await.map_err(GithubError::RequestError)?;
            let is_rate_limit = body.contains("API rate limit exceeded") 
                || body.contains("rate limit")
                || body.contains("X-RateLimit-Remaining")
                || body.contains("rate limiting")
                || status == 429;
            
            if is_rate_limit {
                let error_msg = format!("GitHub API rate limit exceeded. Please wait before trying again.\nResponse: {}", body);
                eprintln!("\n{}", "═".repeat(80).red());
                eprintln!("{}", "  ⛔ RATE LIMIT EXCEEDED".bold().red());
                eprintln!("{}", "═".repeat(80).red());
                eprintln!("{}", error_msg);
                eprintln!("{}", "═".repeat(80).red());
                eprintln!();
                return Err(GithubError::RateLimitExceeded(error_msg));
            }
        }
        return Err(GithubError::InvalidResponse(format!("Failed to get raw file content: status {}", status)));
    }
    
    let body = response.text().await.map_err(GithubError::RequestError)?;
    Ok(body)
}

/// Fetches the date of the last commit. If `path` is Some, requests commits for that file only (last commit that touched the file).
pub async fn get_last_commit_date(token: &str, mut url: String, path: Option<&str>) -> Result<DateTime<Utc>, GithubError> {
    if let Some(p) = path {
        if !p.is_empty() {
            url.push('?');
            url.push_str("path=");
            url.push_str(&urlencoding::encode(p).to_string());
        }
    }
    let client = create_github_client()?;
    let response = client.get(&url)
        .header("User-Agent", "Galgo App")
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(GithubError::RequestError)?;
    
    let status = response.status().as_u16();
    if !response.status().is_success() {
        // Check for rate limit errors
        if status == 403 || status == 429 {
            let body = response.text().await.map_err(GithubError::RequestError)?;
            let is_rate_limit = body.contains("API rate limit exceeded") 
                || body.contains("rate limit")
                || body.contains("X-RateLimit-Remaining")
                || body.contains("rate limiting")
                || status == 429;
            
            if is_rate_limit {
                let error_msg = format!("GitHub API rate limit exceeded. Please wait before trying again.\nResponse: {}", body);
                eprintln!("\n{}", "═".repeat(80).red());
                eprintln!("{}", "  ⛔ RATE LIMIT EXCEEDED".bold().red());
                eprintln!("{}", "═".repeat(80).red());
                eprintln!("{}", error_msg);
                eprintln!("{}", "═".repeat(80).red());
                eprintln!();
                return Err(GithubError::RateLimitExceeded(error_msg));
            }
        }
        return Err(GithubError::InvalidResponse(format!("Failed to get commit date: status {}", status)));
    }
    
    let response_text = response.text().await.map_err(GithubError::RequestError)?;
    let json: Value = serde_json::from_str(&response_text)
        .map_err(|e| GithubError::InvalidResponse(format!("Failed to parse JSON: {}", e)))?;
    
    // Manejo seguro del acceso a la fecha
    let commits = json.as_array()
        .ok_or_else(|| GithubError::InvalidResponse("Expected array of commits".to_string()))?;
    
    if commits.is_empty() {
        return Err(GithubError::InvalidResponse("No commits found".to_string()));
    }
    
    let last_commit_date = commits[0]["commit"]["committer"]["date"]
        .as_str()
        .ok_or_else(|| GithubError::InvalidResponse("Missing commit date".to_string()))?;
    
    let parsed_date = DateTime::parse_from_rfc3339(last_commit_date)
        .map_err(|e| GithubError::InvalidResponse(format!("Failed to parse date: {}", e)))?
        .with_timezone(&Utc);
    Ok(parsed_date)
}

/// Extract owner and repo name from GitHub repository URL
pub fn extract_repo_info(repo_url: &str) -> Option<(String, String)> {
    // Format: https://github.com/owner/repo
    let cleaned = repo_url
        .replace("https://github.com/", "")
        .replace("http://github.com/", "");
    let parts: Vec<&str> = cleaned.split('/').collect();
    
    if parts.len() >= 2 {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}

/// Repository metadata structure
#[derive(Debug, Clone)]
pub struct RepoMetadata {
    pub default_branch: String,
    pub description: Option<String>,
    pub topics: Vec<String>,
    pub owner: String,
    pub name: String,
    pub is_archived: bool,
}

/// Get repository metadata (branch, description, topics, etc.)
pub async fn get_repo_metadata(token: &str, owner: &str, repo: &str) -> Result<RepoMetadata, GithubError> {
    let client = create_github_client()?;
    let url = format!("https://api.github.com/repos/{}/{}", owner, repo);
    
    let response = client.get(&url)
        .header("User-Agent", "Galgo App")
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(GithubError::RequestError)?;
    
    let status = response.status().as_u16();
    if !response.status().is_success() {
        // Read body once for error handling
        let body = response.text().await.map_err(GithubError::RequestError)?;
        
        // Check for rate limit errors
        if status == 403 || status == 429 {
            let is_rate_limit = body.contains("API rate limit exceeded") 
                || body.contains("rate limit")
                || body.contains("X-RateLimit-Remaining")
                || body.contains("rate limiting")
                || status == 429;
            
            if is_rate_limit {
                let error_msg = format!("GitHub API rate limit exceeded. Please wait before trying again.\nResponse: {}", body);
                eprintln!("\n{}", "═".repeat(80).red());
                eprintln!("{}", "  ⛔ RATE LIMIT EXCEEDED".bold().red());
                eprintln!("{}", "═".repeat(80).red());
                eprintln!("{}", error_msg);
                eprintln!("{}", "═".repeat(80).red());
                eprintln!();
                return Err(GithubError::RateLimitExceeded(error_msg));
            }
        }
        return Err(GithubError::InvalidResponse(format!("Failed to get repo info: status {}", status)));
    }
    
    let body = response.text().await.map_err(GithubError::RequestError)?;
    let json: Value = serde_json::from_str(&body)
        .map_err(|e| GithubError::InvalidResponse(format!("Failed to parse JSON: {}", e)))?;
    
    let default_branch = json["default_branch"]
        .as_str()
        .ok_or_else(|| GithubError::InvalidResponse("Missing default_branch".to_string()))?
        .to_string();
    
    let description = json["description"].as_str().map(|s| s.to_string());
    let is_archived = json["archived"].as_bool().unwrap_or(false);
    
    // Get topics from the topics endpoint (more reliable)
    let topics_url = format!("https://api.github.com/repos/{}/{}/topics", owner, repo);
    let topics_response = client.get(&topics_url)
        .header("User-Agent", "Galgo App")
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github.mercy-preview+json")
        .send()
        .await;
    
    let topics = if let Ok(resp) = topics_response {
        let topics_status = resp.status().as_u16();
        // Read body once for both rate limit check and success case
        let topics_body = resp.text().await.unwrap_or_default();
        
        // Check for rate limit in topics response
        if topics_status == 403 || topics_status == 429 {
            let is_rate_limit = topics_body.contains("API rate limit exceeded") 
                || topics_body.contains("rate limit")
                || topics_body.contains("X-RateLimit-Remaining")
                || topics_body.contains("rate limiting")
                || topics_status == 429;
            
            if is_rate_limit {
                let error_msg = format!("GitHub API rate limit exceeded. Please wait before trying again.\nResponse: {}", topics_body);
                eprintln!("\n{}", "═".repeat(80).red());
                eprintln!("{}", "  ⛔ RATE LIMIT EXCEEDED".bold().red());
                eprintln!("{}", "═".repeat(80).red());
                eprintln!("{}", error_msg);
                eprintln!("{}", "═".repeat(80).red());
                eprintln!();
                return Err(GithubError::RateLimitExceeded(error_msg));
            }
        }
        
        // If successful, parse the body
        if topics_status == 200 && !topics_body.is_empty() {
            if let Ok(topics_json) = serde_json::from_str::<Value>(&topics_body) {
                topics_json["names"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    
    Ok(RepoMetadata {
        default_branch,
        description,
        topics,
        owner: owner.to_string(),
        name: repo.to_string(),
        is_archived,
    })
}

/// Get default branch for a repository
pub async fn get_default_branch(token: &str, owner: &str, repo: &str) -> Result<String, GithubError> {
    let metadata = get_repo_metadata(token, owner, repo).await?;
    Ok(metadata.default_branch)
}

/// Check if a repository should be scanned based on various criteria
/// Returns (should_scan, reason) where reason explains why it was skipped (if applicable)
pub fn should_scan_repository(metadata: &RepoMetadata, domain: &str) -> (bool, Option<String>) {
    let repo_name_lower = metadata.name.to_lowercase();
    let owner_lower = metadata.owner.to_lowercase();
    let domain_lower = domain.to_lowercase();
    
    // Patterns that indicate documentation, reports, or analysis repositories
    let skip_patterns = [
        "doc", "docs", "documentation", "document",
        "report", "reports", "analysis", "analyses",
        "wiki", "guide", "guides", "tutorial", "tutorials",
        "example", "examples", "demo", "demos", "sample", "samples",
        "test", "tests", "testing", "spec", "specs",
        "archive", "archived", "backup", "backups",
        "old", "legacy", "deprecated", "obsolete",
        "random", "misc", "miscellaneous", "temp", "tmp",
    ];
    
    // Check repository name patterns
    for pattern in &skip_patterns {
        if repo_name_lower.contains(pattern) {
            return (false, Some(format!("Repository name contains '{}' (likely documentation/report/analysis repo)", pattern)));
        }
    }
    
    // Check if owner matches domain (same company)
    // Extract domain without www, http, https
    let clean_domain = domain_lower
        .replace("www.", "")
        .replace("http://", "")
        .replace("https://", "")
        .split('/')
        .next()
        .unwrap_or(&domain_lower)
        .split(':')
        .next()
        .unwrap_or(&domain_lower)
        .to_string();
    
    // Check if owner matches domain (e.g., owner "example" matches domain "example.com")
    let domain_without_tld = clean_domain.split('.').next().unwrap_or(&clean_domain);
    if owner_lower == domain_without_tld || owner_lower.contains(domain_without_tld) {
        return (false, Some(format!("Repository owner '{}' matches domain '{}' (likely same company)", owner_lower, domain)));
    }
    
    // Check description for skip patterns
    if let Some(ref desc) = metadata.description {
        let desc_lower = desc.to_lowercase();
        for pattern in &skip_patterns {
            if desc_lower.contains(pattern) {
                return (false, Some(format!("Repository description contains '{}' (likely documentation/report/analysis repo)", pattern)));
            }
        }
        
        // Check for documentation indicators in description
        let doc_indicators = ["documentation", "docs", "guide", "tutorial", "wiki", "example", "demo"];
        for indicator in &doc_indicators {
            if desc_lower.contains(indicator) {
                return (false, Some(format!("Repository description indicates it's a {} repository", indicator)));
            }
        }
    }
    
    // Check topics for skip patterns
    for topic in &metadata.topics {
        let topic_lower = topic.to_lowercase();
        for pattern in &skip_patterns {
            if topic_lower.contains(pattern) {
                return (false, Some(format!("Repository topic '{}' indicates it's a {} repository", topic, pattern)));
            }
        }
        
        // Common documentation/example topics
        let doc_topics = ["documentation", "docs", "guide", "tutorial", "example", "demo", "sample", "test"];
        if doc_topics.contains(&topic_lower.as_str()) {
            return (false, Some(format!("Repository has '{}' topic (likely documentation/example repo)", topic)));
        }
    }
    
    // Check if repository is archived
    if metadata.is_archived {
        return (false, Some("Repository is archived".to_string()));
    }
    
    // Repository passed all checks
    (true, None)
}

/// Get all files in a repository recursively
pub async fn get_repo_files_recursive(token: &str, owner: &str, repo: &str, branch: &str) -> Result<Vec<(String, String)>, GithubError> {
    // Use Git Trees API with recursive=1 to get all files
    let client = create_github_client()?;
    let url = format!("https://api.github.com/repos/{}/{}/git/trees/{}?recursive=1", owner, repo, branch);
    
    let response = client.get(&url)
        .header("User-Agent", "Galgo App")
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(GithubError::RequestError)?;
    
    let status = response.status().as_u16();
    if !response.status().is_success() {
        // Read body once for error handling
        let body = response.text().await.map_err(GithubError::RequestError)?;
        
        // Check for rate limit errors
        if status == 403 || status == 429 {
            let is_rate_limit = body.contains("API rate limit exceeded") 
                || body.contains("rate limit")
                || body.contains("X-RateLimit-Remaining")
                || body.contains("rate limiting")
                || status == 429;
            
            if is_rate_limit {
                let error_msg = format!("GitHub API rate limit exceeded. Please wait before trying again.\nResponse: {}", body);
                eprintln!("\n{}", "═".repeat(80).red());
                eprintln!("{}", "  ⛔ RATE LIMIT EXCEEDED".bold().red());
                eprintln!("{}", "═".repeat(80).red());
                eprintln!("{}", error_msg);
                eprintln!("{}", "═".repeat(80).red());
                eprintln!();
                return Err(GithubError::RateLimitExceeded(error_msg));
            }
        }
        return Err(GithubError::InvalidResponse(format!("Failed to get repo tree: status {}", status)));
    }
    
    let body = response.text().await.map_err(GithubError::RequestError)?;
    let json: Value = serde_json::from_str(&body)
        .map_err(|e| GithubError::InvalidResponse(format!("Failed to parse JSON: {}", e)))?;
    
    let tree = json["tree"].as_array()
        .ok_or_else(|| GithubError::InvalidResponse("Missing tree array".to_string()))?;
    
    let mut files: Vec<(String, String)> = Vec::new();
    
    for item in tree {
        let file_type = item["type"].as_str().unwrap_or("");
        if file_type == "blob" {
            if let (Some(path), Some(sha)) = (item["path"].as_str(), item["sha"].as_str()) {
                files.push((path.to_string(), sha.to_string()));
            }
        }
    }
    
    Ok(files)
}

/// Get file content by SHA
pub async fn get_file_content_by_sha(token: &str, owner: &str, repo: &str, sha: &str) -> Result<String, GithubError> {
    let client = create_github_client()?;
    let url = format!("https://api.github.com/repos/{}/{}/git/blobs/{}", owner, repo, sha);
    
    let response = client.get(&url)
        .header("User-Agent", "Galgo App")
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(GithubError::RequestError)?;
    
    let status = response.status().as_u16();
    if !response.status().is_success() {
        // Check for rate limit errors
        if status == 403 || status == 429 {
            let body = response.text().await.map_err(GithubError::RequestError)?;
            let is_rate_limit = body.contains("API rate limit exceeded") 
                || body.contains("rate limit")
                || body.contains("X-RateLimit-Remaining")
                || body.contains("rate limiting")
                || status == 429;
            
            if is_rate_limit {
                let error_msg = format!("GitHub API rate limit exceeded. Please wait before trying again.\nResponse: {}", body);
                eprintln!("\n{}", "═".repeat(80).red());
                eprintln!("{}", "  ⛔ RATE LIMIT EXCEEDED".bold().red());
                eprintln!("{}", "═".repeat(80).red());
                eprintln!("{}", error_msg);
                eprintln!("{}", "═".repeat(80).red());
                eprintln!();
                return Err(GithubError::RateLimitExceeded(error_msg));
            }
        }
        return Err(GithubError::InvalidResponse(format!("Failed to get file content: status {}", status)));
    }
    
    let body = response.text().await.map_err(GithubError::RequestError)?;
    let json: Value = serde_json::from_str(&body)
        .map_err(|e| GithubError::InvalidResponse(format!("Failed to parse JSON: {}", e)))?;
    
    // Check encoding - GitHub API returns base64 encoded content
    let encoding = json["encoding"].as_str().unwrap_or("");
    let content = json["content"].as_str()
        .ok_or_else(|| GithubError::InvalidResponse("Missing content field".to_string()))?;
    
    if encoding == "base64" {
        // Decode base64 content (remove newlines first)
        let cleaned_content = content.replace('\n', "").replace('\r', "");
        let decoded = STANDARD.decode(&cleaned_content)
            .map_err(|e| GithubError::InvalidResponse(format!("Failed to decode base64: {}", e)))?;
        String::from_utf8(decoded)
            .map_err(|e| GithubError::InvalidResponse(format!("Failed to convert to UTF-8: {}", e)))
    } else {
        // Content is already text (shouldn't happen but handle it)
        Ok(content.to_string())
    }
}

pub async fn get_repo_languages(token: &str, owner: &str, repo: &str) -> Result<String, GithubError> {
    let client = create_github_client()?;
    let url = format!("https://api.github.com/repos/{}/{}/languages", owner, repo);
    
    let response = client.get(&url)
        .header("User-Agent", "Galgo App")
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(GithubError::RequestError)?;
    
    let status = response.status().as_u16();
    if !response.status().is_success() {
        // Check for rate limit errors
        if status == 403 || status == 429 {
            let body = response.text().await.map_err(GithubError::RequestError)?;
            let is_rate_limit = body.contains("API rate limit exceeded") 
                || body.contains("rate limit")
                || body.contains("X-RateLimit-Remaining")
                || body.contains("rate limiting")
                || status == 429;
            
            if is_rate_limit {
                let error_msg = format!("GitHub API rate limit exceeded. Please wait before trying again.\nResponse: {}", body);
                eprintln!("\n{}", "═".repeat(80).red());
                eprintln!("{}", "  ⛔ RATE LIMIT EXCEEDED".bold().red());
                eprintln!("{}", "═".repeat(80).red());
                eprintln!("{}", error_msg);
                eprintln!("{}", "═".repeat(80).red());
                eprintln!();
                return Err(GithubError::RateLimitExceeded(error_msg));
            }
        }
        return Err(GithubError::InvalidResponse(format!("Failed to get file content: status {}", status)));
    }
    let body = response.text().await.map_err(GithubError::RequestError)?;
    let json: Value = serde_json::from_str(&body)
        .map_err(|e| GithubError::InvalidResponse(format!("Failed to parse JSON: {}", e)))?;
    Ok(json.to_string())
}