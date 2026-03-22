use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::dependency::{PackageId, ResolvedPackage};
use crate::error::{AlchemyError, AlchemyResult};
use crate::graph::{DepEdge, DependencyGraph};
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
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        rename = "peerDependencies"
    )]
    pub peer_dependencies: BTreeMap<String, String>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        rename = "optionalDependencies"
    )]
    pub optional_dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileResolution {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tarball: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
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
                    git: None,
                    directory: None,
                },
                dependencies: pkg.dependencies.clone(),
                peer_dependencies: pkg.peer_dependencies.clone(),
                optional_dependencies: pkg.optional_dependencies.clone(),
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

    /// Validate that the lockfile is consistent with the given manifest deps.
    /// Returns an error describing mismatches.
    pub fn validate_against_manifest(
        &self,
        deps: &BTreeMap<String, String>,
        dev_deps: &BTreeMap<String, String>,
    ) -> AlchemyResult<()> {
        let root_importer = self
            .importers
            .get(".")
            .ok_or_else(|| AlchemyError::Other("Lockfile has no root importer".to_string()))?;

        // Check all deps are present with matching specifiers
        for (name, specifier) in deps {
            match root_importer.dependencies.get(name) {
                Some(dep_ref) if dep_ref.specifier == *specifier => {}
                Some(dep_ref) => {
                    return Err(AlchemyError::Other(format!(
                        "Lockfile out of date: {} specifier is '{}' but package.json has '{}'",
                        name, dep_ref.specifier, specifier
                    )));
                }
                None => {
                    return Err(AlchemyError::Other(format!(
                        "Lockfile out of date: {} is in package.json but not in lockfile",
                        name
                    )));
                }
            }
        }

        for (name, specifier) in dev_deps {
            match root_importer.dev_dependencies.get(name) {
                Some(dep_ref) if dep_ref.specifier == *specifier => {}
                Some(dep_ref) => {
                    return Err(AlchemyError::Other(format!(
                        "Lockfile out of date: {} specifier is '{}' but package.json has '{}'",
                        name, dep_ref.specifier, specifier
                    )));
                }
                None => {
                    return Err(AlchemyError::Other(format!(
                        "Lockfile out of date: {} is in package.json devDependencies but not in lockfile",
                        name
                    )));
                }
            }
        }

        Ok(())
    }

    /// Reconstruct a ResolutionResult from the lockfile, avoiding full resolution.
    /// Used by `alchemy ci` to install directly from the lockfile.
    pub fn to_resolution_result(&self) -> AlchemyResult<ResolutionResult> {
        let mut graph = DependencyGraph::new();
        let mut packages: HashMap<PackageId, ResolvedPackage> = HashMap::new();
        let mut direct_deps = Vec::new();

        // Parse all packages
        for (key, lock_pkg) in &self.packages {
            let stripped = key
                .strip_prefix('/')
                .ok_or_else(|| AlchemyError::Other(format!("Invalid lockfile key: {}", key)))?;
            let at_pos = stripped
                .rfind('@')
                .ok_or_else(|| AlchemyError::Other(format!("Invalid lockfile key: {}", key)))?;
            let name = &stripped[..at_pos];
            let version = &stripped[at_pos + 1..];

            let id = PackageId::new(name, version);
            graph.add_package(id.clone());

            let resolved = ResolvedPackage {
                id: id.clone(),
                tarball_url: lock_pkg.resolution.tarball.clone().unwrap_or_default(),
                integrity: lock_pkg.resolution.integrity.clone(),
                dependencies: lock_pkg.dependencies.clone(),
                peer_dependencies: lock_pkg.peer_dependencies.clone(),
                optional_dependencies: lock_pkg.optional_dependencies.clone(),
                bin: None,
                os: None,
                cpu: None,
                engines: None,
            };

            packages.insert(id, resolved);
        }

        // Build graph edges
        for (key, lock_pkg) in &self.packages {
            let stripped = key.strip_prefix('/').unwrap();
            let at_pos = stripped.rfind('@').unwrap();
            let parent = PackageId::new(&stripped[..at_pos], &stripped[at_pos + 1..]);

            for (dep_name, dep_ver) in &lock_pkg.dependencies {
                let child = PackageId::new(dep_name, dep_ver);
                if packages.contains_key(&child) {
                    graph.add_package(child.clone());
                    graph.add_dependency(&parent, &child, DepEdge::Normal);
                }
            }
            for (dep_name, dep_ver) in &lock_pkg.peer_dependencies {
                let child = PackageId::new(dep_name, dep_ver);
                if packages.contains_key(&child) {
                    graph.add_package(child.clone());
                    graph.add_dependency(&parent, &child, DepEdge::Peer);
                }
            }
            for (dep_name, dep_ver) in &lock_pkg.optional_dependencies {
                let child = PackageId::new(dep_name, dep_ver);
                if packages.contains_key(&child) {
                    graph.add_package(child.clone());
                    graph.add_dependency(&parent, &child, DepEdge::Optional);
                }
            }
        }

        // Determine direct deps from importers
        if let Some(root) = self.importers.get(".") {
            for (name, dep_ref) in root.dependencies.iter().chain(root.dev_dependencies.iter()) {
                let id = PackageId::new(name, &dep_ref.version);
                if packages.contains_key(&id) {
                    direct_deps.push(id);
                }
            }
        }

        Ok(ResolutionResult {
            graph,
            packages,
            direct_deps,
            peer_warnings: Vec::new(),
        })
    }
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}
