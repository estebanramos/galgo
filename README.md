<p align="center">
  <img src="assets/logo.png" alt="Galgo Logo" width="180"/>
</p>
# Galgo

A Rust-based security tool for scanning GitHub repositories to detect exposed secrets and sensitive information related to a specific domain.

## Features

- 🔍 **Domain-based Search**: Search GitHub repositories for code containing references to a specific domain
- 🔐 **Secret Detection**: Automatically detect potential secrets, API keys, tokens, and sensitive credentials using regex patterns
- 📊 **Repository Analysis**: Analyze repositories and their files to identify security risks
- 🎯 **Single File Mode**: Analyze individual files that match your domain search
- 🚀 **Fast & Efficient**: Built with Rust for performance and reliability

## Installation

### Prerequisites

- Rust 1.70+ (edition 2021)
- A GitHub Personal Access Token (PAT) with appropriate permissions

### Option 1: Install Globally (Recommended)

Install `galgo` as a system-wide binary:

```bash
git clone https://github.com/yourusername/galgo.git
cd galgo
cargo install --path .
```

This will install `galgo` to `~/.cargo/bin/galgo` (or `%USERPROFILE%\.cargo\bin\galgo.exe` on Windows). Make sure this directory is in your `PATH`.

After installation, you can run `galgo` from anywhere:

```bash
galgo -d example.com -s
```

### Option 2: Build and Run Locally

Build the release binary:

```bash
git clone https://github.com/yourusername/galgo.git
cd galgo
cargo build --release
```

The binary will be available at `target/release/galgo` (or `target/release/galgo.exe` on Windows).

Run it directly:

```bash
./target/release/galgo -d example.com -s
```

Or add it to your PATH temporarily:

```bash
export PATH="$PATH:$(pwd)/target/release"
galgo -d example.com -s
```

### Option 3: Run with Cargo (Development)

For development or quick testing, you can run it directly with Cargo:

```bash
cargo run -- -d example.com -s
```

Or in release mode:

```bash
cargo run --release -- -d example.com -s
```

## Usage

### Authentication

Galgo requires a GitHub Personal Access Token. You can provide it in two ways:

1. **Environment Variable** (Recommended):
   ```bash
   export GITHUB_TOKEN="your_github_token_here"
   ```

2. **Token File**:
   Create a `token.txt` file in the project root with your token on the first line.

### Basic Usage

```bash
# Search for a domain and analyze single file matches
galgo -d example.com -s

# Search for a domain and analyze whole repositories (not yet implemented)
galgo -d example.com -w
```

### Command Line Options

- `-d, --domain <DOMAIN>`: Domain to search for (required)
- `-s, --single-file`: Analyze single file matches
- `-w, --whole-repo`: Analyze whole repositories (coming soon)

### Examples

```bash
# Search for references to example.com in GitHub code
galgo -d example.com -s

# Search for a subdomain
galgo -d api.example.com -s
```

## How It Works

1. **Search Phase**: Uses GitHub's Code Search API to find repositories containing references to your specified domain
2. **Analysis Phase**: Downloads and analyzes matching files using regex patterns to detect:
   - API keys
   - Tokens
   - Secrets
   - Private keys
   - Public keys
   - Other sensitive credentials

3. **Reporting**: Displays findings with repository information, file names, and detected patterns

## Security Considerations

⚠️ **Important**: This tool is designed for security research and responsible disclosure. Always:

- Use it only on domains you own or have permission to test
- Respect GitHub's Terms of Service and API rate limits
- Report findings responsibly to affected parties
- Never use exposed credentials maliciously

## Limitations

- GitHub API rate limits apply (5000 requests/hour for authenticated requests)
- Only searches public repositories
- Some file types are skipped (.html, .csv, .md) to reduce noise
- Whole repository analysis mode is not yet implemented

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Disclaimer

This tool is provided for educational and authorized security testing purposes only. Users are responsible for ensuring they have proper authorization before scanning any domains or repositories. The authors are not responsible for any misuse of this tool.
