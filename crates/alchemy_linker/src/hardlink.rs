use std::fs;
use std::path::Path;

/// Create a hard link, copying as fallback (cross-device)
pub fn hardlink_or_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    match fs::hard_link(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Fallback to copy if hard link fails (e.g., cross-device)
            fs::copy(src, dst)?;
            Ok(())
        }
    }
}

/// Recursively hard-link all files from src to dst directory
pub fn hardlink_dir(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            hardlink_dir(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            hardlink_or_copy(&src_path, &dst_path)?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(&src_path)?;
            #[cfg(unix)]
            {
                if dst_path.exists() || dst_path.symlink_metadata().is_ok() {
                    fs::remove_file(&dst_path)?;
                }
                std::os::unix::fs::symlink(&target, &dst_path)?;
            }
        }
    }

    Ok(())
}
