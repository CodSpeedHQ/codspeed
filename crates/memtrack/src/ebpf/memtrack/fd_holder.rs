//! Fork children that hold duplicate fd references, so the terminal close
//! happens in a disposable process rather than here.
//!
//! Closing the last reference to a BPF link fd waits for an RCU-tasks-trace
//! grace period and can hang if the kernel is wedged. [`FdHolderSet`]
//! partitions the fds across children so those waits also run in parallel.

use std::ops::Range;
use std::os::fd::RawFd;
use std::time::{Duration, Instant};

use crate::prelude::*;

pub struct FdHolder {
    child_pid: libc::pid_t,
    write_fd: RawFd,
}

/// Close every open fd except those listed in `keep` (must be sorted
/// ascending, e.g. via `sort_unstable`), by sweeping `close_range` over the
/// gaps between them. `close_range` silently ignores fds that are already
/// closed or out of range, so gaps may safely include fds we never opened.
///
/// # Safety
/// Only safe to call in the single-threaded child right after `fork()`,
/// before any allocation, locking, or `Drop` impl runs — see
/// [`FdHolder::spawn`].
unsafe fn close_fds_except(keep: &[RawFd]) {
    let mut lo: u32 = 0;
    for &fd in keep {
        let fd = fd as u32;
        if fd > lo {
            // SAFETY: caller upholds the fork-child, no-allocation contract.
            unsafe {
                libc::close_range(lo, fd - 1, 0);
            }
        }
        lo = fd.saturating_add(1);
    }
    // SAFETY: same as above.
    unsafe {
        libc::close_range(lo, u32::MAX, 0);
    }
}

impl FdHolder {
    /// Fork a child that owns `all_fds[own]` once the caller drops its copies.
    /// The child waits for a byte or EOF on a private pipe, then exits.
    ///
    /// `fork()` duplicates the whole fd table, so the child first closes
    /// everything but its chunk and the pipe read end; otherwise it would keep
    /// unrelated resources alive. It runs only async-signal-safe libc calls,
    /// since forking a multithreaded process leaves locks and allocator state
    /// unusable.
    pub fn spawn(all_fds: &[RawFd], own: Range<usize>) -> std::io::Result<Self> {
        let mut fds = [0i32; 2];
        // SAFETY: `fds` points to two valid `i32`s, as `pipe(2)` requires.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let [read_fd, write_fd] = fds;

        // Built in the parent, where allocation is still safe. `write_fd` is
        // excluded on purpose: the child must give it up, and the sweep closes
        // anything absent from this list.
        let mut keep: Vec<RawFd> = Vec::with_capacity(all_fds[own.clone()].len() + 1);
        keep.push(read_fd);
        keep.extend_from_slice(&all_fds[own.clone()]);
        keep.sort_unstable();

        // SAFETY: `fork()` itself is always safe to call; the child branch
        // below is restricted to async-signal-safe libc calls until `_exit`.
        let pid = unsafe { libc::fork() };
        match pid {
            -1 => {
                let err = std::io::Error::last_os_error();
                // SAFETY: both fds were just opened by us above.
                unsafe {
                    libc::close(read_fd);
                    libc::close(write_fd);
                }
                Err(err)
            }
            0 => {
                // Keep only the chunk and pipe read end; no Rust code after fork.
                unsafe {
                    close_fds_except(&keep);
                    let mut buf = [0u8; 1];
                    loop {
                        let n = libc::read(read_fd, buf.as_mut_ptr().cast(), buf.len());
                        if n >= 0 {
                            break;
                        }
                    }
                    libc::_exit(0);
                }
            }
            child_pid => {
                // SAFETY: `read_fd` was just opened by us above.
                unsafe {
                    libc::close(read_fd);
                }
                Ok(Self {
                    child_pid,
                    write_fd,
                })
            }
        }
    }

    /// Tell the child to exit. Idempotent, and does not wait for the exit —
    /// see [`Self::release`] and [`FdHolderSet::release_all`] for that.
    fn signal_release(&mut self) {
        if self.write_fd >= 0 {
            // SAFETY: `write_fd` is our open pipe write fd. Writing a byte
            // ensures the child's `read` returns immediately without
            // depending on whether sibling children inherited `write_fd`.
            unsafe {
                let byte = 0u8;
                libc::write(self.write_fd, (&byte as *const u8).cast(), 1);
                libc::close(self.write_fd);
            }
            self.write_fd = -1;
        }
    }

