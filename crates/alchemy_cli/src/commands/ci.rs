use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};

use alchemy_core::config::AlchemyConfig;
use alchemy_core::lockfile::Lockfile;
use alchemy_core::manifest::Manifest;
use alchemy_registry::RegistryClient;
use alchemy_store::ContentStore;

const MAX_PARALLEL_DOWNLOADS: usize = 16;

/// Clean install: install exactly from lockfile, fail if lockfile is missing or stale.
pub async fn run() -> anyhow::Result<()> {
    let start = Instant::now();
    let project_dir = std::env::current_dir()?;

    // 1. Read package.json
    let manifest_path = project_dir.join("package.json");
    if !manifest_path.exists() {
        anyhow::bail!("No package.json found in {}", project_dir.display());
    }
    let manifest = Manifest::from_path(&manifest_path)?;

    // 2. Read lockfile (must exist)
    let lockfile_path = project_dir.join("alchemy-lock.yaml");
    if !lockfile_path.exists() {
        anyhow::bail!(
            "alchemy-lock.yaml not found. Run `alchemy install` first, then use `alchemy ci`."
        );
    }
    let lockfile = Lockfile::read_from_file(&lockfile_path)?;

    // 3. Validate lockfile against package.json
    lockfile.validate_against_manifest(&manifest.dependencies, &manifest.dev_dependencies)?;

    println!("Clean installing from lockfile...");

    // 4. Delete node_modules
    let node_modules = project_dir.join("node_modules");
    if node_modules.exists() {
        std::fs::remove_dir_all(&node_modules)?;
    }

    // 5. Reconstruct resolution from lockfile (no resolver needed)
    let resolution = lockfile.to_resolution_result()?;

    println!("Installing {} packages...", resolution.packages.len());

    // 6. Download missing packages to content store (parallel)
    let config = AlchemyConfig::load_from_dir(&project_dir)?;
    let store = Arc::new(ContentStore::new()?);
    let registry = Arc::new(RegistryClient::new(&config)?);

    let to_download: Vec<_> = resolution
        .packages
        .iter()
        .filter(|(id, pkg)| pkg.source_path.is_none() && !store.has_package(&id.name, &id.version))
        .collect();

    if !to_download.is_empty() {
        let download_bar = ProgressBar::new(to_download.len() as u64);
        download_bar.set_style(
            ProgressStyle::with_template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("█▓░"),
        );

        let results: Vec<anyhow::Result<()>> = futures::stream::iter(to_download)
            .map(|(id, pkg)| {
                let registry = Arc::clone(&registry);
                let store = Arc::clone(&store);
                let id = id.clone();
                let tarball_url = pkg.tarball_url.clone();
                let integrity = pkg.integrity.clone();
                async move {
                    let temp_dir = tempfile::tempdir()?;
                    registry
                        .download_and_extract(
                            &id,
                            &tarball_url,
                            integrity.as_deref(),
                            temp_dir.path(),
                        )
                        .await?;
                    store.store_package(&id.name, &id.version, temp_dir.path())?;
                    anyhow::Ok(())
                }
            })
            .buffer_unordered(MAX_PARALLEL_DOWNLOADS)
            .inspect(|_| download_bar.inc(1))
            .collect()
            .await;

        download_bar.finish_with_message("Downloads complete");

        for r in results {
            r?;
        }
    }

    // 7. Link node_modules
    let link_progress = ProgressBar::new_spinner();
    link_progress.set_message("Linking packages...");
    link_progress.enable_steady_tick(std::time::Duration::from_millis(80));

    alchemy_linker::link_packages(&project_dir, &resolution, store.base_dir())?;
    link_progress.finish_with_message("Linked");

    // 8. Run lifecycle scripts
    if !config.ignore_scripts {
        let script_progress = ProgressBar::new_spinner();
        script_progress.set_style(
            ProgressStyle::with_template("{spinner:.green} {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_spinner())
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );
        script_progress.set_message("Running lifecycle scripts...");
        script_progress.enable_steady_tick(std::time::Duration::from_millis(80));

        alchemy_linker::run_lifecycle_scripts(&project_dir, &resolution)?;
        script_progress.finish_with_message("Lifecycle scripts complete");
    }

    let elapsed = start.elapsed();
    println!(
        "\nDone in {:.2}s — {} packages installed (from lockfile)",
        elapsed.as_secs_f64(),
        resolution.packages.len()
    );

    Ok(())
}
