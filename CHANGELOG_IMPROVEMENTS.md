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
- One report box per repository for both modes (`print_report`), plus a final
  summary line: repositories scanned, with findings, total findings.

---

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
