use divan::Bencher;
use divan::counter::{BytesCount, ItemsCount};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use runner_shared::artifacts::{
    MemtrackEvent, MemtrackEventKind, MemtrackWriter, StackRecord, encode_events,
};

/// Reports allocation counts and bytes next to the timings for local runs. Only
/// tallies the thread running the benchmark, so the parallel encoder's
/// per-worker allocations show up in the single-threaded writer benches
/// instead.
///
/// Left out of CodSpeed builds: wrapping the allocator costs ~15% on
/// allocation-heavy benchmarks, and the memory instrument reports allocations
/// there anyway.
#[cfg(not(codspeed))]
#[global_allocator]
static ALLOC: divan::AllocProfiler = divan::AllocProfiler::system();

fn main() {
    divan::main();
}

/// Generate N random memtrack events with a seeded RNG
fn generate_events(n: usize) -> Vec<MemtrackEvent> {
    let mut rng = StdRng::seed_from_u64(12345);
    let mut events = Vec::with_capacity(n);
    for _ in 0..n {
        let size = rng.gen_range(8..8192);
        let kind = match rng.gen_range(0..10) {
            0 => MemtrackEventKind::Malloc {
                size,
                stack_hash: 0,
            },
            1 => MemtrackEventKind::Free { stack_hash: 0 },
            2 => MemtrackEventKind::Realloc {
                old_addr: Some(rng.r#gen()),
                size,
                stack_hash: 0,
            },
            3 => MemtrackEventKind::Calloc {
                size,
                stack_hash: 0,
            },
            4 => MemtrackEventKind::AlignedAlloc {
                size,
                stack_hash: 0,
            },
            5 => MemtrackEventKind::Mmap { size },
            6 => MemtrackEventKind::Munmap { size },
            7 => MemtrackEventKind::Brk { size },
            8 => MemtrackEventKind::Rss {
                member: rng.gen_range(0..4),
                size,
            },
            9 => MemtrackEventKind::Rmap {
                member: rng.gen_range(0..4),
                delta: rng.gen_range(-1024..1024),
            },
            _ => unreachable!(),
        };

        events.push(MemtrackEvent {
            pid: rng.r#gen(),
            tid: rng.r#gen(),
            timestamp: rng.r#gen(),
            addr: rng.r#gen(),
            kind,
        });
    }

    events
}

/// Throughput of the single-threaded writer path: one zstd frame, no pool.
/// This is the per-worker ceiling the parallel encoder scales from.
#[divan::bench(args = [10_000, 100_000], max_time = 5.0)]
fn write_events(bencher: Bencher, n: usize) {
    let events = generate_events(n);
    let artifact_bytes = write_frame(&events).len();

    bencher
        .counter(ItemsCount::new(n))
        .counter(BytesCount::new(artifact_bytes))
        .bench_local(|| write_frame(&events));
}

fn write_frame(events: &[MemtrackEvent]) -> Vec<u8> {
    let mut writer = MemtrackWriter::new(Vec::new()).unwrap();
    for event in events {
        writer.write_event(event).unwrap();
    }
    writer.finish().unwrap()
}

fn generate_realistic_events(n: usize) -> Vec<MemtrackEvent> {
    const SIZES: [u64; 8] = [16, 24, 32, 48, 64, 96, 128, 512];
    let mut rng = StdRng::seed_from_u64(42);
    let mut events = Vec::with_capacity(n);
    let mut live_heap: Vec<u64> = Vec::new();
    let mut live_mmap: Vec<(u64, u64)> = Vec::new();
    let mut free_list: Vec<u64> = Vec::new();
    let mut next_addr: u64 = 0x5555_5555_0000;
    let mut ts: u64 = 1_700_000_000_000_000_000;
    let pid = 4242;
    let tids = [4242, 4243, 4244, 4245];

    while events.len() < n {
        ts += rng.gen_range(50..2_000);
        let tid = tids[rng.gen_range(0..tids.len())];
        let roll = rng.gen_range(0..100);
        let (addr, kind) = if roll < 48 || live_heap.is_empty() {
            let size = if rng.gen_range(0..100) < 95 {
                SIZES[rng.gen_range(0..SIZES.len())]
            } else {
                rng.gen_range(4096..1 << 20)
            };
            let addr = free_list.pop().unwrap_or_else(|| {
                let addr = next_addr;
                next_addr += (size + 15) & !15;
                addr
            });
            let kind = match rng.gen_range(0..20) {
                0 => MemtrackEventKind::Calloc {
                    size,
                    stack_hash: 0,
                },
                1 => MemtrackEventKind::Mmap { size },
                _ => MemtrackEventKind::Malloc {
                    size,
                    stack_hash: 0,
                },
            };
            if let MemtrackEventKind::Mmap { size } = &kind {
                live_mmap.push((addr, *size));
            } else {
                live_heap.push(addr);
            }
            (addr, kind)
        } else if roll < 90 {
            let idx = rng.gen_range(0..live_heap.len() + live_mmap.len());
            if idx < live_heap.len() {
                let addr = live_heap.swap_remove(idx);
                free_list.push(addr);
                (addr, MemtrackEventKind::Free { stack_hash: 0 })
            } else {
                let (addr, size) = live_mmap.swap_remove(idx - live_heap.len());
                free_list.push(addr);
                (addr, MemtrackEventKind::Munmap { size })
            }
        } else {
            let idx = rng.gen_range(0..live_heap.len());
            let old_addr = live_heap[idx];
            let size = SIZES[rng.gen_range(0..SIZES.len())] * 2;
            let new_addr = if rng.r#gen() {
                old_addr
            } else {
                free_list.push(old_addr);
                free_list.swap_remove(rng.gen_range(0..free_list.len()))
            };
            live_heap[idx] = new_addr;
            (
                new_addr,
                MemtrackEventKind::Realloc {
                    old_addr: Some(old_addr),
                    size,
                    stack_hash: 0,
                },
            )
        };
        events.push(MemtrackEvent {
            pid,
            tid,
            timestamp: ts,
            addr,
            kind,
        });
    }

    events
}

