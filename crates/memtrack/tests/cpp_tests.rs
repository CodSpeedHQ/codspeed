#[macro_use]
mod shared;

use rstest::rstest;
use std::path::Path;
use std::process::Command;

/// A cached cmake configure pins absolute library paths from a previous
/// environment (e.g. garbage-collected nix store entries). On failure, wipe
/// the build dir and retry once from a fresh configure.
///
/// Builds are serialized: parallel test cases share the build dir, and the
/// retry path deletes it.
fn compile_cpp_project(project_dir: &Path, target: &str) -> anyhow::Result<std::path::PathBuf> {
    static BUILD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = BUILD_LOCK.lock().unwrap();

    match build_cpp_target(project_dir, target) {
        Ok(path) => Ok(path),
        Err(_) => {
            std::fs::remove_dir_all(project_dir.join("build"))?;
            build_cpp_target(project_dir, target)
        }
    }
}

fn build_cpp_target(project_dir: &Path, target: &str) -> anyhow::Result<std::path::PathBuf> {
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

    let (events, thread_handle) = shared::track_binary(&binary)?;
    assert_events_with_marker!(target, &events);

    thread_handle.join().unwrap();
    Ok(())
}
