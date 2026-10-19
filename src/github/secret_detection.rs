use crate::token::TokenPool;
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

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    Medium,
    High,
}

impl Severity {
    fn of(secret_type: &str) -> Self {
        match secret_type {
            "Public Key" | "Stripe Publishable Key" | "SSH Host Key" | "Example Pattern" => Severity::Low,
            "XML Username" | "API Key" | "Google API Key" | "Access Token" => Severity::Medium,
            _ => Severity::High,
        }
    }

    fn label(self) -> colored::ColoredString {
        match self {
            Severity::Low => "LOW".blue(),
            Severity::Medium => "MEDIUM".yellow(),
            Severity::High => "HIGH".red().bold(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub variable: String,
    pub secret_type: String,
    pub severity: Severity,
    /// Masked unless `--show-secrets` is given.
    pub value: String,
    /// Link to the exact line on GitHub.
    pub url: String,
}

/// A match on a line that reads its value from the environment (`process.env`, `${VAR}`...): not a leaked secret.
#[derive(Debug, Clone, Serialize)]
pub struct EnvReference {
    pub file: String,
    pub line: usize,
    pub text: String,
}

#[derive(Default)]
pub struct ScanOutput {
    pub findings: Vec<Finding>,
    pub env_references: Vec<EnvReference>,
}

fn mask(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        "****".to_string()
    } else {
        format!("{}****{}", chars[..2].iter().collect::<String>(), chars[chars.len() - 2..].iter().collect::<String>())
    }
}

static ENV_PLACEHOLDER_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$\{\w+\}").unwrap());

/// Best-effort variable name for a match: the first capture group when it looks like an identifier,
/// otherwise inferred from the line.
fn extract_var_name(captures: &regex::Captures, secret_type: &str, line: &str, line_start: usize, match_start: usize) -> String {
    captures.get(1)
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
    })

}

/// Scan one file's content. Every match is reported (not just one per pattern) with its line number.
/// `url` is the GitHub link of the file; findings get `url#L<line>`.
pub fn scan_content(file: &str, url: &str, content: &str, reveal: bool) -> ScanOutput {
    let mut out = ScanOutput::default();
    for (pattern, secret_type) in PATTERNS.iter() {
        for captures in pattern.captures_iter(content) {
            let m = captures.get(0).unwrap();
            let match_start = m.start();
            let line_start = content[..match_start].rfind('\n').map_or(0, |i| i + 1);
            let line_end = content[match_start..].find('\n').map_or(content.len(), |i| match_start + i);
            let line = &content[line_start..line_end];
            let line_no = content.as_bytes()[..match_start].iter().filter(|&&b| b == b'\n').count() + 1;

            let reads_env = line.contains("os.environ")
                || line.contains("process.env")
                || line.contains("getenv()")
                || line.contains("os.getenv")
                || ENV_PLACEHOLDER_REGEX.is_match(line);
            if reads_env {
                if !out.env_references.iter().any(|r| r.line == line_no) {
                    out.env_references.push(EnvReference { file: file.to_string(), line: line_no, text: line.trim().to_string() });
                }
                continue;
            }

            // Never exclude lines that look like XML config (<add key="..." value="...">)
            let looks_like_xml_config = line.contains("<add") && line.contains("key=") && line.contains("value=");
            if !looks_like_xml_config && EXCLUSION_PATTERNS.iter().any(|exclusion| exclusion.is_match(line)) {
                continue;
            }
            if out.findings.iter().any(|f| f.line == line_no && f.secret_type == *secret_type) {
                continue;
            }

            let variable = extract_var_name(&captures, secret_type, line, line_start, match_start);
            // The secret is the last capture group that participated, or the whole match.
            let value = (1..captures.len()).rev().find_map(|i| captures.get(i)).unwrap_or(m).as_str();
            out.findings.push(Finding {
                file: file.to_string(),
                line: line_no,
                variable,
                secret_type: secret_type.to_string(),
                severity: Severity::of(secret_type),
                value: if reveal { value.to_string() } else { mask(value) },
                url: format!("{}#L{}", url, line_no),
            });
        }
    }
    out
}

