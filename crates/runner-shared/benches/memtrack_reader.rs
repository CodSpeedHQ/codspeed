//! Finding the module events (mappings, forks, execs) the runner needs for
//! offline stack attribution inside a full memtrack artifact. These are a
//! handful of records among millions of allocation events, so the cost is
//! dominated by everything the search has to read past.

use clap::ValueEnum;
use divan::Bencher;
use divan::counter::{BytesCount, ItemsCount};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use runner_shared::artifacts::{
    MemtrackArtifact, MemtrackEvent, MemtrackEventKind, StackRecord, encode_events,
};
use runner_shared::measurement_mode::MeasurementMode;
use runner_shared::runtime_env::RUNNER_MODE_ENV;
use std::io::Write;

fn main() {
    divan::main();
}

/// Artifact sizes in events, up to a CI-sized artifact with enough frames to
/// show how the parallel frame scan scales.
const SIZES: &[usize] = &[1_000_000, 10_000_000, 100_000_000];

/// Under the simulation and memory instruments, generating and searching
/// larger artifacts outlasts the CI job, so these modes only search the
/// smallest artifact.
const INSTRUMENTED_SIZES: &[usize] = &[1_000_000];

fn sizes() -> &'static [usize] {
    let mode = std::env::var(RUNNER_MODE_ENV)
        .ok()
        .and_then(|mode| MeasurementMode::from_str(&mode, true).ok());
    match mode {
        Some(MeasurementMode::Simulation | MeasurementMode::Memory) => INSTRUMENTED_SIZES,
        Some(MeasurementMode::Walltime) | None => SIZES,
    }
}

#[derive(Clone, Copy)]
enum Payload {
    Allocations,
    AllocationsAndStacks,
}

/// An artifact without stack capture.
#[divan::bench(args = sizes(), max_time = 10.0)]
fn find_module_events(bencher: Bencher, n: usize) {
    bench(bencher, n, Payload::Allocations);
}

/// An artifact with captured stacks, which make up most of its bytes.
#[divan::bench(args = sizes(), max_time = 10.0)]
fn find_module_events_with_stacks(bencher: Bencher, n: usize) {
    bench(bencher, n, Payload::AllocationsAndStacks);
}

fn bench(bencher: Bencher, n: usize, payload: Payload) {
    // The largest artifacts do not fit in memory, so they are written to disk
    // and mapped: mapped pages stay reclaimable. `/tmp` may be a tmpfs, hence
    // the target directory.
    let file = tempfile::tempfile_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let module_events = write_artifact(n, payload, &file);
    // SAFETY: the file is unlinked and only this process holds it.
    let artifact = unsafe { memmap2::Mmap::map(&file) }.unwrap();

    let find = || {
        MemtrackArtifact::decode_module_events(&artifact)
            .unwrap()
            .len()
    };
    assert_eq!(find(), module_events);

    bencher
        .counter(ItemsCount::new(n))
        .counter(BytesCount::new(artifact.len()))
        .bench_local(find);
}

/// Write an encoded artifact of `n` events laid out like memtrack writes it,
/// and return the number of module events in it: allocations with a fork or
/// exec every so often, then the mapping suffix. Events are generated as the
/// encoder consumes them, so they are never all in memory at once.
fn write_artifact(n: usize, payload: Payload, out: impl Write) -> usize {
    // Executable mappings memtrack appends after the event rings drain.
    const MAPPINGS: usize = 64;
    // Forks and execs come from BPF tracepoints, interleaved with allocations.
    const LIFECYCLE_INTERVAL: usize = 16 * 1024;
    // Keeps a 100M-event artifact with stacks at about 5 GB on disk.
    const STACK_INTERVAL: usize = 400;
    const ENCODE_WORKERS: usize = 4;
    const PID: i32 = 4242;

    let first_mapping = n - MAPPINGS;
    let mut rng = StdRng::seed_from_u64(42);
    let events = (0..n).map(|i| {
        let kind = if i >= first_mapping {
            MemtrackEventKind::Mapping {
                path: format!("/usr/lib/libmodule{i}.so"),
                dev: 0x0800_0001,
                ino: i as u64,
                file_offset: 0x1000,
                len: 0x8_0000,
            }
        } else if i % LIFECYCLE_INTERVAL == 0 {
            if (i / LIFECYCLE_INTERVAL) % 2 == 0 {
                MemtrackEventKind::Fork { parent_pid: PID }
            } else {
                MemtrackEventKind::Exec
            }
        } else if matches!(payload, Payload::AllocationsAndStacks) && i % STACK_INTERVAL == 0 {
            stack_event(&mut rng)
        } else if i % 2 == 0 {
            MemtrackEventKind::Malloc {
                size: rng.gen_range(8..8192),
                stack_hash: rng.r#gen(),
            }
        } else {
            MemtrackEventKind::Free
        };

        MemtrackEvent {
            pid: PID,
            tid: PID,
            timestamp: i as u64,
            addr: rng.r#gen(),
            kind,
        }
    });

    encode_events(events, out, ENCODE_WORKERS).unwrap();
    MAPPINGS + first_mapping.div_ceil(LIFECYCLE_INTERVAL)
}

/// A captured stack sized like memtrack's: its default stack copy budget of
/// incompressible bytes, plus the x86_64 register set.
fn stack_event(rng: &mut StdRng) -> MemtrackEventKind {
    const STACK_BYTES: usize = 8192;
    const REGS: usize = 33;
    const FP_FRAMES: usize = 16;

    let mut bytes = vec![0; STACK_BYTES];
    rng.fill(&mut bytes[..]);
    MemtrackEventKind::Stack {
        record: Box::new(StackRecord {
            hash: rng.r#gen(),
            sp: rng.r#gen(),
            regs: (0..REGS).map(|_| rng.r#gen()).collect(),
            bytes,
            fp_chain: (0..FP_FRAMES).map(|_| rng.r#gen()).collect(),
            truncated: true,
        }),
    }
}
