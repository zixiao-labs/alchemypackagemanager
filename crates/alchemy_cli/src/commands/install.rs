use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};

use alchemy_core::dependency::PackageId;
use alchemy_core::lockfile::Lockfile;
use alchemy_core::manifest::Manifest;
use alchemy_core::resolver::Resolver;
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
    let all_deps = manifest.all_dependencies();

    if all_deps.is_empty() {
        println!("No dependencies to install.");
        return Ok(());
    }

    println!("Installing {} dependencies...", all_deps.len());

    // 2. Resolve dependencies
    let client = RegistryClient::new()?;
    let resolver = Resolver::new(client);

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

    // 3. Download missing packages to content store
    let store = ContentStore::new();
    let registry = RegistryClient::new()?;

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

    // 5. Write lockfile
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
