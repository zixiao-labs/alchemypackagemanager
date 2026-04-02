use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pathdiff::diff_paths;
use tracing::{debug, info, warn};

use alchemy_core::dependency::PackageId;
use alchemy_core::manifest::BinField;
use alchemy_core::resolver::{ResolutionResult, WorkspaceResolutionResult};
use alchemy_core::workspace::Workspace;

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
    // (or from source_path for file:/git:/url: deps)
    for (id, pkg) in &resolution.packages {
        let pkg_pnpm_dir = pnpm_dir
            .join(id.pnpm_dir_name())
            .join("node_modules")
            .join(&id.name);

        let src_dir = if let Some(ref sp) = pkg.source_path {
            // file: / git: / url: dependency — link directly from source
            if !sp.exists() {
                anyhow::bail!("Source path for {} not found at {}", id, sp.display());
            }
            sp.clone()
        } else {
            let store_pkg_dir = store_package_dir(store_dir, &id.name, &id.version);
            if !store_pkg_dir.exists() {
                anyhow::bail!(
                    "Package {} not found in store at {}",
                    id,
                    store_pkg_dir.display()
                );
            }
            store_pkg_dir
        };

        debug!(
            "Hardlinking {} → {}",
            src_dir.display(),
            pkg_pnpm_dir.display()
        );
        hardlink::hardlink_dir(&src_dir, &pkg_pnpm_dir)?;
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
            Some(BinField::Map(map)) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
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

/// Link packages for a workspace: shared .pnpm at workspace root,
/// per-member node_modules with root symlinks, workspace packages symlinked directly.
pub fn link_workspace_packages(
    workspace: &Workspace,
    resolution: &WorkspaceResolutionResult,
    store_dir: &Path,
) -> anyhow::Result<()> {
    let root_node_modules = workspace.root_dir.join("node_modules");
    let pnpm_dir = root_node_modules.join(".pnpm");

    // Clean existing root node_modules
    if root_node_modules.exists() {
        std::fs::remove_dir_all(&root_node_modules)?;
    }
    std::fs::create_dir_all(&pnpm_dir)?;

    // Step 1: Hardlink all resolved packages into shared .pnpm/
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

        hardlink::hardlink_dir(&store_pkg_dir, &pkg_pnpm_dir)?;
    }

    // Step 2: Create dep symlinks within .pnpm (same as single-project)
    for (id, pkg) in &resolution.packages {
        let pkg_node_modules = pnpm_dir.join(id.pnpm_dir_name()).join("node_modules");

        for (dep_name, dep_version) in &pkg.dependencies {
            let dep_id = find_resolved_dep(&resolution.packages, dep_name, dep_version);
            if let Some(dep_id) = dep_id {
                let symlink_path = pkg_node_modules.join(dep_name);
                let target = pnpm_dir
                    .join(dep_id.pnpm_dir_name())
                    .join("node_modules")
                    .join(dep_name);

                if !symlink_path.exists() {
                    let rel_target = relative_target(&target, &symlink_path);
                    symlink::create_symlink(&rel_target, &symlink_path)?;
                }
            }
        }
    }

    // Step 3: Create root symlinks for the root importer's direct deps
    if let Some(root_deps) = resolution.importers.get(".") {
        for id in root_deps {
            let link_path = root_node_modules.join(&id.name);
            let target = pnpm_dir
                .join(id.pnpm_dir_name())
                .join("node_modules")
                .join(&id.name);

            if id.name.starts_with('@') {
                if let Some(scope_dir) = link_path.parent() {
                    std::fs::create_dir_all(scope_dir)?;
                }
            }

            let rel_target = relative_target(&target, &link_path);
            symlink::create_symlink(&rel_target, &link_path)?;
        }
    }

    // Step 4: For each workspace member, create per-member node_modules
    for member in &workspace.members {
        let member_node_modules = member.dir.join("node_modules");

        // Clean existing member node_modules
        if member_node_modules.exists() {
            std::fs::remove_dir_all(&member_node_modules)?;
        }
        std::fs::create_dir_all(&member_node_modules)?;

        // Symlink external direct deps to the shared .pnpm
        if let Some(member_deps) = resolution.importers.get(&member.relative_path) {
            for id in member_deps {
                let link_path = member_node_modules.join(&id.name);
                let target = pnpm_dir
                    .join(id.pnpm_dir_name())
                    .join("node_modules")
                    .join(&id.name);

                if id.name.starts_with('@') {
                    if let Some(scope_dir) = link_path.parent() {
                        std::fs::create_dir_all(scope_dir)?;
                    }
                }

                let rel_target = relative_target(&target, &link_path);
                symlink::create_symlink(&rel_target, &link_path)?;
            }
        }

        // Symlink workspace member inter-dependencies directly to source
        let all_deps = member.manifest.all_dependencies();
        for (dep_name, dep_spec) in &all_deps {
            if dep_spec.starts_with("workspace:") {
                if let Some(dep_member) = workspace.find_member(dep_name) {
                    let link_path = member_node_modules.join(dep_name);
                    if dep_name.starts_with('@') {
                        if let Some(scope_dir) = link_path.parent() {
                            std::fs::create_dir_all(scope_dir)?;
                        }
                    }

                    let rel_target = relative_target(&dep_member.dir, &link_path);
                    info!(
                        "Workspace link {} → {}",
                        link_path.display(),
                        dep_member.dir.display()
                    );
                    symlink::create_symlink(&rel_target, &link_path)?;
                }
            }
        }
    }

    // Step 5: Create .bin entries
    let bin_dir = root_node_modules.join(".bin");
    let mut has_bins = false;

    for (id, pkg) in &resolution.packages {
        let bin_entries = match &pkg.bin {
            Some(BinField::Path(path)) => {
                let bin_name = id.name.rsplit('/').next().unwrap_or(&id.name);
                vec![(bin_name.to_string(), path.clone())]
            }
            Some(BinField::Map(map)) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
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
            let rel_target = relative_target(&target, &link_path);
            symlink::create_symlink(&rel_target, &link_path)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = std::fs::metadata(&target) {
                    let mut perms = metadata.permissions();
                    let mode = perms.mode();
                    perms.set_mode(mode | ((mode & 0o444) >> 2));
                    let _ = std::fs::set_permissions(&target, perms);
                }
            }
        }
    }

    Ok(())
}
