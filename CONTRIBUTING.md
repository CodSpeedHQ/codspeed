# Contributing to CodSpeed Runner

## Initial Setup

After cloning, install the pre-commit hooks:

```bash
prek install
```

## Release Process

This repository is a Cargo workspace containing multiple crates. The release process differs depending on which crate you're releasing.

### Workspace Structure

- **`codspeed-runner`**: The main CLI binary (`codspeed`)
- **`memtrack`**: Memory tracking binary (`codspeed-memtrack`)
- **`exec-harness`**: Execution harness binary
- **`runner-shared`**: Shared library used by other crates

### Releasing Support Crates (memtrack, exec-harness, runner-shared)

For any crate other than the main runner:

```bash
cargo release -p <PACKAGE_NAME> --execute <VERSION_BUMP>
```

Where `<VERSION_BUMP>` is one of: `alpha`, `beta`, `patch`, `minor`, or `major`.

**Examples:**

```bash
# Release a new patch version of memtrack
cargo release -p memtrack --execute patch

# Release a beta version of exec-harness
cargo release -p exec-harness --execute beta
```

#### Post-Release: Update Version References

After releasing `memtrack` or `exec-harness`, you **must** update the version references in the runner code:

1. **For memtrack**: Update the `MEMTRACK_INSTALLER` pin record in `src/binary_pins.rs` (see [Pinned binary hashes](#pinned-binary-hashes) below).

2. **For exec-harness**: Update the `EXEC_HARNESS_INSTALLER` pin record in `src/binary_pins.rs`.

These constants are used by the runner to download and install the correct versions of the binaries from GitHub releases.

### Pinned binary hashes

Every binary the runner downloads at install time (the patched valgrind `.deb`, the memtrack installer, the exec-harness installer, the mongo-tracer installer) is SHA-256-pinned. Each artifact keeps its version, URL template, and hash together in `src/binary_pins.rs`.

When you bump a pinned version, regenerate the hash for each affected URL and update the matching pin record:

```bash
curl -sL '<url>' | sha256sum
```

For valgrind, that is one hash per supported `(distro_version, arch)` combination. `src/binary_pins.rs` also holds `VALGRIND_CODSPEED_VERSION` (the upstream semver, used to detect an already-installed copy) and `VALGRIND_DEB_REV` (the `.deb` revision suffix); the `.deb` package version is `{VALGRIND_CODSPEED_VERSION}-{VALGRIND_DEB_REV}`. Bump `VALGRIND_CODSPEED_VERSION` for a new upstream release, and `VALGRIND_DEB_REV` when the same upstream is repackaged.

After updating, run the network-bound verification test that downloads every pinned URL and checks the bytes against the declared hash:

```bash
GITHUB_ACTIONS=true cargo test --lib binary_pins::tests::all_pinned_binaries_match_their_declared_sha256
```

This is also run in CI, but running it locally before opening the PR avoids a release-time round trip if a hash is wrong.

### Releasing the Main Runner

The main runner (`codspeed-runner`) should be released after ensuring all dependency versions are correct.

#### Pre-Release Check

**Verify binary version references**: Check that version constants in the runner code match the released versions:

- `MEMTRACK_VERSION` in `src/binary_pins.rs`
- `EXEC_HARNESS_VERSION` in `src/binary_pins.rs`

Also confirm the SHA-256 entries in the pin records in `src/binary_pins.rs` match the released artifacts.

#### Release Command

```bash
cargo release --execute <VERSION_BUMP>
```

Where `<VERSION_BUMP>` is one of: `alpha`, `beta`, `patch`, `minor`, or `major`.

**Examples:**

```bash
# Release a new minor version
cargo release --execute minor

# Release a patch version
cargo release --execute patch

# Release a beta version
cargo release --execute beta
```

### Release Flow Details

When you run `cargo release --execute <version>`, the following happens:

1. **cargo-release** bumps the version, creates a commit and a git tag, then pushes them to GitHub
2. **GitHub Actions release workflow** triggers on the tag:
   - Custom `cargo-dist` job creates a draft GitHub release
   - `cargo-dist` builds artifacts for all platforms, uploads them to the draft release, and then publishes it
3. Only if it is a runner release:
   - Custom post announce job marks it as "latest" and triggers action repo workflow

This ensures only stable runner releases are marked as "latest" in GitHub.

## Known issue

- If one of the crates is currenlty in beta version, for example the runner is in beta version 4.4.2-beta.1, any alpha release will fail for the any crate, saying that only minor, major or patch releases is supported.

## Testing

- Some tests require `sudo` access. They are skipped by default unless the `GITHUB_ACTIONS` env var is set.
