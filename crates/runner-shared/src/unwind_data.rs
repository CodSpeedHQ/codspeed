use core::{
    fmt::Debug,
    hash::{Hash, Hasher},
};
use serde::{Deserialize, Serialize};
use std::io::BufWriter;
use std::{hash::DefaultHasher, ops::Range};

pub const UNWIND_FILE_EXT: &str = "unwind_data";

pub type UnwindData = UnwindDataV2;

impl UnwindDataV3 {
    pub fn parse(reader: &[u8]) -> anyhow::Result<Self> {
        let compat: UnwindDataCompat = bincode::deserialize(reader)?;

        match compat {
            UnwindDataCompat::V1(_) => {
                anyhow::bail!("Cannot parse V1 unwind data as V3 (breaking changes)")
            }
            UnwindDataCompat::V2(_) => {
                anyhow::bail!("Cannot parse V2 unwind data as V3 (breaking changes)")
            }
            UnwindDataCompat::V3(v3) => Ok(v3),
        }
    }

    pub fn save_to<P: AsRef<std::path::Path>>(&self, folder: P, key: &str) -> anyhow::Result<()> {
        let path = folder.as_ref().join(format!("{key}.{UNWIND_FILE_EXT}"));
        let compat = UnwindDataCompat::V3(self.clone());
        let file = std::fs::File::create(&path)?;
        const BUFFER_SIZE: usize = 256 * 1024;
        let writer = BufWriter::with_capacity(BUFFER_SIZE, file);
        bincode::serialize_into(writer, &compat)?;
        Ok(())
    }
}

/// A versioned enum for `UnwindData` to allow for future extensions while maintaining backward compatibility.
#[derive(Serialize, Deserialize)]
enum UnwindDataCompat {
    V1(UnwindDataV1),
    V2(UnwindDataV2),
    V3(UnwindDataV3),
}

#[doc(hidden)]
#[derive(Serialize, Deserialize, Clone)]
struct UnwindDataV1 {
    pub path: String,

    pub avma_range: Range<u64>,
    pub base_avma: u64,
    pub base_svma: u64,

    pub eh_frame_hdr: Vec<u8>,
    pub eh_frame_hdr_svma: Range<u64>,

    pub eh_frame: Vec<u8>,
    pub eh_frame_svma: Range<u64>,
}

#[doc(hidden)]
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct UnwindDataV2 {
    pub path: String,

    /// The monotonic timestamp when the unwind data was captured.
    /// Is `None` if unwind data is valid for the whole program execution
    pub timestamp: Option<u64>,

    pub avma_range: Range<u64>,
    pub base_avma: u64,
    pub base_svma: u64,

    pub eh_frame_hdr: Vec<u8>,
    pub eh_frame_hdr_svma: Range<u64>,

    pub eh_frame: Vec<u8>,
    pub eh_frame_svma: Range<u64>,
}

impl UnwindDataV2 {
    /// Parse unwind data bytes, converting V1 to V2 but erroring on V3
    /// (since V3 doesn't have the per-pid fields needed for V2).
    pub fn parse(reader: &[u8]) -> anyhow::Result<Self> {
        let compat: UnwindDataCompat = bincode::deserialize(reader)?;
        match compat {
            UnwindDataCompat::V1(v1) => Ok(v1.into()),
            UnwindDataCompat::V2(v2) => Ok(v2),
            UnwindDataCompat::V3(_) => {
                anyhow::bail!("Cannot parse V3 unwind data as V2 (missing per-pid fields)")
            }
        }
    }

    /// Will be removed once the backend has been deployed and we can merge the changes in the runner
    pub fn save_to<P: AsRef<std::path::Path>>(&self, folder: P, pid: i32) -> anyhow::Result<()> {
        let unwind_data_path = folder.as_ref().join(format!(
            "{}_{:x}_{:x}_{}.{UNWIND_FILE_EXT}",
            pid,
            self.avma_range.start,
            self.avma_range.end,
            self.timestamp.unwrap_or_default()
        ));
        self.to_file(unwind_data_path)?;

        Ok(())
    }

