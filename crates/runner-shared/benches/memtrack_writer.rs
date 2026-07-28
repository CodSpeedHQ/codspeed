use divan::Bencher;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use runner_shared::artifacts::{MemtrackEvent, MemtrackEventKind, MemtrackWriter, encode_events};

fn main() {
    divan::main();
}

/// Generate N random memtrack events with a seeded RNG
fn generate_events(n: usize) -> Vec<MemtrackEvent> {
    let mut rng = StdRng::seed_from_u64(12345);
    let mut events = Vec::with_capacity(n);
    for _ in 0..n {
        let size = rng.gen_range(8..8192);
        let kind = match rng.gen_range(0..8) {
            0 => MemtrackEventKind::Malloc { size },
            1 => MemtrackEventKind::Free,
            2 => MemtrackEventKind::Realloc {
                old_addr: Some(rng.r#gen()),
                size,
            },
            3 => MemtrackEventKind::Calloc { size },
            4 => MemtrackEventKind::AlignedAlloc { size },
            5 => MemtrackEventKind::Mmap { size },
            6 => MemtrackEventKind::Munmap { size },
            7 => MemtrackEventKind::Brk { size },
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

#[divan::bench(args = [10_000, 100_000, 500_000, 1_000_000])]
fn write_events(bencher: Bencher, n: usize) {
    let events = generate_events(n);

    bencher.bench_local(|| {
        let mut output = Vec::new();
        let mut writer = MemtrackWriter::new(&mut output).unwrap();
        for event in &events {
            writer.write_event(event).unwrap();
        }
        writer.finish().unwrap();
    });
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
                0 => MemtrackEventKind::Calloc { size },
                1 => MemtrackEventKind::Mmap { size },
                _ => MemtrackEventKind::Malloc { size },
            };
            if let MemtrackEventKind::Mmap { size } = kind {
                live_mmap.push((addr, size));
            } else {
                live_heap.push(addr);
            }
            (addr, kind)
        } else if roll < 90 {
            let idx = rng.gen_range(0..live_heap.len() + live_mmap.len());
            if idx < live_heap.len() {
                let addr = live_heap.swap_remove(idx);
                free_list.push(addr);
                (addr, MemtrackEventKind::Free)
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

const REALISTIC_EVENTS: usize = 1_000_000;

#[divan::bench(args = [16, 8, 4], max_time = 10.0)]
fn encode_events_realistic(bencher: Bencher, n_workers: usize) {
    let events = generate_realistic_events(REALISTIC_EVENTS);

    bencher.bench_local(|| {
        let mut output = Vec::new();
        encode_events(events.iter().copied(), &mut output, n_workers).unwrap();
        output
    });
}
