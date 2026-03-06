<p align="center">
  <img src="assets/logo.png" alt="Galgo Logo" width="180"/>
</p>

# Galgo

**Find exposed secrets and sensitive data in GitHub repos—by domain.**  
A fast, Rust-based scanner that searches public code for your domain, then analyzes matching files (or whole repos) for API keys, tokens, passwords, and other credentials.

---

## Why Galgo?

- **Domain-first**: "Where does `api.mycompany.com` or `mycompany.com` appear in code?" — then scan those hits for secrets.
- **Three modes**: **Single-file** (only the files that matched search), **whole-repo** (scan likely config/secret files in each repo), or **list** (just enumerate repos/files, optionally as JSON).
- **Smart filtering**: Skips docs/demos/tutorials by repo name and topics; focuses on config-like files in whole-repo mode.
- **Clear output**: Colored, boxed reports per repo with branch, languages, files, and findings—or quiet mode to show only repos with secrets.
- **One token**: Uses a GitHub PAT; respects rate limits and reports HTTP errors at the end.

---

## Quick start (3 steps)

```bash
# 1. Clone and install
git clone https://github.com/yourusername/galgo.git && cd galgo
cargo install --path .

# 2. Set your GitHub token (required)
export GITHUB_TOKEN="ghp_xxxxxxxxxxxx"

# 3. Run (pick one mode)
galgo -d example.com -s          # single-file: analyze only files that mention example.com
galgo -d example.com -w         # whole-repo: scan config/secret files in those repos
galgo -d example.com -l         # list: show repos and matching files, no secret scan
```

---

## What you'll see

### Single-file mode (`-s`)

Galgo searches GitHub for code containing your domain, fetches commit info, then analyzes each matching file. For each repository you get a box like:

```
══════════════════════════════════════════════════════════════════════════════
  🔍 Analyzing 12 repositories for secrets
  📊 Sorted by file commit date: Descending (newest first)
══════════════════════════════════════════════════════════════════════════════

├─ Repository 1/12 ───────────────────────────────────────────────────────────┤
│ 🔗 URL     https://github.com/org/repo
│ ───────────────────────────────────────────────────────────────────────────
│ 🌿 Branch  main
│ 🌐 Languages
│   - C#: 99.78%
│   - Dockerfile: 0.22%
│ 📝 Files    src/config.json, .env.example
│ ───────────────────────────────────────────────────────────────────────────
│ 🚨 Secrets Found  2 potential secret(s)
│ │   ⚠️  src/config.json (apiKey: API Key)
│ │   ⚠️  .env.example (db_password: Password)
│ 📆 Last commit (file)  24/02/2025 10:30:00 UTC
│ 📆 Last commit (repo)  24/02/2025 10:35:00 UTC
└──────────────────────────────────────────────────────────────────────────────┘
```

Repos with no secrets show **✅ Status: No secrets detected**. Use `--quiet` to hide those and only print repos that have findings.

### Whole-repo mode (`-w`)

Same search, but for each repo Galgo fetches the full file tree, keeps only "interesting" files (`.env`, configs, YAML, etc.), then scans them. You see:

- Repo metadata, branch, description, topics
- Last commit dates
- Progress bar while scanning files
- **🚨 Secrets Found** or **✅ No secrets detected**
- **🚨 Hidden/Env-based**: lines that look like env vars (e.g. `os.getenv(...)`) and are skipped as non-secrets
- **⏭️ Skipped**: file count and extensions skipped

Limit how many files per repo with `--max-files` (e.g. `--max-files 500`).

### List mode (`-l`)

No secret analysis—only lists repos and their matching files, with last commit dates. Handy for auditing or feeding into other tools.

```bash
galgo -d example.com -l
# 📋 Found 12 repository(ies) (sorted by file commit date)
# • https://github.com/org/repo1
#   • Last commit (file): 24/02/2025 10:30:00 UTC
#   • Files: src/config.json, .env.example
```

Export to JSON:

```bash
galgo -d example.com -l --json                    # print JSON to stdout
galgo -d example.com -l --json repos.json         # write to file
```

---

## Usage examples

