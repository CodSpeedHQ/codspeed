use std::path::Path;

/// Longest basename a key keeps. Keys are used as file names, which the
/// filesystem caps at 255 bytes, and a JIT symbol can be arbitrarily long.
const MAX_BASENAME_LEN: usize = 200;

/// Build a semantic key from a global index and a path.
///
/// The key is `{index}__{basename}` where `basename` is the last component
/// of the path, truncated to [`MAX_BASENAME_LEN`]. The index ensures uniqueness
/// across all artifact types.
pub fn indexed_semantic_key(index: usize, path: &Path) -> String {
    let basename = path.file_name().unwrap_or_default().to_string_lossy();
    let mut end = MAX_BASENAME_LEN.min(basename.len());
    while !basename.is_char_boundary(end) {
        end -= 1;
    }
    format!("{index}__{}", &basename[..end])
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

    #[test]
    fn test_long_basename_is_truncated_on_a_char_boundary() {
        // "€" is 3 bytes, so the byte at MAX_BASENAME_LEN falls inside one.
        let name = format!("jit_RegExp src: '{}'", "€".repeat(100));
        let key = indexed_semantic_key(7, Path::new(&name));

        assert!(key.len() <= "7__".len() + MAX_BASENAME_LEN);
        assert!(key.starts_with("7__jit_RegExp src: '€"));
    }
}
