use reqwest::Client;
use serde_json::Value;
use std::{fmt, time::Duration};
use std::collections::HashMap;
use std::sync::Arc;
use colored::Colorize;
use once_cell::sync::Lazy;
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

    /// Record the HTTP status embedded in an `InvalidResponse("... status N")` error, if any.
    pub fn record_github_error_opt(&self, err: Option<&GithubError>) {
        if let Some(GithubError::InvalidResponse(msg)) = err {
            if let Some(code) = msg.split("status ").nth(1).and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse().ok()) {
                self.record_error(code);
            }
        }
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
            eprintln!("\n{}", "─".repeat(80).yellow());
            eprintln!("{}", "  ⚠️  HTTP Error Summary".bold().yellow());
            eprintln!("{}", "─".repeat(80).yellow());
            if self.status_401.load(Ordering::Relaxed) > 0 {
                eprintln!("  401 Unauthorized: {}", self.status_401.load(Ordering::Relaxed));
            }
            if self.status_403.load(Ordering::Relaxed) > 0 {
                eprintln!("  403 Forbidden: {}", self.status_403.load(Ordering::Relaxed));
            }
            if self.status_404.load(Ordering::Relaxed) > 0 {
                eprintln!("  404 Not Found: {}", self.status_404.load(Ordering::Relaxed));
            }
            if self.status_422.load(Ordering::Relaxed) > 0 {
                eprintln!("  422 Unprocessable Entity: {}", self.status_422.load(Ordering::Relaxed));
            }
            if self.status_429.load(Ordering::Relaxed) > 0 {
                eprintln!("  429 Rate Limit Exceeded: {}", self.status_429.load(Ordering::Relaxed));
            }
            if self.status_500.load(Ordering::Relaxed) > 0 {
                eprintln!("  500 Internal Server Error: {}", self.status_500.load(Ordering::Relaxed));
            }
            if self.status_503.load(Ordering::Relaxed) > 0 {
                eprintln!("  503 Service Unavailable: {}", self.status_503.load(Ordering::Relaxed));
            }
            if self.other_errors.load(Ordering::Relaxed) > 0 {
                eprintln!("  Other Errors: {}", self.other_errors.load(Ordering::Relaxed));
            }
            eprintln!("  {} Total HTTP Errors: {}", "⚠️".red(), total.to_string().red().bold());
            eprintln!("{}\n", "─".repeat(80).yellow());
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

/// One HTTP client for the whole run: reuses the connection pool and TLS sessions.
static CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .user_agent(USER_AGENT)
        .pool_max_idle_per_host(50)
        .pool_idle_timeout(Duration::from_secs(120))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build HTTP client")
});

/// Longest we are willing to sleep for a single rate-limit window before giving up.
const MAX_RATE_LIMIT_WAIT_SECS: u64 = 120;
const MAX_RATE_LIMIT_RETRIES: u32 = 3;

/// Seconds to wait before retrying, taken from `Retry-After` or `X-RateLimit-Reset`.
fn rate_limit_wait(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let num = |name: &str| headers.get(name)?.to_str().ok()?.parse::<u64>().ok();
    if let Some(secs) = num("retry-after") {
        return Some(secs);
    }
    if num("x-ratelimit-remaining") == Some(0) {
        let reset = num("x-ratelimit-reset")?;
        return Some(reset.saturating_sub(Utc::now().timestamp() as u64) + 1);
    }
    None
}

/// GET `url` and return `(status, body)`.
/// Primary and secondary rate limits are handled here, once for every endpoint:
/// the request sleeps until the window resets (up to `MAX_RATE_LIMIT_WAIT_SECS`) and is retried.
pub async fn get_text(token: &str, url: &str, accept: Option<&str>) -> Result<(u16, String), GithubError> {
    let mut attempt = 0;
    loop {
        let mut req = CLIENT.get(url).bearer_auth(token);
        if let Some(a) = accept {
            req = req.header("Accept", a);
        }
        let resp = req.send().await.map_err(GithubError::RequestError)?;
        let status = resp.status().as_u16();
        let wait = rate_limit_wait(resp.headers());
        let body = resp.text().await.map_err(GithubError::RequestError)?;

        let limited = status == 429
            || (status == 403 && (wait.is_some() || body.to_lowercase().contains("rate limit")));
        if !limited {
            return Ok((status, body));
        }

        // shortcut: secondary limits without a header wait a flat 60s, upgrade if GitHub documents a better signal.
        let secs = wait.unwrap_or(60);
        if attempt >= MAX_RATE_LIMIT_RETRIES || secs > MAX_RATE_LIMIT_WAIT_SECS {
            return Err(GithubError::RateLimitExceeded(format!(
                "GitHub API rate limit exceeded (resets in {}s). Response: {}", secs, body
            )));
        }
        attempt += 1;
        eprintln!("{}", format!("⏳ Rate limited, waiting {}s (retry {}/{})...", secs, attempt, MAX_RATE_LIMIT_RETRIES).yellow());
        tokio::time::sleep(Duration::from_secs(secs)).await;
    }
}

