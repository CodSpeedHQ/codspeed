use crate::prelude::*;
use std::fmt;

/// The running kernel's full release, e.g. `6.12.8+`.
fn kernel_release() -> Result<String> {
    const OSRELEASE_PATH: &str = "/proc/sys/kernel/osrelease";

    std::fs::read_to_string(OSRELEASE_PATH)
        .map(|release| release.trim().to_owned())
        .with_context(|| format!("Failed to read {OSRELEASE_PATH}"))
}

/// A kernel release, ordered by `(major, minor)`. The patch level is ignored:
/// features are introduced in merge windows, never in a stable point release.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct KernelVersion {
    major: u32,
    minor: u32,
}

impl KernelVersion {
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    /// The running kernel's release.
    pub fn current() -> Result<Self> {
        let release = kernel_release()?;
        Self::parse(&release).with_context(|| format!("Failed to parse kernel release {release:?}"))
    }

    /// Parse the leading `<major>.<minor>` of a release string, ignoring
    /// whatever follows it (patch level, `-rc`, distro suffix).
    fn parse(release: &str) -> Result<Self> {
        let mut parts = release.trim().split(['.', '-']);
        let major = parts
            .next()
            .and_then(|part| part.parse().ok())
            .context("no major version")?;
        let minor = parts
            .next()
            .and_then(|part| part.parse().ok())
            .context("no minor version")?;
        Ok(Self::new(major, minor))
    }
}

impl fmt::Display for KernelVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_strings() {
        for (release, expected) in [
            ("6.8", KernelVersion::new(6, 8)),
            ("6.8.0-51-generic\n", KernelVersion::new(6, 8)),
            ("5.15.0-1234-aws", KernelVersion::new(5, 15)),
            ("6.15.0-rc3", KernelVersion::new(6, 15)),
            ("7.1.5-arch1-1", KernelVersion::new(7, 1)),
        ] {
            assert_eq!(
                KernelVersion::parse(release).unwrap(),
                expected,
                "{release}"
            );
        }

        assert!(KernelVersion::parse("6").is_err());
        assert!(KernelVersion::parse("").is_err());
        assert!(KernelVersion::parse("linux").is_err());
    }

    /// Minor versions must compare numerically: a lexical comparison would put
    /// 6.15 before 6.9 and misjudge every floor between them.
    #[test]
    fn orders_by_major_then_minor() {
        assert!(KernelVersion::new(6, 9) < KernelVersion::new(6, 15));
        assert!(KernelVersion::new(5, 15) < KernelVersion::new(6, 8));
        assert!(KernelVersion::new(7, 0) > KernelVersion::new(6, 15));
    }

    /// The path and format must match this host's, or every version gate reads
    /// as an error instead of a version.
    #[test]
    fn reads_running_kernel() {
        KernelVersion::current().unwrap();
    }
}
