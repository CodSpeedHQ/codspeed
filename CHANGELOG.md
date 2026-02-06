# Changelog


<sub>The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).</sub>



## [4.10.5] - 2026-02-06

### <!-- 0 -->🚀 Features
- Support codspeed-node memory profiling by @not-matthias in [#228](https://github.com/CodSpeedHQ/runner/pull/228)

### <!-- 1 -->🐛 Bug Fixes
- Check benchmark URIs rather than memtrack events by @not-matthias in [#230](https://github.com/CodSpeedHQ/runner/pull/230)
- Do not error out if no head report is present after local run by @GuillaumeLagrange in [#233](https://github.com/CodSpeedHQ/runner/pull/233)


## [4.10.4] - 2026-02-02

### <!-- 0 -->🚀 Features
- Check for repository existence before run in local runs by @GuillaumeLagrange in [#229](https://github.com/CodSpeedHQ/runner/pull/229)
- Support pytest-codspeed memory profiling by @not-matthias in [#225](https://github.com/CodSpeedHQ/runner/pull/225)
- Attach to different jemalloc prefixed functions by @not-matthias
- Support tracking `libstdc++` by @not-matthias
- Track `posix_memalign` by @not-matthias

### <!-- 1 -->🐛 Bug Fixes
- Check non-default allocators first by @not-matthias
- Reset owernship by @not-matthias

### <!-- 2 -->🏗️ Refactor
- Use standard allocation API across allocators by @not-matthias in [#212](https://github.com/CodSpeedHQ/runner/pull/212)

### <!-- 7 -->⚙️ Internals
- Use snapshots for c tests by @not-matthias
- Only run tests in CI by @not-matthias
- Use single thread for tests to avoid opening too many files by @not-matthias
- Add cpp allocator test by @not-matthias
- Add rust allocator tests by @not-matthias
- Allow tracking statically linked allocators in tests by @not-matthias


## [4.10.3] - 2026-01-29

### <!-- 1 -->🐛 Bug Fixes
- Deserialize memory results as integer by @not-matthias in [#227](https://github.com/CodSpeedHQ/runner/pull/227)
- Use a BufWriter to write perf.metadata by @GuillaumeLagrange in [#226](https://github.com/CodSpeedHQ/runner/pull/226)


## [4.10.2] - 2026-01-29

### <!-- 0 -->🚀 Features
- Improve efficiency of parse_perf_file by @GuillaumeLagrange
- Parse the perf file memory mappings by @GuillaumeLagrange

### <!-- 1 -->🐛 Bug Fixes
- Remove very spammy trace logs by @GuillaumeLagrange in [#224](https://github.com/CodSpeedHQ/runner/pull/224)

### <!-- 7 -->⚙️ Internals
- Remove useless conversions. by @GuillaumeLagrange


## [4.10.1] - 2026-01-28

### <!-- 0 -->🚀 Features
- Implement `codspeed use` to set mode for shell session by @GuillaumeLagrange in [#221](https://github.com/CodSpeedHQ/runner/pull/221)
- Show codspeed banner in CI when exec_harness is used by @GuillaumeLagrange
- Improve the result table display (#217) by @art049 in [#217](https://github.com/CodSpeedHQ/runner/pull/217)

### <!-- 1 -->🐛 Bug Fixes
- Initialize tracker before spawning sub-process by @not-matthias

### <!-- 2 -->🏗️ Refactor
- Restructure event handling with a common header and improve event parsing by @not-matthias
- Create a module for shared structs between `exec` and `run` by @GuillaumeLagrange
- Remove logic from the run module by @GuillaumeLagrange
- Move the cli related module inside a cli module by @GuillaumeLagrange

### <!-- 7 -->⚙️ Internals
- Bump memtrack version by @adriencaccia
- Turn warning about overwritten unwind_data into debug logs by @GuillaumeLagrange in [#223](https://github.com/CodSpeedHQ/runner/pull/223)
- Fix cargo check on macOs by @adriencaccia
- Remove `codspeed use` without argument in favor of `codspeed show` by @GuillaumeLagrange in [#222](https://github.com/CodSpeedHQ/runner/pull/222)
- Add clang-format pre-commit hook by @not-matthias in [#219](https://github.com/CodSpeedHQ/runner/pull/219)
- Save both old and new addr for `realloc` event by @not-matthias


## [4.10.0] - 2026-01-26

### <!-- 0 -->🚀 Features
- Add pre-commit check to sync schema with code by @GuillaumeLagrange
- Add binary to generate project config schema by @GuillaumeLagrange
- Add command aliases by @art049 in [#216](https://github.com/CodSpeedHQ/runner/pull/216)
- Improve readme by @art049 in [#214](https://github.com/CodSpeedHQ/runner/pull/214)

### <!-- 1 -->🐛 Bug Fixes
- Rename `targets` to `benchmarks` in the project_config to match spec by @GuillaumeLagrange in [#220](https://github.com/CodSpeedHQ/runner/pull/220)
- Use child process instead of pid/perf fifo for FIFO health check by @not-matthias in [#201](https://github.com/CodSpeedHQ/runner/pull/201)
- Make FIFO cancel-safe by @not-matthias
- Dont run health check in hot path by @not-matthias
- Update warning message format for go runner installation by @not-matthias in [#218](https://github.com/CodSpeedHQ/runner/pull/218)
- Tweaked exec-harness build for gcc15+ by @art049

### <!-- 2 -->🏗️ Refactor
- Move the whole runner logic under `lib.rs` to add other binaries by @GuillaumeLagrange

### <!-- 7 -->⚙️ Internals
- Add cancel-safety test for FIFO by @not-matthias
- Remove cargo check from pre-commit by @GuillaumeLagrange in [#215](https://github.com/CodSpeedHQ/runner/pull/215)
- Remove runner_mode from project config by @GuillaumeLagrange


## [4.9.0] - 2026-01-23

### <!-- 0 -->🚀 Features
- Flatten the walltime options in the yaml by @GuillaumeLagrange in [#211](https://github.com/CodSpeedHQ/runner/pull/211)
- Add go-runner-version CLI argument by @not-matthias in [#213](https://github.com/CodSpeedHQ/runner/pull/213)
- Add `allow_empty` in upload metadata by @fargito in [#208](https://github.com/CodSpeedHQ/runner/pull/208)
- No longer warn on missing results by @fargito
- Clean-up the build script by @GuillaumeLagrange
- Bail when used with a statically linked binary by @GuillaumeLagrange
- Embed the preload library in the binary by @GuillaumeLagrange
- Add a codspeed_preload library for valgrind instrumentation by @GuillaumeLagrange
- Print command under test before running it by @GuillaumeLagrange
- Skip upload if `--allow-empty` is set and no results were produced by @fargito in [#205](https://github.com/CodSpeedHQ/runner/pull/205)

### <!-- 1 -->🐛 Bug Fixes
- Follow frames until we find a non inlined functions in debug info by @GuillaumeLagrange in [#202](https://github.com/CodSpeedHQ/runner/pull/202)

### <!-- 2 -->🏗️ Refactor
- Move the preload_file management to its own module by @GuillaumeLagrange

### <!-- 7 -->⚙️ Internals
- Bump exec-harness version by @adriencaccia
- Remove the "Fetching the results" log group by @GuillaumeLagrange
- Make logs text only by @GuillaumeLagrange
- Make the runner less verbose in info logs by @GuillaumeLagrange
- Revamp readme by @art049 in [#178](https://github.com/CodSpeedHQ/runner/pull/178)
- Replace CodSpeedHQ/runner with CodSpeedHQ/codspeed by @art049 in [#210](https://github.com/CodSpeedHQ/runner/pull/210)
- Build exec-harness in gnu for release by @GuillaumeLagrange in [#209](https://github.com/CodSpeedHQ/runner/pull/209)
- Change arm runner by @GuillaumeLagrange
- Move back the received command log to trace by @GuillaumeLagrange


## [4.8.2] - 2026-01-21

### <!-- 1 -->🐛 Bug Fixes
- Check whether artifact contains any elements by @not-matthias in [#204](https://github.com/CodSpeedHQ/runner/pull/204)


## [4.8.1] - 2026-01-21

### <!-- 0 -->🚀 Features
- Check if memtrack produced artifacts by @not-matthias in [#195](https://github.com/CodSpeedHQ/runner/pull/195)
- Require minimum protocol version for memtrack by @not-matthias

### <!-- 1 -->🐛 Bug Fixes
- Add explicit min version check for memory executor by @not-matthias in [#203](https://github.com/CodSpeedHQ/runner/pull/203)


## [4.8.0] - 2026-01-20

### <!-- 0 -->🚀 Features
- Allow defining targets in codspeed yaml by @GuillaumeLagrange in [#197](https://github.com/CodSpeedHQ/runner/pull/197)
- Add an stdin mode to provide multiple commands by @GuillaumeLagrange
- Add a run index suffix by @fargito in [#199](https://github.com/CodSpeedHQ/runner/pull/199)
- Make the config less strict about its config by @GuillaumeLagrange in [#198](https://github.com/CodSpeedHQ/runner/pull/198)
- Add support for analysis mode with memory instrument by @GuillaumeLagrange
- Reduce shared fifo timeout to end executor runs faster after the program has ended by @GuillaumeLagrange in [#193](https://github.com/CodSpeedHQ/runner/pull/193)
- Use a bufwriter to save unwind data by @GuillaumeLagrange
- Support dynamically and statically linked allocators by @not-matthias
- Track `memalign` calls by @not-matthias
- Accept IPC before creating tracker to avoid timeouts by @not-matthias
- Add integration tests for complex cli commands by @GuillaumeLagrange
- Add the full command to the uri and handle hyphens better by @GuillaumeLagrange
- Add yaml project config discovery and loading by @GuillaumeLagrange in [#189](https://github.com/CodSpeedHQ/runner/pull/189)
- Implement the walltime benchmarking algorithm by @GuillaumeLagrange
- Use rayon to collect perf walltime data by @GuillaumeLagrange
- Handle memory executor when displaying results by @GuillaumeLagrange in [#190](https://github.com/CodSpeedHQ/runner/pull/190)
- Do not draw a table if there is only one benchmark by @GuillaumeLagrange
- Stop using the deprecated `time` field and use `value` instead by @GuillaumeLagrange

### <!-- 1 -->🐛 Bug Fixes
- Fix behavior when multiple benchmarks are ran by @GuillaumeLagrange
- Simplify `run_part_id` computation by @fargito in [#200](https://github.com/CodSpeedHQ/runner/pull/200)
- Write perf map all at once instead of line by line by @GuillaumeLagrange
- Drop privileges when running commands under sudo by @not-matthias
- Make walltime results stat computation the same as pytest-codspeed by @GuillaumeLagrange in [#185](https://github.com/CodSpeedHQ/runner/pull/185)

### <!-- 2 -->🏗️ Refactor
- Move project config structs to interfaces mod by @GuillaumeLagrange
- Move all walltime benchmarking to its dedicated module by @GuillaumeLagrange

### <!-- 7 -->⚙️ Internals
- Bump memtrack version by @adriencaccia
- Ignore .codspeed for exec-harness output by @GuillaumeLagrange
- Remove useless clone derive for project config by @GuillaumeLagrange
- Cleanup output by @not-matthias in [#196](https://github.com/CodSpeedHQ/runner/pull/196)
- Use simulation in codspeed action by @GuillaumeLagrange in [#194](https://github.com/CodSpeedHQ/runner/pull/194)
- Cache benchmark executable by @GuillaumeLagrange
- Add prelude and remove linter dead_code suppression by @GuillaumeLagrange
- Reduce logs verbosity of debug info and perf file serialization by @GuillaumeLagrange
- Remove unused execve event by @not-matthias in [#192](https://github.com/CodSpeedHQ/runner/pull/192)
- Add prelude by @not-matthias
- Print number of events written to disk by @not-matthias
- Switch to rust-cache to cache builds of the installed workspace binaries by @GuillaumeLagrange in [#191](https://github.com/CodSpeedHQ/runner/pull/191)
- Install exec-harness before runner tests tests by @GuillaumeLagrange
- Remove double metadata information by @GuillaumeLagrange
- Add link to relevant issue for codspeed-rust metadata by @GuillaumeLagrange
- Use fork supporting compressed events for linux-perf-data by @GuillaumeLagrange
- Run exec-harness tests in separate CI job to prevent fifo issues by @GuillaumeLagrange
- Downgrade flate because 1.1.7 was yanked by @GuillaumeLagrange


## [4.7.0] - 2026-01-12

### <!-- 0 -->🚀 Features
- Add l2d cache events to perf command by @GuillaumeLagrange in [#188](https://github.com/CodSpeedHQ/runner/pull/188)


## [4.6.0] - 2026-01-07

### <!-- 0 -->🚀 Features
- Check for perf compression and enable multi-events conditionally by @GuillaumeLagrange
- Enable perf data compression by @art049
- Add more events to perf monitoring by @GuillaumeLagrange
- Add zstd compression support by @not-matthias

### <!-- 1 -->🐛 Bug Fixes
- Do not cat stdout from command to pipe.data by @GuillaumeLagrange in [#182](https://github.com/CodSpeedHQ/runner/pull/182)
- Drain events arriving after process terminated by @not-matthias
- Support non-x86 libc paths by @not-matthias in [#183](https://github.com/CodSpeedHQ/runner/pull/183)

### <!-- 7 -->⚙️ Internals
- Add instructions event, and use runner_shared for monitored events by @GuillaumeLagrange in [#154](https://github.com/CodSpeedHQ/runner/pull/154)
- Bump memtrack version by @not-matthias in [#187](https://github.com/CodSpeedHQ/runner/pull/187)
- Prevent gh release edit from automatically setting release as latest by @GuillaumeLagrange in [#186](https://github.com/CodSpeedHQ/runner/pull/186)
- Add memtrack serialization benchmark by @not-matthias in [#184](https://github.com/CodSpeedHQ/runner/pull/184)


## [4.5.2] - 2025-12-23

### <!-- 0 -->🚀 Features
- Update release flow to make sure only runner releases are marked as latest (#180) by @GuillaumeLagrange in [#180](https://github.com/CodSpeedHQ/runner/pull/180)
- Improve `UNAUTHENTICATED` error message by @fargito in [#181](https://github.com/CodSpeedHQ/runner/pull/181)

### <!-- 7 -->⚙️ Internals
- Bump cargo dist to 0.30.3 by @art049


## [4.5.1] - 2025-12-22

### <!-- 1 -->🐛 Bug Fixes
- Filter out arm debugging symbols by @GuillaumeLagrange in [#179](https://github.com/CodSpeedHQ/runner/pull/179)


## [4.5.0] - 2025-12-19

### <!-- 0 -->🚀 Features
- Remove projects query from the exec polling by @GuillaumeLagrange in [#173](https://github.com/CodSpeedHQ/runner/pull/173)
- Fetch project from API when running outside of git repo by @GuillaumeLagrange
- Add get or create project repository query by @GuillaumeLagrange
- Automatically install exec-harness for exec subcommand by @GuillaumeLagrange
- Auto install codspeed-memtrack during executor setup by @GuillaumeLagrange
- Serialize events serially to allow streamed decoding by @not-matthias in [#172](https://github.com/CodSpeedHQ/runner/pull/172)
- Parse perf file for memmap events instead of relying on /proc/pid/maps by @GuillaumeLagrange
- Use the projects upload enpdoint in exec command by @GuillaumeLagrange
- Add exec subcommand and refactor run subcommand by @GuillaumeLagrange
- Add exec-harness binary by @GuillaumeLagrange
- Add memory executor by @not-matthias
- Add memtrack crate by @not-matthias
- Add artifact types by @not-matthias
- Add shared fifo by @not-matthias
- Add new fifo commands by @not-matthias
- Support simulation for free-threaded python by @adriencaccia in [#167](https://github.com/CodSpeedHQ/runner/pull/167)

### <!-- 1 -->🐛 Bug Fixes
- Filter out empty named symbols when building perf-map by @GuillaumeLagrange in [#176](https://github.com/CodSpeedHQ/runner/pull/176)
- Do not break support for no reason when changing integration hooks protocol version by @GuillaumeLagrange in [#175](https://github.com/CodSpeedHQ/runner/pull/175)
- Remove dirty retry on timeout and simply increase timeout for GQL client by @GuillaumeLagrange
- Stop hanging indefinitely if process fails to start in memory executor by @GuillaumeLagrange in [#171](https://github.com/CodSpeedHQ/runner/pull/171)
- Remove the password prompt from the run_with_sudo dialog by @GuillaumeLagrange
- Collect events in thread to avoid mutex overhead by @not-matthias
- Convert events in thread to avoid blocking at the end by @not-matthias
- Compress only when size exceeds threshold by @not-matthias
- Forward environment in memory executor by @not-matthias
- Fix plan test in CI by @GuillaumeLagrange in [#165](https://github.com/CodSpeedHQ/runner/pull/165)
- Prevent nextest from running valgrind and memcheck concurrently by @GuillaumeLagrange
- Stop ignoring samples by @GuillaumeLagrange
- Use correct name for unwind_data trait declaration by @GuillaumeLagrange
- Stop filtering out zero sized symbol by @GuillaumeLagrange
- Request OIDC token after creating profile archive by @fargito in [#170](https://github.com/CodSpeedHQ/runner/pull/170)
- Remove snapshots that were not part of lfs by @not-matthias in [#166](https://github.com/CodSpeedHQ/runner/pull/166)
- Always print memory mapping logs by @not-matthias

### <!-- 2 -->🏗️ Refactor
- Create a dedicated execution_context that holds runtime information by @GuillaumeLagrange
- Move executor and instruments modules out of `run` module by @GuillaumeLagrange

### <!-- 7 -->⚙️ Internals
- Ignore some tags in the changelog by @adriencaccia
- Bump protocol version by @not-matthias in [#174](https://github.com/CodSpeedHQ/runner/pull/174)
- Add CONTRIBUTING.md by @GuillaumeLagrange
- Add cargo-dist arguments for release by @GuillaumeLagrange
- Reset exec-harness and memtrack crate versions to 1.0.0 ahead of first release by @GuillaumeLagrange
- Switch to pr run mode plan only for pr by @GuillaumeLagrange
- Add test to ensure path is properly forwarded by @not-matthias in [#169](https://github.com/CodSpeedHQ/runner/pull/169)
- Make the exec command work outside of git repos by @GuillaumeLagrange
- Do not publish memtrack to crates.io by @adriencaccia
- Dont run valgrind and memory tests at the same time by @not-matthias in [#164](https://github.com/CodSpeedHQ/runner/pull/164)
- Add test-log to see output on failures by @not-matthias
- Add workspace dependencies by @not-matthias


## [4.4.1] - 2025-11-21

### <!-- 0 -->🚀 Features
- Display oidc as announcement by @fargito
- Add --allow-empty run option by @GuillaumeLagrange in [#160](https://github.com/CodSpeedHQ/runner/pull/160)

### <!-- 1 -->🐛 Bug Fixes
- Do not espace trailing newlines in logger by @fargito
- Make multiline logs appear correctly in summary by @fargito in [#162](https://github.com/CodSpeedHQ/runner/pull/162)
- Request OIDC token just before upload by @fargito
- Update docs links to oidc by @fargito in [#159](https://github.com/CodSpeedHQ/runner/pull/159)


## [4.4.0] - 2025-11-19

### <!-- 0 -->🚀 Features
- Add support for oidc token authentication by @fargito in [#156](https://github.com/CodSpeedHQ/runner/pull/156)
- Accept simulation as runner mode by @GuillaumeLagrange in [#152](https://github.com/CodSpeedHQ/runner/pull/152)
- Add a comment explaining why we do not check for emptiness in valgrind teardown by @GuillaumeLagrange in [#157](https://github.com/CodSpeedHQ/runner/pull/157)
- Validate walltime results before uploading by @GuillaumeLagrange
- Import walltime_results from monorepo by @GuillaumeLagrange

### <!-- 1 -->🐛 Bug Fixes
- Dont start perf unless it's not already started by @not-matthias in [#158](https://github.com/CodSpeedHQ/runner/pull/158)
- Use a line buffer when reading stdout/stderr streams by @GuillaumeLagrange

### <!-- 7 -->⚙️ Internals
- Update AGENTS.md to use cargo nextest if available by @GuillaumeLagrange


## [4.3.4] - 2025-11-10

### <!-- 0 -->🚀 Features
- Make `get_commit_hash` different based on the provider by @GuillaumeLagrange in [#151](https://github.com/CodSpeedHQ/runner/pull/151)

### <!-- 1 -->🐛 Bug Fixes
- Use GITHUB_WORKSPACE env var when computing root path by @GuillaumeLagrange
- Ensure perf also fails when the command fails by @not-matthias in [#150](https://github.com/CodSpeedHQ/runner/pull/150)
- Ensure working directory is used when running the cmd by @not-matthias
- Use debug! instead of println for debug data by @art049


## [4.3.3] - 2025-11-07

### <!-- 1 -->🐛 Bug Fixes
- Run cp with bash to expand glob patterns by @not-matthias in [#148](https://github.com/CodSpeedHQ/runner/pull/148)


## [4.3.2] - 2025-11-07

### <!-- 0 -->🚀 Features
- Update valgrind codspeed to 3.26.0-0codspeed0 by @adriencaccia in [#147](https://github.com/CodSpeedHQ/runner/pull/147)
- Add --config-name argument to allow multiple configs by @GuillaumeLagrange in [#145](https://github.com/CodSpeedHQ/runner/pull/145)
- Output perf data directly to profile folder by @GuillaumeLagrange in [#138](https://github.com/CodSpeedHQ/runner/pull/138)
- Emit perf data in pipe mode by @art049
- Properly handle sudo with a command builder (#143) by @art049 in [#143](https://github.com/CodSpeedHQ/runner/pull/143)

### <!-- 7 -->⚙️ Internals
- Use info instead of warn for some cache and valgrind setup logs by @adriencaccia in [#142](https://github.com/CodSpeedHQ/runner/pull/142)


## [4.3.1] - 2025-10-24

### <!-- 0 -->🚀 Features
- Enable read-inline-info to support inlined frames by @not-matthias in [#139](https://github.com/CodSpeedHQ/runner/pull/139)

### <!-- 1 -->🐛 Bug Fixes
- Sudo behavior when root or not available (#141) by @art049 in [#141](https://github.com/CodSpeedHQ/runner/pull/141)


## [4.3.0] - 2025-10-23

### <!-- 0 -->🚀 Features
- Allow shorthand for selecting the mode by @art049
- Improve results display when running locally by @art049
- Bump to valgrind-codspeed 3.25.1-3codspeed2 by @art049 in [#137](https://github.com/CodSpeedHQ/runner/pull/137)
- Allow wider range of valgrind codspeed version usage by @art049
- Automatically open the auth URL by @art049 in [#133](https://github.com/CodSpeedHQ/runner/pull/133)
- Proper interactive sudo support by @art049
- Dump debug info of loaded modules by @not-matthias in [#128](https://github.com/CodSpeedHQ/runner/pull/128)

### <!-- 1 -->🐛 Bug Fixes
- Dont immediately add load_bias to symbol offset by @not-matthias

### <!-- 7 -->⚙️ Internals
- Fix fmt error by @adriencaccia


## [4.2.1] - 2025-10-17

### <!-- 0 -->🚀 Features
- Use a prime number as frequency to avoid synchronization with periodic tasks by @adriencaccia

### <!-- 1 -->🐛 Bug Fixes
- Ensure perf is always found on the machine by @adriencaccia in [#132](https://github.com/CodSpeedHQ/runner/pull/132)
- Correctly check if perf is installed by @adriencaccia

### <!-- 7 -->⚙️ Internals
- Add a metadata file in the cache that lists the installed packages by @adriencaccia


## [4.2.0] - 2025-10-16

### <!-- 0 -->🚀 Features
- Allow caching installed executor instruments on ubuntu/debian by @GuillaumeLagrange in [#129](https://github.com/CodSpeedHQ/runner/pull/129)
- Automatically compress archive if profile folder is above a certain threshold by @GuillaumeLagrange

### <!-- 1 -->🐛 Bug Fixes
- Bump git2 to latest to support sparse checkout by @adriencaccia in [#131](https://github.com/CodSpeedHQ/runner/pull/131)

### <!-- 7 -->⚙️ Internals
- Make fifo command dump trace level by @GuillaumeLagrange in [#130](https://github.com/CodSpeedHQ/runner/pull/130)


## [4.1.1] - 2025-10-06

### <!-- 1 -->🐛 Bug Fixes
- Decrease stack sampling size for python (#125) by @not-matthias in [#125](https://github.com/CodSpeedHQ/runner/pull/125)
- Break when parsing invalid command by @not-matthias in [#122](https://github.com/CodSpeedHQ/runner/pull/122)


## [4.1.0] - 2025-10-02

### <!-- 0 -->🚀 Features
- Add timestamp to unwind data by @not-matthias in [#120](https://github.com/CodSpeedHQ/runner/pull/120)
- Add unwind data v2 format with base_svma by @not-matthias
- Add perf v2 support by @not-matthias in [#119](https://github.com/CodSpeedHQ/runner/pull/119)
- Add runner-shared crate by @not-matthias
- Add content encoding to upload metadata by @adriencaccia
- Do not compress profile archive for walltime runs by @adriencaccia
- Detect stack size at runtime by @not-matthias in [#103](https://github.com/CodSpeedHQ/runner/pull/103)
- Add unwind data tests by @not-matthias
- Run python with perf jit dump by @not-matthias

### <!-- 1 -->🐛 Bug Fixes
- Use shared elf_helper for unwind and symbol information by @not-matthias
- Cargo clippy lints by @not-matthias
- Only enable debug logs GH action is debugged by @not-matthias in [#118](https://github.com/CodSpeedHQ/runner/pull/118)
- Forward go runner exit status by @not-matthias in [#115](https://github.com/CodSpeedHQ/runner/pull/115)
- Ignore statically linked python by @not-matthias
- Codspeed debug check by @not-matthias
- Create perf map for jitdump by @not-matthias

### <!-- 2 -->🏗️ Refactor
- Store upload metadata latest version in a const by @adriencaccia in [#117](https://github.com/CodSpeedHQ/runner/pull/117)
- Refactor profile-archive by @adriencaccia

### <!-- 7 -->⚙️ Internals
- Fix the release commit message by @art049
- Make runner-shared not publishable by @art049
- Add debug log for /proc/<pid>/maps by @not-matthias


## [4.0.1] - 2025-09-09

### <!-- 1 -->🐛 Bug Fixes
- Url for codspeed-go-runner installer by @not-matthias in [#112](https://github.com/CodSpeedHQ/runner/pull/112)


## [4.0.0] - 2025-09-01

### <!-- 0 -->🚀 Features
- Make perf enabled by default by @GuillaumeLagrange in [#110](https://github.com/CodSpeedHQ/runner/pull/110)
- Make runner mode argument mandatory by @GuillaumeLagrange
- Use introspected node in walltime mode by @GuillaumeLagrange in [#108](https://github.com/CodSpeedHQ/runner/pull/108)
- Add instrumented go shell script by @not-matthias in [#102](https://github.com/CodSpeedHQ/runner/pull/102)

### <!-- 1 -->🐛 Bug Fixes
- Compute proper load bias by @not-matthias in [#107](https://github.com/CodSpeedHQ/runner/pull/107)
- Increase timeout for first perf ping by @GuillaumeLagrange
- Prevent running with valgrind by @not-matthias in [#106](https://github.com/CodSpeedHQ/runner/pull/106)

### <!-- 2 -->🏗️ Refactor
- Change go-runner binary name by @not-matthias in [#111](https://github.com/CodSpeedHQ/runner/pull/111)

### <!-- 7 -->⚙️ Internals
- Add AGENTS.md by @GuillaumeLagrange


## [3.8.1] - 2025-08-25

### <!-- 1 -->🐛 Bug Fixes
- Dont show error when libpython is not found by @not-matthias

### <!-- 2 -->🏗️ Refactor
- Improve conditional compilation in get_pipe_open_options by @art049 in [#100](https://github.com/CodSpeedHQ/runner/pull/100)

### <!-- 7 -->⚙️ Internals
- Change log level to warn for venv_compat error by @not-matthias in [#104](https://github.com/CodSpeedHQ/runner/pull/104)


## [3.8.0] - 2025-07-18

### <!-- 1 -->🐛 Bug Fixes
- Adjust offset for symbols of module loaded at preferred base by @not-matthias in [#97](https://github.com/CodSpeedHQ/runner/pull/97)
- Run with --scope to allow perf to trace the benchmark process by @not-matthias
- Run with bash to support complex scripts by @not-matthias
- Execute pre- and post-bench scripts for non-perf walltime runner by @not-matthias in [#96](https://github.com/CodSpeedHQ/runner/pull/96)

### <!-- 2 -->🏗️ Refactor
- Process memory mappings in separate function by @not-matthias

### <!-- 7 -->⚙️ Internals
- Add debug logs for perf.map collection by @not-matthias
- Add complex cmd and env tests by @not-matthias


## [3.7.0] - 2025-07-08

### <!-- 0 -->🚀 Features
- Add pre- and post-benchmark scripts by @not-matthias
- Add cli args for perf by @not-matthias in [#94](https://github.com/CodSpeedHQ/runner/pull/94)

### <!-- 1 -->🐛 Bug Fixes
- Forward environment to systemd-run cmd by @not-matthias
- Only panic in upload for non-existing integration by @not-matthias
- Multi-line commands in valgrind by @not-matthias
- Symlink libpython doesn't work for statically linked python by @not-matthias in [#89](https://github.com/CodSpeedHQ/runner/pull/89)
- Run perf with sudo; support systemd-run for non-perf walltime by @not-matthias
- Use correct path for unwind info by @not-matthias

### <!-- 7 -->⚙️ Internals
- Add executor tests by @not-matthias in [#95](https://github.com/CodSpeedHQ/runner/pull/95)
- Add log to detect invalid origin url by @not-matthias
- Upgrade to edition 2024 by @not-matthias
- Add debug logs for proc maps by @not-matthias in [#88](https://github.com/CodSpeedHQ/runner/pull/88)


## [3.6.1] - 2025-06-16

### <!-- 0 -->🚀 Features
- Run benchmark with systemd (for optional cpu isolation) by @not-matthias in [#86](https://github.com/CodSpeedHQ/runner/pull/86)

### <!-- 1 -->🐛 Bug Fixes
- Only show perf output at debug or trace level by @not-matthias in [#87](https://github.com/CodSpeedHQ/runner/pull/87)


## [3.6.0] - 2025-06-10

### <!-- 0 -->🚀 Features
- Allow setting upload url via env var for convenience by @GuillaumeLagrange in [#85](https://github.com/CodSpeedHQ/runner/pull/85)
- Send unknown cpu_brand when it is not recognized by @adriencaccia
- Allow only running the benchmarks, and only uploading the results by @GuillaumeLagrange in [#81](https://github.com/CodSpeedHQ/runner/pull/81)
- Install perf on setup by @not-matthias
- Add perf integration for python by @not-matthias
- Add perf integration for rust by @not-matthias
- Add fifo ipc by @not-matthias
- Use custom time formatting to be in line with the rest of CodSpeed by @GuillaumeLagrange in [#77](https://github.com/CodSpeedHQ/runner/pull/77)
- Output information about benches after a local run by @GuillaumeLagrange in [#76](https://github.com/CodSpeedHQ/runner/pull/76)
- Allow specifying oauth token through CLI by @GuillaumeLagrange in [#75](https://github.com/CodSpeedHQ/runner/pull/75)
- Add option to output structured json by @GuillaumeLagrange in [#74](https://github.com/CodSpeedHQ/runner/pull/74)
- Add flags to specify repository from CLI by @GuillaumeLagrange
- Improve error handling for valgrind by @not-matthias in [#67](https://github.com/CodSpeedHQ/runner/pull/67)
- Handle local run failure by @adriencaccia in [#71](https://github.com/CodSpeedHQ/runner/pull/71)

### <!-- 1 -->🐛 Bug Fixes
- Persist logs when running with skip_upload by @GuillaumeLagrange in [#84](https://github.com/CodSpeedHQ/runner/pull/84)
- Valgrind crash for unresolved libpython by @not-matthias in [#82](https://github.com/CodSpeedHQ/runner/pull/82)
- Support trailing slash in origin url by @not-matthias in [#83](https://github.com/CodSpeedHQ/runner/pull/83)
- Use bash to ensure correct behavior across systems by @not-matthias
- Fix test randomly failing due to other test run in parallel by @GuillaumeLagrange
- Check child status code after valgrind by @not-matthias in [#72](https://github.com/CodSpeedHQ/runner/pull/72)

### <!-- 7 -->⚙️ Internals
- Dont use regex in perf map harvest by @not-matthias
- Switch to astral-sh/cargo-dist by @adriencaccia in [#80](https://github.com/CodSpeedHQ/runner/pull/80)


## [3.5.0] - 2025-03-13

### <!-- 0 -->🚀 Features
- Add mode command arg by @adriencaccia in [#69](https://github.com/CodSpeedHQ/runner/pull/69)
- Reduce spacing between groups by @art049
- Improve log messages verbosity and style by @art049
- Add a global setup command to preinstall executors by @art049
- Allow usage on any x86 or arm os with a warning by @GuillaumeLagrange in [#66](https://github.com/CodSpeedHQ/runner/pull/66)

### <!-- 1 -->🐛 Bug Fixes
- Fix valgrind version checks (#65) by @art049 in [#65](https://github.com/CodSpeedHQ/runner/pull/65)

### <!-- 3 -->📚 Documentation
- Add a setup command to the README by @art049 in [#61](https://github.com/CodSpeedHQ/runner/pull/61)


## [3.4.0] - 2025-02-19

### <!-- 0 -->🚀 Features
- Add run_part to upload metadata by @fargito in [#57](https://github.com/CodSpeedHQ/runner/pull/57)

### <!-- 1 -->🐛 Bug Fixes
- Fix stderr error display by @art049 in [#63](https://github.com/CodSpeedHQ/runner/pull/63)

### <!-- 7 -->⚙️ Internals
- Remove useless `get_run_environment_name` method by @fargito
- Rename `platform` to `RunEnvironment` by @fargito
- Add missing spellings by @fargito
- Bump toolchain from 1.79.0 to 1.84.0 by @fargito


## [3.3.1] - 2025-02-13

### <!-- 0 -->🚀 Features
- Bail when performance report s3 upload does not work by @adriencaccia

### <!-- 1 -->🐛 Bug Fixes
- Catch server error as well as client in upload error handling by @adriencaccia in [#64](https://github.com/CodSpeedHQ/runner/pull/64)


## [3.3.0] - 2025-02-12

### <!-- 0 -->🚀 Features
- Allow downgrades while installing valgrind-codspeed by @art049
- Update sysinfo crate by @adriencaccia in [#62](https://github.com/CodSpeedHQ/runner/pull/62)

### <!-- 1 -->🐛 Bug Fixes
- Unify environments between the two modes by @art049 in [#59](https://github.com/CodSpeedHQ/runner/pull/59)

### <!-- 7 -->⚙️ Internals
- Bump valgrind-codspeed version to 3.24.0-0codspeed1 and change supported systems by @art049


## [3.2.2] - 2025-01-14

### <!-- 0 -->🚀 Features
- Add cmd base env to all executors by @adriencaccia in [#56](https://github.com/CodSpeedHQ/runner/pull/56)

### <!-- 1 -->🐛 Bug Fixes
- Support https repo uri not ending with .git by @art049 in [#54](https://github.com/CodSpeedHQ/runner/pull/54)


## [3.2.1] - 2024-11-29

### <!-- 0 -->🚀 Features
- Add support for pipelines triggered through the api by @fargito in [#52](https://github.com/CodSpeedHQ/runner/pull/52)

### <!-- 1 -->🐛 Bug Fixes
- Use correct ref for tag pipelines by @fargito
- Git-cliff configuration for changelog generation by @art049

### <!-- 3 -->📚 Documentation
- Add link to GitLab CI docs by @fargito in [#51](https://github.com/CodSpeedHQ/runner/pull/51)

### <!-- 7 -->⚙️ Internals
- Skip changelog generation for pre-releases by @art049
- Bump pre-commit action by @art049
- Fix changelog markdown template whitespaces by @art049


## [3.2.0] - 2024-11-22

### <!-- 0 -->🚀 Features
- Implement gitlab ci provider by @fargito in [#47](https://github.com/CodSpeedHQ/runner/pull/47)
- Add repository provider to upload metadata by @fargito
- Use system distribution id instead of name by @fargito

### <!-- 2 -->🏗️ Refactor
- Move sender out of ghdata by @fargito
- Rename provider to ci provider by @fargito
- Use string for runId by @fargito
- Improve string interpolation by @fargito

### <!-- 7 -->⚙️ Internals
- Configure git-cliff for changelog generation by @art049
- Add rust settings by @fargito


## [3.1.0] - 2024-11-05

### <!-- 0 -->🚀 Features
- Only pass `PYTHONMALLOC` to the valgrind instrument (#48) by @art049
- Support --version flag by @adriencaccia
- Add cpu and memory data to SystemInfo by @adriencaccia
- Add executor property to UploadMetadata by @adriencaccia
- Add WallTimeExecutor by @adriencaccia
- Support arm64 architecture by @adriencaccia in [#38](https://github.com/CodSpeedHQ/runner/pull/38)

### <!-- 1 -->🐛 Bug Fixes
- Ensure executor logs are not passed to provider logger by @adriencaccia

### <!-- 2 -->🏗️ Refactor
- Use singular for enum InstrumentName by @adriencaccia in [#39](https://github.com/CodSpeedHQ/runner/pull/39)
- Rename introspected_node module into introspected_nodejs to be more specific by @adriencaccia
- Create executor abstraction and add ValgrindExecutor by @adriencaccia

### <!-- 7 -->⚙️ Internals
- Port ubuntu 24 compatibility (#44) by @art049
- Add arm64 Linux musl target by @adriencaccia
- Update cargo-dist to latest version by @adriencaccia


## [3.0.0] - 2024-07-26

### <!-- 0 -->🚀 Features
- Bump rust toolchain by @adriencaccia
- Handle invalid token by @adriencaccia
- Update some logging by @adriencaccia
- Do not display codspeed banner during local run by @adriencaccia
- Disallow empty bench command by @adriencaccia
- Prevent trace valgrind logs to duplicate spinner lines by @adriencaccia
- Update style of terminal output by @adriencaccia
- Change verbs tense to continuous by @adriencaccia
- Add regressions threshold, colors and better style to logs by @adriencaccia
- Style auth link log by @adriencaccia
- Add log groups by @adriencaccia
- Create custom local logger with spinner by @adriencaccia
- Update CLI style by @adriencaccia
- Add system info to upload metadata runner property by @adriencaccia
- Support arm64 arch by @adriencaccia
- Do not install valgrind if correct version is installed by @adriencaccia
- Handle local run by @adriencaccia
- Add local provider by @adriencaccia
- First implementation of auth login command by @adriencaccia

### <!-- 1 -->🐛 Bug Fixes
- Fix malformed valgrind download url by @adriencaccia

### <!-- 2 -->🏗️ Refactor
- Do not create system info inside check_system by @adriencaccia in [#37](https://github.com/CodSpeedHQ/runner/pull/37)
- Move local logger to its own file by @adriencaccia in [#36](https://github.com/CodSpeedHQ/runner/pull/36)
- Move logger group logic to root logger by @adriencaccia
- Rename bin to codspeed by @adriencaccia
- Move runner to run subcommand by @adriencaccia

### <!-- 3 -->📚 Documentation
- Update readme with CLI usage by @adriencaccia

### <!-- 7 -->⚙️ Internals
- Allow some prelude unused imports by @adriencaccia
- Remove useless code in BuildkiteProvider by @adriencaccia
- Remove useless code in GitHubActionsProvide by @adriencaccia
- Remove useless snapshots by @adriencaccia
- Run ci on every pull request by @adriencaccia in [#23](https://github.com/CodSpeedHQ/runner/pull/23)


## [2.4.3] - 2024-07-12

### <!-- 7 -->⚙️ Internals
- Add error chain debug by @adriencaccia in [#34](https://github.com/CodSpeedHQ/runner/pull/34)


## [2.4.2] - 2024-06-14

### <!-- 0 -->🚀 Features
- Better upload endpoint error handling by @adriencaccia in [#29](https://github.com/CodSpeedHQ/runner/pull/29)


## [2.4.1] - 2024-04-29

### <!-- 1 -->🐛 Bug Fixes
- Retrieve root_repository_path from git dir by @adriencaccia in [#20](https://github.com/CodSpeedHQ/runner/pull/20)


## [2.4.0] - 2024-04-26

### <!-- 0 -->🚀 Features
- Use current checked out commit hash in UploadMetadata by @adriencaccia in [#18](https://github.com/CodSpeedHQ/runner/pull/18)


## [2.3.1] - 2024-04-24

### <!-- 1 -->🐛 Bug Fixes
- Properly display stderr and stdout when a setup command fails by @art049 in [#19](https://github.com/CodSpeedHQ/runner/pull/19)


## [2.3.0] - 2024-03-21

### <!-- 0 -->🚀 Features
- Support debian 11 and 12 by @adriencaccia in [#17](https://github.com/CodSpeedHQ/runner/pull/17)

### <!-- 1 -->🐛 Bug Fixes
- Change bump-action job name by @art049


## [2.2.1] - 2024-02-22

### <!-- 0 -->🚀 Features
- Handle symlinks in ignored objects by @art049 in [#16](https://github.com/CodSpeedHQ/runner/pull/16)

### <!-- 7 -->⚙️ Internals
- Add a post anounce bump workflow by @art049 in [#15](https://github.com/CodSpeedHQ/runner/pull/15)


## [2.2.0] - 2024-02-22

### <!-- 0 -->🚀 Features
- Include the execution output in the logs by @art049
- Upload execution logs with the profile by @art049

### <!-- 1 -->🐛 Bug Fixes
- Properly handle log levels with buildkite by @art049 in [#14](https://github.com/CodSpeedHQ/runner/pull/14)

### <!-- 7 -->⚙️ Internals
- Enforce tag signing with cargo release by @art049


## [2.1.1] - 2024-01-30

### <!-- 0 -->🚀 Features
- Send error to error log when logging is enabled by @adriencaccia

### <!-- 1 -->🐛 Bug Fixes
- Use IP address instead of localhost for MongoDB URI by @adriencaccia

### <!-- 2 -->🏗️ Refactor
- Use clap env feature instead of manually checking by @adriencaccia

### <!-- 6 -->🧪 Testing
- Add MongoTracer::try_from tests by @adriencaccia

### <!-- 7 -->⚙️ Internals
- Add codspeed badge by @adriencaccia in [#13](https://github.com/CodSpeedHQ/runner/pull/13)


## [2.1.0] - 2024-01-17

### <!-- 0 -->🚀 Features
- Use instruments list as arg and move instruments inside config by @adriencaccia
- Add debug logging for MongoDB tracer by @adriencaccia
- Allow mongo destination to be dynamically set by @adriencaccia
- Add instruments with mongodb by @adriencaccia

### <!-- 2 -->🏗️ Refactor
- Use shorthand bail by @adriencaccia in [#9](https://github.com/CodSpeedHQ/runner/pull/9)
- Move instruments versions to main by @adriencaccia
- Abstract common upload metadata to trait by @adriencaccia

### <!-- 7 -->⚙️ Internals
- Add comment on dump_log by @adriencaccia


## [2.0.3] - 2024-01-04

### <!-- 1 -->🐛 Bug Fixes
- Bump cargo-dist to remove broken pipe logs by @adriencaccia in [#12](https://github.com/CodSpeedHQ/runner/pull/12)
- Handle error response when retrieving upload data by @adriencaccia in [#11](https://github.com/CodSpeedHQ/runner/pull/11)


## [2.0.2] - 2023-12-04

### <!-- 1 -->🐛 Bug Fixes
- Control cargo-codspeed running environment by @adriencaccia in [#8](https://github.com/CodSpeedHQ/runner/pull/8)

### <!-- 6 -->🧪 Testing
- Add Config::test() factory by @adriencaccia


## [2.0.1] - 2023-12-01

### <!-- 1 -->🐛 Bug Fixes
- Print all lines with the github actions prefix when logging by @adriencaccia in [#7](https://github.com/CodSpeedHQ/runner/pull/7)
- Better handle logging by @adriencaccia


## [2.0.0] - 2023-11-30

### <!-- 0 -->🚀 Features
- Preserve order of struct when serializing in json by @adriencaccia in [#5](https://github.com/CodSpeedHQ/runner/pull/5)
- Handle log level with CODSPEED_LOG variable by @adriencaccia
- Add start_opened_group log macro by @adriencaccia
- Add repositoryRootPath to the upload metadata by @adriencaccia
- Propagate benchmark process error by @adriencaccia
- Change CODSPEED_ENV to generic value by @adriencaccia
- Use sudo if available in setup by @adriencaccia
- Use apt-get instead of apt by @adriencaccia
- Implement builkite provider by @adriencaccia
- Add platform metadata by @adriencaccia
- Use enum for run event instead of strings by @adriencaccia
- Change implem of get_provider to allow different providers by @adriencaccia
- Log everything in GitHub Actions by @adriencaccia in [#4](https://github.com/CodSpeedHQ/runner/pull/4)
- Implement provider specific loggers by @art049
- Switch to musl build target by @art049
- Share REQUEST_CLIENT across crate by @adriencaccia
- Log info and above by default by @adriencaccia
- First implementation by @adriencaccia
- Implement the runner by @art049
- Initial commit by @art049

### <!-- 1 -->🐛 Bug Fixes
- Emove codspeed_introspected_node from PATH to prevent infinite loop by @adriencaccia in [#6](https://github.com/CodSpeedHQ/runner/pull/6)
- Return node script folder instead of file by @adriencaccia
- Use correct tokenless hash log format by @adriencaccia
- Fix fork implementation by @adriencaccia
- Use .tar.gz archive instead of .xz by @adriencaccia
- Use vendored openssl by @art049
- Use correct arg format by @adriencaccia
- Use sudo apt instead of apt by @adriencaccia
- Use corrent node command by @adriencaccia

### <!-- 2 -->🏗️ Refactor
- Make ghData optional by @adriencaccia
- Move ci_provider out of the upload by @art049
- Use async reqwest by @adriencaccia
- Use info instead of println by @adriencaccia

### <!-- 7 -->⚙️ Internals
- Update README.md by @art049
- Remove useless log level default by @adriencaccia
- Update README by @adriencaccia
- Update README by @adriencaccia
- Add some rust settings by @adriencaccia
- Fix skip_setup doc comment by @adriencaccia
- Setup cargo dist by @art049 in [#1](https://github.com/CodSpeedHQ/runner/pull/1)
- Add linting components to the toolchain by @art049


[4.10.5]: https://github.com/CodSpeedHQ/runner/compare/v4.10.4..v4.10.5
[4.10.4]: https://github.com/CodSpeedHQ/runner/compare/v4.10.3..v4.10.4
[4.10.3]: https://github.com/CodSpeedHQ/runner/compare/v4.10.2..v4.10.3
[4.10.2]: https://github.com/CodSpeedHQ/runner/compare/v4.10.1..v4.10.2
[4.10.1]: https://github.com/CodSpeedHQ/runner/compare/v4.10.0..v4.10.1
[4.10.0]: https://github.com/CodSpeedHQ/runner/compare/v4.9.0..v4.10.0
[4.9.0]: https://github.com/CodSpeedHQ/runner/compare/v4.8.2..v4.9.0
[4.8.2]: https://github.com/CodSpeedHQ/runner/compare/v4.8.1..v4.8.2
[4.8.1]: https://github.com/CodSpeedHQ/runner/compare/v4.8.0..v4.8.1
[4.8.0]: https://github.com/CodSpeedHQ/runner/compare/v4.7.0..v4.8.0
[4.7.0]: https://github.com/CodSpeedHQ/runner/compare/v4.6.0..v4.7.0
[4.6.0]: https://github.com/CodSpeedHQ/runner/compare/v4.5.2..v4.6.0
[4.5.2]: https://github.com/CodSpeedHQ/runner/compare/v4.5.1..v4.5.2
[4.5.1]: https://github.com/CodSpeedHQ/runner/compare/v4.5.0..v4.5.1
[4.5.0]: https://github.com/CodSpeedHQ/runner/compare/v4.4.1..v4.5.0
[4.4.1]: https://github.com/CodSpeedHQ/runner/compare/v4.4.0..v4.4.1
[4.4.0]: https://github.com/CodSpeedHQ/runner/compare/v4.3.4..v4.4.0
[4.3.4]: https://github.com/CodSpeedHQ/runner/compare/v4.3.3..v4.3.4
[4.3.3]: https://github.com/CodSpeedHQ/runner/compare/v4.3.2..v4.3.3
[4.3.2]: https://github.com/CodSpeedHQ/runner/compare/v4.3.1..v4.3.2
[4.3.1]: https://github.com/CodSpeedHQ/runner/compare/v4.3.0..v4.3.1
[4.3.0]: https://github.com/CodSpeedHQ/runner/compare/v4.2.1..v4.3.0
[4.2.1]: https://github.com/CodSpeedHQ/runner/compare/v4.2.0..v4.2.1
[4.2.0]: https://github.com/CodSpeedHQ/runner/compare/v4.1.1..v4.2.0
[4.1.1]: https://github.com/CodSpeedHQ/runner/compare/v4.1.0..v4.1.1
[4.1.0]: https://github.com/CodSpeedHQ/runner/compare/v4.0.1..v4.1.0
[4.0.1]: https://github.com/CodSpeedHQ/runner/compare/v4.0.0..v4.0.1
[4.0.0]: https://github.com/CodSpeedHQ/runner/compare/v3.8.1..v4.0.0
[3.8.1]: https://github.com/CodSpeedHQ/runner/compare/v3.8.0..v3.8.1
[3.8.0]: https://github.com/CodSpeedHQ/runner/compare/v3.7.0..v3.8.0
[3.7.0]: https://github.com/CodSpeedHQ/runner/compare/v3.6.1..v3.7.0
[3.6.1]: https://github.com/CodSpeedHQ/runner/compare/v3.6.0..v3.6.1
[3.6.0]: https://github.com/CodSpeedHQ/runner/compare/v3.5.0..v3.6.0
[3.5.0]: https://github.com/CodSpeedHQ/runner/compare/v3.4.0..v3.5.0
[3.4.0]: https://github.com/CodSpeedHQ/runner/compare/v3.3.1..v3.4.0
[3.3.1]: https://github.com/CodSpeedHQ/runner/compare/v3.3.0..v3.3.1
[3.3.0]: https://github.com/CodSpeedHQ/runner/compare/v3.2.2..v3.3.0
[3.2.2]: https://github.com/CodSpeedHQ/runner/compare/v3.2.1..v3.2.2
[3.2.1]: https://github.com/CodSpeedHQ/runner/compare/v3.2.0..v3.2.1
[3.2.0]: https://github.com/CodSpeedHQ/runner/compare/v3.1.0..v3.2.0
[3.1.0]: https://github.com/CodSpeedHQ/runner/compare/v3.0.0..v3.1.0
[3.0.0]: https://github.com/CodSpeedHQ/runner/compare/v2.4.3..v3.0.0
[2.4.3]: https://github.com/CodSpeedHQ/runner/compare/v2.4.2..v2.4.3
[2.4.2]: https://github.com/CodSpeedHQ/runner/compare/v2.4.1..v2.4.2
[2.4.1]: https://github.com/CodSpeedHQ/runner/compare/v2.4.0..v2.4.1
[2.4.0]: https://github.com/CodSpeedHQ/runner/compare/v2.3.1..v2.4.0
[2.3.1]: https://github.com/CodSpeedHQ/runner/compare/v2.3.0..v2.3.1
[2.3.0]: https://github.com/CodSpeedHQ/runner/compare/v2.2.1..v2.3.0
[2.2.1]: https://github.com/CodSpeedHQ/runner/compare/v2.2.0..v2.2.1
[2.2.0]: https://github.com/CodSpeedHQ/runner/compare/v2.1.1..v2.2.0
[2.1.1]: https://github.com/CodSpeedHQ/runner/compare/v2.1.0..v2.1.1
[2.1.0]: https://github.com/CodSpeedHQ/runner/compare/v2.0.3..v2.1.0
[2.0.3]: https://github.com/CodSpeedHQ/runner/compare/v2.0.2..v2.0.3
[2.0.2]: https://github.com/CodSpeedHQ/runner/compare/v2.0.1..v2.0.2
[2.0.1]: https://github.com/CodSpeedHQ/runner/compare/v2.0.0..v2.0.1

<!-- generated by git-cliff -->