/// GET and parse a JSON body; any non-2xx status becomes `InvalidResponse("Failed to get <what>: status N")`.
async fn get_json(token: &str, url: &str, accept: Option<&str>, what: &str) -> Result<Value, GithubError> {
    let (status, body) = get_text(token, url, accept).await?;
    if !(200..300).contains(&status) {
        return Err(GithubError::InvalidResponse(format!("Failed to get {}: status {}", what, status)));
    }
    serde_json::from_str(&body).map_err(|e| GithubError::InvalidResponse(format!("Failed to parse JSON: {}", e)))
}

/// Search GitHub code for a domain, with pagination.
/// Returns the unique repositories (every matching file is attached to its repository) and the error counter.
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

    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS));
    let error_counter = HttpErrorCounter::new();
    let mut repository_list: HashMap<String, Repository> = HashMap::new();
    let mut page = 1;
    let mut total_items_processed = 0;

    use indicatif::{ProgressBar, ProgressStyle};

    eprintln!("\n{}", "═".repeat(80).cyan());
    eprintln!("{}", format!("  🔍 Step 1/3: Searching GitHub for repositories matching '{}'", domain).bold().cyan());
    eprintln!("{}\n", "═".repeat(80).cyan());

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
                        .map(|e| e.trim().trim_start_matches('.').to_lowercase())
                        .filter(|e| !e.is_empty())
                        .map(|e| format!("extension:{}", e))
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

        let (status, body) = get_text(token, &url, None).await?;
        if !(200..300).contains(&status) {
            error_counter.record_error(status);
            match status {
                401 => {
                    eprintln!("[401] Unauthorized => most likely the token is invalid or expired");
                    return Err(GithubError::InvalidResponse("Unauthorized".to_string()));
                }
                403 => {
                    eprintln!("[403] Forbidden: {}", body);
                    return Err(GithubError::InvalidResponse(format!("Forbidden: {}", body)));
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
                    eprintln!("[{}] Error fetching page {}, continuing...", status, page);
                    page += 1;
                    // Search never serves more than MAX_TOTAL_RESULTS hits; do not loop forever on a persistent 5xx.
                    if page > MAX_TOTAL_RESULTS / PER_PAGE {
                        break;
                    }
                    continue;
                }
            }
        }

        let json: Value = serde_json::from_str(&body)
            .map_err(|e| GithubError::InvalidResponse(format!("Failed to parse JSON: {}", e)))?;

        let items = json["items"].as_array()
            .ok_or_else(|| GithubError::InvalidResponse("Missing 'items' array in response".to_string()))?;

        let total_count = json["total_count"].as_u64().unwrap_or(0);

        if page == 1 {
            search_pb.finish_with_message("✅ Found matches, processing...");
            let hits_to_process = total_count.min(MAX_TOTAL_RESULTS as u64);
            eprintln!("\n{}", "─".repeat(80).cyan());
            eprintln!("{}", format!("  🔄 Step 2/3: Fetching commit dates ({} matching files)...", hits_to_process).bold().cyan());
            eprintln!("{}\n", "─".repeat(80).cyan());

            let pb = ProgressBar::new(hits_to_process);
            pb.set_style(ProgressStyle::default_bar()
                .template("{spinner:.yellow} [{elapsed_precise}] [{bar:20.yellow/blue}] {pos:>4}/{len:4} ({percent:>3}%) {msg:.dim}")
                .unwrap()
                .progress_chars("█▉▊▋▌▍▎▏░"));
            pb.set_message("Fetching commit dates...");
            pb.enable_steady_tick(std::time::Duration::from_millis(100));
            commit_pb = Some(pb);
        }

        if page > 1 {
            search_pb.set_message("Fetching more pages...");
        }

        if items.is_empty() {
            break;
        }

        // Group hits by repository: every matching file of a repo is analysed, not just the first one.
        // `new_repos` holds repositories first seen on this page, with the commits URL needed for their dates.
        let mut new_repos: Vec<(Repository, String)> = Vec::new();
        for x in items {
            let missing = |field: &str| GithubError::InvalidResponse(format!("Missing {}", field));
            let file_url = x["html_url"].as_str().ok_or_else(|| missing("html_url"))?.to_string();
            let file_url_api = x["url"].as_str().ok_or_else(|| missing("url"))?.to_string();
            let repository_name = x["repository"]["name"].as_str().ok_or_else(|| missing("repository name"))?.to_string();
            let repository_url = x["repository"]["html_url"].as_str().ok_or_else(|| missing("repository html_url"))?.to_string();

            let existing = repository_list.get_mut(&repository_url)
                .or_else(|| new_repos.iter_mut().find(|(r, _)| r.repository_url == repository_url).map(|(r, _)| r));
            if let Some(repo) = existing {
                if !repo.files_urls.contains(&file_url) {
                    repo.files_names.push(clean_file_name(file_url.clone()));
                    repo.files_urls.push(file_url);
                    repo.files_urls_api.push(file_url_api);
                }
                continue;
            }

            if let Some(limit) = max_repos {
                if repository_list.len() + new_repos.len() >= limit {
                    continue;
                }
            }

            let commits_url = x["repository"]["commits_url"].as_str().ok_or_else(|| missing("commits_url"))?
                .replace("{/sha}", "");
            let author = x["repository"]["owner"]["html_url"].as_str().ok_or_else(|| missing("owner html_url"))?.to_string();

            new_repos.push((Repository {
                repository_url,
                repository_name,
                author,
                files_names: vec![clean_file_name(file_url.clone())],
                files_urls: vec![file_url],
                files_urls_api: vec![file_url_api],
                last_commit_date: Utc::now(),
                last_commit_date_file: Utc::now(),
            }, commits_url));
        }
        total_items_processed += items.len();
        if let Some(pb) = &commit_pb {
            pb.set_position(total_items_processed as u64);
        }

        // Fetch repo and file commit dates in parallel (dates are those of the first matching file)
        let date_futures = new_repos.iter().map(|(repo, commits_url)| {
            let semaphore = semaphore.clone();
            let counter = error_counter.clone();
            let file_path = extract_file_path(&repo.files_urls[0]);
            async move {
                let _permit = semaphore.acquire().await.unwrap();
                let (repo_date, file_date) = tokio::join!(
                    get_last_commit_date(token, commits_url.clone(), None),
                    get_last_commit_date(token, commits_url.clone(), Some(file_path.as_str())),
                );
                counter.record_github_error_opt(repo_date.as_ref().err());
                counter.record_github_error_opt(file_date.as_ref().err());
                (repo_date.unwrap_or_else(|_| Utc::now()), file_date.unwrap_or_else(|_| Utc::now()))
            }
        });
        let dates = futures::future::join_all(date_futures).await;
        for ((mut repo, _), (repo_date, file_date)) in new_repos.into_iter().zip(dates) {
            repo.last_commit_date = repo_date;
            repo.last_commit_date_file = file_date;
            repository_list.insert(repo.repository_url.clone(), repo);
        }

        if let Some(limit) = max_repos {
            if repository_list.len() >= limit {
                break;
            }
        }

        // Fewer than PER_PAGE results means this was the last page
        if items.len() < PER_PAGE {
            break;
        }

        if total_items_processed >= MAX_TOTAL_RESULTS {
            eprintln!("   ⚠️  Reached GitHub API limit ({} results)", MAX_TOTAL_RESULTS);
            break;
        }

        page += 1;
        search_pb.set_message("Fetching more pages...");
    }

    search_pb.finish_with_message("✅ Search complete!");
    if let Some(pb) = commit_pb {
        pb.finish_with_message("✅ Commit dates fetched!");
    }

    let file_count: usize = repository_list.values().map(|r| r.files_urls.len()).sum();
    eprintln!("\n{}", "─".repeat(80).cyan());
    eprintln!("{}", format!("  📊 Step 3/3: Found {} unique repositories ({} matching files)", repository_list.len(), file_count).bold().cyan());
    eprintln!("{}\n", "─".repeat(80).cyan());

    Ok((repository_list, error_counter))
}

