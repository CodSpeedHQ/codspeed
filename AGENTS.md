# AGENTS.md

This file provides guidance to AI coding agents when working with code in this repository.

## Overview

CodSpeed Runner is a Rust CLI application for gathering performance data and uploading reports to CodSpeed. The binary is named `codspeed` and supports local and CI environments including GitHub Actions, GitLab CI, and Buildkite.

## Common Development Commands

### Building and Testing

```bash
# Build the project
cargo build

# Build in release mode
cargo build --release

# Run tests
cargo test

# Run specific test
cargo test <test_name>

# Run tests with output
cargo test -- -nocapture
```

### Running the Application

```bash
# Build and run
cargo run -- <subcommand> <args>

# Examples:
cargo run -- auth login
cargo run -- run "cargo bench"
cargo run -- setup
```

### Code Quality

```bash
# Check code without building
cargo check

# Format code
cargo fmt

# Run linter
cargo clippy
```

## Architecture

The application follows a modular structure:

### Core Modules

- **`main.rs`**: Entry point with error handling and logging setup
- **`app.rs`**: CLI definition using clap with subcommands (Run, Auth, Setup)
- **`api_client.rs`**: CodSpeed GraphQL API client
- **`auth.rs`**: Authentication management
- **`config.rs`**: Configuration loading and management

### Run Module (`src/run/`)

The core functionality for running benchmarks:

- **`run_environment/`**: CI provider implementations (GitHub Actions, GitLab CI, Buildkite, local)
- **`runner/`**: Execution modes:
  - **`valgrind/`**: Instrumentation mode using custom Valgrind
  - **`wall_time/perf/`**: Walltime mode with perf integration
- **`uploader/`**: Results upload to CodSpeed

### Key Dependencies

- `clap`: CLI framework with derive macros
- `tokio`: Async runtime (current_thread flavor)
- `reqwest`: HTTP client with middleware/retry
- `serde`/`serde_json`: Serialization
- `gql_client`: Custom GraphQL client
- `tabled`: Table formatting for CLI output (https://docs.rs/tabled/latest/tabled/index.html)
- Platform-specific: `procfs` (Linux), `linux-perf-data`

## Environment Variables

- `CODSPEED_LOG`: Set logging level (debug, info, warn, error)
- `CODSPEED_API_URL`: Override API endpoint (default: https://gql.codspeed.io/)
- `CODSPEED_OAUTH_TOKEN`: Authentication token

## Testing

The project uses:

- `cargo test`
- `insta` for snapshot testing
- `rstest` for parameterized tests
- `temp-env` for environment variable testing

Test files include snapshots in `snapshots/` directories for various run environment providers.

**Important**:

- Some tests require `sudo` access. They are skipped by default unless the `GITHUB_ACTIONS` env var is set.
