use reqwest::{Client, header::HeaderMap};
use serde_json::Value;
use std::{fmt, time::Duration};
use std::collections::HashMap;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use super::utils::{clean_file_name, clean_url};
use chrono::{DateTime, Utc};

const USER_AGENT: &str = "Galgo App";

pub struct Repository {
    pub repository_url: String,
    pub repository_name: String,
    pub author: String,
    pub files_names: Vec<String>,
    pub files_urls: Vec<String>,
    pub files_urls_api: Vec<String>,
    pub last_commit_date: DateTime<Utc>
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
}

impl std::error::Error for GithubError {}

impl fmt::Display for GithubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GithubError::RequestError(e) => write!(f, "Request Error: {}", e),
            GithubError::InvalidResponse(e) => write!(f, "Invalid Response: {}", e)
        }
    }
}

fn create_github_client() -> Result<Client, GithubError> {
    let mut headers = HeaderMap::new();
    headers.insert("User-Agent", USER_AGENT.parse().map_err(|e| {
        GithubError::InvalidResponse(format!("Invalid User-Agent header: {}", e))
    })?);
    Client::builder()
        .pool_max_idle_per_host(20)  // Más conexiones para GitHub API
        .pool_idle_timeout(Duration::from_secs(120))
        .default_headers(headers)
        .build()
        .map_err(GithubError::RequestError)
}

/// Search GitHub API for a given domain
/// Returns a vector of raw JSON items from the GitHub API
// Searchs /code api for a given domain
pub async fn search(token: &str, domain: &str) -> Result<HashMap<String, Repository>, GithubError> {
    // Github Public API 
    let url = format!("https://api.github.com/search/code?per_page=100&s=indexed&type=Code&o=desc&q={}&page=1", domain);
    let client = create_github_client()?;
    println!("[1/3] 🔍 Searching for repositories...");
    let response = client.get(&url)
        .header("User-Agent", "Galgo App")
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(GithubError::RequestError)?;
    
    if !response.status().is_success() {
        match response.status().as_u16() {
            401 => {
                eprintln!("[401] Unauthorized => most likely the token is invalid or expired");
            }
            _ => {
                return Err(GithubError::InvalidResponse(format!("Failed to get repositories: {}", response.status())));
            }
        }
    }

    let body = response.text().await.map_err(GithubError::RequestError)?;
    let json: Value = serde_json::from_str(&body)
        .map_err(|e| GithubError::InvalidResponse(format!("Failed to parse JSON: {}", e)))?;
    let items = json["items"].as_array()
        .ok_or_else(|| GithubError::InvalidResponse("Missing 'items' array in response".to_string()))?;
    let pb = ProgressBar::new(items.len() as u64);
    pb.set_style(ProgressStyle::default_bar()
         .template("[{elapsed_precise}] {bar:40.cyan/grey} {pos:>7}/{len:7} {msg}")
         .unwrap()
         .progress_chars("█░"));
    println!("[2/3] 🔄 Search returned {} results", items.len());
    
    let mut repository_list: HashMap<String, Repository> = HashMap::new();

    for (i, x) in items.iter().enumerate() {
        pb.set_message(format!("Processing results {}/{}", i + 1, items.len()));
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
        let commits_url = x["repository"]["commits_url"].as_str()
            .ok_or_else(|| GithubError::InvalidResponse("Missing commits_url".to_string()))?
            .to_string()
            .replace("{/sha}", "");
        let last_commit_date = get_last_commit_date(token, commits_url).await?;

        let author = x["repository"]["owner"]["html_url"].as_str()
            .ok_or_else(|| GithubError::InvalidResponse("Missing owner html_url".to_string()))?
            .to_string();
        
        if let Some(repo) = repository_list.get_mut(&repository_url) {
            repo.files_urls.push(file_url);
            repo.files_names.push(clean_file);
            repo.files_urls_api.push(file_url_api);
        } else {
            let repository = Repository {
                repository_url: repository_url.clone(),
                repository_name: repository_name.clone(),
                author,
                files_urls: vec![file_url],
                files_urls_api: vec![file_url_api],
                files_names: vec![clean_file],
                last_commit_date
            };  
            repository_list.insert(repository_url, repository);
        }
        pb.inc(1);
    }
    pb.finish_with_message("Done!");
    println!("[3/3] 📊 Found {} unique repositories matching the domain: {}", repository_list.keys().len().to_string().yellow(), domain.green());
    Ok(repository_list)
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

pub async fn get_last_commit_date(token: &str, url: String) -> Result<DateTime<Utc>, GithubError> {
    let client = Client::new();
    let response = client.get(url)
        .header("User-Agent", "Galgo App")
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(GithubError::RequestError)?;
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