    pub fn to_file<P: AsRef<std::path::Path>>(&self, path: P) -> anyhow::Result<()> {
        if let Ok(true) = std::fs::exists(path.as_ref()) {
            // This happens in CI for the root `systemd-run` process which execs into bash which
            // also execs into bash, each process reloading common libraries like `ld-linux.so`.
            // We detect this when we harvest unwind_data by parsing the perf data (exec-harness).
            // Until we properly handle the process tree and deduplicate unwind data, just debug
            // log here
            // Any relevant occurence should have other symptoms reported by users.
            log::debug!(
                "{} already exists, file will be truncated",
                path.as_ref().display()
            );
            log::debug!("{} {:x?}", self.path, self.avma_range);
        }

        let compat = UnwindDataCompat::V2(self.clone());
        let file = std::fs::File::create(path.as_ref())?;
        const BUFFER_SIZE: usize = 256 * 1024 /* 256 KB */;

        let writer = BufWriter::with_capacity(BUFFER_SIZE, file);
        bincode::serialize_into(writer, &compat)?;

        Ok(())
    }
}

impl From<UnwindDataV1> for UnwindDataV2 {
    fn from(v1: UnwindDataV1) -> Self {
        Self {
            path: v1.path,
            timestamp: None,
            avma_range: v1.avma_range,
            base_avma: v1.base_avma,
            base_svma: v1.base_svma,
            eh_frame_hdr: v1.eh_frame_hdr,
            eh_frame_hdr_svma: v1.eh_frame_hdr_svma,
            eh_frame: v1.eh_frame,
            eh_frame_svma: v1.eh_frame_svma,
        }
    }
}

/// Pid-agnostic unwind data.
/// Contains only the data that is common across all PIDs loading the same shared library.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
pub struct UnwindDataV3 {
    pub path: String,
    pub base_svma: u64,
    pub eh_frame_hdr: Vec<u8>,
    pub eh_frame_hdr_svma: Range<u64>,
    pub eh_frame: Vec<u8>,
    pub eh_frame_svma: Range<u64>,
}

impl From<UnwindDataV2> for UnwindDataV3 {
    fn from(v2: UnwindDataV2) -> Self {
        Self {
            path: v2.path,
            base_svma: v2.base_svma,
            eh_frame_hdr: v2.eh_frame_hdr,
            eh_frame_hdr_svma: v2.eh_frame_hdr_svma,
            eh_frame: v2.eh_frame,
            eh_frame_svma: v2.eh_frame_svma,
        }
    }
}

impl Debug for UnwindData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let eh_frame_hdr_hash = {
            let mut hasher = DefaultHasher::new();
            self.eh_frame_hdr.hash(&mut hasher);
            hasher.finish()
        };
        let eh_frame_hash = {
            let mut hasher = DefaultHasher::new();
            self.eh_frame.hash(&mut hasher);
            hasher.finish()
        };

        f.debug_struct("UnwindData")
            .field("path", &self.path)
            .field("timestamp", &self.timestamp)
            .field("avma_range", &format_args!("{:x?}", self.avma_range))
            .field("base_avma", &format_args!("{:x}", self.base_avma))
            .field("base_svma", &format_args!("{:x}", self.base_svma))
            .field(
                "eh_frame_hdr_svma",
                &format_args!("{:x?}", self.eh_frame_hdr_svma),
            )
            .field("eh_frame_hdr_hash", &format_args!("{eh_frame_hdr_hash:x}"))
            .field("eh_frame_hash", &format_args!("{eh_frame_hash:x}"))
            .field("eh_frame_svma", &format_args!("{:x?}", self.eh_frame_svma))
            .finish()
    }
}

impl Debug for UnwindDataV3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let eh_frame_hdr_hash = {
            let mut hasher = DefaultHasher::new();
            self.eh_frame_hdr.hash(&mut hasher);
            hasher.finish()
        };
        let eh_frame_hash = {
            let mut hasher = DefaultHasher::new();
            self.eh_frame.hash(&mut hasher);
            hasher.finish()
        };

        f.debug_struct("UnwindData")
            .field("path", &self.path)
            .field("base_svma", &format_args!("{:x}", self.base_svma))
            .field(
                "eh_frame_hdr_svma",
                &format_args!("{:x?}", self.eh_frame_hdr_svma),
            )
            .field("eh_frame_hdr_hash", &format_args!("{eh_frame_hdr_hash:x}"))
            .field("eh_frame_hash", &format_args!("{eh_frame_hash:x}"))
            .field("eh_frame_svma", &format_args!("{:x?}", self.eh_frame_svma))
            .finish()
    }
}

