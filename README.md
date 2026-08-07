<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://codspeed.io/codspeed-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="https://codspeed.io/codspeed-logo-light.svg">
    <img alt="CodSpeed logo" src="https://codspeed.io/codspeed-logo-light.svg" width="400px">
  </picture>
</p>

<h3 align="center">Optimize code performance and catch regressions early.</h3>

<p align="center"><a href="https://codspeed.io/login?flow=get-started&utm_source=github-readme">Get Started</a> · <a href="https://codspeed.io/docs?utm_source=github-readme">Documentation</a></p>

<br/>

<p align="center">
  <a href="https://github.com/CodSpeedHQ/codspeed/releases/latest"><img src="https://img.shields.io/github/v/release/CodSpeedHQ/codspeed" alt="Latest Release"></a>
  <a href="https://github.com/CodSpeedHQ/codspeed/releases"><img src="https://img.shields.io/github/downloads/CodSpeedHQ/codspeed/total?logo=github" alt="Downloads"></a>
  <a href="https://github.com/CodSpeedHQ/codspeed/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/CodSpeedHQ/codspeed/ci.yml?branch=main&logo=github&label=CI" alt="CI Status"></a>
  <a href="https://github.com/CodSpeedHQ/codspeed/blob/main/LICENSE-APACHE"><img src="https://img.shields.io/github/license/CodSpeedHQ/codspeed" alt="License"></a>
  <a href="https://codspeed.io/discord"><img src="https://img.shields.io/badge/chat%20on-discord-7289da.svg" alt="Discord Chat"></a>
  <a href="https://codspeed.io/?utm_source=badge"><img src="https://img.shields.io/endpoint?url=https://codspeed.io/badge.json" alt="CodSpeed Badge"></a>
</p>

