use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

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
            let resolution = if let Some(ref path) = pkg.source_path {
                // file: local dependency — store the directory path
                LockfileResolution {
                    integrity: None,
                    tarball: None,
                    git: None,
                    directory: Some(path.to_string_lossy().into_owned()),
                }
            } else {
                LockfileResolution {
                    integrity: pkg.integrity.clone(),
                    tarball: if pkg.tarball_url.is_empty() {
                        None
                    } else {
                        Some(pkg.tarball_url.clone())
                    },
                    git: None,
                    directory: None,
                }
            };
            let lock_pkg = LockfilePackage {
                resolution,
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

            let source_path = lock_pkg.resolution.directory.as_deref().map(PathBuf::from);

            let resolved = ResolvedPackage {
                id: id.clone(),
                tarball_url: lock_pkg.resolution.tarball.clone().unwrap_or_default(),
                integrity: lock_pkg.resolution.integrity.clone(),
                source_path,
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
            let stripped = key.strip_prefix('/').ok_or_else(|| {
                AlchemyError::Other(format!(
                    "invalid lockfile package key (must start with '/'): {key}"
                ))
            })?;
            let at_pos = stripped.rfind('@').ok_or_else(|| {
                AlchemyError::Other(format!(
                    "invalid lockfile package key (missing @version): {key}"
                ))
            })?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dependency::ResolvedPackage;
    use crate::graph::DependencyGraph;
    use crate::resolver::ResolutionResult;
    use std::collections::HashMap;

    fn make_resolution(packages: Vec<(&str, &str, &str)>) -> ResolutionResult {
        let mut pkgs: HashMap<PackageId, ResolvedPackage> = HashMap::new();
        let mut direct_deps = Vec::new();

        for (name, version, tarball) in &packages {
            let id = PackageId::new(*name, *version);
            pkgs.insert(
                id.clone(),
                ResolvedPackage {
                    id: id.clone(),
                    tarball_url: tarball.to_string(),
                    integrity: Some(format!("sha512-fake-{}", name)),
                    source_path: None,
                    dependencies: BTreeMap::new(),
                    peer_dependencies: BTreeMap::new(),
                    optional_dependencies: BTreeMap::new(),
                    bin: None,
                    os: None,
                    cpu: None,
                    engines: None,
                },
            );
            direct_deps.push(id);
        }

        ResolutionResult {
            graph: DependencyGraph::new(),
            packages: pkgs,
            direct_deps,
            peer_warnings: Vec::new(),
        }
    }

    #[test]
    fn test_lockfile_roundtrip() {
        let resolution = make_resolution(vec![
            (
                "express",
                "4.18.0",
                "https://example.com/express-4.18.0.tgz",
            ),
            (
                "lodash",
                "4.17.21",
                "https://example.com/lodash-4.17.21.tgz",
            ),
        ]);

        let root_deps: BTreeMap<String, String> = [
            ("express".to_string(), "^4.17.0".to_string()),
            ("lodash".to_string(), "^4.0.0".to_string()),
        ]
        .into();
        let dev_deps: BTreeMap<String, String> = BTreeMap::new();

        let lockfile = Lockfile::from_resolution(&resolution, &root_deps, &dev_deps);
        assert_eq!(lockfile.packages.len(), 2);
        assert!(lockfile.packages.contains_key("/express@4.18.0"));
        assert!(lockfile.packages.contains_key("/lodash@4.17.21"));

        // Roundtrip through YAML
        let yaml = serde_yaml::to_string(&lockfile).unwrap();
        let restored: Lockfile = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(restored.packages.len(), 2);

        // Reconstruct resolution result
        let result = restored.to_resolution_result().unwrap();
        assert_eq!(result.packages.len(), 2);
        assert!(result
            .packages
            .keys()
            .any(|id| id.name == "express" && id.version == "4.18.0"));
    }

    #[test]
    fn test_lockfile_preserves_integrity() {
        let resolution =
            make_resolution(vec![("pkg", "1.0.0", "https://example.com/pkg-1.0.0.tgz")]);
        let root_deps: BTreeMap<String, String> =
            [("pkg".to_string(), "^1.0.0".to_string())].into();
        let lockfile = Lockfile::from_resolution(&resolution, &root_deps, &BTreeMap::new());

        let pkg_entry = lockfile.packages.get("/pkg@1.0.0").unwrap();
        assert_eq!(
            pkg_entry.resolution.integrity.as_deref(),
            Some("sha512-fake-pkg")
        );
    }

    #[test]
    fn test_malformed_key_returns_error_not_panic() {
        let mut lockfile = Lockfile::new();
        // Insert a package with a key missing the '/' prefix
        lockfile.packages.insert(
            "no-slash@1.0.0".to_string(),
            LockfilePackage {
                resolution: LockfileResolution {
                    integrity: None,
                    tarball: Some("https://example.com/pkg.tgz".to_string()),
                    git: None,
                    directory: None,
                },
                dependencies: BTreeMap::new(),
                peer_dependencies: BTreeMap::new(),
                optional_dependencies: BTreeMap::new(),
            },
        );
        // Also add a valid importer so the first parse loop passes
        lockfile.importers.insert(
            ".".to_string(),
            LockfileImporter {
                dependencies: BTreeMap::new(),
                dev_dependencies: BTreeMap::new(),
            },
        );

        let result = lockfile.to_resolution_result();
        assert!(
            result.is_err(),
            "malformed lockfile key must return Err, not panic"
        );
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("no-slash@1.0.0") || err_msg.contains("invalid lockfile"),
            "error message should mention the bad key"
        );
    }

    #[test]
    fn test_validate_against_manifest_passes_when_in_sync() {
        let resolution = make_resolution(vec![("lodash", "4.17.21", "url")]);
        let root_deps: BTreeMap<String, String> =
            [("lodash".to_string(), "^4.0.0".to_string())].into();
        let lockfile = Lockfile::from_resolution(&resolution, &root_deps, &BTreeMap::new());
        // Validation should succeed when specifiers match
        assert!(lockfile
            .validate_against_manifest(&root_deps, &BTreeMap::new())
            .is_ok());
    }

    #[test]
    fn test_validate_against_manifest_fails_on_drift() {
        let resolution = make_resolution(vec![("lodash", "4.17.21", "url")]);
        let old_deps: BTreeMap<String, String> =
            [("lodash".to_string(), "^4.0.0".to_string())].into();
        let lockfile = Lockfile::from_resolution(&resolution, &old_deps, &BTreeMap::new());
        // Now manifest wants a different specifier (e.g. ^5.0.0)
        let new_deps: BTreeMap<String, String> =
            [("lodash".to_string(), "^5.0.0".to_string())].into();
        assert!(lockfile
            .validate_against_manifest(&new_deps, &BTreeMap::new())
            .is_err());
    }
}