/// Fetch a file through the contents API (raw media type: no JSON, no base64).
pub async fn get_raw_file_content_from_api(url: String, token: &str) -> Result<String, GithubError> {
    let (status, body) = get_text(token, &clean_url(&url), Some("application/vnd.github.raw")).await?;
    if !(200..300).contains(&status) {
        return Err(GithubError::InvalidResponse(format!("Failed to get raw file content: status {}", status)));
    }
    Ok(body)
}

/// Fetches the date of the last commit. If `path` is Some, requests commits for that file only (last commit that touched the file).
pub async fn get_last_commit_date(token: &str, mut url: String, path: Option<&str>) -> Result<DateTime<Utc>, GithubError> {
    if let Some(p) = path.filter(|p| !p.is_empty()) {
        url.push_str("?path=");
        url.push_str(&urlencoding::encode(p));
    }
    let json = get_json(token, &url, None, "commit date").await?;
    let commits = json.as_array()
        .ok_or_else(|| GithubError::InvalidResponse("Expected array of commits".to_string()))?;
    let date = commits.first()
        .ok_or_else(|| GithubError::InvalidResponse("No commits found".to_string()))?["commit"]["committer"]["date"]
        .as_str()
        .ok_or_else(|| GithubError::InvalidResponse("Missing commit date".to_string()))?;
    Ok(DateTime::parse_from_rfc3339(date)
        .map_err(|e| GithubError::InvalidResponse(format!("Failed to parse date: {}", e)))?
        .with_timezone(&Utc))
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

/// Get repository metadata (branch, description, topics, etc.) in a single request
pub async fn get_repo_metadata(token: &str, owner: &str, repo: &str) -> Result<RepoMetadata, GithubError> {
    let url = format!("https://api.github.com/repos/{}/{}", owner, repo);
    let json = get_json(token, &url, None, "repo info").await?;

    let default_branch = json["default_branch"]
        .as_str()
        .ok_or_else(|| GithubError::InvalidResponse("Missing default_branch".to_string()))?
        .to_string();

    // The repo object already carries its topics: no second request to /topics needed.
    let topics = json["topics"].as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    Ok(RepoMetadata {
        default_branch,
        description: json["description"].as_str().map(String::from),
        topics,
        owner: owner.to_string(),
        name: repo.to_string(),
        is_archived: json["archived"].as_bool().unwrap_or(false),
    })
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
    let url = format!("https://api.github.com/repos/{}/{}/git/trees/{}?recursive=1", owner, repo, branch);
    let json = get_json(token, &url, None, "repo tree").await?;

    let tree = json["tree"].as_array()
        .ok_or_else(|| GithubError::InvalidResponse("Missing tree array".to_string()))?;

    Ok(tree.iter()
        .filter(|item| item["type"].as_str() == Some("blob"))
        .filter_map(|item| Some((item["path"].as_str()?.to_string(), item["sha"].as_str()?.to_string())))
        .collect())
}

/// Get file content by SHA
pub async fn get_file_content_by_sha(token: &str, owner: &str, repo: &str, sha: &str) -> Result<String, GithubError> {
    let url = format!("https://api.github.com/repos/{}/{}/git/blobs/{}", owner, repo, sha);
    let json = get_json(token, &url, None, "file content").await?;

    let encoding = json["encoding"].as_str().unwrap_or("");
    let content = json["content"].as_str()
        .ok_or_else(|| GithubError::InvalidResponse("Missing content field".to_string()))?;

    if encoding == "base64" {
        let cleaned_content = content.replace('\n', "").replace('\r', "");
        let decoded = STANDARD.decode(&cleaned_content)
            .map_err(|e| GithubError::InvalidResponse(format!("Failed to decode base64: {}", e)))?;
        String::from_utf8(decoded)
            .map_err(|e| GithubError::InvalidResponse(format!("Failed to convert to UTF-8: {}", e)))
    } else {
        Ok(content.to_string())
    }
}

/// Language name -> bytes of code.
pub async fn get_repo_languages(token: &str, owner: &str, repo: &str) -> Result<HashMap<String, u64>, GithubError> {
    let url = format!("https://api.github.com/repos/{}/{}/languages", owner, repo);
    let json = get_json(token, &url, None, "languages").await?;
    serde_json::from_value(json).map_err(|e| GithubError::InvalidResponse(format!("Failed to parse languages: {}", e)))
}
