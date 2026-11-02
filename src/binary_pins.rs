// Pinned downloads. Each artifact record keeps the version, URL template, and
// expected SHA-256 together so bumps happen in one place. See CONTRIBUTING.md
// for the regeneration workflow.

use semver::Version;
use std::sync::LazyLock;

/// Upstream valgrind-codspeed version. Single source of truth for the .deb
/// download (combined with `VALGRIND_DEB_REV`) and for detecting an already
/// installed copy.
pub const VALGRIND_CODSPEED_VERSION: Version = Version::new(3, 26, 0);
/// CodSpeed repackaging iteration of `VALGRIND_CODSPEED_VERSION`. Bumps when
/// the .deb is repackaged without a new upstream valgrind release. Appears in
/// the .deb package version (`3.26.0-0codspeed3`) and in `valgrind --version`
/// output (`valgrind-3.26.0.codspeed3`).
pub const VALGRIND_CODSPEED_ITERATION: u32 = 12;
/// Suffix appended to `VALGRIND_CODSPEED_VERSION` to form the .deb package version.
static VALGRIND_DEB_REV: LazyLock<String> =
    LazyLock::new(|| format!("0codspeed{VALGRIND_CODSPEED_ITERATION}"));
/// String form of the pinned version as it appears in `valgrind --version`
/// output, used to identify a CodSpeed build at runtime.
pub static VALGRIND_CODSPEED_VERSION_STRING: LazyLock<String> =
    LazyLock::new(|| format!("{VALGRIND_CODSPEED_VERSION}.codspeed{VALGRIND_CODSPEED_ITERATION}"));

#[derive(Debug, Clone, Copy)]
struct BinaryPin {
    version: &'static str,
    url_template: &'static str,
    sha256: &'static str,
}

impl BinaryPin {
    fn url(&self) -> String {
        self.url_template.replace("{version}", self.version)
    }
}

/// Ubuntu release for which CodSpeed publishes a patched valgrind .deb.
/// Variants double as the value used in the download URL and as the
/// installation key, so any `ValgrindTarget` constructed in the runner
/// resolves to a real pin without a runtime fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistroVersion {
    Ubuntu2204,
    Ubuntu2404,
    Ubuntu2604,
}

impl DistroVersion {
    fn as_str(self) -> &'static str {
        match self {
            DistroVersion::Ubuntu2204 => "22.04",
            DistroVersion::Ubuntu2404 => "24.04",
            DistroVersion::Ubuntu2604 => "26.04",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    Amd64,
    Arm64,
}

impl Arch {
    fn as_str(self) -> &'static str {
        match self {
            Arch::Amd64 => "amd64",
            Arch::Arm64 => "arm64",
        }
    }
}

/// A `(DistroVersion, Arch)` pair for which the runner ships a pinned
/// valgrind .deb. Both `url()` and `sha256()` are exhaustive matches over
/// the type, so any value constructible here resolves to a real pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValgrindTarget {
    pub distro_version: DistroVersion,
    pub arch: Arch,
}

static VALGRIND_DEB_VERSION: LazyLock<String> =
    LazyLock::new(|| format!("{VALGRIND_CODSPEED_VERSION}-{}", VALGRIND_DEB_REV.as_str()));
const VALGRIND_DEB_URL_TEMPLATE: &str = "https://github.com/CodSpeedHQ/valgrind-codspeed/releases/download/{version}/valgrind_{version}_ubuntu-{distro_version}_{arch}.deb";

impl ValgrindTarget {
    fn url(self) -> String {
        VALGRIND_DEB_URL_TEMPLATE
            .replace("{version}", &VALGRIND_DEB_VERSION)
            .replace("{distro_version}", self.distro_version.as_str())
            .replace("{arch}", self.arch.as_str())
    }

    fn sha256(self) -> &'static str {
        match (self.distro_version, self.arch) {
            (DistroVersion::Ubuntu2204, Arch::Amd64) => {
                "6997ef32914abc650e48117ff7789528ea1c29cba5fd21e6f57f4a0ec0b8ac78"
            }
            (DistroVersion::Ubuntu2204, Arch::Arm64) => {
                "26dd496d0cdb77365af811b24e48c2039cbb7c5febb7fa00a2a164d5cb9d44ab"
            }
            (DistroVersion::Ubuntu2404, Arch::Amd64) => {
                "92a3883455642b12a8c3d4cfc14469de8f871869bc6a3f494d65522cebf8b124"
            }
            (DistroVersion::Ubuntu2404, Arch::Arm64) => {
                "6c8842dc4976a89281dd4a1cf54fe3d228a56c706e0df17c6e8b64a9b6e33871"
            }
            (DistroVersion::Ubuntu2604, Arch::Amd64) => {
                "f5efc61204c660a02bf99aaf25fee3730d4e8bbd5123f22a0147ad5e9706fa25"
            }
            (DistroVersion::Ubuntu2604, Arch::Arm64) => {
                "e533ddde9f57f6126ff089e53a4c0790ba938075c4ea5a606cd7edb3fc56da24"
            }
        }
    }
}

