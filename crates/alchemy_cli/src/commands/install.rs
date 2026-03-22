use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};

use alchemy_core::config::AlchemyConfig;
use alchemy_core::dependency::PackageId;
use alchemy_core::lockfile::Lockfile;
use alchemy_core::manifest::Manifest;
use alchemy_core::resolver::Resolver;
use alchemy_core::workspace::Workspace;
use alchemy_registry::RegistryClient;
use alchemy_store::ContentStore;

/// Run the install command: resolve → download → link
pub async fn run() -> anyhow::Result<()> {
    let start = Instant::now();
    let project_dir = std::env::current_dir()?;

    // 1. Read package.json
    let manifest_path = project_dir.join("package.json");
    if !manifest_path.exists() {
        anyhow::bail!("No package.json found in {}", project_dir.display());
    }
    let manifest = Manifest::from_path(&manifest_path)?;

    // Check for workspace
    if manifest.workspaces.is_some() {
        if let Some(workspace) = Workspace::discover(&project_dir)? {
            return run_workspace(workspace).await;
        }
    }

    let all_deps = manifest.all_dependencies();

    if all_deps.is_empty() {
        println!("No dependencies to install.");
        return Ok(());
    }

    println!("Installing {} dependencies...", all_deps.len());

    // Load config
    let config = AlchemyConfig::load_from_dir(&project_dir)?;

    // 2. Resolve dependencies
    let cache = alchemy_registry::cache::MetadataCache::new(
        &config.store_dir,
        std::time::Duration::from_secs(config.metadata_cache_ttl),
    );
    let client = RegistryClient::new(&config)?.with_cache(cache);
    let resolver = Resolver::new(client).with_auto_install_peers(config.auto_install_peers);

    let progress = ProgressBar::new_spinner();
    progress.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    progress.set_message("Resolving dependencies...");
    progress.enable_steady_tick(std::time::Duration::from_millis(80));

    let resolution = resolver.resolve(&all_deps).await?;

    progress.finish_with_message(format!("Resolved {} packages", resolution.packages.len()));

    // Print peer dependency warnings
    for warning in &resolution.peer_warnings {
        match &warning.found {
            Some(found) => {
                eprintln!(
                    "WARN: {} requires peer {} {}, but {} is installed",
                    warning.package, warning.peer_name, warning.required, found
                );
            }
            None => {
                eprintln!(
                    "WARN: {} requires peer {} {}, which is not installed",
                    warning.package, warning.peer_name, warning.required
                );
            }
        }
    }

    // 3. Download missing packages to content store
    let store = ContentStore::new();
    let registry = RegistryClient::new(&config)?;

    let to_download: Vec<&PackageId> = resolution
        .packages
        .keys()
        .filter(|id| !store.has_package(&id.name, &id.version))
        .collect();

    if !to_download.is_empty() {
        let download_bar = ProgressBar::new(to_download.len() as u64);
        download_bar.set_style(
            ProgressStyle::with_template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("█▓░"),
        );

        for id in &to_download {
            let pkg = &resolution.packages[*id];
            download_bar.set_message(format!("{}", id));

            // Download and extract to a temp dir first
            let temp_dir = tempfile::tempdir()?;
            registry
                .download_and_extract(id, &pkg.tarball_url, temp_dir.path())
                .await?;

            // Move to content store
            store.store_package(&id.name, &id.version, temp_dir.path())?;

            download_bar.inc(1);
        }

        download_bar.finish_with_message("Downloads complete");
    } else {
        println!("All packages already in store.");
    }

    // 4. Link node_modules
    let link_progress = ProgressBar::new_spinner();
    link_progress.set_message("Linking packages...");
    link_progress.enable_steady_tick(std::time::Duration::from_millis(80));

    alchemy_linker::link_packages(&project_dir, &resolution, store.base_dir())?;

    link_progress.finish_with_message("Linked");

    // 5. Run lifecycle scripts
    if !config.ignore_scripts {
        let script_progress = ProgressBar::new_spinner();
        script_progress.set_style(
            ProgressStyle::with_template("{spinner:.green} {msg}")
                .unwrap()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );
        script_progress.set_message("Running lifecycle scripts...");
        script_progress.enable_steady_tick(std::time::Duration::from_millis(80));

        alchemy_linker::run_lifecycle_scripts(&project_dir, &resolution)?;

        script_progress.finish_with_message("Lifecycle scripts complete");
    }

    // 6. Write lockfile
    let lockfile = Lockfile::from_resolution(
        &resolution,
        &manifest.dependencies,
        &manifest.dev_dependencies,
    );
    lockfile.write_to_file(&project_dir.join("alchemy-lock.yaml"))?;

    let elapsed = start.elapsed();
    println!(
        "\nDone in {:.2}s — {} packages installed",
        elapsed.as_secs_f64(),
        resolution.packages.len()
    );

    Ok(())
}

