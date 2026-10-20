# Improvements: API layer, findings, and CLI output

Branch: `improvements/api-findings-output`

This document records what changed relative to `main`, why, and what behaviour
differs for users. It follows the four priorities from the code review.

---

## 1. API layer (`src/github/github.rs`)

### Shared HTTP client
Every request used to call `create_github_client()`, building a new connection
pool and doing a fresh TLS handshake each time. There is now a single
`static CLIENT` (user agent, 50 idle connections per host, 30 s timeout) reused
by all requests.

### One rate-limit handler: `get_text()`
The same ~12-line `body.contains("rate limit")` block was copy-pasted into
eight functions, and any rate limit aborted the whole run. All endpoints now go
through `get_text(token, url, accept)`, which:

- detects primary and secondary limits (HTTP 429, or 403 with `Retry-After`,
  `X-RateLimit-Remaining: 0`, or a "rate limit" body);
- sleeps until `Retry-After` / `X-RateLimit-Reset` and retries (max 3 retries,
  max 120 s per wait), printing the wait on stderr;
- only returns `RateLimitExceeded` when the wait is too long or retries ran out.

`get_json()` sits on top of it and turns non-2xx into
`InvalidResponse("Failed to get <what>: status N")`, the format the error
counter already parsed. This removed roughly 250 lines.

### Fewer requests
- `get_repo_metadata` no longer calls `/topics`: the repository object already
  contains `topics`. One request saved per scanned repository.
- The fixed 50 ms sleep between search pages was removed (the rate-limit
  handler replaces it).
- Metadata and languages are fetched concurrently with the file scan in
  single-file mode.
- Regex matching runs in `spawn_blocking` so it no longer stalls the async
  runtime.

### Search fixes
- A persistent 5xx on a search page used to loop forever (`page += 1; continue`).
  Pagination now stops after the 10 pages GitHub allows.
- `total_items_processed` counted repositories; the 1000-result cap is on hits,
  so it now counts hits.
- Commit-date progress is a single bar over hits instead of resetting per page.

### Removed
`get_raw_file_content` (dead), `get_default_branch` (dead), the `blocking`
reqwest feature (unused).

---

## 2. Single-file mode analyses every matching file

`search()` skipped any repository it had already seen
(`if repository_list.contains_key(..) { continue; }`), so a repository with five
files mentioning the domain was analysed through only the first. `Repository`
already had `Vec` fields for files, but they only ever held one entry.

Hits are now grouped per repository (across pages and within a page) and each
file is attached to its repository, de-duplicated by URL. Commit dates are
fetched once per repository, using the first matching file for the "file" date.

---

## 3. Shared scanner and richer findings (`src/github/secret_detection.rs`)

The two ~400-line detection functions duplicated the matching logic and had
diverged. There is now one `scan_content()` used by both modes, and one driver,
`run_scan()`.

### `Finding`
Each finding carries `file`, `line`, `variable`, `secret_type`, `severity`,
`value` and `url` (a link to `…#L<line>`).

- **Line numbers** for every finding.
- **Masked values** by default (`AK****LE`; values of 8 characters or fewer
  become `****`). `--show-secrets` prints the full value.
- **Severity** (`low` / `medium` / `high`) derived from the secret type: public
  keys, publishable keys and host keys are low; usernames, generic API keys and
  access tokens medium; everything else high. Findings are sorted by severity,
  then file and line.

### Behaviour changes in detection
- **All matches are reported**, not one per pattern per file (whole-repo mode
  used to `break` after the first match).
- **Env-based lines no longer discard the whole file.** Previously a single
  `process.env` / `${VAR}` line caused the entire file to be skipped, hiding
  real secrets elsewhere in it. Those lines are now listed as `env_references`
  and the rest of the file is still scanned.
- In single-file mode, `${VAR}` lines are now treated as env references instead
  of findings (whole-repo already did this).
- A failure fetching languages or metadata no longer hides the repository's
  findings (single-file mode used to `continue` before printing them).

The `unit tests` at the bottom of `secret_detection.rs` cover line numbers,
masking, multiple matches and env handling.

---

## 4. CLI output and exit codes

### Streams
- Results (boxes, or JSON) go to **stdout**.
- Headers, progress, steps, summaries and the HTTP error summary go to
  **stderr**. `galgo … --json > out.json` now yields clean JSON.

### `--json [FILE]` in every mode
Previously list mode only. Scan modes emit an array of repository reports
(`status`, `branch`, `languages`, `matched_files`, `findings`,
`env_references`, dates, …). In JSON mode the boxes are not printed.

