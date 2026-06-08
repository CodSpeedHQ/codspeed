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
pub const VALGRIND_CODSPEED_ITERATION: u32 = 3;
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
}

impl DistroVersion {
    fn as_str(self) -> &'static str {
        match self {
            DistroVersion::Ubuntu2204 => "22.04",
            DistroVersion::Ubuntu2404 => "24.04",
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
                "2b8c0975e16a771585d5d1e5a61392c2fdc103c53307d04773c9376d7bd782f8"
            }
            (DistroVersion::Ubuntu2404, Arch::Amd64) => {
                "e6da56bd90d7a22ed451c2108002a3f172914c77c705d0c8c4fedf6196830dee"
            }
            (DistroVersion::Ubuntu2204, Arch::Arm64) => {
                "001b6d2a491f6a25235098e4247e3e2983786645920ce0be701a31daa465839f"
            }
            (DistroVersion::Ubuntu2404, Arch::Arm64) => {
                "f40ef91593510c4f9bae3c889055a5442df101e1bcc5adfedf4898d3f50807fc"
            }
        }
    }
}

const MEMTRACK_INSTALLER: BinaryPin = BinaryPin {
    version: "1.2.3",
    url_template: "https://github.com/CodSpeedHQ/codspeed/releases/download/memtrack-v{version}/memtrack-installer.sh",
    sha256: "67f30ebe17d5da4246b51d8663394026385d95203ff09e81289772159e969603",
};
pub const MEMTRACK_VERSION: &str = MEMTRACK_INSTALLER.version;

const EXEC_HARNESS_INSTALLER: BinaryPin = BinaryPin {
    version: "1.3.0",
    url_template: "https://github.com/CodSpeedHQ/codspeed/releases/download/exec-harness-v{version}/exec-harness-installer.sh",
    sha256: "75cbff4fdaefe98927d24fff43fd600c621eb1263b0c40b0fd32c68fa6d88ebd",
};
pub const EXEC_HARNESS_VERSION: &str = EXEC_HARNESS_INSTALLER.version;

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
    MemtrackInstaller,
    ExecHarnessInstaller,
    MongoTracerInstaller,
}

impl PinnedBinary {
    pub fn url(&self) -> String {
        match self {
            PinnedBinary::ValgrindDeb(target) => target.url(),
            PinnedBinary::MemtrackInstaller => MEMTRACK_INSTALLER.url(),
            PinnedBinary::ExecHarnessInstaller => EXEC_HARNESS_INSTALLER.url(),
            PinnedBinary::MongoTracerInstaller => MONGO_TRACER_INSTALLER.url(),
        }
    }

    pub fn sha256(&self) -> &'static str {
        match self {
            PinnedBinary::ValgrindDeb(target) => target.sha256(),
            PinnedBinary::MemtrackInstaller => MEMTRACK_INSTALLER.sha256,
            PinnedBinary::ExecHarnessInstaller => EXEC_HARNESS_INSTALLER.sha256,
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
        PinnedBinary::ExecHarnessInstaller,
        PinnedBinary::MongoTracerInstaller,
    ];

    const ALL_VALGRIND_TARGETS: &[ValgrindTarget] = &[
        ValgrindTarget {
            distro_version: DistroVersion::Ubuntu2204,
            arch: Arch::Amd64,
        },
        ValgrindTarget {
            distro_version: DistroVersion::Ubuntu2404,
            arch: Arch::Amd64,
        },
        ValgrindTarget {
            distro_version: DistroVersion::Ubuntu2204,
            arch: Arch::Arm64,
        },
        ValgrindTarget {
            distro_version: DistroVersion::Ubuntu2404,
            arch: Arch::Arm64,
        },
    ];

    fn assert_installer_variant_is_listed(binary: PinnedBinary) {
        match binary {
            PinnedBinary::ValgrindDeb(_) => {}
            PinnedBinary::MemtrackInstaller
            | PinnedBinary::ExecHarnessInstaller
            | PinnedBinary::MongoTracerInstaller => {
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
        assert_installer_variant_is_listed(PinnedBinary::ExecHarnessInstaller);
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
