use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tracing::debug;

use crate::metadata::PackageMetadata;

/// Disk-based metadata cache for npm registry responses.
/// Stores JSON in `~/.alchemy-store/metadata-cache/`.
pub struct MetadataCache {
    cache_dir: PathBuf,
    ttl: Duration,
}

impl MetadataCache {
    pub fn new(store_dir: &Path, ttl: Duration) -> Self {
        let cache_dir = store_dir.join("metadata-cache");
        Self { cache_dir, ttl }
    }

    /// Get cached metadata if present and not expired.
    pub fn get(&self, package_name: &str) -> Option<PackageMetadata> {
        let json_path = self.cache_path(package_name);
        if !json_path.exists() {
            return None;
        }

        // Check TTL via file modification time
        if let Ok(metadata) = fs::metadata(&json_path) {
            if let Ok(modified) = metadata.modified() {
                if let Ok(elapsed) = SystemTime::now().duration_since(modified) {
                    if elapsed > self.ttl {
                        debug!("Cache expired for {}", package_name);
                        return None;
                    }
                }
            }
        }

        // Read and deserialize
        let data = fs::read(&json_path).ok()?;
        match serde_json::from_slice::<PackageMetadata>(&data) {
            Ok(metadata) => {
                debug!("Cache hit for {}", package_name);
                Some(metadata)
            }
            Err(e) => {
                debug!("Cache parse error for {}: {}", package_name, e);
                // Remove corrupt cache file
                let _ = fs::remove_file(&json_path);
                None
            }
        }
    }

    /// Write metadata to disk cache.
    pub fn set(&self, package_name: &str, metadata: &PackageMetadata) {
        if let Err(e) = self.set_inner(package_name, metadata) {
            debug!("Failed to write cache for {}: {}", package_name, e);
        }
    }

    fn set_inner(&self, package_name: &str, metadata: &PackageMetadata) -> std::io::Result<()> {
        fs::create_dir_all(&self.cache_dir)?;
        let json_path = self.cache_path(package_name);
        let data = serde_json::to_vec(metadata).map_err(std::io::Error::other)?;
        fs::write(&json_path, data)?;
        Ok(())
    }

    /// Remove a specific cache entry.
    pub fn invalidate(&self, package_name: &str) {
        let json_path = self.cache_path(package_name);
        let _ = fs::remove_file(&json_path);
    }

    /// Remove all cached metadata.
    pub fn clear(&self) {
        let _ = fs::remove_dir_all(&self.cache_dir);
    }

    /// Compute cache file path, encoding scoped names.
    fn cache_path(&self, package_name: &str) -> PathBuf {
        // @scope/name → @scope+name.json
        let encoded = package_name.replace('/', "+");
        self.cache_dir.join(format!("{}.json", encoded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn test_metadata() -> PackageMetadata {
        PackageMetadata {
            name: "test-pkg".to_string(),
            versions: BTreeMap::new(),
            dist_tags: BTreeMap::new(),
        }
    }

    #[test]
    fn test_cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = MetadataCache::new(dir.path(), Duration::from_secs(300));

        assert!(cache.get("test-pkg").is_none());

        cache.set("test-pkg", &test_metadata());
        let cached = cache.get("test-pkg").unwrap();
        assert_eq!(cached.name, "test-pkg");
    }

    #[test]
    fn test_cache_scoped_package() {
        let dir = tempfile::tempdir().unwrap();
        let cache = MetadataCache::new(dir.path(), Duration::from_secs(300));

        let mut meta = test_metadata();
        meta.name = "@scope/pkg".to_string();
        cache.set("@scope/pkg", &meta);

        let cached = cache.get("@scope/pkg").unwrap();
        assert_eq!(cached.name, "@scope/pkg");
    }

    #[test]
    fn test_cache_expired() {
        let dir = tempfile::tempdir().unwrap();
        let cache = MetadataCache::new(dir.path(), Duration::from_secs(0));

        cache.set("test-pkg", &test_metadata());
        // TTL=0 means immediately expired
        // File was just written so it might not be expired yet on fast systems
        // but with TTL=0, the next read after any delay will be expired
        std::thread::sleep(Duration::from_millis(10));
        assert!(cache.get("test-pkg").is_none());
    }

    #[test]
    fn test_cache_invalidate() {
        let dir = tempfile::tempdir().unwrap();
        let cache = MetadataCache::new(dir.path(), Duration::from_secs(300));

        cache.set("test-pkg", &test_metadata());
        assert!(cache.get("test-pkg").is_some());

        cache.invalidate("test-pkg");
        assert!(cache.get("test-pkg").is_none());
    }
}
