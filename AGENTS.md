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

## Browser Benchmarks

The browser runs as a child process of the bench command, so the runner traces it like any other child. The Chromium switches and V8 flags (`--single-process`, `--js-flags=--perf-prof …`, `--no-sandbox`) belong to the integration's launch arguments, not to the runner.

- **Simulation**: `--trace-children=yes` covers the browser, and its `/tmp/perf-<pid>.map` is harvested from the `<pid>.out` it produces. The measured window is driven from outside the traced process, with `callgrind_control -i on|off <pid>` and `callgrind_control --dump=<uri> <pid>`; a dump requested that way is recorded as `dump <uri>`, not as the `Client Request: <uri>` an in-process client request produces.
- **Walltime**: force `--walltime-profiler perf`, since samply harvests neither perf maps nor jit dumps, and report the browser process with `setExecutedBenchmark(<pid>, uri)`. Record with `--perf-unwinding-mode fp`: on the same benchmark, `perf script` resolved 84731 JS frames out of the frame-pointer recording (4746 samples, 54.7 frames deep) and none at all out of the dwarf one (4754 samples, 5.8 frames deep).

Running it locally needs no token and creates no run on CodSpeed:

```bash
cargo run -- run -m walltime --walltime-profiler perf --perf-unwinding-mode fp \
  --skip-upload --allow-empty --profile-folder /tmp/pf '<bench command>'
# the tarball is only built by the uploader, so build it by hand to feed the parser
tar czf sample.tar.gz -C /tmp/pf .
```
