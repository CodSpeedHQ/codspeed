use crate::prelude::*;
use perf_event_open_sys::bindings::{
    PERF_COUNT_SW_DUMMY, PERF_FLAG_FD_CLOEXEC, PERF_RECORD_LOST, PERF_RECORD_MMAP2,
    PERF_SAMPLE_TID, PERF_SAMPLE_TIME, PERF_TYPE_SOFTWARE, perf_event_attr, perf_event_header,
    perf_event_mmap_page,
};
use runner_shared::artifacts::{MemtrackEvent, MemtrackEventKind};
use std::io;
use std::mem::size_of;
use std::os::fd::RawFd;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

const DATA_PAGES: usize = 64;

struct PerfRing {
    fd: RawFd,
    mapping: *mut u8,
    mapping_len: usize,
    data_offset: usize,
    data_size: usize,
    enabled: bool,
}

// The mapping is exclusively consumed by the poll thread.
unsafe impl Send for PerfRing {}

impl PerfRing {
    fn open(pid: libc::pid_t, cpu: u32, page_size: usize) -> Result<Self> {
        let mapping_len = page_size
            .checked_mul(DATA_PAGES + 1)
            .context("perf ring mapping size overflow")?;
        ensure!(
            mapping_len >= size_of::<perf_event_mmap_page>(),
            "perf ring mapping is smaller than its metadata page"
        );
        let mut attr = perf_event_attr {
            type_: PERF_TYPE_SOFTWARE,
            size: size_of::<perf_event_attr>() as u32,
            config: PERF_COUNT_SW_DUMMY as u64,
            sample_type: (PERF_SAMPLE_TID | PERF_SAMPLE_TIME) as u64,
            // PERF_FORMAT_LOST cannot account for inherited child events from this
            // parent fd, so PERF_RECORD_LOST remains the complete loss signal.
            read_format: 0,
            clockid: libc::CLOCK_MONOTONIC,
            ..Default::default()
        };
        attr.__bindgen_anon_2.wakeup_events = 1;
        attr.set_disabled(1);
        attr.set_inherit(1);
        attr.set_mmap(1);
        attr.set_sample_id_all(1);
        attr.set_mmap2(1);
        attr.set_use_clockid(1);

        let fd = unsafe {
            perf_event_open_sys::perf_event_open(
                &mut attr,
                pid,
                cpu as _,
                -1,
                PERF_FLAG_FD_CLOEXEC as _,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error())
                .with_context(|| format!("perf_event_open failed for pid {pid} on CPU {cpu}"));
        }

        let mapping = unsafe {
            libc::mmap(
                ptr::null_mut(),
                mapping_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if mapping == libc::MAP_FAILED {
            let error = io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(error).context("failed to mmap perf mapping-event ring buffer");
        }

        let page = unsafe { &*(mapping.cast::<perf_event_mmap_page>()) };
        let data_offset = match usize::try_from(page.data_offset) {
            Ok(value) => value,
            Err(_) => {
                unsafe {
                    libc::munmap(mapping, mapping_len);
                    libc::close(fd);
                }
                bail!("kernel returned an invalid perf ring data offset");
            }
        };
        let data_size = match usize::try_from(page.data_size) {
            Ok(value) => value,
            Err(_) => {
                unsafe {
                    libc::munmap(mapping, mapping_len);
                    libc::close(fd);
                }
                bail!("kernel returned an invalid perf ring data size");
            }
        };
        let ring = Self {
            fd,
            mapping: mapping.cast(),
            mapping_len,
            data_offset,
            data_size,
            enabled: false,
        };

        ensure!(
            data_offset >= page_size && data_offset % page_size == 0,
            "kernel returned an invalid perf ring data offset"
        );
        ensure!(
            data_size >= size_of::<perf_event_header>()
                && data_size % page_size == 0
                && data_size.is_power_of_two(),
            "kernel returned an invalid perf ring data size"
        );
        let data_end = data_offset
            .checked_add(data_size)
            .context("perf ring data range overflow")?;
        ensure!(
            data_end <= mapping_len,
            "kernel returned a perf ring outside the mapped area"
        );

        Ok(ring)
    }

    fn enable(&mut self) -> Result<()> {
        if unsafe { perf_event_open_sys::ioctls::ENABLE(self.fd, 0) } < 0 {
            return Err(io::Error::last_os_error()).context("failed to enable perf mapping events");
        }
        self.enabled = true;
        Ok(())
    }

    fn drain(&mut self, mappings: &mut Vec<MemtrackEvent>, lost: &AtomicU64) {
        let page = unsafe { &mut *(self.mapping.cast::<perf_event_mmap_page>()) };
        let head = unsafe { ptr::read_volatile(&page.data_head) };
        std::sync::atomic::fence(Ordering::Acquire);
        let mut tail = unsafe { ptr::read_volatile(&page.data_tail) };
        let available = head.wrapping_sub(tail);

        // Once the producer has lapped the consumer, the beginning of the
        // stream no longer has a record boundary. Skip the corrupt prefix and
        // let the kernel's PERF_RECORD_LOST record account for normal overflow.
        if available > self.data_size as u64 {
            lost.fetch_add(1, Ordering::Relaxed);
            tail = head;
        } else {
            while tail != head {
                let available = head.wrapping_sub(tail);
                if available < size_of::<perf_event_header>() as u64 {
                    lost.fetch_add(1, Ordering::Relaxed);
                    tail = head;
                    break;
                }

                let header = self.copy_from_ring(tail, size_of::<perf_event_header>());
                let size = u16::from_ne_bytes([header[6], header[7]]) as usize;
                if !(size_of::<perf_event_header>()..=self.data_size).contains(&size)
                    || size as u64 > available
                {
                    lost.fetch_add(1, Ordering::Relaxed);
                    tail = head;
                    break;
                }

                let record = self.copy_from_ring(tail, size);
                self.handle_record(&record, mappings, lost);
                tail = tail.wrapping_add(size as u64);
            }
        }

        std::sync::atomic::fence(Ordering::Release);
        unsafe { ptr::write_volatile(&mut page.data_tail, tail) };
    }

    fn copy_from_ring(&self, offset: u64, len: usize) -> Vec<u8> {
        debug_assert!(len <= self.data_size);
        let start = offset as usize & (self.data_size - 1);
        let first_len = len.min(self.data_size - start);
        let data = unsafe { self.mapping.add(self.data_offset) };
        let mut out = Vec::with_capacity(len);
        unsafe {
            out.extend_from_slice(std::slice::from_raw_parts(data.add(start), first_len));
            if first_len < len {
                out.extend_from_slice(std::slice::from_raw_parts(data, len - first_len));
            }
        }
        out
    }

    fn handle_record(&self, record: &[u8], mappings: &mut Vec<MemtrackEvent>, lost: &AtomicU64) {
        match read_u32(record, 0) {
            Some(PERF_RECORD_MMAP2) => {
                if let Some(event) = parse_mmap2(record) {
                    mappings.push(event);
                }
            }
            Some(PERF_RECORD_LOST) => match read_u64(record, 16) {
                Some(count) => {
                    lost.fetch_add(count, Ordering::Relaxed);
                }
                None => {
                    lost.fetch_add(1, Ordering::Relaxed);
                }
            },
            _ => {}
        }
    }
}

impl Drop for PerfRing {
    fn drop(&mut self) {
        unsafe {
            if self.enabled {
                let _ = perf_event_open_sys::ioctls::DISABLE(self.fd, 0);
            }
            libc::munmap(self.mapping.cast(), self.mapping_len);
            libc::close(self.fd);
        }
    }
}

pub(crate) struct PerfMappingPoller {
    ctl: Option<Sender<Sender<()>>>,
    thread: Option<JoinHandle<()>>,
}

impl PerfMappingPoller {
    pub(crate) fn start(
        pid: libc::pid_t,
        tx: Sender<MemtrackEvent>,
        lost: Arc<AtomicU64>,
    ) -> Result<Self> {
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        ensure!(page_size > 0, "failed to read the system page size");

        let cpus = online_cpus()?;
        ensure!(!cpus.is_empty(), "no online CPUs reported by the kernel");
        let mut rings = Vec::with_capacity(cpus.len());
        for cpu in cpus {
            rings.push(PerfRing::open(pid, cpu, page_size as usize)?);
        }
        for ring in &mut rings {
            ring.enable()?;
        }

        let (ctl, ctl_rx) = mpsc::channel::<Sender<()>>();
        let thread = std::thread::spawn(move || {
            let mut mappings = Vec::new();
            loop {
                match ctl_rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(ack) => {
                        for ring in &mut rings {
                            ring.drain(&mut mappings, &lost);
                        }
                        let _ = ack.send(());
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        for ring in &mut rings {
                            ring.drain(&mut mappings, &lost);
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        for ring in &mut rings {
                            ring.drain(&mut mappings, &lost);
                        }
                        mappings.sort_unstable_by_key(|event| (event.pid, event.timestamp));
                        for mapping in mappings {
                            let _ = tx.send(mapping);
                        }
                        break;
                    }
                }
            }
        });

        Ok(Self {
            ctl: Some(ctl),
            thread: Some(thread),
        })
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

fn parse_mmap2(record: &[u8]) -> Option<MemtrackEvent> {
    const FIXED_END: usize = 72;
    const SAMPLE_ID_SIZE: usize = 16;
    if record.len() < FIXED_END + SAMPLE_ID_SIZE
        || read_u32(record, 0)? != PERF_RECORD_MMAP2
        || read_u16(record, 6)? as usize != record.len()
    {
        return None;
    }

    let prot = read_u32(record, 64)?;
    if prot & libc::PROT_EXEC as u32 == 0 {
        return None;
    }

    let path_end = record.len() - SAMPLE_ID_SIZE;
    let path_bytes = &record[FIXED_END..path_end];
    let nul = path_bytes.iter().position(|byte| *byte == 0)?;
    let path = std::str::from_utf8(&path_bytes[..nul]).ok()?;
    if !path.starts_with('/') {
        return None;
    }

    let major = read_u32(record, 40)? as u64;
    let minor = read_u32(record, 44)? as u64;
    Some(MemtrackEvent {
        pid: read_u32(record, 8)? as libc::pid_t,
        tid: read_u32(record, 12)? as libc::pid_t,
        timestamp: read_u64(record, record.len() - 8)?,
        addr: read_u64(record, 16)?,
        kind: MemtrackEventKind::Mapping {
            path: path.to_owned(),
            dev: (major << 20) | minor,
            ino: read_u64(record, 48)?,
            file_offset: read_u64(record, 32)?,
            len: read_u64(record, 24)?,
        },
    })
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_ne_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_ne_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
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

    #[test]
    fn parses_executable_mmap2() {
        let path = b"/tmp/module.so\0";
        let mut record = vec![0; 72 + path.len() + 16];
        record[0..4].copy_from_slice(&PERF_RECORD_MMAP2.to_ne_bytes());
        let size = record.len() as u16;
        record[6..8].copy_from_slice(&size.to_ne_bytes());
        record[8..12].copy_from_slice(&7_u32.to_ne_bytes());
        record[12..16].copy_from_slice(&8_u32.to_ne_bytes());
        record[16..24].copy_from_slice(&0x4000_u64.to_ne_bytes());
        record[24..32].copy_from_slice(&0x2000_u64.to_ne_bytes());
        record[32..40].copy_from_slice(&0x1000_u64.to_ne_bytes());
        record[40..44].copy_from_slice(&1_u32.to_ne_bytes());
        record[44..48].copy_from_slice(&2_u32.to_ne_bytes());
        record[48..56].copy_from_slice(&42_u64.to_ne_bytes());
        record[64..68].copy_from_slice(&(libc::PROT_EXEC as u32).to_ne_bytes());
        record[72..72 + path.len()].copy_from_slice(path);
        let timestamp = 99_u64;
        let time_offset = record.len() - 8;
        record[time_offset..].copy_from_slice(&timestamp.to_ne_bytes());

        assert_eq!(
            parse_mmap2(&record),
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