/// Everything known about one scanned repository. Printed as a box, or serialized with `--json`.
#[derive(Default, Serialize)]
pub struct RepoReport {
    pub repository: String,
    /// "scanned", "skipped" or "error"
    pub status: &'static str,
    pub note: Option<String>,
    pub branch: Option<String>,
    pub description: Option<String>,
    pub topics: Vec<String>,
    pub languages: Vec<(String, f64)>,
    pub matched_files: Vec<String>,
    pub files_scanned: usize,
    pub skipped_files: usize,
    pub skipped_extensions: Vec<String>,
    pub last_commit_file: Option<chrono::DateTime<chrono::Utc>>,
    pub last_commit_repo: Option<chrono::DateTime<chrono::Utc>>,
    pub findings: Vec<Finding>,
    pub env_references: Vec<EnvReference>,
    pub stars: Option<u64>,
    pub forks: Option<u64>,
    pub license: Option<String>,
    pub is_fork: bool,
    pub archived: bool,
    pub pushed_at: Option<String>,
    /// Single-file mode: what happened to each file the search matched.
    pub files: Vec<FileEntry>,
}

#[derive(Serialize)]
pub struct FileEntry {
    pub path: String,
    pub url: String,
    /// "scanned", "skipped" (ignored extension) or "failed" (could not be fetched)
    pub status: &'static str,
    pub findings: usize,
}

impl RepoReport {
    fn apply_meta(&mut self, m: super::github::RepoMetadata) {
        self.branch = Some(m.default_branch);
        self.description = m.description.filter(|d| !d.is_empty());
        self.topics = m.topics;
        self.stars = Some(m.stars);
        self.forks = Some(m.forks);
        self.license = m.license.filter(|l| l != "NOASSERTION");
        self.is_fork = m.is_fork;
        self.archived = m.is_archived;
        self.pushed_at = m.pushed_at;
    }

    fn new(repo: &Repository) -> Self {
        RepoReport {
            repository: repo.repository_url.clone(),
            status: "scanned",
            last_commit_file: Some(repo.last_commit_date_file),
            last_commit_repo: Some(repo.last_commit_date),
            ..Default::default()
        }
    }

    fn fail(mut self, status: &'static str, note: impl Into<String>) -> Self {
        self.status = status;
        self.note = Some(note.into());
        self
    }

    /// Highest severity first, then by file and line.
    fn sort_findings(&mut self) {
        self.findings.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| (&a.file, a.line).cmp(&(&b.file, b.line))));
    }

    fn merge(&mut self, path: &str, outcome: FileOutcome) {
        match outcome {
            FileOutcome::Skipped => {
                self.skipped_files += 1;
                if let Some(ext) = path.rsplit('.').next() {
                    if !self.skipped_extensions.iter().any(|e| e == ext) {
                        self.skipped_extensions.push(ext.to_string());
                    }
                }
            }
            FileOutcome::Failed => {}
            FileOutcome::Scanned(out) => {
                self.files_scanned += 1;
                self.findings.extend(out.findings);
                self.env_references.extend(out.env_references);
            }
        }
    }
}

fn language_shares(langs: HashMap<String, u64>) -> Vec<(String, f64)> {
    let total: u64 = langs.values().sum();
    let mut shares: Vec<(String, f64)> = langs.into_iter()
        .map(|(l, b)| (l, b as f64 / total.max(1) as f64 * 100.0))
        .collect();
    shares.sort_by(|a, b| b.1.total_cmp(&a.1));
    shares
}

enum FileOutcome {
    Skipped,
    Failed,
    Scanned(ScanOutput),
}

/// Fetch (via `fetch`) and scan one file, bounded by `semaphore`. The regex work runs off the async runtime.
async fn process_file<F>(
    path: String,
    url: String,
    reveal: bool,
    semaphore: &tokio::sync::Semaphore,
    counter: &super::github::HttpErrorCounter,
    pb: Option<&indicatif::ProgressBar>,
    fetch: F,
) -> (String, FileOutcome)
where
    F: std::future::Future<Output = Result<String, super::github::GithubError>>,
{
    let outcome = if FILES_EXTENSIONS.iter().any(|ext| path.ends_with(ext)) {
        FileOutcome::Skipped
    } else {
        let _permit = semaphore.acquire().await.unwrap();
        match fetch.await {
            Ok(content) => {
                let p = path.clone();
                match tokio::task::spawn_blocking(move || scan_content(&p, &url, &content, reveal)).await {
                    Ok(out) => FileOutcome::Scanned(out),
                    Err(_) => FileOutcome::Failed,
                }
            }
            Err(e) => {
                counter.record_github_error_opt(Some(&e));
                FileOutcome::Failed
            }
        }
    };
    if let Some(pb) = pb {
        pb.inc(1);
    }
    (path, outcome)
}

