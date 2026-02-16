use super::Repository;
use super::github::get_raw_file_content_from_api;
use std::collections::HashMap;
use colored::Colorize;
use super::utils::{create_box_header, create_box_footer, create_box_separator, create_box_row, clean_url, extract_file_path};
use regex::Regex;
use once_cell::sync::Lazy;

const FILES_EXTENSIONS: &[&str] = &[".html", ".csv", ".md"];

static PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r#"(?i)"apikey[^"]*"\s*:\s*"[^"]*""#).unwrap(),
        Regex::new(r#"(?i)"token[^"]*"\s*:\s*"[^"]*""#).unwrap(),
        Regex::new(r#"(?i)"secret[^"]*"\s*:\s*"[^"]*""#).unwrap(),
        Regex::new(r#"(?i)publickey\s*:\s*\S+"#).unwrap(),
        Regex::new(r#"(?i)privatekey\s*:\s*\S+"#).unwrap(),
        Regex::new(r#"(?i)\b\w+_public_key\s*=\s*'[^']*'"#).unwrap(),
        Regex::new(r#"(?i)\b\w+_private_key\s*=\s*'[^']*'"#).unwrap(),
        Regex::new(r#"(?i)publicapikey\s*=\s*[^;]+;"#).unwrap(),
        Regex::new(r#"(?i)privateapikey\s*=\s*[^;]+;"#).unwrap(),
    ]
});



pub async fn single_file_detection_with_regex(result: HashMap<String, Repository>, token: &str) {
    const BOX_WIDTH: usize = 80;
    let mut index = 1;
    let total_repos = result.len();
    
    println!("\n{}", "═".repeat(BOX_WIDTH).cyan());
    println!("{}", format!("  🔍 Found {} Repository(ies)", total_repos).bold().cyan());
    println!("{}\n", "═".repeat(BOX_WIDTH).cyan());
    
    for (_, repo) in result {
        let repo_url = repo.repository_url.clone();
        let mut skipped_files = 0;
        let mut skipped_files_extensions: Vec<String> = Vec::new();
        let mut secrets_found: Vec<(String, String)> = Vec::new();
        
        // Box header
        println!("{}", create_box_header("Repository", index, total_repos, BOX_WIDTH).cyan().bold());
        
        // Repository URL
        println!("{}", create_box_row("🔗 URL", &repo_url, BOX_WIDTH - 2));
        println!("{}", create_box_separator(BOX_WIDTH));
        
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
        
        // Analyze files for secrets
        for url in repo.files_urls_api {
            let url_clean = clean_url(&url);
            if !FILES_EXTENSIONS.iter().any(|ext| url_clean.ends_with(ext)) {
                match get_raw_file_content_from_api(url.clone(), token).await {
                    Ok(content) => {
                        for pattern in PATTERNS.iter() {
                            if pattern.is_match(content.as_str()) {
                                let pattern_desc = pattern.as_str();
                                secrets_found.push((url_clean.clone(), pattern_desc.to_string()));
                            }
                        }
                    }
                    Err(_) => {
                        // Silently skip errors for cleaner output
                    }
                }
            } else {
                skipped_files += 1;
                if let Some(ext) = url_clean.split('.').last() {
                    skipped_files_extensions.push(ext.to_string());
                }
            }
        }
        
        // Secrets found
        if !secrets_found.is_empty() {
            println!("{}", create_box_row("🚨 Secrets Found", &format!("{} potential secret(s)", secrets_found.len()), BOX_WIDTH - 2));
            for (file_url, pattern) in &secrets_found {
                let file_path = extract_file_path(file_url);
                println!("│   {} {} {}", "⚠️".red(), file_path.bold().yellow(), format!("({})", pattern).dimmed());
            }
            println!("{}", create_box_separator(BOX_WIDTH));
        } else {
            println!("{}", create_box_row("✅ Status", "No secrets detected", BOX_WIDTH - 2));
            println!("{}", create_box_separator(BOX_WIDTH));
        }
        
        // Last commit date
        let commit_date = repo.last_commit_date.format("%d/%m/%Y %H:%M:%S UTC").to_string();
        println!("{}", create_box_row("📆 Last Commit", &commit_date, BOX_WIDTH - 2));
        
        // Skipped files info
        if skipped_files > 0 {
            println!("{}", create_box_separator(BOX_WIDTH));
            let skipped_info = format!("{} file(s) skipped ({})", skipped_files, skipped_files_extensions.join(", "));
            println!("{}", create_box_row("⏭️  Skipped", &skipped_info, BOX_WIDTH - 2));
        }
        
        // Box footer
        println!("{}\n", create_box_footer(BOX_WIDTH));
        
        index += 1;
    }
    
    println!("{}", "═".repeat(BOX_WIDTH).cyan());
    println!("{}", "  ✨ Analysis Complete".bold().green());
    println!("{}\n", "═".repeat(BOX_WIDTH).cyan());
}

#[allow(dead_code)]
pub async fn whole_repo_detection(_result: HashMap<String, Repository>, _token: &str) {
    const BOX_WIDTH: usize = 80;
    println!("\n{}", create_box_header("⚠️  Notice", 1, 1, BOX_WIDTH).yellow().bold());
    println!("{}", create_box_row("Status", "Whole repository detection is not yet implemented", BOX_WIDTH - 2));
    println!("{}\n", create_box_footer(BOX_WIDTH));
}

