<p align="center">
  <img src="assets/logo.png" alt="Galgo Logo" width="180"/>
</p>

<h1 align="center">Galgo</h1>

<p align="center">
  <b>Find exposed secrets on GitHub, starting from your domain.</b><br/>
  A fast Rust scanner that searches public code for a domain, then analyzes the matching files (or whole repositories) for API keys, tokens, passwords and other credentials.
</p>

<p align="center">
  <img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"/>
  <img alt="Rust 2021" src="https://img.shields.io/badge/rust-2021-orange.svg"/>
</p>

---

## Contents

- [Why Galgo?](#why-galgo)
- [Features](#features)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Authentication and token rotation](#authentication-and-token-rotation)
- [Modes](#modes)
- [Reading the output](#reading-the-output)
- [Usage examples](#usage-examples)
- [Command reference](#command-reference)
- [JSON output and CI](#json-output-and-ci)
- [Custom detection patterns](#custom-detection-patterns)
- [Troubleshooting](#troubleshooting)
- [Limitations](#limitations)
- [Responsible use](#responsible-use)
- [Documentation](#documentation)
- [License](#license)

---

## Why Galgo?

Credentials end up in public repositories more often than anyone would like, and
they rarely sit in the repository you own: a contractor, a former employee or a
partner pushes a config file that mentions `api.mycompany.com` together with a key.

Galgo starts from what you know, **your domain**, and works outward:

> *Where does `api.mycompany.com` appear in public code, and does that code leak credentials?*

## Features

- **Domain-first search.** Uses GitHub code search to find files that mention your domain.
- **Three modes.** `single-file` (only the matching files), `whole-repo` (every config-like file of each repo) and `list` (no scanning).
- **Findings you can act on.** Each one has file, **line number**, variable name, type, **severity** and a link to the exact line. Values are **masked** by default (`AK****LE`).
- **Fewer false alarms.** Skips docs/demo/tutorial repositories and the company's own repositories in whole-repo mode, ignores placeholders, and reports `process.env` / `${VAR}` lines separately instead of as leaks.
- **Live, readable output.** A status line shows progress while repositories are printed above it; clean repositories take one line, repositories with findings get a full box, and a final summary lists everything by severity.
- **Scriptable.** JSON output in every mode, results on stdout and progress on stderr, exit codes for CI.
- **Rate-limit aware.** Waits for the reset window and **rotates between several tokens** when one is exhausted.
- **Customizable.** Detection rules live in a plain `patterns.json`.

## Installation

You need a recent stable [Rust toolchain](https://rustup.rs/) (edition 2021).

```bash
git clone https://github.com/estebanramos/galgo.git
cd galgo
cargo install --path .        # installs the `galgo` binary
```

Or build without installing:

```bash
cargo build --release          # binary at target/release/galgo
```

## Quick start

```bash
# 1. Give Galgo a GitHub token (see "Authentication" below)
export GITHUB_TOKEN="ghp_xxxxxxxxxxxx"

# 2. Run one of the modes
galgo -d example.com -s        # single-file: analyze files that mention example.com
galgo -d example.com -w        # whole-repo: scan config/secret files of those repos
galgo -d example.com -l        # list: repositories and files only, no scan
```

On start Galgo reports the tokens it loaded and checks them:

```
🔑 1 token(s) loaded from GITHUB_TOKEN environment variable
   ✅ #1 (classic PAT, ends in …1234) @alice: core 4990/5000, search 30/30
```

## Authentication and token rotation

Galgo needs a **GitHub personal access token**. A classic PAT with `public_repo`
(or a fine-grained token with read access to public repositories) is enough.
Create one at <https://github.com/settings/tokens>.

Tokens are read from the first of these that has any:

| Source | Format |
|--------|--------|
| `GITHUB_TOKENS` | Comma, space or newline separated list |
| `GITHUB_TOKEN` | One token (a list also works) |
| `./token.txt` | One token per line; `#` starts a comment |

```bash
export GITHUB_TOKENS="ghp_aaa,ghp_bbb"      # several tokens
```

**Rotation.** Galgo uses one token until GitHub rate-limits it, then switches to
the next free one (`🔁 Token #1 rate limited, rotating`). Search and the rest of
the API are tracked separately, so a token exhausted for regular calls can
still search. If every token is limited, Galgo waits for the earliest reset (up
to 120 s, three times) and then stops with exit code `2`.

> **Tip.** GitHub's code-search limit is per **account**. Several tokens from
> the *same* account do not add search capacity; use tokens from different
> accounts. Galgo warns you when it detects this.

Tokens are never printed; only their type and last four characters are shown.
`token.txt` is in `.gitignore`.

## Modes

### Single-file (`-s`)

Galgo searches GitHub for code containing your domain, groups the hits by
repository, and analyzes **every matching file**. Fast and focused: it only
looks at files that already mention your domain.

### Whole-repo (`-w`)

Same search, but for each repository Galgo downloads the file tree, keeps
config-like files (`.env`, `.json`, `.yaml`, `.properties`, `*config*`,
`*secret*`, …) and scans all of them. Use it to find secrets that sit *next to*
the file mentioning your domain.

- Skips documentation / example / test / archive repositories (by name, description and topics).
- Skips repositories **owned by the domain's own organization** (`acme/…` for `acme.com`): those are your code, not a leak.
- `--max-files N` caps the files scanned per repository.

### List (`-l`)

No scanning: lists repositories and matching files with their last commit dates.
Useful for audits or to feed other tools (`--json`).

## Reading the output

Repositories without findings take one line; repositories with findings get a full
box. A status line at the bottom shows progress (`[12/38] org/repo  3 finding(s) so far`).

```
✓ acme/website  12 file(s) scanned, no secrets
⏭  acme/docs  documentation repository
✗ acme/gone  Failed to get repository files: status 404
┌ Repository [4/4] ──────────────────────────────────────────────────────────────┐
│ 🔗 URL: https://github.com/acme/billing-service
├────────────────────────────────────────────────────────────────────────────────┤
│ 🌿 Branch: main
│ 🌐 Languages: Rust 80.0% · Shell 19.5% · Other 0.5%
├────────────────────────────────────────────────────────────────────────────────┤
│ 📝 Files: deploy/prod/config/application-production.properties
│ 📊 Scanned: 12 file(s)
├────────────────────────────────────────────────────────────────────────────────┤
│ 🚨 Secrets Found: 2 potential secret(s)
│   [HIGH] deploy/prod/config/v…oduction.properties:14 (apiKey: AWS Access Key) AK****LE
│   [LOW] src/settings.json:3 (apiKey: Public Key) pk****xx
└────────────────────────────────────────────────────────────────────────────────┘
════════════════════════════════════════════════════════════════════════════════
  🚨 2 finding(s), highest severity first
════════════════════════════════════════════════════════════════════════════════
[ HIGH ] acme/billing-service  deploy/prod/confi…uction.properties:14
         AWS Access Key AK****LE
[ LOW  ] acme/billing-service  src/settings.json:3
         Public Key pk****xx
```

*(Sample rendered with synthetic data. Run `cargo test render_demo -- --ignored --nocapture` to reproduce it.)*

| Symbol | Meaning |
|--------|---------|
| `✓` | Scanned, nothing found |
| `⏭` | Skipped (documentation repo, owner matches the domain, too many files…) |
| `✗` | Error (the reason is shown) |
| `ℹ️ Env-based` | Lines such as `password = process.env.DB_PASS`: reported as a count, not as secrets |

**Severity**: `HIGH` (private keys, passwords, cloud and service secrets),
`MEDIUM` (generic API keys, access tokens, usernames), `LOW` (public or
publishable keys, host keys).

**Streams.** Results go to **stdout**; banners, progress, the final tally and
HTTP error summary go to **stderr**. Piping or redirecting stdout gives you clean data.

**Exit codes**

| Code | Meaning |
|------|---------|
| `0` | Scan finished, no secrets found |
| `1` | At least one secret found |
| `2` | Usage, token or API error |

## Usage examples

| Goal | Command |
|------|---------|
| Find secrets in files that mention your domain | `galgo -d api.mycompany.com -s` |
| Scan whole repos (config/secret files only) | `galgo -d mycompany.com -w` |
| Only show repos that have secrets | `galgo -d mycompany.com -s --quiet` |
| Full box for every repo | `galgo -d mycompany.com -s --verbose` |
| Limit to 20 repos | `galgo -d mycompany.com -s --max-repos 20` |
| Oldest first | `galgo -d mycompany.com -s --sort asc` |
| Sort by the repo's last commit instead of the file's | `galgo -d mycompany.com -s --commit-date repo` |
| Only search `.env` and `.yaml` files | `galgo -d mycompany.com -s --include-ext .env,.yaml` |
| Whole-repo, max 200 files per repo | `galgo -d mycompany.com -w --max-files 200` |
| Findings as JSON in a file | `galgo -d mycompany.com -s --json findings.json` |
| List repos and files | `galgo -d mycompany.com -l` |
| Show real values (careful) | `galgo -d mycompany.com -s --show-secrets` |

## Command reference

**Required**

- `-d, --domain <DOMAIN>`: domain to search for in code (e.g. `example.com`).

**Mode (pick one)**

- `-s, --single-file`: analyze the files that matched the search.
- `-w, --whole-repo`: scan likely config/secret files of each matching repo.
- `-l, --list`: list repositories and files only.

**Search and scope**

- `--max-repos <N>`: limit the number of repositories.
- `--sort <asc|desc>`: order by commit date (default `desc`).
- `--commit-date <file|repo>`: use the last commit of the matched file or of the repo (default `file`).
- `--include-ext <ext1,ext2>`: restrict the search to these extensions (also filters the list in list mode).
- `--exclude-ext <ext1,ext2>`: hide these extensions in list mode.
- `--max-files <N>`: whole-repo, max files scanned per repository.

**Output (all modes)**

- `--json [FILE]`: JSON instead of text; stdout by default, or to `FILE` (`-` means stdout).
- `-q, --quiet`: only report repositories with findings (errors are still shown).
- `-v, --verbose`: full box for every repository.
- `--show-secrets`: print real values instead of masked ones.
- `--no-color`: disable colors (automatic when stdout is not a terminal; `NO_COLOR` is honored).

Run `galgo --help` for the same list.

## JSON output and CI

`--json` prints an array with one object per repository (findings with file,
line, type, severity, masked value and URL; env references; languages; dates).
The schema is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#10-output-contract).

```bash
# Count high-severity findings
galgo -d mycompany.com -s --json | jq '[.[].findings[] | select(.severity=="high")] | length'

# Just the links to the leaking lines
galgo -d mycompany.com -s --json | jq -r '.[].findings[].url'
```

Because of the exit codes, Galgo can gate a pipeline. Example for GitHub Actions
(untested sketch, adapt it to your setup):

```yaml
- name: Scan for leaked secrets
  env:
    GITHUB_TOKEN: ${{ secrets.GALGO_TOKEN }}
  run: |
    cargo install --git https://github.com/estebanramos/galgo
    galgo -d mycompany.com -s --quiet --json galgo.json   # exits 1 if secrets are found
- uses: actions/upload-artifact@v4
  if: always()
  with: { name: galgo-report, path: galgo.json }
```

## Custom detection patterns

Rules live in [`patterns.json`](patterns.json) and are embedded at build time, so
**rebuild after editing**.

```json
{
  "pattern": "(?i)([A-Za-z_][A-Za-z0-9_]*(?:api[_-]?key))\\s*[=:]\\s*[\"']?([A-Za-z0-9_\\-]{20,})[\"']?",
  "secret_type": "API Key",
  "description": "Generic API key assignment",
  "disabled": false
}
```

- Capture group 1 = variable name; the **last** capture group = the secret value (this is what gets masked).
- Set `"disabled": true` to turn a rule off.
- Give new types a severity in `Severity::of` (`src/github/secret_detection.rs`); unknown types default to `high`.

Details and the full detection algorithm: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#7-detection-engine).

## Troubleshooting

| Symptom | Cause / fix |
|---------|-------------|
| `No GitHub token found` | Set `GITHUB_TOKEN`/`GITHUB_TOKENS` or create `./token.txt` |
| `Unauthorized: the token is invalid or expired` | Regenerate the token; Galgo exits `2` if no token is valid |
| `⏳ All N token(s) rate limited, waiting…` | Normal. Add tokens from other accounts to avoid waiting |
| `rate limit exceeded on all N token(s)` and exit `2` | Every token is limited for more than 120 s; try later or add tokens |
| Only ~1000 results for a popular domain | GitHub code search returns at most 1000 hits per query; narrow it with `--include-ext` |
| No colors when piping | Intentional: colors are off whenever stdout is not a terminal |
| A secret you expected is missing | See [limitations](#limitations); try `--show-secrets` and check the file with `-w` |

## Limitations

- Searches GitHub's **code search index** (default branches, at most 1000 hits per query).
- Detection is regex based. It will miss secrets that no pattern describes and will flag some false positives. Treat findings as leads to verify.
- A known weakness: the "empty value" exclusion also discards lines that merely end in a quote, so some real secrets in quoted JSON/YAML lines can be missed. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#14-known-limitations) for this and other limits.
- Repositories are scanned one after another; large whole-repo scans can take a while.
- Findings are not validated against the issuing service (no "is this key live?" check).

Ideas and plans are collected in [IDEAS.md](IDEAS.md).

## Responsible use

**Use Galgo only on domains and organizations you are authorized to assess.**
Respect GitHub's Terms of Service and API limits. If you find exposed
credentials, report them to the owner responsibly and do not use them. Galgo
does not store or transmit your tokens anywhere other than the GitHub API, and
masks secret values by default; treat `--show-secrets` output and JSON reports
as sensitive.

## Documentation

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): how Galgo works, at code level (pipeline, data model, detection engine, rate limiting and token rotation, output contract, extension guide).
- [CHANGELOG_IMPROVEMENTS.md](CHANGELOG_IMPROVEMENTS.md): what changed in the latest refactor and why.
- [GITHUB_API.md](GITHUB_API.md): notes on the GitHub REST API endpoints used.
- [IDEAS.md](IDEAS.md): possible future features.

## License

[MIT](LICENSE)
