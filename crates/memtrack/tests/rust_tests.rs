#[macro_use]
mod shared;

use rstest::rstest;
use std::path::Path;

#[test_with::env(GITHUB_ACTIONS)]
#[rstest]
#[case("system", &[])]
#[case("jemalloc", &["with-jemalloc"])]
#[case("mimalloc", &["with-mimalloc"])]
#[test_log::test]
fn test_rust_alloc_tracking(
    #[case] name: &str,
    #[case] features: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let crate_path = Path::new("testdata/alloc_rust");
    let binary = shared::compile_rust_binary(crate_path, "alloc_rust", features)?;

    // No extra allocators: the watcher must discover the static allocator itself.
    let (events, thread_handle) = shared::track_binary(&binary)?;
    assert_events_with_marker!(name, &events);

    thread_handle.join().unwrap();
    Ok(())
}