/// One event per frame slot of a full window, so every worker count from 1 to
/// `WINDOW_FRAMES` has a frame to take. Sizing below this hides pool scaling:
/// the encoder can only parallelize across whole frames.
const REALISTIC_EVENTS: usize = 16 * 64 * 1024;

/// Throughput of the artifact encoder over a realistic allocation mix, as a
/// function of the worker pool size.
#[divan::bench(args = [1, 2, 4, 8, 16], max_time = 10.0)]
fn encode_events_realistic(bencher: Bencher, n_workers: usize) {
    let events = generate_realistic_events(REALISTIC_EVENTS);
    let artifact_bytes = encode(&events, n_workers).len();

    bencher
        .counter(ItemsCount::new(events.len()))
        .counter(BytesCount::new(artifact_bytes))
        .bench_local(|| encode(&events, n_workers));
}

/// Single-frame throughput on captured stacks: each `Stack` event carries a
/// register set and a raw stack copy, so payloads are orders of magnitude
/// larger than an allocation record and the byte rate is what matters.
#[divan::bench(max_time = 10.0)]
fn write_stack_events(bencher: Bencher) {
    let events = generate_stack_events(16 * 1024);
    let artifact_bytes = write_frame(&events).len();

    bencher
        .counter(ItemsCount::new(events.len()))
        .counter(BytesCount::new(artifact_bytes))
        .bench_local(|| write_frame(&events));
}

fn encode(events: &[MemtrackEvent], n_workers: usize) -> Vec<u8> {
    let mut output = Vec::new();
    encode_events(events.iter().cloned(), &mut output, n_workers).unwrap();
    output
}

/// A stack-capture heavy stream: one `Stack` record per allocation, sized like
/// the kernel's stack copies (2 KiB payload, x86_64 register set).
fn generate_stack_events(n: usize) -> Vec<MemtrackEvent> {
    const STACK_BYTES: usize = 2048;
    let mut rng = StdRng::seed_from_u64(7);
    let mut events = Vec::with_capacity(n * 2);

    while events.len() < n * 2 {
        let hash: u64 = rng.r#gen();
        let record = StackRecord {
            hash,
            sp: 0x7fff_0000_0000 | (rng.gen_range(0..1u64 << 20) << 4),
            regs: (0..33).map(|_| rng.r#gen()).collect(),
            bytes: (0..STACK_BYTES).map(|_| rng.r#gen()).collect(),
            fp_chain: (0..16).map(|_| rng.r#gen()).collect(),
            truncated: false,
        };
        let addr: u64 = rng.r#gen();
        let timestamp: u64 = rng.r#gen();

        events.push(MemtrackEvent {
            pid: 4242,
            tid: 4242,
            timestamp,
            addr,
            kind: MemtrackEventKind::Stack {
                record: Box::new(record),
            },
        });
        events.push(MemtrackEvent {
            pid: 4242,
            tid: 4242,
            timestamp: timestamp + 1,
            addr,
            kind: MemtrackEventKind::Malloc {
                size: rng.gen_range(8..8192),
                stack_hash: hash,
            },
        });
    }

    events
}
