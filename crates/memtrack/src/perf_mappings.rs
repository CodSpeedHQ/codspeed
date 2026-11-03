use crate::prelude::*;
use byteorder::NativeEndian;
use linux_perf_event_reader::{
    Endianness, LostRecord, Mmap2FileId, Mmap2Record, PerfEventAttr, RawData, RawEventRecord,
    RecordParseInfo, RecordType,
};
use perf_event::{Builder, Clock, ReadFormat, SampleFlag, Sampler, events::Software};
use runner_shared::artifacts::{MemtrackEvent, MemtrackEventKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

const DATA_PAGES: usize = 64;

pub(crate) struct PerfMappingPoller {
    ctl: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl PerfMappingPoller {
    pub(crate) fn start(
        pid: libc::pid_t,
        tx: Sender<Vec<MemtrackEvent>>,
        lost: Arc<AtomicU64>,
    ) -> Result<Self> {
        let mut builder = Self::sampler_builder(pid);
        let parse_info = Self::parse_info(&builder)?;
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        ensure!(page_size > 0, "failed to read the system page size");
        let cpus = online_cpus()?;
        ensure!(!cpus.is_empty(), "no online CPUs reported by the kernel");
        let mut samplers = Vec::with_capacity(cpus.len());
        for cpu in cpus {
            // Inheritance with any_cpu() prevents the kernel from creating an mmap ring.
            let sampler = builder
                .one_cpu(cpu as usize)
                .build()
                .with_context(|| format!("perf_event_open failed for pid {pid} on CPU {cpu}"))?
                .sampled(page_size as usize * DATA_PAGES)
                .context("failed to mmap perf mapping-event ring buffer")?;
            samplers.push(sampler);
        }
        for sampler in &mut samplers {
            sampler
                .enable()
                .context("failed to enable perf mapping events")?;
        }

        let (ctl, ctl_rx) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let mut mappings = Vec::new();
            while ctl_rx.recv_timeout(Duration::from_millis(10)) == Err(RecvTimeoutError::Timeout) {
                for sampler in &mut samplers {
                    Self::drain(sampler, parse_info, &mut mappings, &lost);
                }
            }
            for sampler in &mut samplers {
                if let Err(error) = sampler.disable() {
                    log::warn!("failed to disable perf mapping events: {error}");
                }
            }
            for sampler in &mut samplers {
                Self::drain(sampler, parse_info, &mut mappings, &lost);
            }
            mappings.sort_unstable_by_key(|event| (event.pid, event.timestamp));
            if !mappings.is_empty() {
                crate::ebpf::stats::add_sent(mappings.len());
                let _ = tx.send(mappings);
            }
        });

        Ok(Self {
            ctl: Some(ctl),
            thread: Some(thread),
        })
    }

    fn sampler_builder(pid: libc::pid_t) -> Builder<'static> {
        let mut builder = Builder::new(Software::DUMMY);
        builder
            .observe_pid(pid)
            .inherit(true)
            // User mappings do not require permission to profile kernel execution.
            .exclude_kernel(true)
            .exclude_hv(false)
            .mmap(true)
            .mmap2(true)
            .sample(SampleFlag::TID | SampleFlag::TIME)
            .sample_id_all(true)
            // Inherited losses are reported by PERF_RECORD_LOST, not the parent fd.
            .read_format(ReadFormat::empty())
            .clockid(Clock::new(libc::CLOCK_MONOTONIC))
            .wakeup_events(1);
        builder
    }

    fn parse_info(builder: &Builder<'_>) -> Result<RecordParseInfo> {
        let attrs = builder.attrs();
        // The initialized repr(C) attributes use the kernel's perf_event_attr layout.
        let attr_bytes = unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref(attrs).cast::<u8>(),
                std::mem::size_of_val(attrs),
            )
        };
        let (attrs, _) = PerfEventAttr::parse::<_, NativeEndian>(attr_bytes)?;
        Ok(RecordParseInfo::new(&attrs, Endianness::NATIVE))
    }

    fn drain(
        sampler: &mut Sampler,
        parse_info: RecordParseInfo,
        mappings: &mut Vec<MemtrackEvent>,
        lost: &AtomicU64,
    ) {
        while let Some(record) = sampler.next_record() {
            let body = match record.data() {
                [first] => RawData::Single(first),
                [first, second] => RawData::Split(first, second),
                _ => unreachable!("perf records contain one or two slices"),
            };
            match RecordType(record.ty()) {
                RecordType::MMAP2 => {
                    let raw =
                        RawEventRecord::new(RecordType::MMAP2, record.misc(), body, parse_info);
                    if let Some(event) = parse_mmap2(raw) {
                        mappings.push(event);
                    }
                }
                RecordType::LOST => {
                    let count =
                        LostRecord::parse::<NativeEndian>(body).map_or(1, |lost| lost.count);
                    lost.fetch_add(count, Ordering::Relaxed);
                }
                _ => {}
            }
        }
    }
}

