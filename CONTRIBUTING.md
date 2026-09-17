# Contributing to CodSpeed Runner

## Initial Setup

After cloning, install the pre-commit hooks:

```bash
prek install
```

## Release Process

This repository is a Cargo workspace containing multiple crates, but only one of them is released: the main runner. Everything else is linked into its binary.

### Workspace Structure

- **`codspeed-runner`**: The main CLI binary (`codspeed`)
- **`memtrack`**: Memory tracker, built into `codspeed` and reached as `codspeed memtrack`
- **`exec-harness`**: Execution harness, built into `codspeed` and reached as `codspeed exec-harness`
- **`runner-shared`**: Shared library used by other crates

`memtrack` and `exec-harness` are **not released on their own**. They are linked into the
`codspeed` binary and invoked as hidden subcommands, so one tag produces one artifact set and
there is no version for the runner to be out of step with. Their `[[bin]]` targets remain for
development and for the tests, which build them to exercise the standalone path.

Both still keep their own `version` in `Cargo.toml` — that is what
`codspeed exec-harness --version` reports — but bumping it is a plain edit, not a release.

### Pinned binary hashes

Every binary the runner downloads at install time is SHA-256-pinned. The pins live in two places:

- **`src/binary_pins.rs`** — the patched valgrind `.deb` and the mongo-tracer installer. Each artifact keeps its version, URL template, and hash together in a pin record.
- **`src/executor/helpers/introspected_golang/go.sh`** — the go-runner installer published by [CodSpeedHQ/codspeed-go](https://github.com/CodSpeedHQ/codspeed-go), one `<version> <sha256>` row per release in the `GO_RUNNER_INSTALLER_SHA256S` table. `DEFAULT_GO_RUNNER_VERSION` (just below the table) selects the row used by default.

When you bump a pinned version (or add a new go-runner row), update the matching pin record / table row with the new version and its SHA-256.

#### Getting the hash from the verification test

The fastest way to get the right hash is to let the verification test tell you, instead of computing it by hand:

1. Add the new version with a **placeholder** hash — any 64 hex chars work (e.g. copy an existing row's hash).
2. Run the network-bound verification tests, which download every pinned URL and assert the bytes match the declared hash:

   ```bash
   GITHUB_ACTIONS=true cargo test all_pinned
   ```

   The `all_pinned` filter matches **both** tests at once — `all_pinned_binaries_match_their_declared_sha256` (the `src/binary_pins.rs` pins) and `all_pinned_go_runner_installers_match_their_declared_sha256` (the go-runner table). They are skipped unless `GITHUB_ACTIONS` is set.
3. The test fails with `expected <placeholder>, got <actual>`. Paste the `<actual>` value into the table row / pin record and re-run — it should now pass.

You can also compute a hash directly if you prefer:

```bash
curl -sL '<url>' | sha256sum
```

For valgrind, that is one hash per supported `(distro_version, arch)` combination. `src/binary_pins.rs` also holds `VALGRIND_CODSPEED_VERSION` (the upstream semver, used to detect an already-installed copy) and `VALGRIND_DEB_REV` (the `.deb` revision suffix); the `.deb` package version is `{VALGRIND_CODSPEED_VERSION}-{VALGRIND_DEB_REV}`. Bump `VALGRIND_CODSPEED_VERSION` for a new upstream release, and `VALGRIND_DEB_REV` when the same upstream is repackaged.

These tests also run in CI, but running them locally before opening the PR avoids a release-time round trip if a hash is wrong.

### Releasing the Main Runner

The main runner (`codspeed-runner`) is the only crate that is released.

#### Pre-Release Check

Confirm the SHA-256 entries in the pin records in `src/binary_pins.rs` match the released
artifacts they point at.

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