### Exit codes
| Code | Meaning |
|------|---------|
| 0 | Scan completed, no secrets |
| 1 | Secrets found |
| 2 | Usage, token, auth or API error (including no mode selected) |

Previously a missing token printed an error and exited `0`.

### Other
- `--quiet` applies to both scan modes and is documented as such.
- `--no-color`; colour is also disabled automatically when stdout is not a
  terminal. `NO_COLOR` is honoured by the `colored` crate.
- New `OUTPUT` help section (`--json`, `--quiet`, `--no-color`,
  `--show-secrets`).
- Token lookup is silent when it succeeds (no more "Trying to read token…"
  noise or a warning when the env var is unset but `token.txt` exists).
- One report box per repository for both modes (`print_report`).

### Live output (second pass)
- **Live status line** at the bottom: `[12/38] org/repo  3 finding(s) so far`
  (indicatif `MultiProgress`). Finished repos are printed above it, so output
  never garbles. In whole-repo mode the per-file bar sits under it.
- **Clean, skipped and failed repos take one line** (`✓` / `⏭` / `✗`). A full
  box is printed only for repos with findings; `-v/--verbose` restores boxes for
  every repo. `--quiet` still hides clean and skipped ones, but errors always
  show.
- **Findings summary at the end**: every finding across repos, highest severity
  first, followed by a one-line tally (scanned / with findings / skipped /
  errors) on stderr.
- **Adaptive width** (terminal width clamped to 60-110, 80 when piped); long
  file paths are shortened in the middle, keeping the file name.
- **Compact banners**: the three "Step n/3" blocks of the search became one
  line each; the scan header is a single line.
- The env-based lines are only counted in the box (details in `--json`);
  language shares are one line (`Rust 80.0% · Shell 19.5%`).
- `cargo test render_demo -- --ignored --nocapture` renders sample output for
  visual checks without a token.

---

### Matched files and repo details (third pass)
- Single-file mode lists, under each repo line, which matched file was scanned,
  skipped (ignored extension, e.g. `.md`) or could not be fetched.
- Repo lines show stars; `-v` boxes add stars, forks, license, fork/archived
  flags, last push, and one row per matched file with its outcome.
- JSON gains `stars`, `forks`, `license`, `is_fork`, `archived`, `pushed_at`
  and `files[] {path, status, findings}` (single-file mode).

### Usability pass (fourth pass)
- **Skip filter matches whole words.** `should_scan_repository` used substring
  matching, so `penetration-testing` hit `test`, `springdoc-openapi` hit `doc`
  and `gpstest` hit `test`, silently dropping valid repos. Names, descriptions,
  topics and the owner/domain check now compare whole words. `testing` was
  removed from the skip list (it matched security-testing repos).
- **Findings first in the repo box.** Order is now: findings, then Stats /
  Languages / Scanned / Last commit. Branch, description, topics, last push and
  matched files only with `-v`. Each finding takes two lines so the full path
  fits.
- **No duplicated findings list.** The end summary now appears only when two or
  more repos have findings, as one line per repo with counts per severity.

### False-positive control and rate-limit abort (fifth pass)
- **Confidence on every finding** (`confidence`: high/medium/low, plus `reasons`
  in JSON and a dim `↓ low confidence: …` line in the box). Low confidence caps
  severity at LOW, medium at MEDIUM. Lowered when: the value is a function call
  or a bare variable in a code file (unquoted), looks like a placeholder
  (`****`, `<…>`, `{{…}}`, `password`, `default`…), is a plain word/identifier
  with low entropy (generic types only: Password, API Key, Credentials…), or
  (medium) the file is under a test/demo/example/fixtures path.
- **More secret stores treated as references** (listed under env-based lines, not
  reported): `os.Getenv`, `System.getenv/getProperty`,
  `Environment.GetEnvironmentVariable`, `ENV[`/`ENV.fetch`, `import.meta.env`,
  `Deno.env`, `${{ … }}`, Helm `{{ .Values }}`, `secretsmanager`, `getpass`,
  `keyring.get_password`.
- Trailing quote/comma/`;`/`)` swallowed by pattern character classes is trimmed
  from the reported value (it showed as `sh****",`).
- **Rate-limit abort.** When every token is limited for longer than the 120 s
  wait cap, the scan stops once with the reset time and the number of repos not
  scanned (instead of one error line per remaining repo), JSON lists them as
  `skipped`, and the exit code is 2 unless findings exist. Startup shows `⚠️`
  when core quota is 0.

