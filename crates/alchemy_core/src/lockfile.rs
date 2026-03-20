use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use crate::dependency::PackageId;
use crate::error::AlchemyResult;
use crate::resolver::ResolutionResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    #[serde(rename = "lockfileVersion")]
    pub lockfile_version: String,
    pub importers: BTreeMap<String, LockfileImporter>,
    pub packages: BTreeMap<String, LockfilePackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileImporter {
    #[serde(default)]
    pub dependencies: BTreeMap<String, LockfileDependencyRef>,
    #[serde(default, rename = "devDependencies")]
    pub dev_dependencies: BTreeMap<String, LockfileDependencyRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileDependencyRef {
    pub specifier: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfilePackage {
    pub resolution: LockfileResolution,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileResolution {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tarball: Option<String>,
}

impl Lockfile {
    pub fn new() -> Self {
        Self {
            lockfile_version: "1.0".to_string(),
            importers: BTreeMap::new(),
            packages: BTreeMap::new(),
        }
    }

    /// Build lockfile from resolution result
    pub fn from_resolution(
        resolution: &ResolutionResult,
        root_deps: &BTreeMap<String, String>,
        root_dev_deps: &BTreeMap<String, String>,
    ) -> Self {
        let mut lockfile = Lockfile::new();

        // Build importer for root project
        let mut importer = LockfileImporter {
            dependencies: BTreeMap::new(),
            dev_dependencies: BTreeMap::new(),
        };

        for id in &resolution.direct_deps {
            let specifier = root_deps
                .get(&id.name)
                .or_else(|| root_dev_deps.get(&id.name))
                .cloned()
                .unwrap_or_default();

            let dep_ref = LockfileDependencyRef {
                specifier,
                version: id.version.clone(),
            };

            if root_dev_deps.contains_key(&id.name) {
                importer.dev_dependencies.insert(id.name.clone(), dep_ref);
            } else {
                importer.dependencies.insert(id.name.clone(), dep_ref);
            }
        }

        lockfile.importers.insert(".".to_string(), importer);

        // Build packages
        for (id, pkg) in &resolution.packages {
            let key = format!("/{}@{}", id.name, id.version);
            let lock_pkg = LockfilePackage {
                resolution: LockfileResolution {
                    integrity: pkg.integrity.clone(),
                    tarball: Some(pkg.tarball_url.clone()),
                },
                dependencies: pkg.dependencies.clone(),
            };
            lockfile.packages.insert(key, lock_pkg);
        }

        lockfile
    }

    pub fn write_to_file(&self, path: &Path) -> AlchemyResult<()> {
        let yaml = serde_yaml::to_string(self)?;
        std::fs::write(path, yaml)?;
        Ok(())
    }

    pub fn read_from_file(path: &Path) -> AlchemyResult<Self> {
        let content = std::fs::read_to_string(path)?;
        let lockfile: Lockfile = serde_yaml::from_str(&content)?;
        Ok(lockfile)
    }

    /// Get all resolved PackageIds from the lockfile
    pub fn all_package_ids(&self) -> Vec<PackageId> {
        self.packages
            .keys()
            .filter_map(|key| {
                // key format: /name@version
                let key = key.strip_prefix('/')?;
                let at_pos = key.rfind('@')?;
                let name = &key[..at_pos];
                let version = &key[at_pos + 1..];
                Some(PackageId::new(name, version))
            })
            .collect()
    }
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}
