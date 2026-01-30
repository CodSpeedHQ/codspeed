#[cfg(use_instrument_hooks)]
mod ffi;

#[cfg(use_instrument_hooks)]
mod linux_impl {

    use super::ffi;
    use std::ffi::CString;
    use std::sync::OnceLock;

    #[derive(PartialEq)]
    pub struct InstrumentHooks(*mut ffi::InstrumentHooks);

    unsafe impl Send for InstrumentHooks {}
    unsafe impl Sync for InstrumentHooks {}

    impl InstrumentHooks {
        #[inline(always)]
        pub fn new() -> Option<Self> {
            let ptr = unsafe { ffi::instrument_hooks_init() };
            if ptr.is_null() {
                None
            } else {
                Some(InstrumentHooks(ptr))
            }
        }

        /// Returns a singleton instance of `InstrumentHooks`.
        #[inline(always)]
        pub fn instance(integration: &str, version: &str) -> &'static Self {
            static INSTANCE: OnceLock<InstrumentHooks> = OnceLock::new();
            INSTANCE.get_or_init(|| {
                let instance =
                    InstrumentHooks::new().expect("Failed to initialize InstrumentHooks");
                instance
                    .set_integration(integration, version)
                    .expect("Failed to set integration");
                instance
            })
        }

        #[inline(always)]
        pub fn is_instrumented(&self) -> bool {
            unsafe { ffi::instrument_hooks_is_instrumented(self.0) }
        }

        #[inline(always)]
        pub fn start_benchmark(&self) -> Result<(), u8> {
            let result = unsafe { ffi::instrument_hooks_start_benchmark(self.0) };
            if result == 0 { Ok(()) } else { Err(result) }
        }

        #[inline(always)]
        pub fn stop_benchmark(&self) -> Result<(), u8> {
            let result = unsafe { ffi::instrument_hooks_stop_benchmark(self.0) };
            if result == 0 { Ok(()) } else { Err(result) }
        }

        #[inline(always)]
        pub fn set_executed_benchmark(&self, uri: &str) -> Result<(), u8> {
            let pid = std::process::id() as i32;
            let c_uri = CString::new(uri).map_err(|_| 1u8)?;
            let result = unsafe {
                ffi::instrument_hooks_set_executed_benchmark(self.0, pid, c_uri.as_ptr())
            };
            if result == 0 { Ok(()) } else { Err(result) }
        }

        #[inline(always)]
        pub fn set_integration(&self, name: &str, version: &str) -> Result<(), u8> {
            let c_name = CString::new(name).map_err(|_| 1u8)?;
            let c_version = CString::new(version).map_err(|_| 1u8)?;
            let result = unsafe {
                ffi::instrument_hooks_set_integration(self.0, c_name.as_ptr(), c_version.as_ptr())
            };
            if result == 0 { Ok(()) } else { Err(result) }
        }

        #[inline(always)]
        pub fn add_benchmark_timestamps(&self, start: u64, end: u64) {
            let pid = std::process::id();

            unsafe {
                ffi::instrument_hooks_add_marker(
                    self.0,
                    pid,
                    ffi::MARKER_TYPE_BENCHMARK_START as u8,
                    start,
                )
            };
            unsafe {
                ffi::instrument_hooks_add_marker(
                    self.0,
                    pid,
                    ffi::MARKER_TYPE_BENCHMARK_END as u8,
                    end,
                )
            };
        }

        #[inline(always)]
        pub fn add_sample_timestamps(&self, start: u64, end: u64) {
            let pid = std::process::id();

            unsafe {
                ffi::instrument_hooks_add_marker(
                    self.0,
                    pid,
                    ffi::MARKER_TYPE_SAMPLE_START as u8,
                    start,
                )
            };

            unsafe {
                ffi::instrument_hooks_add_marker(
                    self.0,
                    pid,
                    ffi::MARKER_TYPE_SAMPLE_END as u8,
                    end,
                )
            };
        }

        #[inline(always)]
        pub fn current_timestamp() -> u64 {
            #[cfg(target_os = "linux")]
            {
                use nix::sys::time::TimeValLike;

                nix::time::clock_gettime(nix::time::ClockId::CLOCK_MONOTONIC)
                    .expect("Failed to get current time")
                    .num_nanoseconds() as u64
            }

            #[cfg(not(target_os = "linux"))]
            unsafe {
                ffi::instrument_hooks_current_timestamp()
            }
        }

        pub fn disable_callgrind_markers() {
            unsafe {
                ffi::instrument_hooks_set_feature(
                    ffi::instrument_hooks_feature_t_FEATURE_DISABLE_CALLGRIND_MARKERS,
                    true,
                )
            };
        }
    }

    impl Drop for InstrumentHooks {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { ffi::instrument_hooks_deinit(self.0) };
            }
        }
    }
}

#[cfg(not(use_instrument_hooks))]
mod other_impl {
    #[derive(PartialEq)]
    pub struct InstrumentHooks;

    impl InstrumentHooks {
        pub fn instance() -> &'static Self {
            static INSTANCE: InstrumentHooks = InstrumentHooks;
            &INSTANCE
        }

        pub fn is_instrumented(&self) -> bool {
            false
        }

        pub fn start_benchmark(&self) -> Result<(), u8> {
            Ok(())
        }

        pub fn stop_benchmark(&self) -> Result<(), u8> {
            Ok(())
        }

        pub fn set_executed_benchmark(&self, _uri: &str) -> Result<(), u8> {
            Ok(())
        }

        pub fn set_integration(&self, _name: &str, _version: &str) -> Result<(), u8> {
            Ok(())
        }

        pub fn add_benchmark_timestamps(&self, _start: u64, _end: u64) {}

        pub fn current_timestamp() -> u64 {
            0
        }

        pub fn disable_callgrind_markers() {}
    }
}

#[cfg(use_instrument_hooks)]
pub use linux_impl::InstrumentHooks;

#[cfg(not(use_instrument_hooks))]
pub use other_impl::InstrumentHooks;

#[cfg(test)]
mod tests {
    use super::InstrumentHooks;

    #[test]
    fn test_instrument_hooks() {
        let hooks = InstrumentHooks::instance("test_integration", "1.0.0");
        assert!(!hooks.is_instrumented() || hooks.start_benchmark().is_ok());
        assert!(hooks.set_executed_benchmark("test_uri").is_ok());
        assert!(hooks.set_integration("test_integration", "1.0.0").is_ok());
        let start = InstrumentHooks::current_timestamp();
        let end = start + 1_000_000; // Simulate 1ms later
        hooks.add_benchmark_timestamps(start, end);
        assert!(!hooks.is_instrumented() || hooks.stop_benchmark().is_ok());
    }
}