    /// Signal the child and wait up to `timeout` for it to exit.
    ///
    /// `false` means the holder is abandoned: the kernel is still tearing down
    /// its fds, and init reaps it once that finishes.
    pub fn release(mut self, timeout: Duration) -> bool {
        self.signal_release();

        let deadline = Instant::now() + timeout;
        loop {
            let mut status = 0i32;
            // SAFETY: `child_pid` is our own child; `status` is a valid
            // out-pointer. `WNOHANG` never blocks.
            let ret = unsafe { libc::waitpid(self.child_pid, &mut status, libc::WNOHANG) };
            if ret == self.child_pid {
                return true;
            }
            if ret == -1 {
                // ECHILD: nothing left to wait for, already reaped. Any
                // other errno (notably EINTR) is transient — fall through
                // and retry instead of reporting a false success.
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) {
                    return true;
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for FdHolder {
    fn drop(&mut self) {
        self.signal_release();
    }
}

/// Forked holders for disjoint fd chunks.
pub struct FdHolderSet(Vec<FdHolder>);

impl FdHolderSet {
    /// Fork up to `k` holders over roughly equal, contiguous chunks of `fds`.
    ///
    /// If a fork fails, release holders already created and return an empty set
    /// so the caller falls back to direct teardown.
    pub fn spawn(fds: &[RawFd], k: usize) -> Self {
        if fds.is_empty() {
            return Self(Vec::new());
        }
        let k = k.clamp(1, fds.len());
        let chunk_len = fds.len().div_ceil(k);

        let mut holders = Vec::with_capacity(k);
        for start in (0..fds.len()).step_by(chunk_len) {
            let own = start..(start + chunk_len).min(fds.len());
            match FdHolder::spawn(fds, own) {
                Ok(holder) => holders.push(holder),
                Err(err) => {
                    debug!(
                        "Failed to fork fd holder child ({err:#}); falling back to a direct drop for all {} fds",
                        fds.len()
                    );
                    for holder in holders {
                        holder.release(Duration::from_secs(5));
                    }
                    return Self(Vec::new());
                }
            }
        }
        Self(holders)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Signal every holder first, then poll them against one shared `timeout`,
    /// instead of spending a separate budget on each.
    ///
    /// `false` means at least one holder is abandoned: the kernel is still
    /// tearing down its fds, and init reaps them once that finishes.
    pub fn release_all(mut self, timeout: Duration) -> bool {
        for holder in &mut self.0 {
            holder.signal_release();
        }

        let mut pending: Vec<libc::pid_t> = self.0.iter().map(|holder| holder.child_pid).collect();
        let deadline = Instant::now() + timeout;
        loop {
            pending.retain(|&pid| {
                let mut status = 0i32;
                // SAFETY: `pid` is one of our own children; `status` is a
                // valid out-pointer. `WNOHANG` never blocks.
                let ret = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
                if ret == pid {
                    return false;
                }
                if ret == -1 {
                    // ECHILD: nothing left to wait for. Any other errno
                    // (notably EINTR) is transient; keep polling instead of
                    // treating it as a reap.
                    return std::io::Error::last_os_error().raw_os_error() != Some(libc::ECHILD);
                }
                true
            });
            if pending.is_empty() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holder_exits_only_after_release() {
        let holder = FdHolder::spawn(&[], 0..0).unwrap();
        let pid = holder.child_pid;

        let mut status = 0i32;
        // SAFETY: `pid` is our own child; `status` is a valid out-pointer.
        let ret = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        assert_eq!(ret, 0, "holder exited before release() was called");

        assert!(
            holder.release(Duration::from_secs(5)),
            "holder did not exit within the timeout after release()"
        );

        // SAFETY: same as above.
        let ret = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        assert_eq!(ret, -1, "child pid was still waitable after being reaped");
    }

    /// Set a pipe read end non-blocking so a `read` on it reports "no data
    /// yet" (`EAGAIN`) rather than blocking, letting the test distinguish
    /// that from EOF (`read` returning `0`).
    fn set_nonblocking(fd: RawFd) {
        // SAFETY: `fd` is a valid, open fd owned by the caller.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }

    /// `true` if `fd` currently reports EOF (every writer closed), `false`
    /// if it reports "no data yet" (at least one writer still open).
    fn is_eof(fd: RawFd) -> bool {
        let mut buf = [0u8; 1];
        // SAFETY: `fd` is a valid, open, non-blocking pipe read end; `buf`
        // is a valid 1-byte out-buffer.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n == 0 {
            return true;
        }
        assert_eq!(
            n, -1,
            "expected EAGAIN (no data) or EOF (0), got {n} bytes of unexpected data"
        );
        let errno = std::io::Error::last_os_error();
        assert_eq!(
            errno.raw_os_error(),
            Some(libc::EAGAIN),
            "unexpected read error: {errno}"
        );
        false
    }

    /// Poll `fd` for EOF for up to `timeout`, to avoid a race between a
    /// just-`fork`ed child's close sweep and this process's own check.
    fn wait_for_eof(fd: RawFd, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if is_eof(fd) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The fd-ownership contract every consumer relies on: after a chunk's
    /// fds are handed to a holder and the caller closes its own copies,
    /// releasing one holder tears down only *its* chunk, leaving fds owned
    /// by other holders untouched.
    #[test]
    fn release_only_tears_down_its_own_chunk() {
        let mut pipe_a = [0i32; 2];
        let mut pipe_b = [0i32; 2];
        // SAFETY: both arrays point to two valid `i32`s, as `pipe(2)`
        // requires.
        unsafe {
            assert_eq!(libc::pipe(pipe_a.as_mut_ptr()), 0);
            assert_eq!(libc::pipe(pipe_b.as_mut_ptr()), 0);
        }
        let [read_a, write_a] = pipe_a;
        let [read_b, write_b] = pipe_b;
        set_nonblocking(read_a);
        set_nonblocking(read_b);

        let fds = [write_a, write_b];
        let holder_a = FdHolder::spawn(&fds, 0..1).unwrap();
        let holder_b = FdHolder::spawn(&fds, 1..2).unwrap();

        // SAFETY: both fds were just opened by us above.
        unsafe {
            libc::close(write_a);
            libc::close(write_b);
        }

        assert!(!is_eof(read_a), "holder_a should still hold write_a");
        assert!(!is_eof(read_b), "holder_b should still hold write_b");

        assert!(holder_a.release(Duration::from_secs(5)));
        assert!(is_eof(read_a), "releasing holder_a should close write_a");
        assert!(
            !is_eof(read_b),
            "releasing holder_a must not affect holder_b's write_b"
        );

        assert!(holder_b.release(Duration::from_secs(5)));
        assert!(is_eof(read_b), "releasing holder_b should close write_b");

        // SAFETY: our own read ends, still open.
        unsafe {
            libc::close(read_a);
            libc::close(read_b);
        }
    }

    /// A holder must close inherited descriptors outside its assigned chunk, or
    /// those descriptors can keep unrelated pipes or resources alive.
    #[test]
    fn holder_closes_fds_outside_its_chunk() {
        let mut pipe_out = [0i32; 2];
        // SAFETY: `pipe_out` points to two valid `i32`s, as `pipe(2)`
        // requires.
        unsafe {
            assert_eq!(libc::pipe(pipe_out.as_mut_ptr()), 0);
        }
        let [read_out, write_out] = pipe_out;
        set_nonblocking(read_out);

        let holder = FdHolder::spawn(&[], 0..0).unwrap();

        // SAFETY: `write_out` was just opened by us above.
        unsafe {
            libc::close(write_out);
        }

        assert!(
            wait_for_eof(read_out, Duration::from_secs(5)),
            "holder kept an fd open that it was never given ownership of"
        );

        assert!(holder.release(Duration::from_secs(5)));
        // SAFETY: our own read end, still open.
        unsafe {
            libc::close(read_out);
        }
    }
}
