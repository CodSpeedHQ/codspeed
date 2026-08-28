use std::path::Path;

/// Build a semantic key from a global index and a path.
///
/// The key is `{index}__{basename}` where `basename` is the last component
/// of the path. The index ensures uniqueness across all artifact types.
pub fn indexed_semantic_key(index: usize, path: &Path) -> String {
    format!(
        "{index}__{}",
        path.file_name().unwrap_or_default().to_string_lossy()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_path() {
        let key = indexed_semantic_key(0, Path::new("/usr/lib/libc.so.6"));
        assert_eq!(key, "0__libc.so.6");
    }

    #[test]
    fn test_jit_path() {
        let key = indexed_semantic_key(5, Path::new("/tmp/jit-12345.so"));
        assert_eq!(key, "5__jit-12345.so");
    }

    #[test]
    fn test_same_basename_different_paths() {
        let key1 = indexed_semantic_key(0, Path::new("/usr/lib/libc.so.6"));
        let key2 = indexed_semantic_key(1, Path::new("/opt/lib/libc.so.6"));
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_bare_filename() {
        let key = indexed_semantic_key(3, Path::new("libfoo.so"));
        assert_eq!(key, "3__libfoo.so");
    }
}