### Detection fixes and fixtures (sixth pass)
- **False negatives fixed.** Three exclusions discarded real secrets:
  `["']\s*$` (any line ending in a quote, e.g. `KEY = "value"`), the
  line-wide `example|test_|demo_|sample_` placeholder match, and `(#|//).*(test|example…)`
  (any URL containing `//`, so `postgres://…/test` was dropped). The "empty value"
  exclusion was removed (every pattern already requires a minimum length); the
  comment exclusion now applies to whole-line comments only, and placeholders
  (`your_`, `changeme`, `<…>`, `example`, `xxx`, `test_`/`demo_`/`sample_` prefix)
  are checked against the **value** only. Expect more findings than before.
- **Wrong captured value fixed.** Nested groups in `patterns.json` (`(live|test)`,
  `(RSA\s+)?`, `(OPENSSH\s+)?`) made the reported value `test` or `RSA `; they are
  non-capturing now. Stripe `sk_test_`/`pk_test_` keys are low confidence.
- **`tests/fixtures/cases.tsv`**: expected outcome (find / skip / env / lowconf)
  per line of code, checked by `fixtures_match_expected_outcomes`. Add a row for
  every false positive or negative you fix.
- **CI** (`.github/workflows/ci.yml`): build, test, clippy on push and PR.
- Known limit: bare `password:` / `password =` (no prefix in the name) is not
  matched by the Password pattern, nor bare `API_KEY` by the API Key pattern.

### Distribution and configuration (seventh pass)
- `--patterns FILE` replaces the embedded rules at run time (validated up front).
- `.galgoignore` / `--ignore-file FILE`: `owner/repo`, `owner/*`, `path:<text>`,
  `rule:<Type>`; ignored repos are skipped and ignored findings are counted.
- reqwest now uses `rustls-tls` (no OpenSSL dependency), which makes release
  binaries and the Docker image portable.
- `.github/workflows/release.yml` builds Linux/macOS/Windows binaries on a `v*`
  tag and attaches them to the release (not run yet).
- `Dockerfile` (multi-stage, tested: builds and queries the live API).

## 5. Token handling and rotation

### Sources
First source that yields a token wins; each accepts several tokens separated by
commas, spaces or newlines (`#` comment lines and duplicates are ignored):

1. `GITHUB_TOKENS`
2. `GITHUB_TOKEN`
3. `./token.txt` (one token per line)

### Startup report (stdout; stderr when `--json` writes to stdout)
```
🔑 2 token(s) loaded from GITHUB_TOKENS environment variable
   ✅ #1 (classic PAT, ends in …1234) @alice: core 4990/5000, search 30/30
   ✅ #2 (fine-grained PAT, ends in …abcd) @bob: core 5000/5000, search 30/30
```
Tokens are never printed, only their kind and last 4 characters. Each one is
validated against `/rate_limit` (free) and `/user`. Invalid tokens are dropped
with a message; if none is valid the run exits with code 2. Two tokens of the
same account trigger a warning, because the search limit is per account and
they add no search capacity.

### Rotation (`TokenPool` in `src/token.rs`)
- The current token is used until it is rate limited; then the request moves to
  the next free token and a `🔁 Token #n rate limited, rotating` line is printed
  once.
- Limits are tracked per token **and per bucket** (search vs. everything else),
  so a token exhausted on `core` still serves search requests.
- A response whose `X-RateLimit-Remaining` is 0 parks that token immediately,
  avoiding one failed request.
- When every token is limited, the run waits for the soonest reset (max 120 s,
  3 times) and aborts with exit code 2 if that is not enough. With one token
  this is the previous behaviour.
- The pool is passed where `token: &str` used to be, so no call sites changed.

## Not done / known issues

- **Exclusion regex** `["']\s*$` in `EXCLUSION_PATTERNS` ("empty or very short
  values") matches *any line ending in a quote*, so e.g.
  `STRIPE_API_KEY = "abcdef1234567890abcdef"` is excluded. Found while writing
  the tests (they end the line with `;`). Not changed here because it alters
  detection results; it likely deserves its own fix and a look at the other
  exclusions (`test_`, `example`… are applied to the whole line, not the value).
- The `Utc::now()` fallback for failed commit-date lookups is unchanged, so a
  failure sorts the repository as "newest".
- The two commit-date calls per repository are unchanged.
- Whole-repo mode still scans repositories sequentially and fetches one blob
  per file (a tarball download would be a single request).
- `--sort` / `--commit-date` are still validated strings, not `ValueEnum`.
- Only unit tests were run; nothing was exercised against the live GitHub API
  (no token in this session), so rate-limit waiting and the grouped search are
  untested end to end.