/// Per-pid mounting info referencing a deduplicated unwind data entry.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MappedProcessUnwindData {
    pub unwind_data_key: String,
    #[serde(flatten)]
    pub inner: ProcessUnwindData,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ProcessUnwindData {
    pub timestamp: Option<u64>,
    pub avma_range: Range<u64>,
    pub base_avma: u64,
}

impl Debug for ProcessUnwindData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessUnwindData")
            .field("timestamp", &self.timestamp)
            .field("avma_range", &format_args!("{:x?}", self.avma_range))
            .field("base_avma", &format_args!("{:x}", self.base_avma))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V2_BINARY: &[u8] = include_bytes!("../testdata/unwind_data_v2.bin");
    const V3_BINARY: &[u8] = include_bytes!("../testdata/unwind_data_v3.bin");

    fn create_sample_v2() -> UnwindDataV2 {
        UnwindDataV2 {
            path: "/lib/test.so".to_string(),
            timestamp: Some(12345),
            avma_range: 0x1000..0x2000,
            base_avma: 0x1000,
            base_svma: 0x0,
            eh_frame_hdr: vec![1, 2, 3, 4],
            eh_frame_hdr_svma: 0x100..0x200,
            eh_frame: vec![5, 6, 7, 8],
            eh_frame_svma: 0x200..0x300,
        }
    }

    fn create_sample_v3() -> UnwindDataV3 {
        UnwindDataV3 {
            path: "/lib/test.so".to_string(),
            base_svma: 0x0,
            eh_frame_hdr: vec![1, 2, 3, 4],
            eh_frame_hdr_svma: 0x100..0x200,
            eh_frame: vec![5, 6, 7, 8],
            eh_frame_svma: 0x200..0x300,
        }
    }

    #[test]
    fn test_parse_v2_as_v3_should_error() {
        // Try to parse V2 binary artifact as V3 using UnwindData::parse
        let result = UnwindDataV3::parse(V2_BINARY);

        // Should error due to breaking changes between V2 and V3
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("Cannot parse V2 unwind data as V3"),
            "Expected error message about V2->V3 incompatibility, got: {err}"
        );
    }

    #[test]
    fn test_parse_v3_as_v2_should_error() {
        // Try to parse V3 binary artifact as V2 using UnwindDataV2::parse
        let result = UnwindDataV2::parse(V3_BINARY);

        // Should error with specific message about missing per-pid fields
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("Cannot parse V3 unwind data as V2"),
            "Expected error message about V3->V2 incompatibility, got: {err}"
        );
    }

    #[test]
    fn test_parse_v3_as_v3() {
        // Parse V3 binary artifact as V3 using UnwindData::parse
        let parsed_v3 = UnwindDataV3::parse(V3_BINARY).expect("Failed to parse V3 data as V3");

        // Should match expected V3 data
        let expected_v3 = create_sample_v3();
        assert_eq!(parsed_v3, expected_v3);
    }

    #[test]
    fn test_parse_v2_as_v2() {
        // Parse V2 binary artifact as V2 using UnwindDataV2::parse
        let parsed_v2 = UnwindDataV2::parse(V2_BINARY).expect("Failed to parse V2 data as V2");

        // Should match expected V2 data
        let expected_v2 = create_sample_v2();
        assert_eq!(parsed_v2.path, expected_v2.path);
        assert_eq!(parsed_v2.timestamp, expected_v2.timestamp);
        assert_eq!(parsed_v2.avma_range, expected_v2.avma_range);
        assert_eq!(parsed_v2.base_avma, expected_v2.base_avma);
        assert_eq!(parsed_v2.base_svma, expected_v2.base_svma);
        assert_eq!(parsed_v2.eh_frame_hdr, expected_v2.eh_frame_hdr);
        assert_eq!(parsed_v2.eh_frame_hdr_svma, expected_v2.eh_frame_hdr_svma);
        assert_eq!(parsed_v2.eh_frame, expected_v2.eh_frame);
        assert_eq!(parsed_v2.eh_frame_svma, expected_v2.eh_frame_svma);
    }
}
