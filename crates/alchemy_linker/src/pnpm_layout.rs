use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pathdiff::diff_paths;
use tracing::{debug, info, warn};

use alchemy_core::dependency::PackageId;
use alchemy_core::manifest::BinField;
use alchemy_core::resolver::ResolutionResult;

use crate::hardlink;
use crate::symlink;

/// Compute a relative symlink target: the relative path from the link's parent directory to `target`.
fn relative_target(target: &Path, link: &Path) -> PathBuf {
    let link_parent = link.parent().expect("symlink path must have a parent");
    diff_paths(target, link_parent).unwrap_or_else(|| target.to_path_buf())
}

/// Create pnpm-style node_modules layout:
///
/// ```text
/// node_modules/
/// ├── .pnpm/
/// │   └── pkg@version/
/// │       └── node_modules/
/// │           ├── pkg/           ← hardlinked from store
/// │           └── dep → ../../dep@ver/node_modules/dep
/// └── direct-dep → .pnpm/direct-dep@ver/node_modules/direct-dep
/// ```
pub fn link_packages(
    project_dir: &Path,
    resolution: &ResolutionResult,
    store_dir: &Path,
) -> anyhow::Result<()> {
    let node_modules = project_dir.join("node_modules");
    let pnpm_dir = node_modules.join(".pnpm");

    // Clean existing node_modules
    if node_modules.exists() {
        std::fs::remove_dir_all(&node_modules)?;
    }
    std::fs::create_dir_all(&pnpm_dir)?;

    // Step 1: Create .pnpm virtual store — hardlink each package from the global store
    for id in resolution.packages.keys() {
        let pkg_pnpm_dir = pnpm_dir
            .join(id.pnpm_dir_name())
            .join("node_modules")
            .join(&id.name);

        let store_pkg_dir = store_package_dir(store_dir, &id.name, &id.version);

        if !store_pkg_dir.exists() {
            anyhow::bail!(
                "Package {} not found in store at {}",
                id,
                store_pkg_dir.display()
            );
        }

        debug!(
            "Hardlinking {} → {}",
            store_pkg_dir.display(),
            pkg_pnpm_dir.display()
        );
        hardlink::hardlink_dir(&store_pkg_dir, &pkg_pnpm_dir)?;
    }

    // Step 2: Create symlinks for transitive dependencies within .pnpm
    for (id, pkg) in &resolution.packages {
        let pkg_node_modules = pnpm_dir.join(id.pnpm_dir_name()).join("node_modules");

        for (dep_name, dep_version) in &pkg.dependencies {
            // Find the resolved version of this dependency
            let dep_id = find_resolved_dep(&resolution.packages, dep_name, dep_version);

            if let Some(dep_id) = dep_id {
                let symlink_path = pkg_node_modules.join(dep_name);
                let target = pnpm_dir
                    .join(dep_id.pnpm_dir_name())
                    .join("node_modules")
                    .join(dep_name);

                if !symlink_path.exists() {
                    let rel_target = relative_target(&target, &symlink_path);
                    debug!(
                        "Symlinking dep {} → {}",
                        symlink_path.display(),
                        rel_target.display()
                    );
                    symlink::create_symlink(&rel_target, &symlink_path)?;
                }
            }
        }
    }

    // Step 3: Create root symlinks for direct dependencies
    for id in &resolution.direct_deps {
        let link_path = node_modules.join(&id.name);
        let target = pnpm_dir
            .join(id.pnpm_dir_name())
            .join("node_modules")
            .join(&id.name);

        // Handle scoped packages — create @scope/ directory
        if id.name.starts_with('@') {
            if let Some(scope_dir) = link_path.parent() {
                std::fs::create_dir_all(scope_dir)?;
            }
        }

        info!("Linking {} → {}", link_path.display(), target.display());
        let rel_target = relative_target(&target, &link_path);
        symlink::create_symlink(&rel_target, &link_path)?;
    }

    // Step 4: Create node_modules/.bin/ symlinks for packages with bin fields
    let bin_dir = node_modules.join(".bin");
    let mut has_bins = false;

    for (id, pkg) in &resolution.packages {
        let bin_entries = match &pkg.bin {
            Some(BinField::Path(path)) => {
                // Single binary — name is the package name without scope
                let bin_name = id.name.rsplit('/').next().unwrap_or(&id.name);
                vec![(bin_name.to_string(), path.clone())]
            }
            Some(BinField::Map(map)) => {
                map.iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            }
            None => continue,
        };

        if !has_bins {
            std::fs::create_dir_all(&bin_dir)?;
            has_bins = true;
        }

        for (bin_name, bin_path) in bin_entries {
            let target = pnpm_dir
                .join(id.pnpm_dir_name())
                .join("node_modules")
                .join(&id.name)
                .join(&bin_path);

            let link_path = bin_dir.join(&bin_name);

            debug!("Bin link {} → {}", link_path.display(), target.display());
            let rel_target = relative_target(&target, &link_path);
            symlink::create_symlink(&rel_target, &link_path)?;

            // Make the target executable on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = std::fs::metadata(&target) {
                    let mut perms = metadata.permissions();
                    let mode = perms.mode();
                    // Add execute bits where there are read bits
                    perms.set_mode(mode | ((mode & 0o444) >> 2));
                    if let Err(e) = std::fs::set_permissions(&target, perms) {
                        warn!("Failed to chmod +x {}: {}", target.display(), e);
                    }
                }
            }
        }
    }

    if has_bins {
        info!("Linked bin entries to {}", bin_dir.display());
    }

    Ok(())
}

fn store_package_dir(store_dir: &Path, name: &str, version: &str) -> PathBuf {
    store_dir.join("packages").join(name).join(version)
}

fn find_resolved_dep<'a>(
    packages: &'a HashMap<PackageId, alchemy_core::dependency::ResolvedPackage>,
    name: &str,
    _version_req: &str,
) -> Option<&'a PackageId> {
    // Find the package with matching name (we only resolve one version per package)
    packages.keys().find(|id| id.name == name)
}
