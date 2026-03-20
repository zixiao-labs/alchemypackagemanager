use std::fs;
use std::path::Path;

/// Create a symlink (directory symlink on all platforms)
pub fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }

    // Remove existing symlink/file
    if link.symlink_metadata().is_ok() {
        if link.is_dir() && !link.symlink_metadata()?.is_symlink() {
            fs::remove_dir_all(link)?;
        } else {
            fs::remove_file(link).or_else(|_| fs::remove_dir_all(link))?;
        }
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link)?;

    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(target, link)?;

    Ok(())
}
