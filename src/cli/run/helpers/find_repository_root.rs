use std::path::{Path, PathBuf};

// during normal execution, we want to find the repository root by looking for a .git directory
// during tests, we want `find_repository_root` to always return `None`, so that we don't have to
// create a git repository for each test

#[cfg(not(test))]
pub fn find_repository_root(base_dir: &Path) -> Option<PathBuf> {
    _find_repository_root(base_dir)
}

#[cfg(test)]
pub fn find_repository_root(_base_dir: &Path) -> Option<PathBuf> {
    None
}

// the core logic is extracted into a separate function so that it can be tested
fn _find_repository_root(base_dir: &Path) -> Option<PathBuf> {
    let current_dir = base_dir.canonicalize().ok()?;

    for ancestor in current_dir.ancestors() {
        let git_dir = ancestor.join(".git");
        if git_dir.exists() {
            return Some(ancestor.to_path_buf());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_repository_root() {
        // create an empty directory in a tmp directory, add a nested .git directory
        // and check if the repository root is found when calling _find_repository_root from a nested directory
        let tmp_dir = tempfile::tempdir().unwrap();
        let base_dir = tmp_dir.path().join("base-dir");
        let git_dir = base_dir.join(".git");
        std::fs::create_dir_all(git_dir).unwrap();
        let nested_current_dir = base_dir.join("nested").join("deeply");
        std::fs::create_dir_all(&nested_current_dir).unwrap();

        let repository_root = _find_repository_root(&nested_current_dir).unwrap();
        assert_eq!(repository_root, base_dir.canonicalize().unwrap());

        tmp_dir.close().unwrap();
    }

    #[test]
    fn test_find_repository_root_no_git_dir() {
        // create an empty directory in a tmp directory and check if the repository root is not found
        let tmp_dir = tempfile::tempdir().unwrap();
        let base_dir = tmp_dir.path().join("base-dir");
        std::fs::create_dir_all(&base_dir).unwrap();

        let repository_root = _find_repository_root(&base_dir);
        assert_eq!(repository_root, None);

        tmp_dir.close().unwrap();
    }
}