/// Workspace-aware install: resolve all members together, shared .pnpm store.
async fn run_workspace(workspace: Workspace) -> anyhow::Result<()> {
    let start = Instant::now();
    let config = AlchemyConfig::load_from_dir(&workspace.root_dir)?;

    println!(
        "Workspace detected with {} members",
        workspace.members.len()
    );
    for member in &workspace.members {
        println!(
            "  - {} ({})",
            member.manifest.name.as_deref().unwrap_or("<unnamed>"),
            member.relative_path
        );
    }

    // Collect all external deps across all importers
    let importers = workspace.collect_all_dependencies();
    let total_deps: usize = importers.values().map(|d| d.len()).sum();
    println!("Resolving {} total dependencies...", total_deps);

    // Resolve
    let cache = alchemy_registry::cache::MetadataCache::new(
        &config.store_dir,
        std::time::Duration::from_secs(config.metadata_cache_ttl),
    );
    let client = RegistryClient::new(&config)?.with_cache(cache);
    let resolver = Resolver::new(client).with_auto_install_peers(config.auto_install_peers);

    let progress = ProgressBar::new_spinner();
    progress.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    progress.set_message("Resolving workspace dependencies...");
    progress.enable_steady_tick(std::time::Duration::from_millis(80));

    let resolution = resolver.resolve_workspace(&importers).await?;

    progress.finish_with_message(format!("Resolved {} packages", resolution.packages.len()));

    // Print peer warnings
    for warning in &resolution.peer_warnings {
        match &warning.found {
            Some(found) => {
                eprintln!(
                    "WARN: {} requires peer {} {}, but {} is installed",
                    warning.package, warning.peer_name, warning.required, found
                );
            }
            None => {
                eprintln!(
                    "WARN: {} requires peer {} {}, which is not installed",
                    warning.package, warning.peer_name, warning.required
                );
            }
        }
    }

    // Download
    let store = ContentStore::new();
    let registry = RegistryClient::new(&config)?;

    let to_download: Vec<&PackageId> = resolution
        .packages
        .keys()
        .filter(|id| !store.has_package(&id.name, &id.version))
        .collect();

    if !to_download.is_empty() {
        let download_bar = ProgressBar::new(to_download.len() as u64);
        download_bar.set_style(
            ProgressStyle::with_template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("█▓░"),
        );

        for id in &to_download {
            let pkg = &resolution.packages[*id];
            download_bar.set_message(format!("{}", id));

            let temp_dir = tempfile::tempdir()?;
            registry
                .download_and_extract(id, &pkg.tarball_url, temp_dir.path())
                .await?;
            store.store_package(&id.name, &id.version, temp_dir.path())?;
            download_bar.inc(1);
        }

        download_bar.finish_with_message("Downloads complete");
    }

    // Link
    let link_progress = ProgressBar::new_spinner();
    link_progress.set_message("Linking workspace packages...");
    link_progress.enable_steady_tick(std::time::Duration::from_millis(80));

    alchemy_linker::link_workspace_packages(&workspace, &resolution, store.base_dir())?;

    link_progress.finish_with_message("Linked");

    let elapsed = start.elapsed();
    println!(
        "\nDone in {:.2}s — {} packages installed across {} workspace members",
        elapsed.as_secs_f64(),
        resolution.packages.len(),
        workspace.members.len()
    );

    Ok(())
}