const MEMTRACK_INSTALLER: BinaryPin = BinaryPin {
    version: "1.6.1",
    url_template: "https://github.com/CodSpeedHQ/codspeed/releases/download/memtrack-v{version}/memtrack-installer.sh",
    sha256: "3912e92bed8fe5d4ad85bb4208edec71e5b707a6fd8fb73dd9625b890b04f438",
};
#[cfg(target_os = "linux")]
pub const MEMTRACK_VERSION: &str = MEMTRACK_INSTALLER.version;

const MONGO_TRACER_INSTALLER: BinaryPin = BinaryPin {
    version: "cs-mongo-tracer-v0.2.0",
    url_template: "https://codspeed-public-assets.s3.eu-west-1.amazonaws.com/mongo-tracer/{version}/cs-mongo-tracer-installer.sh",
    sha256: "685f1d540cb24c2aa6f447991958339c6b70ec7664df2dba2713b8b3d77687e7",
};

/// A binary the runner downloads at install time. The download helper looks
/// up the URL and SHA-256 via `url()` and `sha256()` and rejects the install
/// if the bytes don't match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinnedBinary {
    ValgrindDeb(ValgrindTarget),
    // Only installed by the Linux-only memory executor.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    MemtrackInstaller,
    MongoTracerInstaller,
}

impl PinnedBinary {
    pub fn url(&self) -> String {
        match self {
            PinnedBinary::ValgrindDeb(target) => target.url(),
            PinnedBinary::MemtrackInstaller => MEMTRACK_INSTALLER.url(),
            PinnedBinary::MongoTracerInstaller => MONGO_TRACER_INSTALLER.url(),
        }
    }

    pub fn sha256(&self) -> &'static str {
        match self {
            PinnedBinary::ValgrindDeb(target) => target.sha256(),
            PinnedBinary::MemtrackInstaller => MEMTRACK_INSTALLER.sha256,
            PinnedBinary::MongoTracerInstaller => MONGO_TRACER_INSTALLER.sha256,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::run::helpers::download_pinned_file;
    use tempfile::NamedTempFile;

    const INSTALLER_BINARIES: &[PinnedBinary] = &[
        PinnedBinary::MemtrackInstaller,
        PinnedBinary::MongoTracerInstaller,
    ];

    const ALL_VALGRIND_TARGETS: &[ValgrindTarget] = &[
        ValgrindTarget {
            distro_version: DistroVersion::Ubuntu2204,
            arch: Arch::Amd64,
        },
        ValgrindTarget {
            distro_version: DistroVersion::Ubuntu2204,
            arch: Arch::Arm64,
        },
        ValgrindTarget {
            distro_version: DistroVersion::Ubuntu2404,
            arch: Arch::Amd64,
        },
        ValgrindTarget {
            distro_version: DistroVersion::Ubuntu2404,
            arch: Arch::Arm64,
        },
        ValgrindTarget {
            distro_version: DistroVersion::Ubuntu2604,
            arch: Arch::Amd64,
        },
        ValgrindTarget {
            distro_version: DistroVersion::Ubuntu2604,
            arch: Arch::Arm64,
        },
    ];

    fn assert_installer_variant_is_listed(binary: PinnedBinary) {
        match binary {
            PinnedBinary::ValgrindDeb(_) => {}
            PinnedBinary::MemtrackInstaller | PinnedBinary::MongoTracerInstaller => {
                assert!(INSTALLER_BINARIES.contains(&binary));
            }
        }
    }

    fn all_pinned_binaries() -> impl Iterator<Item = PinnedBinary> {
        ALL_VALGRIND_TARGETS
            .iter()
            .copied()
            .map(PinnedBinary::ValgrindDeb)
            .chain(INSTALLER_BINARIES.iter().copied())
    }

    #[test]
    fn installer_variant_list_is_exhaustive() {
        assert_installer_variant_is_listed(PinnedBinary::MemtrackInstaller);
        assert_installer_variant_is_listed(PinnedBinary::MongoTracerInstaller);
    }

    // Network-bound: downloads every pinned URL and asserts its bytes hash to
    // the declared SHA-256. Skipped locally; CI sets `GITHUB_ACTIONS=true`.
    // Run after bumping a version to make sure the release won't ship a stale
    // or mistyped hash.
    #[test_with::env(GITHUB_ACTIONS)]
    #[tokio::test(flavor = "multi_thread")]
    async fn all_pinned_binaries_match_their_declared_sha256() {
        // Downloads occasionally fail with a 200 with GitHub...
        const MAX_ATTEMPTS: u32 = 3;

        let mut last_failures: Vec<String> = Vec::new();
        for attempt in 1..=MAX_ATTEMPTS {
            let results =
                futures::future::join_all(all_pinned_binaries().map(|binary| async move {
                    let temp = NamedTempFile::new().expect("failed to create temp file");
                    download_pinned_file(binary, temp.path())
                        .await
                        .map_err(|e| format!("{binary:?} ({}): {e}", binary.url()))
                }))
                .await;

            last_failures = results.into_iter().filter_map(Result::err).collect();
            if last_failures.is_empty() {
                return;
            }

            eprintln!(
                "attempt {attempt}/{MAX_ATTEMPTS} failed for {} binaries:\n  - {}",
                last_failures.len(),
                last_failures.join("\n  - "),
            );
        }

        panic!(
            "pinned binaries failed verification after {MAX_ATTEMPTS} attempts:\n  - {}",
            last_failures.join("\n  - "),
        );
    }
}
