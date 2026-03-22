use alchemy_core::config::AlchemyConfig;
use alchemy_core::manifest::Manifest;

/// Link a package for local development.
///
/// - No args: register current package globally
/// - With package name: create symlink in node_modules to globally linked package
pub fn run(package: Option<&str>) -> anyhow::Result<()> {
    let project_dir = std::env::current_dir()?;
    let config = AlchemyConfig::load_from_dir(&project_dir)?;
    let links_dir = config.store_dir.join("links");

    match package {
        None => register_link(&project_dir, &links_dir),
        Some(name) => consume_link(&project_dir, &links_dir, name),
    }
}

/// Remove a local link and re-install from registry.
pub fn unlink(package: &str) -> anyhow::Result<()> {
    let project_dir = std::env::current_dir()?;
    let node_modules = project_dir.join("node_modules");

    let link_path = node_modules.join(package);

    if link_path.is_symlink() {
        std::fs::remove_file(&link_path)?;
        println!("Removed link for {}", package);
        println!("Run `alchemy install` to restore from registry.");
    } else {
        anyhow::bail!("{} is not a linked package", package);
    }

    Ok(())
}

/// Register the current package globally: ~/.alchemy-store/links/{name} → CWD
fn register_link(project_dir: &std::path::Path, links_dir: &std::path::Path) -> anyhow::Result<()> {
    let manifest_path = project_dir.join("package.json");
    if !manifest_path.exists() {
        anyhow::bail!("No package.json found in {}", project_dir.display());
    }

    let manifest = Manifest::from_path(&manifest_path)?;
    let name = manifest
        .name
        .ok_or_else(|| anyhow::anyhow!("package.json has no 'name' field"))?;

    std::fs::create_dir_all(links_dir)?;

    // Handle scoped packages: @scope/name → links/@scope/name
    let link_path = links_dir.join(&name);
    if let Some(parent) = link_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Remove existing link if present
    if link_path.exists() || link_path.is_symlink() {
        std::fs::remove_file(&link_path)?;
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(project_dir, &link_path)?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(project_dir, &link_path)?;

    println!("Registered {} → {}", name, project_dir.display());
    println!(
        "Use `alchemy link {}` in another project to consume this link.",
        name
    );

    Ok(())
}

/// Consume a globally registered link: node_modules/{name} → linked path
fn consume_link(
    project_dir: &std::path::Path,
    links_dir: &std::path::Path,
    name: &str,
) -> anyhow::Result<()> {
    let link_source = links_dir.join(name);

    if !link_source.exists() {
        anyhow::bail!(
            "No globally linked package '{}' found. Run `alchemy link` in the {} directory first.",
            name,
            name
        );
    }

    // Read the actual path the global link points to
    let target = std::fs::read_link(&link_source)?;

    let node_modules = project_dir.join("node_modules");
    std::fs::create_dir_all(&node_modules)?;

    // Handle scoped packages
    let dest = node_modules.join(name);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Remove existing (symlink or directory)
    if dest.is_symlink() {
        std::fs::remove_file(&dest)?;
    } else if dest.is_dir() {
        std::fs::remove_dir_all(&dest)?;
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &dest)?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&target, &dest)?;

    println!("Linked {} → {}", name, target.display());

    Ok(())
}
