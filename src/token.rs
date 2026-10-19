use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use colored::Colorize;

#[derive(Debug)]
pub enum TokenError {
    NotFound,
    IoError(std::io::Error),
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenError::NotFound => write!(f, "{}", "❌ No GitHub token found: set GITHUB_TOKENS / GITHUB_TOKEN or put one token per line in ./token.txt".red()),
            TokenError::IoError(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for TokenError {}

/// Split on commas, whitespace and newlines; drop empty entries, `#` comment lines and duplicates.
fn parse_tokens(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in raw.lines().filter(|l| !l.trim_start().starts_with('#')) {
        for t in line.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()) {
            if !out.iter().any(|o| o == t) {
                out.push(t.to_string());
            }
        }
    }
    out
}

/// Returns every configured token and a description of where they came from (never the tokens themselves).
/// First source that yields a token wins: `GITHUB_TOKENS`, `GITHUB_TOKEN`, then `./token.txt`.
pub fn get_tokens() -> Result<(Vec<String>, &'static str), TokenError> {
    for (var, source) in [("GITHUB_TOKENS", "GITHUB_TOKENS environment variable"), ("GITHUB_TOKEN", "GITHUB_TOKEN environment variable")] {
        if let Ok(raw) = std::env::var(var) {
            let tokens = parse_tokens(&raw);
            if !tokens.is_empty() {
                return Ok((tokens, source));
            }
        }
    }
    match std::fs::read_to_string("token.txt") {
        Ok(raw) => match parse_tokens(&raw) {
            t if t.is_empty() => Err(TokenError::NotFound),
            t => Ok((t, "./token.txt")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(TokenError::NotFound),
        Err(e) => Err(TokenError::IoError(e)),
    }
}

/// Safe-to-print hint of a token: its kind and last 4 characters.
pub fn describe_token(token: &str) -> String {
    let kind = if token.starts_with("github_pat_") { "fine-grained PAT" }
        else if token.starts_with("ghp_") { "classic PAT" }
        else if token.starts_with("gho_") || token.starts_with("ghu_") || token.starts_with("ghs_") { "app/OAuth token" }
        else { "token" };
    let tail: String = token.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{}, ends in …{}", kind, tail)
}

/// GitHub rate-limits search separately from everything else, so each token is tracked per bucket.
#[derive(Clone, Copy)]
pub enum Bucket {
    Core = 0,
    Search = 1,
}

pub enum Pick {
    Ready(usize, String),
    /// Every token is limited in this bucket; the soonest one frees up after this long.
    AllLimited(Duration),
}

/// Tokens used in rotation: the current one is kept until it is rate limited, then the next free one takes over.
pub struct TokenPool {
    tokens: Vec<String>,
    /// `blocked_until[token][bucket]`
    blocked_until: Mutex<Vec<[Option<Instant>; 2]>>,
}

impl TokenPool {
    pub fn new(tokens: Vec<String>) -> Self {
        let blocked_until = Mutex::new(vec![[None, None]; tokens.len()]);
        TokenPool { tokens, blocked_until }
    }

    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    pub fn pick(&self, bucket: Bucket) -> Pick {
        let now = Instant::now();
        let blocked = self.blocked_until.lock().unwrap();
        let free = |i: usize| blocked[i][bucket as usize].map_or(true, |t| t <= now);
        if let Some(i) = (0..self.tokens.len()).find(|&i| free(i)) {
            return Pick::Ready(i, self.tokens[i].clone());
        }
        let soonest = blocked.iter().filter_map(|b| b[bucket as usize]).min().unwrap_or(now);
        Pick::AllLimited(soonest.saturating_duration_since(now))
    }

    /// Mark a token limited for `wait`. Returns true the first time (so callers log the switch once).
    pub fn mark_limited(&self, index: usize, bucket: Bucket, wait: Duration) -> bool {
        let now = Instant::now();
        let mut blocked = self.blocked_until.lock().unwrap();
        let slot = &mut blocked[index][bucket as usize];
        let was_free = slot.map_or(true, |t| t <= now);
        *slot = Some(slot.map_or(now + wait, |t| t.max(now + wait)));
        was_free
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commas_lines_comments_and_duplicates() {
        assert_eq!(parse_tokens("a, b\n# c\nb\n  d \n"), vec!["a", "b", "d"]);
    }

    #[test]
    fn rotates_when_limited_and_reports_wait_when_all_limited() {
        let pool = TokenPool::new(vec!["a".into(), "b".into()]);
        assert!(matches!(pool.pick(Bucket::Core), Pick::Ready(0, _)));
        assert!(pool.mark_limited(0, Bucket::Core, Duration::from_secs(30)));
        assert!(!pool.mark_limited(0, Bucket::Core, Duration::from_secs(30)));
        assert!(matches!(pool.pick(Bucket::Core), Pick::Ready(1, _)));
        // search is a separate bucket: token 0 is still free there
        assert!(matches!(pool.pick(Bucket::Search), Pick::Ready(0, _)));
        pool.mark_limited(1, Bucket::Core, Duration::from_secs(10));
        match pool.pick(Bucket::Core) {
            Pick::AllLimited(d) => assert!(d <= Duration::from_secs(10)),
            _ => panic!("expected all limited"),
        }
    }
}
