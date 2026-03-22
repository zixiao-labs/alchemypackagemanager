use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{AlchemyError, AlchemyResult};
use crate::lockfile::{
    Lockfile, LockfileDependencyRef, LockfileImporter, LockfilePackage, LockfileResolution,
};

/// npm package-lock.json v3 structures
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NpmLockfile {
    #[serde(default)]
    lockfile_version: u32,
    #[serde(default)]
    packages: BTreeMap<String, NpmPackageEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NpmPackageEntry {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    resolved: Option<String>,
    #[serde(default)]
    integrity: Option<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default)]
    dev_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    peer_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    optional_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    dev: Option<bool>,
    #[serde(default)]
    link: Option<bool>,
}

/// Import an npm package-lock.json (v2/v3) into Alchemy lockfile format.
pub fn import_npm_lockfile(path: &Path) -> AlchemyResult<Lockfile> {
    let content = std::fs::read_to_string(path)?;
    let npm_lock: NpmLockfile = serde_json::from_str(&content)
        .map_err(|e| AlchemyError::Other(format!("Failed to parse package-lock.json: {}", e)))?;

    if npm_lock.lockfile_version < 2 {
        return Err(AlchemyError::Other(
            "Only package-lock.json v2/v3 is supported (lockfileVersion >= 2)".to_string(),
        ));
    }

    let mut lockfile = Lockfile::new();
    let mut importer = LockfileImporter {
        dependencies: BTreeMap::new(),
        dev_dependencies: BTreeMap::new(),
    };

    // The root entry has key "" and contains the project's direct deps
    let root_entry = npm_lock.packages.get("");

    for (nm_path, entry) in &npm_lock.packages {
        // Skip the root entry
        if nm_path.is_empty() {
            continue;
        }

        // Skip linked packages
        if entry.link == Some(true) {
            continue;
        }

        let version = match &entry.version {
            Some(v) => v.clone(),
            None => continue,
        };

        // Extract package name from the node_modules path
        // e.g., "node_modules/express" → "express"
        // e.g., "node_modules/@scope/name" → "@scope/name"
        // e.g., "node_modules/express/node_modules/debug" → "debug" (nested)
        let name = match extract_package_name(nm_path) {
            Some(n) => n,
            None => continue,
        };

        let key = format!("/{}@{}", name, version);

        let lock_pkg = LockfilePackage {
            resolution: LockfileResolution {
                integrity: entry.integrity.clone(),
                tarball: entry.resolved.clone(),
                git: None,
                directory: None,
            },
            dependencies: entry.dependencies.clone(),
            peer_dependencies: entry.peer_dependencies.clone(),
            optional_dependencies: entry.optional_dependencies.clone(),
        };

        lockfile.packages.insert(key, lock_pkg);

        // Check if this is a direct dependency (top-level node_modules/*)
        if is_direct_dep(nm_path) {
            if let Some(root) = root_entry {
                // Determine the specifier from the root entry
                let specifier = root
                    .dependencies
                    .get(&name)
                    .or_else(|| root.dev_dependencies.get(&name))
                    .or_else(|| root.optional_dependencies.get(&name))
                    .cloned()
                    .unwrap_or_else(|| format!("^{}", version));

                let dep_ref = LockfileDependencyRef {
                    specifier,
                    version: version.clone(),
                };

                if entry.dev == Some(true) || root.dev_dependencies.contains_key(&name) {
                    importer.dev_dependencies.insert(name, dep_ref);
                } else {
                    importer.dependencies.insert(name, dep_ref);
                }
            }
        }
    }

    lockfile.importers.insert(".".to_string(), importer);

    Ok(lockfile)
}

/// Extract package name from node_modules path.
/// "node_modules/express" → "express"
/// "node_modules/@scope/name" → "@scope/name"
/// "node_modules/a/node_modules/b" → "b"
fn extract_package_name(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.rsplitn(2, "node_modules/").collect();
    let last = parts.first()?;
    if last.is_empty() {
        return None;
    }
    // Remove any trailing path segments (shouldn't happen in v3 format)
    Some(last.trim_end_matches('/').to_string())
}

/// Check if a path represents a direct (top-level) dependency.
/// "node_modules/express" → true
/// "node_modules/express/node_modules/debug" → false
fn is_direct_dep(path: &str) -> bool {
    // Count occurrences of "node_modules/"
    path.matches("node_modules/").count() == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_package_name() {
        assert_eq!(
            extract_package_name("node_modules/express"),
            Some("express".to_string())
        );
        assert_eq!(
            extract_package_name("node_modules/@types/node"),
            Some("@types/node".to_string())
        );
        assert_eq!(
            extract_package_name("node_modules/a/node_modules/b"),
            Some("b".to_string())
        );
    }

    #[test]
    fn test_is_direct_dep() {
        assert!(is_direct_dep("node_modules/express"));
        assert!(!is_direct_dep("node_modules/express/node_modules/debug"));
    }

    #[test]
    fn test_import_npm_lockfile_v3() {
        let json = r#"{
            "name": "test-project",
            "version": "1.0.0",
            "lockfileVersion": 3,
            "packages": {
                "": {
                    "name": "test-project",
                    "version": "1.0.0",
                    "dependencies": {
                        "express": "^4.18.0"
                    },
                    "devDependencies": {
                        "jest": "^29.0.0"
                    }
                },
                "node_modules/express": {
                    "version": "4.18.2",
                    "resolved": "https://registry.npmjs.org/express/-/express-4.18.2.tgz",
                    "integrity": "sha512-abc123",
                    "dependencies": {
                        "body-parser": "1.20.1"
                    }
                },
                "node_modules/jest": {
                    "version": "29.7.0",
                    "resolved": "https://registry.npmjs.org/jest/-/jest-29.7.0.tgz",
                    "integrity": "sha512-def456",
                    "dev": true
                },
                "node_modules/body-parser": {
                    "version": "1.20.1",
                    "resolved": "https://registry.npmjs.org/body-parser/-/body-parser-1.20.1.tgz",
                    "integrity": "sha512-ghi789"
                }
            }
        }"#;

        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("package-lock.json");
        std::fs::write(&lock_path, json).unwrap();

        let lockfile = import_npm_lockfile(&lock_path).unwrap();

        assert_eq!(lockfile.lockfile_version, "1.0");
        assert_eq!(lockfile.packages.len(), 3);

        // Check express
        let express = lockfile.packages.get("/express@4.18.2").unwrap();
        assert_eq!(
            express.resolution.tarball.as_deref(),
            Some("https://registry.npmjs.org/express/-/express-4.18.2.tgz")
        );
        assert_eq!(express.dependencies.get("body-parser").unwrap(), "1.20.1");

        // Check importers
        let root = lockfile.importers.get(".").unwrap();
        assert!(root.dependencies.contains_key("express"));
        assert!(root.dev_dependencies.contains_key("jest"));
        assert_eq!(root.dependencies["express"].specifier, "^4.18.0");
    }
}