| Goal | Command |
|------|--------|
| Find secrets in files that mention your domain | `galgo -d api.mycompany.com -s` |
| Scan whole repos (config/secret files only) | `galgo -d mycompany.com -w` |
| Only show repos that have secrets (single-file) | `galgo -d mycompany.com -s --quiet` |
| Limit to 20 repos | `galgo -d mycompany.com -s --max-repos 20` |
| Oldest-first order | `galgo -d mycompany.com -s --sort asc` |
| Sort by repo’s last commit (not file’s) | `galgo -d mycompany.com -s --commit-date repo` |
| List repos + files as JSON | `galgo -d mycompany.com -l --json out.json` |
| List only repos with .py/.env files | `galgo -d mycompany.com -l --include-ext .py,.env` |
| Whole-repo, max 200 files per repo | `galgo -d mycompany.com -w --max-files 200` |

---

## Command reference

**Required**

- `-d, --domain <DOMAIN>` — Domain to search for in code (e.g. `example.com`).

**Mode (pick one)**

- `-s, --single-file` — Analyze only the files that matched the domain search.
- `-w, --whole-repo` — For each matching repo, scan likely config/secret files.
- `-l, --list` — List repos and matching files only; no secret scan.

**Search & scope**

- `--max-repos <N>` — Limit number of repositories (default: unlimited up to API limits).
- `--sort <asc|desc>` — Sort by commit date (default: `desc`).
- `--commit-date <file|repo>` — Use last commit of the matched file (`file`) or of the repo (`repo`) for sorting and display (default: `file`).
- `--include-ext <ext1,ext2>` — Restrict code search to these extensions (e.g. `--include-ext .rs,.toml`). In list mode, filters the file list.

**Single-file / whole-repo**

- `-q, --quiet` — (Single-file) Only print repos that have at least one secret.
- `--max-files <N>` — (Whole-repo) Max files to analyze per repository.

**List**

- `--exclude-ext <ext1,ext2>` — Hide these extensions from the file list.
- `--json [FILE]` — Output list as JSON. Omit FILE to print to stdout; use `-` for stdout explicitly.

---

## Authentication

Galgo needs a **GitHub Personal Access Token** with at least `public_repo` (read access to public repos). Code search and repo metadata work with a normal PAT.

1. **Environment (recommended)**  
   ```bash
   export GITHUB_TOKEN="ghp_xxxxxxxxxxxx"
   ```

2. **File**  
   Put the token on the first line of `token.txt` in the project root.

---

## How it works

1. **Search** — GitHub Code Search API: finds files that contain your domain (and optional `--include-ext`). Results are deduplicated by repo.
2. **Metadata** — For each repo: default branch, description, topics (and in single-file mode, languages). Repos that look like docs/demos/tutorials can be skipped in whole-repo mode.
3. **Analysis**  
   - **Single-file**: Fetch only the matched file contents and run secret-detection patterns.  
   - **Whole-repo**: Fetch full tree (Git Trees API), keep “interesting” paths (e.g. `.env`, `.yaml`, `*config*`), then fetch each file (Blobs API) and run patterns.
4. **Report** — Per-repo boxes with URL, branch, languages (single-file), files, findings (or “no secrets”), and last commit info. Whole-repo also shows env-based skipped lines and file counts.

Secret patterns (API keys, passwords, tokens, etc.) are defined in `patterns.json`; you can enable/disable or add new ones.

---

## Security & limits

- **Use only on domains you’re authorized to test.** Respect GitHub’s ToS and API rate limits (e.g. 5,000 REST requests/hour; code search has additional limits).
- Report findings responsibly; don’t misuse exposed credentials.
- Galgo does not upload or store your token beyond sending it to the GitHub API.

**Limitations**

- Public repositories only.
- Code search indexes the default branch; single-file mode analyzes the files returned by that search.
- Some file types are skipped (e.g. `.html`, `.csv`, `.md`) to reduce noise.
- Whole-repo mode caps at 50,000 “interesting” files per repo (configurable in code) to avoid API overload.

---

## Installation (detailed)

**Prerequisites:** Rust 1.70+ (edition 2021), GitHub PAT.

```bash
git clone https://github.com/yourusername/galgo.git
cd galgo
cargo install --path .
# run from anywhere
galgo -d example.com -s
```

Or build and run locally:

```bash
cargo build --release
./target/release/galgo -d example.com -s
```

Or run without installing:

```bash
cargo run --release -- -d example.com -s
```

---

## Contributing

Contributions are welcome. Open an issue or submit a Pull Request.

---

## License

MIT — see [LICENSE](LICENSE).

**Disclaimer** — For authorized security testing and research only. You are responsible for ensuring you have permission to scan the domains and repositories you target. The authors are not responsible for misuse.
