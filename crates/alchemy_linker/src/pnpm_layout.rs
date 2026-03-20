use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::{debug, info};

use alchemy_core::dependency::PackageId;
use alchemy_core::resolver::ResolutionResult;

use crate::hardlink;
use crate::symlink;

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
    for (id, _pkg) in &resolution.packages {
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
                    debug!(
                        "Symlinking dep {} → {}",
                        symlink_path.display(),
                        target.display()
                    );
                    symlink::create_symlink(&target, &symlink_path)?;
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
        symlink::create_symlink(&target, &link_path)?;
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
