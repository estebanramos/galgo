use std::path::Path;

/// Rules from a `.galgoignore` file. One rule per line, `#` starts a comment:
///   owner/repo        skip that repository (case-insensitive); a trailing `*` matches a prefix (`owner/*`)
///   path:<text>       drop findings whose file path contains <text>
///   rule:<Type>       drop findings of that secret type (e.g. `rule:Public Key`)
#[derive(Default)]
pub struct IgnoreList {
    repos: Vec<String>,
    paths: Vec<String>,
    rules: Vec<String>,
}

impl IgnoreList {
    pub fn parse(text: &str) -> Self {
        let mut list = IgnoreList::default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            if let Some(p) = line.strip_prefix("path:") {
                list.paths.push(p.trim().to_string());
            } else if let Some(r) = line.strip_prefix("rule:") {
                list.rules.push(r.trim().to_lowercase());
            } else {
                list.repos.push(line.to_lowercase());
            }
        }
        list
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        std::fs::read_to_string(path).map(|t| Self::parse(&t)).map_err(|e| format!("cannot read {}: {}", path.display(), e))
    }

    pub fn len(&self) -> usize {
        self.repos.len() + self.paths.len() + self.rules.len()
    }

    /// `name` is `owner/repo`.
    pub fn ignores_repo(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        self.repos.iter().any(|r| match r.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => *r == name,
        })
    }

    pub fn ignores_finding(&self, file: &str, secret_type: &str) -> bool {
        self.paths.iter().any(|p| file.contains(p.as_str())) || self.rules.contains(&secret_type.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repo_path_and_rule_lines() {
        let l = IgnoreList::parse("# comment\nAcme/Docs\nacme-labs/*\npath:vendor/\nrule: Public Key\n\n");
        assert_eq!(l.len(), 4);
        assert!(l.ignores_repo("acme/docs") && l.ignores_repo("ACME-LABS/anything") && !l.ignores_repo("acme/app"));
        assert!(l.ignores_finding("src/vendor/x.js", "Password") && l.ignores_finding("a.txt", "public key"));
        assert!(!l.ignores_finding("src/a.js", "Password"));
    }
}
