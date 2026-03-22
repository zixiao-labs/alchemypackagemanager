use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tracing::debug;

use crate::error::{AlchemyError, AlchemyResult};
use crate::manifest::Manifest;

/// A discovered workspace configuration.
pub struct Workspace {
    /// Root directory of the workspace
    pub root_dir: PathBuf,
    /// Root package.json manifest
    pub root_manifest: Manifest,
    /// Discovered workspace member packages
    pub members: Vec<WorkspaceMember>,
}

/// A single workspace member package.
pub struct WorkspaceMember {
    /// Relative path from workspace root (e.g., "packages/ui")
    pub relative_path: String,
    /// Absolute path to the member directory
    pub dir: PathBuf,
    /// Parsed package.json
    pub manifest: Manifest,
}

impl Workspace {
    /// Discover workspace from a root directory.
    /// Returns None if no workspaces field is present in the root package.json.
    pub fn discover(root_dir: &Path) -> AlchemyResult<Option<Self>> {
        let manifest_path = root_dir.join("package.json");
        if !manifest_path.exists() {
            return Err(AlchemyError::Other(format!(
                "No package.json found in {}",
                root_dir.display()
            )));
        }

        let root_manifest = Manifest::from_path(&manifest_path)?;

        let patterns = match &root_manifest.workspaces {
            Some(patterns) if !patterns.is_empty() => patterns.clone(),
            _ => return Ok(None),
        };

        let mut members = Vec::new();

        for pattern in &patterns {
            let full_pattern = root_dir.join(pattern);
            let pattern_str = full_pattern.to_string_lossy().to_string();

            let entries = glob::glob(&pattern_str).map_err(|e| {
                AlchemyError::Other(format!("Invalid glob pattern '{}': {}", pattern, e))
            })?;

            for entry in entries {
                let dir = entry.map_err(|e| AlchemyError::Other(format!("Glob error: {}", e)))?;

                // Each matched path should be a directory with a package.json
                let member_manifest_path = if dir.join("package.json").exists() {
                    dir.join("package.json")
                } else {
                    continue;
                };

                let member_manifest = Manifest::from_path(&member_manifest_path)?;

                let relative_path = dir
                    .strip_prefix(root_dir)
                    .map_err(|_| {
                        AlchemyError::Other(format!(
                            "Workspace member {} is not under root {}",
                            dir.display(),
                            root_dir.display()
                        ))
                    })?
                    .to_string_lossy()
                    .to_string();

                debug!(
                    "Found workspace member: {} ({})",
                    member_manifest.name.as_deref().unwrap_or("<unnamed>"),
                    relative_path
                );

                members.push(WorkspaceMember {
                    relative_path,
                    dir,
                    manifest: member_manifest,
                });
            }
        }

        Ok(Some(Workspace {
            root_dir: root_dir.to_path_buf(),
            root_manifest,
            members,
        }))
    }

    /// Get a map of workspace member package names to their versions.
    pub fn member_versions(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        for member in &self.members {
            if let (Some(name), Some(version)) = (&member.manifest.name, &member.manifest.version) {
                map.insert(name.clone(), version.clone());
            }
        }
        map
    }

    /// Resolve a workspace: specifier to an actual version range.
    /// - "workspace:*" → "*" (any version, linked locally)
    /// - "workspace:^" → "^{actual_version}"
    /// - "workspace:~" → "~{actual_version}"
    /// - "workspace:^1.0.0" → "^1.0.0"
    pub fn resolve_workspace_specifier(spec: &str, member_version: &str) -> String {
        match spec {
            "*" => "*".to_string(),
            "^" => format!("^{}", member_version),
            "~" => format!("~{}", member_version),
            other => other.to_string(),
        }
    }

    /// Collect all external dependencies across all members, resolving workspace: protocol.
    /// Returns (importer_key → deps) map for multi-root resolution.
    pub fn collect_all_dependencies(&self) -> BTreeMap<String, BTreeMap<String, String>> {
        let member_versions = self.member_versions();
        let mut importers = BTreeMap::new();

        // Root project deps
        let mut root_deps = self.root_manifest.all_dependencies();
        resolve_workspace_deps(&mut root_deps, &member_versions);
        importers.insert(".".to_string(), root_deps);

        // Each member's deps
        for member in &self.members {
            let mut deps = member.manifest.all_dependencies();
            resolve_workspace_deps(&mut deps, &member_versions);
            importers.insert(member.relative_path.clone(), deps);
        }

        importers
    }

    /// Find a workspace member by package name.
    pub fn find_member(&self, name: &str) -> Option<&WorkspaceMember> {
        self.members
            .iter()
            .find(|m| m.manifest.name.as_deref() == Some(name))
    }
}

/// Replace workspace: deps with the actual version from the workspace member.
/// Deps that reference workspace members are removed (they'll be symlinked, not resolved).
fn resolve_workspace_deps(
    deps: &mut BTreeMap<String, String>,
    member_versions: &BTreeMap<String, String>,
) {
    let ws_deps: Vec<(String, String)> = deps
        .iter()
        .filter(|(_, spec)| spec.starts_with("workspace:"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    for (name, spec) in ws_deps {
        let ws_spec = spec.strip_prefix("workspace:").unwrap();
        if let Some(member_ver) = member_versions.get(&name) {
            // This is a workspace member reference — remove from external deps
            // (it will be symlinked directly)
            let _resolved = Workspace::resolve_workspace_specifier(ws_spec, member_ver);
            deps.remove(&name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_workspace_specifier() {
        assert_eq!(Workspace::resolve_workspace_specifier("*", "1.0.0"), "*");
        assert_eq!(
            Workspace::resolve_workspace_specifier("^", "1.2.3"),
            "^1.2.3"
        );
        assert_eq!(
            Workspace::resolve_workspace_specifier("~", "2.0.0"),
            "~2.0.0"
        );
        assert_eq!(
            Workspace::resolve_workspace_specifier("^1.0.0", "1.2.3"),
            "^1.0.0"
        );
    }
}