/// Single-file mode: analyze only the files that matched the search.
async fn scan_single(repo: &Repository, token: &TokenPool, counter: &super::github::HttpErrorCounter, reveal: bool) -> RepoReport {
    use super::github::{extract_repo_info, get_repo_metadata, get_repo_languages, get_raw_file_content_from_api};
    const MAX_CONCURRENT_FILE_REQUESTS: usize = 30;

    let mut report = RepoReport::new(repo);
    report.matched_files = repo.files_urls.iter().map(|x| extract_file_path(x)).collect();

    let semaphore = tokio::sync::Semaphore::new(MAX_CONCURRENT_FILE_REQUESTS);
    let files = futures::future::join_all(repo.files_urls.iter().zip(&repo.files_urls_api).map(|(html, api)| {
        process_file(extract_file_path(html), clean_url(html), reveal, &semaphore, counter, None,
            get_raw_file_content_from_api(api.clone(), token))
    }));
    // Metadata and languages are decoration: a failure there must never hide findings.
    let meta = async {
        let (owner, name) = extract_repo_info(&repo.repository_url)?;
        let (m, l) = tokio::join!(get_repo_metadata(token, &owner, &name), get_repo_languages(token, &owner, &name));
        counter.record_github_error_opt(m.as_ref().err());
        counter.record_github_error_opt(l.as_ref().err());
        Some((m.ok(), l.ok()))
    };
    let (files, meta) = tokio::join!(files, meta);

    if let Some((m, l)) = meta {
        if let Some(m) = m { report.apply_meta(m); }
        report.languages = l.map(language_shares).unwrap_or_default();
    }
    for (path, outcome) in files {
        let url = repo.files_urls.iter().find(|u| extract_file_path(u) == path).map(|u| clean_url(u)).unwrap_or_default();
        let (status, findings) = match &outcome {
            FileOutcome::Scanned(o) => ("scanned", o.findings.len()),
            FileOutcome::Skipped => ("skipped", 0),
            FileOutcome::Failed => ("failed", 0),
        };
        report.files.push(FileEntry { path: path.clone(), url, status, findings });
        report.merge(&path, outcome);
    }
    report.sort_findings();
    report
}

