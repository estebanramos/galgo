use regex::Regex;
use colored::Colorize;

#[allow(dead_code)]
pub fn extract_urls(raw_content: &str, domain: &str) -> Vec<String>{
    let pattern = format!(
        r"https?://(?:([0-9a-z\-\.]+)\.)?{}",
        regex::escape(domain)
    );
    let domain_regexp = Regex::new(&pattern).unwrap();
    let matches: Vec<String> = domain_regexp.find_iter(raw_content).map(|x| x.as_str().to_string()).collect();
    // Remove duplicates
    let mut unique_matches: Vec<String> = Vec::new();
    for item in matches {
        if !unique_matches.contains(&item) {
            unique_matches.push(item);
        }
    }
    return unique_matches;
}

#[allow(dead_code)]
pub fn extract_subdomains(raw_content: &str, domain: &str) -> Vec<String>{
    let pattern = format!(
        r"https?://(?:([0-9a-z\-\.]+)\.)?{}",
        regex::escape(domain)
    );
    let domain_regexp = Regex::new(&pattern).unwrap();
    let matches: Vec<String> = domain_regexp.find_iter(raw_content).map(|x| x.as_str().to_string()).collect();
    // Remove duplicates
    let mut unique_matches: Vec<String> = Vec::new();
    for item in matches {
        let raw_item = item.replace("https://", "").replace("http://", "");
        if !unique_matches.contains(&raw_item) {
            unique_matches.push(raw_item);
        }
    }
    return unique_matches;
}

/// Clean URL by removing query parameters and quotes
pub fn clean_url(url: &str) -> String {
    url.split('?').next().unwrap_or(url)
        .replace("\"", "")
        .trim()
        .to_string()
}

/// Extract just the filename from a URL
pub fn clean_file_name(url: String) -> String {
    let cleaned = clean_url(&url);
    cleaned.split("/").last().unwrap_or(&cleaned).to_string()
}

/// Extract the full file path from a GitHub URL
/// Example: https://github.com/user/repo/blob/branch/path/to/file.ext -> path/to/file.ext
pub fn extract_file_path(url: &str) -> String {
    let cleaned = clean_url(url);
    
    // Handle GitHub blob URLs: https://github.com/user/repo/blob/branch/path/to/file
    if cleaned.contains("/blob/") {
        if let Some(blob_part) = cleaned.split("/blob/").nth(1) {
            // Remove the branch/commit part (everything before the first / after blob/)
            if let Some(path) = blob_part.splitn(2, '/').nth(1) {
                return path.to_string();
            }
        }
    }
    
    // Handle raw GitHub URLs: https://raw.githubusercontent.com/user/repo/branch/path/to/file
    if cleaned.contains("raw.githubusercontent.com/") {
        if let Some(raw_part) = cleaned.split("raw.githubusercontent.com/").nth(1) {
            // Skip user/repo/branch and get the path
            let parts: Vec<&str> = raw_part.split('/').collect();
            if parts.len() > 3 {
                return parts[3..].join("/");
            }
        }
    }
    
    // Fallback: return the last part of the URL
    cleaned.split("/").last().unwrap_or(&cleaned).to_string()
}

/// Format text to fit within a specific width, wrapping if necessary
pub fn format_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_line = String::new();
    
    for word in text.split_whitespace() {
        if current_line.len() + word.len() + 1 <= width {
            if !current_line.is_empty() {
                current_line.push(' ');
            }
            current_line.push_str(word);
        } else {
            if !current_line.is_empty() {
                lines.push(current_line.clone());
                current_line.clear();
            }
            // If a single word is longer than width, truncate it
            if word.len() > width {
                lines.push(word.chars().take(width - 3).collect::<String>() + "...");
            } else {
                current_line = word.to_string();
            }
        }
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Create a boxed table row
pub fn create_box_row(label: &str, value: &str, width: usize) -> String {
    let label_formatted = format!("{}:", label);
    let label_width = label.chars().count() + 1; // +1 for the colon
    let value_width = width.saturating_sub(label_width + 3); // 3 for "│ " and one space
    let value_lines = format_text(value, value_width.max(1));
    
    if value_lines.len() == 1 {
        format!("│ {} {}", label_formatted.bold(), value_lines[0])
    } else {
        let mut result = format!("│ {} {}", label_formatted.bold(), value_lines[0]);
        for line in value_lines.iter().skip(1) {
            result.push_str(&format!("\n│ {} {}", " ".repeat(label_width + 1), line));
        }
        result
    }
}

/// Create a box header
pub fn create_box_header(title: &str, index: usize, total: usize, width: usize) -> String {
    let header_text = format!(" {} [{}/{}] ", title, index, total);
    let padding = width.saturating_sub(header_text.len());
    format!("┌{}{}┐", header_text, "─".repeat(padding.max(0)))
}

/// Create box footer
pub fn create_box_footer(width: usize) -> String {
    format!("└{}┘", "─".repeat(width))
}

/// Create box separator
pub fn create_box_separator(width: usize) -> String {
    format!("├{}┤", "─".repeat(width))
}