impl Drop for PerfMappingPoller {
    fn drop(&mut self) {
        drop(self.ctl.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn parse_mmap2(raw: RawEventRecord<'_>) -> Option<MemtrackEvent> {
    let timestamp = raw.timestamp()?;
    let record = Mmap2Record::parse::<NativeEndian>(raw.data, raw.misc).ok()?;
    if record.protection & libc::PROT_EXEC as u32 == 0 {
        return None;
    }
    let Mmap2FileId::InodeAndVersion(file) = record.file_id else {
        return None;
    };
    let path = record.path.as_slice();
    let path = std::str::from_utf8(&path).ok()?;
    if !path.starts_with('/') {
        return None;
    }

    Some(MemtrackEvent {
        pid: record.pid,
        tid: record.tid,
        timestamp,
        addr: record.address,
        kind: MemtrackEventKind::Mapping {
            path: path.to_owned(),
            dev: (u64::from(file.major) << 20) | u64::from(file.minor),
            ino: file.inode,
            file_offset: record.page_offset,
            len: record.length,
        },
    })
}

fn online_cpus() -> Result<Vec<u32>> {
    let spec = std::fs::read_to_string("/sys/devices/system/cpu/online")
        .context("failed to read online CPUs")?;
    parse_cpu_list(spec.trim())
}

fn parse_cpu_list(spec: &str) -> Result<Vec<u32>> {
    let mut cpus = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        ensure!(!part.is_empty(), "invalid empty CPU range");
        let (start, end) = match part.split_once('-') {
            Some((start, end)) => (start.parse::<u32>()?, end.parse::<u32>()?),
            None => {
                let cpu = part.parse::<u32>()?;
                (cpu, cpu)
            }
        };
        ensure!(start <= end, "invalid CPU range {part}");
        cpus.extend(start..=end);
    }
    Ok(cpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_ranges() {
        assert_eq!(parse_cpu_list("0-2,5,8-9").unwrap(), vec![0, 1, 2, 5, 8, 9]);
    }

    #[rstest::rstest]
    #[case(false)]
    #[case(true)]
    fn parses_executable_mmap2(#[case] include_cpu: bool) {
        let mut builder = PerfMappingPoller::sampler_builder(7);
        if include_cpu {
            builder.sample(SampleFlag::CPU);
        }
        let parse_info = PerfMappingPoller::parse_info(&builder).unwrap();
        let path = b"/tmp/module.so\0";
        let mut record = vec![0; 64 + path.len() + 16];
        record[0..4].copy_from_slice(&7_u32.to_ne_bytes());
        record[4..8].copy_from_slice(&8_u32.to_ne_bytes());
        record[8..16].copy_from_slice(&0x4000_u64.to_ne_bytes());
        record[16..24].copy_from_slice(&0x2000_u64.to_ne_bytes());
        record[24..32].copy_from_slice(&0x1000_u64.to_ne_bytes());
        record[32..36].copy_from_slice(&1_u32.to_ne_bytes());
        record[36..40].copy_from_slice(&2_u32.to_ne_bytes());
        record[40..48].copy_from_slice(&42_u64.to_ne_bytes());
        record[56..60].copy_from_slice(&(libc::PROT_EXEC as u32).to_ne_bytes());
        record[64..64 + path.len()].copy_from_slice(path);
        let timestamp = 99_u64;
        let time_offset = record.len() - 8;
        record[time_offset..].copy_from_slice(&timestamp.to_ne_bytes());
        if include_cpu {
            record.extend_from_slice(&3_u64.to_ne_bytes());
        }

        assert_eq!(
            parse_mmap2(RawEventRecord::new(
                RecordType::MMAP2,
                0,
                RawData::Single(&record),
                parse_info,
            )),
            Some(MemtrackEvent {
                pid: 7,
                tid: 8,
                timestamp,
                addr: 0x4000,
                kind: MemtrackEventKind::Mapping {
                    path: "/tmp/module.so".into(),
                    dev: (1 << 20) | 2,
                    ino: 42,
                    file_offset: 0x1000,
                    len: 0x2000,
                },
            })
        );
    }
}
