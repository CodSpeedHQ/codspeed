#[macro_use]
mod shared;

use memtrack::AllocatorLib;
use rstest::rstest;
use std::path::Path;
use std::process::Command;

fn compile_cpp_project(project_dir: &Path, target: &str) -> anyhow::Result<std::path::PathBuf> {
    let build_exists = project_dir.join("build").exists();
    if !build_exists {
        // Configure with cmake -B build
        let config = Command::new("cmake")
            .current_dir(project_dir)
            .args(["-B", "build", "-DCMAKE_BUILD_TYPE=Release"])
            .output()?;

        if !config.status.success() {
            eprintln!(
                "cmake configure failed: {}",
                String::from_utf8_lossy(&config.stderr)
            );
            return Err(anyhow::anyhow!("Failed to configure C++ project"));
        }
    }

    // Build specific target
    let build = Command::new("cmake")
        .current_dir(project_dir)
        .args(["--build", "build", "--target", target, "-j"])
        .output()?;

    if !build.status.success() {
        eprintln!(
            "cmake build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        eprintln!("cmake stdout: {}", String::from_utf8_lossy(&build.stdout));
        return Err(anyhow::anyhow!("Failed to build target: {target}"));
    }

    let binary_path = project_dir.join(format!("build/{target}"));
    Ok(binary_path)
}

#[test_with::env(GITHUB_ACTIONS)]
#[rstest]
#[case("alloc_cpp_system")]
#[case("alloc_cpp_jemalloc_static")]
#[case("alloc_cpp_jemalloc_dynamic")]
#[case("alloc_cpp_mimalloc_static")]
#[case("alloc_cpp_mimalloc_dynamic")]
#[case("alloc_cpp_tcmalloc_static")]
#[case("alloc_cpp_tcmalloc_dynamic")]
#[test_log::test]
fn test_cpp_alloc_tracking(#[case] target: &str) -> Result<(), Box<dyn std::error::Error>> {
    let project_path = Path::new("testdata/alloc_cpp");
    let binary = compile_cpp_project(project_path, target)?;

    // Try to find a static allocator in the binary, then attach to it as well
    // This is needed because the CWD is different, which breaks the heuristics.
    let allocators = AllocatorLib::from_path_static(&binary)
        .map(|a| vec![a])
        .unwrap_or_default();

    let (events, thread_handle) = shared::track_binary_with_opts(&binary, &allocators)?;
    assert_events_with_marker!(target, &events);

    thread_handle.join().unwrap();
    Ok(())
}