/// Whole-repo mode: fetch the file tree and scan the config-like files.
async fn scan_whole(repo: &Repository, token: &TokenPool, domain: &str, counter: &super::github::HttpErrorCounter, max_files: Option<usize>, reveal: bool, mp: &indicatif::MultiProgress) -> RepoReport {
    use super::github::{extract_repo_info, get_repo_metadata, should_scan_repository, get_repo_files_recursive, get_file_content_by_sha};
    use indicatif::{ProgressBar, ProgressStyle};
    const MAX_CONCURRENT_FILE_REQUESTS: usize = 40;

    let report = RepoReport::new(repo);
    let Some((owner, name)) = extract_repo_info(&repo.repository_url) else {
        return report.fail("error", "Failed to parse repository URL");
    };
    let metadata = match get_repo_metadata(token, &owner, &name).await {
        Ok(m) => m,
        Err(e) => {
            counter.record_github_error_opt(Some(&e));
            return report.fail("error", format!("Failed to get repository metadata: {}", e));
        }
    };
    let (should_scan, reason) = should_scan_repository(&metadata, domain);
    if !should_scan {
        return report.fail("skipped", reason.unwrap_or_else(|| "Repository filtered out".to_string()));
    }
    let mut report = report;
    report.apply_meta(metadata.clone());

    let all_files = match get_repo_files_recursive(token, &owner, &name, &metadata.default_branch).await {
        Ok(files) => files,
        Err(e) => {
            counter.record_github_error_opt(Some(&e));
            return report.fail("error", format!("Failed to get repository files: {}", e));
        }
    };
    let mut files: Vec<(String, String)> = all_files.into_iter().filter(|(p, _)| is_interesting_config_file(p)).collect();
    let total_interesting = files.len();
    if let Some(limit) = max_files {
        files.truncate(limit);
    }
    if files.len() > MAX_FILES_PER_REPO {
        return report.fail("skipped", format!("{} interesting files (limit {}); use --max-files", total_interesting, MAX_FILES_PER_REPO));
    }
    if files.len() < total_interesting {
        report.note = Some(format!("--max-files applied: {} of {} interesting files scanned", files.len(), total_interesting));
    }

    let pb = mp.add(ProgressBar::new(files.len() as u64));
    pb.set_style(ProgressStyle::default_bar()
        .template("  {spinner:.magenta} [{bar:20.magenta/blue}] {pos}/{len} files {msg:.dim}")
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏░"));
        pb.enable_steady_tick(std::time::Duration::from_millis(100));

    let semaphore = tokio::sync::Semaphore::new(MAX_CONCURRENT_FILE_REQUESTS);
    let results = futures::future::join_all(files.iter().map(|(path, sha)| {
        let url = format!("https://github.com/{}/{}/blob/{}/{}", owner, name, metadata.default_branch, path);
        process_file(path.clone(), url, reveal, &semaphore, counter, Some(&pb), get_file_content_by_sha(token, &owner, &name, sha))
    })).await;
    pb.finish_and_clear();

    for (path, outcome) in results {
        report.merge(&path, outcome);
    }
    report.sort_findings();
    report
}

/// Terminal width clamped to a readable range (80 when stdout is not a terminal).
fn box_width() -> usize {
    let term = console::Term::stdout();
    if term.is_term() { (term.size().1 as usize).clamp(60, 110) } else { 80 }
}

/// Shorten `s` to `max` chars by cutting the middle, keeping both ends (file names matter).
fn middle(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max || max < 8 {
        return s.to_string();
    }
    let keep = max - 1;
    let tail = keep / 2;
    format!("{}…{}", chars[..keep - tail].iter().collect::<String>(), chars[chars.len() - tail..].iter().collect::<String>())
}

fn short_name(url: &str) -> &str {
    url.trim_start_matches("https://github.com/")
}

/// One-line result for repos that do not deserve a box.
fn print_line(r: &RepoReport) {
    match r.status {
        "skipped" => println!("{} {}  {}", "⏭ ".dimmed(), link(short_name(&r.repository), &r.repository).dimmed(), r.note.as_deref().unwrap_or("").dimmed()),
        "error" => println!("{} {}  {}", "✗".red().bold(), link(short_name(&r.repository), &r.repository), r.note.as_deref().unwrap_or("").red()),
        _ => {
            let stars = r.stars.map(|s| format!("★ {}  ", compact_count(s))).unwrap_or_default();
            println!("{} {}  {}{}", "✓".green(), link(short_name(&r.repository), &r.repository), stars.yellow(), format!("{} file(s) scanned, no secrets", r.files_scanned).dimmed());
            print_files(r);
        }
    }
}

/// Clickable terminal hyperlink (OSC 8); plain text when output is piped or colors are off.
fn link(text: &str, url: &str) -> String {
    if url.is_empty() || !console::Term::stdout().is_term() || !colored::control::SHOULD_COLORIZE.should_colorize() {
        return text.to_string();
    }
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

fn file_label(f: &FileEntry) -> String {
    match f.status {
        "skipped" => "skipped (extension ignored)".to_string(),
        "failed" => "could not be fetched".to_string(),
        _ if f.findings > 0 => format!("{} finding(s)", f.findings),
        _ => "clean".to_string(),
    }
}

/// Which files matched the domain and what became of each (single-file mode).
fn print_files(r: &RepoReport) {
    const SHOWN: usize = 5;
    for f in r.files.iter().take(SHOWN) {
        println!("    {} {}  {}", "↳".dimmed(), link(&f.path, &f.url), file_label(f).dimmed());
    }
    if r.files.len() > SHOWN {
        println!("    {}", format!("… +{} more matching file(s)", r.files.len() - SHOWN).dimmed());
    }
}

fn compact_count(n: u64) -> String {
    if n >= 1000 { format!("{:.1}k", n as f64 / 1000.0) } else { n.to_string() }
}

fn print_report(r: &RepoReport, index: usize, total: usize, w: usize, verbose: bool) {
    let row = |label: &str, value: &str| println!("{}", create_box_row(label, value, w - 2));
    let sep = || println!("{}", create_box_separator(w));

    println!("{}", create_box_header("Repository", index, total, w).cyan().bold());
    row("🔗 URL", &r.repository);
    sep();

    match (r.status, &r.note) {
        ("skipped", note) => row("⏭️  Skipped", note.as_deref().unwrap_or("")),
        ("error", note) => row("❌ Error", note.as_deref().unwrap_or("")),
        _ => {}
    }
    if r.status != "scanned" {
        println!("{}\n", create_box_footer(w));
        return;
    }

    // What matters first: the findings.
    if r.findings.is_empty() {
        row("✅ Status", "No secrets detected");
    } else {
        row("🚨 Secrets Found", &format!("{} potential secret(s)", r.findings.len()));
        for f in &r.findings {
            let what = if f.variable != "unknown" { format!("{}: {}", f.variable.bold().cyan(), f.secret_type) } else { f.secret_type.clone() };
            println!("│  [{}] {}:{}", f.severity.label(), link(&middle(&f.file, w.saturating_sub(18).max(20)), &f.url).bold().yellow(), f.line);
            println!("│         {}  {}", what, f.value.dimmed());
        }
    }
    if !r.env_references.is_empty() {
        row("ℹ️  Env-based", &format!("{} line(s) read env vars (not reported)", r.env_references.len()));
    }
    sep();

    // Context: a few rows by default, everything with -v.
    if let (Some(st), Some(fk)) = (r.stars, r.forks) {
        let mut tags = vec![format!("★ {}", compact_count(st)), format!("⑂ {}", compact_count(fk))];
        if let Some(l) = &r.license { tags.push(l.clone()); }
        if r.is_fork { tags.push("fork".into()); }
        if r.archived { tags.push("archived".into()); }
        row("⭐ Stats", &tags.join(" · "));
    }
    if !r.languages.is_empty() {
        let (shown, other): (Vec<_>, Vec<_>) = r.languages.iter().partition(|(_, p)| *p > 1.0);
        let mut parts: Vec<String> = shown.iter().map(|(l, p)| format!("{} {}", l.bold().cyan(), format!("{p:.1}%").yellow())).collect();
        if !other.is_empty() {
            parts.push(format!("{} {}", "Other".bold().cyan(), format!("{:.1}%", other.iter().map(|(_, p)| p).sum::<f64>()).yellow()));
        }
        row("🌐 Languages", &parts.join(" · "));
    }
    row("📊 Scanned", &format!("{} file(s)", r.files_scanned));
    if let Some(note) = &r.note { row("ℹ️  Note", note); }
    if let Some(c) = r.last_commit_repo { row("📆 Last commit", &c.format("%d/%m/%Y %H:%M UTC").to_string()); }

    if verbose {
        sep();
        if let Some(b) = &r.branch { row("🌿 Branch", b); }
        if let Some(d) = &r.description { row("📝 Description", d); }
        if !r.topics.is_empty() { row("🏷️  Topics", &r.topics.join(", ")); }
        if let Some(p) = &r.pushed_at { row("🕒 Last push", p.trim_end_matches('Z').replace('T', " ").as_str()); }
        if let Some(f) = r.last_commit_file { row("📆 Matched file changed", &f.format("%d/%m/%Y %H:%M UTC").to_string()); }
        if r.files.is_empty() {
            if !r.matched_files.is_empty() { row("📝 Files", &r.matched_files.join(", ")); }
        } else {
            for f in &r.files {
                println!("│ 📝 Matched file: {}  [{}]", link(&f.path, &f.url), file_label(f));
            }
        }
        if r.skipped_files > 0 {
            row("⏭️  Skipped", &format!("{} file(s) skipped ({})", r.skipped_files, r.skipped_extensions.join(", ")));
        }
    }
    println!("{}\n", create_box_footer(w));
}

#[derive(Clone, Copy, PartialEq)]
pub enum ScanMode {
    SingleFile,
    WholeRepo,
}

pub struct ScanOptions {
    pub mode: ScanMode,
    pub domain: String,
    pub sort_order: String,
    pub commit_date_kind: String,
    pub max_files: Option<usize>,
    pub quiet: bool,
    /// Print a full box for every repository, even clean ones.
    pub verbose: bool,
    pub show_secrets: bool,
    /// `Some(None)` = JSON to stdout, `Some(Some(path))` = JSON to file.
    pub json: Option<Option<std::path::PathBuf>>,
}

/// Scan every repository and report. Boxes (or JSON) go to stdout, headers and summaries to stderr.
/// Returns the total number of findings.
pub async fn run_scan(result: HashMap<String, Repository>, token: &TokenPool, error_counter: super::github::HttpErrorCounter, opts: ScanOptions) -> usize {
    let mut repos: Vec<Repository> = result.into_values().collect();
    let key = |r: &Repository| repo_sort_date(r, &opts.commit_date_kind);
    match opts.sort_order.as_str() {
        "asc" => repos.sort_by_key(|r| key(r)),
        _ => repos.sort_by_key(|r| std::cmp::Reverse(key(r))),
    }

    use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
    let total = repos.len();
    let w = box_width();
    let mode_name = if opts.mode == ScanMode::SingleFile { "single-file" } else { "whole-repo" };
    eprintln!("{}", format!("🔍 Scanning {} repositories ({}), {} commit date {}",
        total, mode_name, opts.commit_date_kind, if opts.sort_order == "asc" { "oldest first" } else { "newest first" }).bold().cyan());

    // One live status line at the bottom; finished repos are printed above it.
    let mp = MultiProgress::new();
    let status = mp.add(ProgressBar::new(total as u64));
    status.set_style(ProgressStyle::default_bar()
        .template("{spinner:.cyan} [{pos}/{len}] {msg}")
        .unwrap());
    status.enable_steady_tick(std::time::Duration::from_millis(100));
    let human = opts.json.is_none();

    let mut reports: Vec<RepoReport> = Vec::new();
    let mut findings_so_far = 0;
    for (i, repo) in repos.iter().enumerate() {
        status.set_message(format!("{}  {}", short_name(&repo.repository_url).bold(), format!("{} finding(s) so far", findings_so_far).dimmed()));
        let report = match opts.mode {
            ScanMode::SingleFile => scan_single(repo, token, &error_counter, opts.show_secrets).await,
            ScanMode::WholeRepo => scan_whole(repo, token, &opts.domain, &error_counter, opts.max_files, opts.show_secrets, &mp).await,
        };
        status.inc(1);
        findings_so_far += report.findings.len();
        let hidden = opts.quiet && report.findings.is_empty() && report.status != "error";
        if human && !hidden {
            mp.suspend(|| {
                if opts.verbose || !report.findings.is_empty() {
                    print_report(&report, i + 1, total, w, opts.verbose);
                } else {
                    print_line(&report);
                }
            });
        }
        if !hidden {
            reports.push(report);
        }
    }
    status.finish_and_clear();

    if let Some(dest) = &opts.json {
        write_json(&reports, dest.as_deref());
    }

    let findings: usize = reports.iter().map(|r| r.findings.len()).sum();
    if human {
        print_findings_summary(&reports, w);
    }
    let with_findings = reports.iter().filter(|r| !r.findings.is_empty()).count();
    let errors = reports.iter().filter(|r| r.status == "error").count();
    let skipped = reports.iter().filter(|r| r.status == "skipped").count();
    let summary = format!("✨ {} repositories: {} with findings ({} total), {} skipped, {} errors",
        total, with_findings, findings, skipped, errors);
    eprintln!("{}", if findings > 0 { summary.bold().red() } else { summary.bold().green() });
    error_counter.print_summary();
    findings
}

/// Per-repo ranking for runs where findings are spread over several repositories (the boxes have scrolled by).
/// With a single affected repo its box already says everything, so nothing is printed.
fn print_findings_summary(reports: &[RepoReport], w: usize) {
    let mut hit: Vec<&RepoReport> = reports.iter().filter(|r| !r.findings.is_empty()).collect();
    if hit.len() < 2 {
        return;
    }
    let top = |r: &RepoReport| r.findings.iter().map(|f| f.severity).max();
    hit.sort_by(|a, b| top(b).cmp(&top(a)).then(b.findings.len().cmp(&a.findings.len())));
    println!("{}", "═".repeat(w).cyan());
    println!("{}", format!("  🚨 {} finding(s) in {} repositories, highest severity first", hit.iter().map(|r| r.findings.len()).sum::<usize>(), hit.len()).bold().red());
    println!("{}", "═".repeat(w).cyan());
    for r in hit {
        let count = |sev| r.findings.iter().filter(|f| f.severity == sev).count();
        let counts: Vec<String> = [(Severity::High, "high"), (Severity::Medium, "medium"), (Severity::Low, "low")].iter()
            .filter(|(s, _)| count(*s) > 0).map(|(s, n)| format!("{} {}", count(*s), n)).collect();
        println!("[{:^6}] {}  {}", top(r).map(|s| s.label()).unwrap_or_default(), link(short_name(&r.repository), &r.repository).bold(), counts.join(" · ").dimmed());
    }
    println!();
}

/// Write a JSON payload to stdout (`None` or `-`) or to a file.
fn write_json<T: Serialize>(payload: &T, dest: Option<&Path>) {
    let payload = serde_json::to_string_pretty(payload).expect("serialize to JSON");
    match dest {
        None => println!("{}", payload),
        Some(path) if path == Path::new("-") => println!("{}", payload),
        Some(path) => {
            let mut f = std::fs::File::create(path).expect("create JSON output file");
            f.write_all(payload.as_bytes()).expect("write JSON to file");
        }
    }
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
        write_json(&list, json_dest.as_deref());
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_reports_every_match_with_line_and_masks_value() {
        let content = "a = 1\naws_access_key_id = \"AKIAIOSFODNN7REALKEY\";\naws_secret_access_key = \"AKIAIOSFODNN7OTHERKY\";\nDB_PASSWORD = process.env.DB_PASSWORD\n";
        let out = scan_content("c.env", "https://github.com/o/r/blob/main/c.env", content, false);
        let aws: Vec<_> = out.findings.iter().filter(|f| f.secret_type == "AWS Access Key").collect();
        assert_eq!(aws.len(), 2, "{:?}", out.findings);
        assert_eq!((aws[0].line, aws[1].line), (2, 3));
        assert!(aws[0].value.starts_with("AK****") && !aws[0].value.contains("REAL"));
        assert!(aws[0].url.ends_with("c.env#L2"));
        assert!(out.findings.iter().all(|f| f.line != 4));
    }

    #[test]
    fn env_reads_are_not_findings_and_do_not_hide_other_lines() {
        let content = "DB_PASSWORD = process.env.DB_PASSWORD\nSTRIPE_API_KEY = \"abcdef1234567890abcdef\";\n";
        let out = scan_content("a.js", "u", content, true);
        assert!(out.env_references.iter().any(|r| r.line == 1));
        assert!(out.findings.iter().all(|f| f.line != 1));
        assert!(out.findings.iter().any(|f| f.line == 2 && f.value == "abcdef1234567890abcdef"));
    }

    #[test]
    fn mask_hides_short_and_long_values() {
        assert_eq!(mask("short"), "****");
        assert_eq!(mask("abcdefghijkl"), "ab****kl");
    }

    fn finding(file: &str, line: usize, t: &str, sev: Severity, v: &str) -> Finding {
        Finding { file: file.into(), line, variable: "apiKey".into(), secret_type: t.into(), severity: sev, value: v.into(), url: String::new() }
    }

    /// Visual check of the live output: `cargo test render_demo -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn render_demo() {
        colored::control::set_override(false);
        let base = |url: &str| RepoReport { repository: url.into(), status: "scanned", files_scanned: 12, ..Default::default() };
        let mut hit = base("https://github.com/acme/billing-service");
        hit.branch = Some("main".into());
        hit.languages = vec![("Rust".into(), 80.0), ("Shell".into(), 19.5), ("Make".into(), 0.5)];
        hit.matched_files = vec!["deploy/prod/config/application-production.properties".into()];
        hit.findings = vec![
            finding("deploy/prod/config/very/long/nested/path/application-production.properties", 14, "AWS Access Key", Severity::High, "AK****LE"),
            finding("src/settings.json", 3, "Public Key", Severity::Low, "pk****xx"),
        ];
        hit.env_references = vec![EnvReference { file: "a.js".into(), line: 2, text: "pw = process.env.PW".into() }];
        let clean = base("https://github.com/acme/website");
        let mut skipped = base("https://github.com/acme/docs");
        skipped.status = "skipped";
        skipped.note = Some("documentation repository".into());
        let mut failed = base("https://github.com/acme/gone");
        failed.status = "error";
        failed.note = Some("Failed to get repository files: status 404".into());

        let w = 80;
        print_line(&clean);
        print_line(&skipped);
        print_line(&failed);
        print_report(&hit, 4, 4, w, false);
        print_findings_summary(&[hit, clean], w);
    }
}
