use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use tracing::debug;

/// Content-addressable store at ~/.alchemy-store/
pub struct ContentStore {
    base_dir: PathBuf,
}

impl ContentStore {
    pub fn new() -> Self {
        let base = dirs_home().join(".alchemy-store");
        Self { base_dir: base }
    }

    pub fn with_base(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Directory for a specific package version's extracted files
    pub fn package_dir(&self, name: &str, version: &str) -> PathBuf {
        self.base_dir.join("packages").join(name).join(version)
    }

    /// Check if a package version is already in the store
    pub fn has_package(&self, name: &str, version: &str) -> bool {
        self.package_dir(name, version).exists()
    }

    /// Store a package's extracted files. The source directory is moved/copied
    /// into the content store.
    pub fn store_package(
        &self,
        name: &str,
        version: &str,
        extracted_dir: &Path,
    ) -> anyhow::Result<PathBuf> {
        let dest = self.package_dir(name, version);

        if dest.exists() {
            debug!("Package {name}@{version} already in store");
            return Ok(dest);
        }

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }

        // Copy the extracted directory into the store
        copy_dir_recursive(extracted_dir, &dest)?;

        debug!("Stored {name}@{version} in {}", dest.display());
        Ok(dest)
    }

    /// Get the store base directory
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

impl Default for ContentStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute SHA-256 hash of file contents
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    hex_encode(&result)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Recursively copy a directory
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path)?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(&src_path)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &dst_path)?;
        }
    }

    Ok(())
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}
