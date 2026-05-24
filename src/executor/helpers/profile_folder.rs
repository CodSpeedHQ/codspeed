use crate::prelude::*;

use rand::distr::Alphanumeric;
use rand::distr::SampleString;
use std::env;
use std::fs;
use std::path::PathBuf;

pub fn create_profile_folder() -> Result<PathBuf> {
    let folder_name = format!(
        "profile.{}.out",
        Alphanumeric.sample_string(&mut rand::rng(), 10)
    );
    let mut folder_path = env::temp_dir();
    folder_path.push(folder_name);
    fs::create_dir_all(&folder_path).map_err(|e| {
        anyhow!(
            "Failed to create profile folder: {}, {}",
            folder_path.display(),
            e
        )
    })?;
    debug!("Created profile folder: {}", folder_path.display());
    Ok(folder_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_profile_folder() -> Result<()> {
        let folder_path = create_profile_folder()?;
        assert!(folder_path.exists());
        assert!(folder_path.is_dir());
        Ok(())
    }
}