[![Video Demo](./assets/readme-video.gif)](https://codspeed.io/?utm_source=github-readme)

# Key features

- 🎯 **<1% variance** in measurements using CPU simulation - no more flaky benchmarks.
- 🔥 **Differential flamegraphs** to pinpoint exactly what got slower, commit by commit.
- 💬 **PR comments & status checks** showing performance impact directly in your workflow.
- 🛡️ **Merge protection** to block PRs that degrade performance beyond your threshold.
- 🐍 **Multi-language support** for Python, Rust, Node.js, Go, C/C++ and more.
- 🏠 **Run locally or in CI** - works on your machine and integrates with GitHub Actions, GitLab CI, and more.
- 🔌 **Plug your existing benchmarks** in less than 5 minutes - works with pytest, vitest, criterion, and more.

## Installation

```bash
curl -fsSL https://codspeed.io/install.sh | bash
```

> [!NOTE]
> The CodSpeed CLI officially supports Ubuntu 20.04, 22.04, 24.04 and Debian 11, 12.
> Other Linux distributions may work, but are not officially supported.

## Quick Start

First, authenticate to keep your benchmark results linked to your CodSpeed account:

```bash
codspeed auth login
```

The simplest way to get started is to benchmark any executable program directly:

```bash
# Benchmark a single command
codspeed exec -- ./my-binary --arg1 value

# Benchmark a script
codspeed exec -- python my_script.py

# Benchmark with specific instrument
codspeed exec --mode walltime -- node app.js
```

This approach requires no code changes and works with any executable. CodSpeed will measure the performance provide the instrument results.

## Deeper integration with harnesses using `codspeed run`

For more control and integration with your existing benchmark suite, you can use language-specific harnesses. This allows you to:

- Define multiple benchmarks and keep them versioned in your codebase
- Scope benchmarks to specific functions or modules
- Integrate with existing benchmark suites (pytest, criterion, vitest, etc.)

```bash
# Using the Rust harness with criterion
codspeed run cargo codspeed run

# Using the Python harness with pytest
codspeed run pytest ./tests --codspeed

# Using the Node.js harness with vitest
codspeed run pnpm vitest bench
```

These harnesses provide deeper instrumentation and allow you to write benchmarks using familiar testing frameworks.

### Languages Integrations

CodSpeed provides first-class integrations for multiple languages and frameworks:

| Language        | Repository                                                       | Supported Frameworks                  |
| --------------- | ---------------------------------------------------------------- | ------------------------------------- |
| Rust            | [codspeed-rust](https://github.com/CodSpeedHQ/codspeed-rust)     | `divan`, `criterion.rs`, `bencher`    |
| C/C++           | [codspeed-cpp](https://github.com/CodSpeedHQ/codspeed-cpp)       | `google-benchmark`                    |
| Python          | [pytest-codspeed](https://github.com/CodSpeedHQ/pytest-codspeed) | `pytest` plugin                       |
| Node.js         | [codspeed-node](https://github.com/CodSpeedHQ/codspeed-node)     | `vitest`, `tinybench`, `benchmark.js` |
| Go              | [codspeed-go](https://github.com/CodSpeedHQ/codspeed-go)         | builtin `testing` package integration |
| Zig (community) | [codspeed-zig](https://github.com/james-elicx/codspeed-zig)      | custom                                |

Need to bench another language or framework? Open [an issue](https://github.com/CodSpeedHQ/codspeed/issues) or let us know on [Discord](https://codspeed.io/discord)!

### CLI Harness: `codspeed.yml` configuration

The CLI also offers a built-in harness that allows you to define benchmarks directly.

You can define multiple `codspeed exec` benchmark targets and configure options in a `codspeed.yml` file.
This is useful when you want to benchmark several commands with different configurations.

Create a `codspeed.yml` file in your project root:

```yaml
# Global options applied to all benchmarks
options:
  warmup-time: "0.2s"
  max-time: 1s

# List of benchmarks to run
benchmarks:
  - name: "Fast operation"
    exec: ./my_binary --mode fast
    options:
      max-rounds: 20

  - name: "Slow operation"
    exec: ./my_binary --mode slow
    options:
      max-time: 200ms

  - name: "Script benchmark"
    exec: python scripts/benchmark.py
```

Then run all benchmarks with:

```bash
codspeed run --mode walltime
```

> [!TIP]
> For more details on configuration options, see the [CLI documentation](https://codspeed.io/docs/cli).

## Performance Instruments

CodSpeed provides multiple instruments to measure different aspects of your code's performance. Choose the one that best fits your use case:

### CPU Simulation

Simulates CPU behavior for **<1% variance** regardless of system load. Hardware-agnostic measurements with automatic flame graphs.

**Best for:** CPU-intensive code, CI regression detection, cross-platform comparison

```bash
codspeed exec --mode simulation -- ./my-binary
```

### Memory

Tracks heap allocations (peak usage, count, allocation size) with eBPF profiling.

**Best for:** Memory optimization, leak detection, constrained environments

**Supported:** Rust, C/C++ with libc, jemalloc, mimalloc

```bash
codspeed exec --mode memory -- ./my-binary
```

### Walltime

Measures real-world execution time including I/O, system calls, and multi-threading effects.

**Best for:** API tests, I/O-heavy workloads, multi-threaded applications

```bash
codspeed exec --mode walltime -- ./my-api-test
```

> [!WARNING]
> Using the `walltime` mode on traditional VMs/Hosted Runners will lead to inconsistent data. For the best results, we recommend using CodSpeed Hosted Macro Runners, which are fine-tuned for performance measurement consistency.
> Check out the [Walltime Instrument Documentation](https://docs.codspeed.io/instruments/walltime/) for more details.

> [!TIP]
> For detailed information on each instrument, see the [Instruments documentation](https://codspeed.io/docs/instruments).

## Usage In CI environments

Running CodSpeed in CI allows you to automatically detect performance regressions on every pull request and track performance evolution over time.

### GitHub Actions

We recommend using our official GitHub Action: [@CodSpeedHQ/action](https://github.com/CodSpeedHQ/action).

Here is a sample `.github/workflows/codspeed.yml` workflow for Python:

```yaml
name: CodSpeed Benchmarks

on:
  push:
    branches:
      - "main" # or "master"
  pull_request:
  workflow_dispatch:

permissions:
  contents: read
  id-token: write

jobs:
  benchmarks:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      # Set up your language/environment here
      # For Python:
      - uses: actions/setup-python@v5
        with:
          python-version: "3.12"
      - run: pip install -r requirements.txt

      # Run benchmarks with CodSpeed
      - uses: CodSpeedHQ/action@v5
        with:
          mode: instrumentation
          run: pytest tests/ --codspeed
```

### GitLab CI

Here is a sample `.gitlab-ci.yml` configuration for Python:

```yaml
workflow:
  rules:
    - if: $CI_PIPELINE_SOURCE == 'merge_request_event'
    - if: $CI_COMMIT_BRANCH == $CI_DEFAULT_BRANCH

codspeed:
  stage: test
  image: python:3.12
  id_tokens:
    CODSPEED_TOKEN:
      aud: codspeed.io
  before_script:
    - pip install -r requirements.txt
    - curl -fsSL https://codspeed.io/install.sh | bash -s -- --quiet
  script:
    - codspeed run --mode simulation -- pytest tests/ --codspeed
```

> [!TIP]
> For more CI integration examples and advanced configurations, check out the [CI Integration Documentation](https://codspeed.io/docs/integrations/ci/).
