---
name: codspeed-write-benchmarks
description: "Write, fix, and maintain benchmarks so that CodSpeed measurements are valid and stable. Use this skill when adding benchmarks to a project that already runs CodSpeed, when a benchmark is noisy, flaky, or changes without a related code change, when CodSpeed reports a benchmark as optimized out or measuring no execution time, when auditing or cleaning up a benchmark suite, when renaming, moving, or removing benchmarks, or when benchmarks need updating after the code under test changed. To install a benchmark framework or wire up CodSpeed for the first time, use codspeed-setup-harness instead."
---

# Write Benchmarks

A benchmark is useful when a change in its result means a change in the code. Apply the rules below when writing a benchmark, and use them as the checklist when one misbehaves.

Run benchmarks through the CodSpeed CLI (`codspeed run`, `codspeed exec`) and read results with the CodSpeed MCP tools, not from raw framework output.

## Rules

1. **Relevance**: One benchmark answers one question about a real hot path or user-facing operation. Use inputs with a realistic size and distribution.

2. **Validity**: The code under test must actually run. Use its result (return it, or pass it to the framework's black box) and keep inputs opaque to the optimizer, otherwise the work can be removed or computed at compile time. See [optimized-out benchmarks](https://codspeed.io/docs/troubleshooting).

3. **Measured region**: Measure only the operation under test. Build inputs outside the measured code. If the operation mutates its input, give every iteration a fresh copy through the framework's setup hook. Every iteration must do the same work.

4. **Determinism**: The same code must do the same work on every run. Seed random generators, use fixed-seed hashing or ordered containers, sort directory listings, and read committed fixtures. No network, clock, or environment-dependent branches. See [Benchmark Variance](https://codspeed.io/docs/instruments/cpu/regression-causes).

5. **State isolation**: A result must not depend on which benchmarks ran before it. Avoid shared caches and globals. Allocator and garbage collector state carries over between benchmarks, so allocate inputs in setup rather than in the measured code. If a micro-benchmark still moves with unrelated code changes, build it into its own binary. See [Reducing Variance](https://codspeed.io/docs/instruments/cpu/reducing-variance).

6. **Sizing**: Size each benchmark so it gives a valid result under every instrument it runs with. One benchmark can run under several instruments, and each one has its own limits:
   - **Simulation** runs the code once on a simulated CPU and excludes system calls. Use it for CPU-bound code.
   - **Walltime** measures real time over repeated rounds and includes I/O. Very short operations need more iterations per round: set the framework's time or iteration options. Avoid running benchmark processes in parallel.
   - **Memory** runs the code once and tracks every allocation. Keep a benchmark under about 2 million allocations and split larger ones. Allocation order across threads is not deterministic.

7. **Naming**: A benchmark's identity sets its history. Framework integrations use the file path, name, and parameters. The exec harness uses the `name` field, or the full command when there is none. Keep the identity stable, unique within a run, and descriptive of the scenario.

8. **Environment**: Pin the toolchain and commit lockfiles. Avoid `latest` and `stable` tags in CI. Run walltime benchmarks on [CodSpeed Macro Runners](https://codspeed.io/docs/features/macro-runners). Base and head must run on the same hardware.

## Verify

1. Read the benchmark against the rules.
2. If the CodSpeed CLI is authenticated and the CodSpeed MCP tools are available, run the benchmark once with the CLI and check with the MCP tools:
   - `get_run` reports no issue on it.
   - The value is plausible and scales with input size the way the operation should, for example flat for a hash lookup and linear for a scan.
   - `query_flamegraph` shows the function under test, not setup code, as the main cost.
3. Only when stability is in question, run it again without any change.
   - Simulation and memory results should be nearly identical. A visible difference means the benchmark breaks rule 4 or 5.
   - Local walltime results vary with the machine. Use them to rank suspects only. Judge stability from CI runs on macro runners, where `get_benchmark_result` shows the spread across rounds and the outliers.

If the CLI or the MCP tools are not available, report the findings from step 1 and state that nothing was measured.

## Maintain

### A benchmark is noisy

1. Call `compare_runs` on two runs that disagree. If it lists environment differences (hardware, toolchain, integration version), fix rule 8 first.
2. Call `query_flamegraph` on both runs and find what differs. Allocator, garbage collector, hashing, or I/O frames point to rules 4 and 5.
3. Fix the benchmark, then verify.
4. If the variance belongs to what is measured, such as real I/O, ask the user to set a [per-benchmark regression threshold](https://codspeed.io/docs/features/customization). [Ignoring the benchmark](https://codspeed.io/docs/features/ignoring-benchmarks) is the last resort. Both are dashboard actions.

### Audit a suite

List the benchmarks with `get_run`. Merge or remove benchmarks that answer the same question. Fix or remove those that break a rule. Shrink or split those that dominate the run time. Report hot paths without a benchmark, but do not add speculative ones.

### Rename, move, or remove

A change to a benchmark's identity (rule 7) starts a new history, and the old benchmark shows up as skipped. Do not change it without a reason. After a rename, move, or removal, confirm the old benchmark no longer runs, then tell the user to [archive](https://codspeed.io/docs/features/archiving-benchmarks) it in the dashboard (organization admins only).

### The code under test changed

Keep the name, the input, and the measured region so the history stays comparable. If the benchmark now measures something different, give it a new name.

## Language notes

The rules apply to every language. These are the points that differ. For framework APIs, follow the linked docs.

- **Rust** ([docs](https://codspeed.io/docs/benchmarks/rust)): Return the measured value from the benchmark function and wrap constant inputs in `black_box`. With divan, `with_inputs(..)` provides a fresh input per iteration: use `bench_values` to take it by value, or `bench_refs` / `bench_local_refs` to mutate it in place. `HashMap` seeds are random per process, so use a fixed hasher or a `BTreeMap` for benchmark data.
- **Node.js** ([docs](https://codspeed.io/docs/benchmarks/nodejs/overview)): Use the result so the JIT cannot drop the work. Keep object shapes and types the same across iterations to avoid deoptimization. Allocate inputs in the framework's hooks, not in the measured function, to keep garbage collection out of the measurement.
- **Python** ([docs](https://codspeed.io/docs/benchmarks/python)): `@pytest.mark.benchmark` measures the whole test. Use the `benchmark` fixture to measure only the call, and `benchmark.pedantic(setup=...)` for a fresh input per round.
- **Exec harness** ([docs](https://codspeed.io/docs/benchmarks/cli-commands)): It measures the whole command, including process startup. Make the command do enough work to dominate startup, or declare an `entrypoint` with a custom harness to measure a sub-part. Keep debug symbols so flamegraphs are readable.
- **Other languages**: [docs](https://codspeed.io/docs/benchmarks/overview).
