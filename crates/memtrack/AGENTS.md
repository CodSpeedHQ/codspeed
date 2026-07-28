# Repository Guidelines

`memtrack` is the eBPF-based memory-allocation tracker of the CodSpeed runner (workspace member, Linux-only). This guide covers the crate at `crates/memtrack/`; the workspace root has its own `AGENTS.md`.

## Project Overview

Attaches uprobes/uretprobes to allocator functions (`malloc`/`free`/`calloc`/`realloc`/`aligned_alloc`/`memalign`) and tracepoints to `mmap`/`munmap`/`brk` + `sched_process_fork` in a target process tree, streams allocation events through a BPF ring buffer to userspace, and writes them to a `MemtrackArtifact` file. Ships a CLI binary `codspeed-memtrack track`.

## Architecture & Data Flow

Allocation → disk pipeline:

1. Allocator entry uprobe stores args in a per-tid BPF hash map (`<name>_arg` / `<name>_args`).
2. Uretprobe reads the stored arg + `PT_REGS_RC`. The `SUBMIT_EVENT` macro gates on `is_tracked(pid)` (`tracked_pids` map, ancestor walk ≤5 levels via `pids_ppid`, plus `sched_fork` auto-tracking of children) **and** `is_enabled()` (`tracking_enabled` map), then `bpf_ringbuf_reserve`/`submit` into the 256 MiB `events` ring buffer. Reserve failure atomically bumps `dropped_events`. `bpf_ringbuf_submit` uses `wake_flags()`: it wakes the consumer only once ≥64 KiB (`WAKEUP_DATA_SIZE`) of unconsumed data has accumulated (`BPF_RB_FORCE_WAKEUP`), otherwise `BPF_RB_NO_WAKEUP` — amortizing per-event wakeups to ~1 per thousand events.
3. `RingBufferPoller` (`src/ebpf/poller.rs`) polls every 10 ms; each `poll()` is followed by a `consume()` that drains the buffer to empty in one call (no extra `epoll_wait`), and the 10 ms timeout flushes the sub-watermark tail. `parse_event` (`src/ebpf/events.rs`) casts raw bytes to the bindgen `event` struct and maps `EVENT_TYPE_*` → `runner_shared::MemtrackEventKind` → mpsc channel.
4. `Tracker::track` (`src/ebpf/tracker.rs`) forwards poller events over a keep-alive thread.
5. `main.rs` spawns one pipeline thread running `encode_events` (`runner-shared`), which fans out across `available_parallelism() - 2` rayon workers (two cores reserved for the poll thread + tracked command so encode workers don't starve the poller and overflow the ring buffer) and batches through `MemtrackWriter`. After the child exits, it checks `dropped_events_count()` and **bails if non-zero** (incomplete trace).

Control plane: `src/ipc.rs` exposes an out-of-band `ipc-channel` protocol (`Enable`/`Disable`/`Ping`) so the runner toggles the `tracking_enabled` map at runtime. Without `--ipc-server`, tracking is enabled up front.

Allocator discovery (`src/allocators/`): `AllocatorLib::find_all()` = dynamic (glob shared libs incl. `/nix/store/*` hints) + static-linked (scan build-dir ELF symbols) + env (`CODSPEED_MEMTRACK_BINARIES`). Each `AllocatorKind` (`Libc`/`LibCpp`/`Jemalloc`/`Mimalloc`/`Tcmalloc`) maps to best-effort attach helpers; only libc must succeed.

> Note: the "on-demand attach" design in `.agents/docs/` (AttachWorker, `CODSPEED_MEMTRACK_ONDEMAND`, SIGSTOP/SIGCONT) is a **plan, not yet in source**. Current behavior is upfront attach + `sched_fork` auto-tracking.

## Key Directories

- `src/ebpf/` — BPF stack (feature-gated `ebpf`): `tracker.rs` (facade), `memtrack/` (libbpf-rs wrapper + generated skeleton, split into `mod.rs`/`macros.rs`/`maps.rs`/`allocator.rs`/`tracking.rs`), `poller.rs`, `events.rs`, `c/main.bpf.c` + `c/event.h` + `c/utils/*.h` + `c/allocator.h`.
- `src/allocators/` — allocator classification: `mod.rs`, `dynamic.rs`, `static_linked.rs`.
- `tests/` — integration tests + `snapshots/` (insta).
- `testdata/` — allocation fixtures: `*.c` (gcc), `alloc_cpp/` (cmkr/CMake), `alloc_rust/` + `spawn_wrapper/` (standalone Cargo workspaces).
- `.agents/docs/`, `.claude/` — design notes (some current, some historical/stale).

## Development Commands

```bash
cargo build                 # default features include `ebpf`
cargo check
cargo fmt
cargo clippy
cargo test --lib            # unit tests (no root)

# Run the tracker (needs root); tracks a shell command's whole process tree:
sudo -E cargo run --bin codspeed-memtrack -- track "<command>" --output <dir>
# e.g. sudo -E cargo run --bin codspeed-memtrack -- track "ls / >/dev/null" --output .

# Integration tests need BPF privilege + GITHUB_ACTIONS gate + single-threaded:
export GITHUB_ACTIONS=1
sudo -E cargo test --test c_tests -- --test-threads 1
```

`--test-threads 1` is **mandatory** — eBPF probes cannot self-overlap. CI runs `sudo -E cargo test --lib --test <name> -- --test-threads 1`; the main workspace `tests` job excludes memtrack (`--exclude memtrack`).

## Code Conventions & Common Patterns

- **Errors:** `anyhow` only (`Result`/`Context`/`bail`/`ensure` via `src/prelude.rs`); no `thiserror`.
- **Concurrency:** no async runtime — `std::thread` + `std::sync::mpsc` + `Arc<Mutex<_>>`.
- **Naming:** snake_case modules, PascalCase types. Macro-generated `try_<name>` (fallible) vs `<name>_if_found` (best-effort, trace-logs errors) attach helpers via `paste!`.
- **BPF C:** macro-heavy (`UPROBE_ARG_RET`/`UPROBE_RET`/`UPROBE_ARGS_RET`/`SUBMIT_EVENT`); `.clang-format` is Google-based, 4-space indent, 100-col, left pointers; vmlinux.h include wrapped in `// clang-format off/on`.
- **Module layout:** `src/lib.rs` re-exports and `#[cfg(feature = "ebpf")]` gates the whole BPF stack + binary; `prelude.rs` centralizes error/log imports.
- **Cross-crate types:** `MemtrackEvent` / `MemtrackEventKind` live in `runner-shared/src/artifacts/memtrack.rs` (a breaking change there ripples here).

## Important Files

- `src/main.rs` — CLI entry (`codspeed-memtrack track`, requires `ebpf`); drops sudo privileges to `SUDO_UID`/`SUDO_GID` so the tracked child runs unprivileged.
- `src/lib.rs` — crate root / re-exports.
- `src/ebpf/c/main.bpf.c` + `c/event.h` + `c/utils/*.h` + `c/allocator.h` — BPF program (includes only) and event struct definitions; maps/programs live in headers.
- `build.rs` — feature-gated: `libbpf_cargo::SkeletonBuilder` compiles `main.bpf.c` → `OUT_DIR/memtrack.skel.rs`; `bindgen` on `wrapper.h` → `OUT_DIR/event.rs`. Reruns on `src/ebpf/c` changes and on `GITHUB_ACTIONS` env change.
- `wrapper.h` — bindgen entry (`stdint.h` + `src/ebpf/c/event.h`).
- `Cargo.toml` — lib `memtrack` + bin `codspeed-memtrack`; feature `ebpf` (default) pulls `libbpf-rs`/`libbpf-cargo`/`vmlinux`.

## Runtime / Tooling Prerequisites

- **Linux only** — consumed from workspace root under `cfg(target_os = "linux")`, targets `x86_64`/`aarch64-unknown-linux-gnu`.
- **Root / BPF privilege** required at runtime (bumps `RLIMIT_MEMLOCK`, loads BPF).
- **Build toolchain:** `clang` + BTF/vmlinux headers, `libbpf-dev`, `zlib1g-dev`, `pkgconf`, `build-essential`; vendored libbpf also needs `autopoint`/`bison`/`flex`.
- `vmlinux.h` is pinned to a specific git rev; `libbpf-rs` uses the `vendored` feature (dist links `libbpf-rs/static`).

Env vars actually wired: `CODSPEED_MEMTRACK_BINARIES` (extra static-allocator binaries), `CODSPEED_LOG` (log filter, default `info`), `SUDO_UID`/`SUDO_GID` (privilege drop), `GITHUB_ACTIONS` (build rebuild trigger + test gate).

## Testing & QA

- **Frameworks:** `insta` (snapshots), `rstest` (parametrized `#[case]`), `test-log`, `test-with` (`#[test_with::env(GITHUB_ACTIONS)]`), `tempfile`.
- **Harness:** `tests/shared.rs` — `assert_events_snapshot!` / `assert_events_with_marker!` (dedup by addr+discriminant, `Realloc` strips `old_addr`, `0xC0D59EED` marker windowing), `compile_rust_binary`, `track_command`/`track_binary[_with_opts]`.
- **Suites:** `c_tests` (8 gcc-compiled C fixtures, no marker), `cpp_tests` (7 cmkr targets: system + jemalloc/mimalloc/tcmalloc × static/dynamic), `rust_tests` (system/jemalloc/mimalloc via features), `spawn_tests` (static-allocator discovery across `exec`).
- **Snapshots:** `tests/snapshots/<binary>__<case>.snap`, address-stripped and deduped. Every integration test is `GITHUB_ACTIONS`-gated and needs BPF privilege; unit tests (`src/ebpf/events.rs`) run without root.
