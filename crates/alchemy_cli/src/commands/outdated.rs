use alchemy_core::config::AlchemyConfig;
use alchemy_core::lockfile::Lockfile;
use alchemy_registry::RegistryClient;

struct OutdatedInfo {
    name: String,
    current: String,
    wanted: String,
    latest: String,
    dep_type: &'static str,
}

/// Show packages that have newer versions available.
pub async fn run() -> anyhow::Result<()> {
    let project_dir = std::env::current_dir()?;

    let lockfile_path = project_dir.join("alchemy-lock.yaml");
    if !lockfile_path.exists() {
        anyhow::bail!("No alchemy-lock.yaml found. Run `alchemy install` first.");
    }
    let lockfile = Lockfile::read_from_file(&lockfile_path)?;

    let config = AlchemyConfig::load_from_dir(&project_dir)?;
    // Use zero TTL to always fetch fresh metadata
    let cache =
        alchemy_registry::cache::MetadataCache::new(&config.store_dir, std::time::Duration::ZERO);
    let client = RegistryClient::new(&config)?.with_cache(cache);

    let root = lockfile
        .importers
        .get(".")
        .ok_or_else(|| anyhow::anyhow!("No root importer in lockfile"))?;

    // Collect all deps with their specifiers and current versions
    let mut to_check: Vec<(&str, &str, &str, &'static str)> = Vec::new(); // (name, specifier, current, type)
    for (name, dep_ref) in &root.dependencies {
        to_check.push((name, &dep_ref.specifier, &dep_ref.version, "dependencies"));
    }
    for (name, dep_ref) in &root.dev_dependencies {
        to_check.push((
            name,
            &dep_ref.specifier,
            &dep_ref.version,
            "devDependencies",
        ));
    }

    let mut outdated: Vec<OutdatedInfo> = Vec::new();

    for (name, specifier, current, dep_type) in &to_check {
        let metadata = match client.fetch_package_metadata(name).await {
            Ok(m) => m,
            Err(_) => continue,
        };

        let latest = metadata
            .dist_tags
            .get("latest")
            .cloned()
            .unwrap_or_default();

        // Find wanted: highest version matching the specifier
        let version_strings: Vec<String> = metadata.versions.keys().cloned().collect();
        let wanted =
            find_wanted(&version_strings, specifier).unwrap_or_else(|| current.to_string());

        if wanted != *current || latest != *current {
            outdated.push(OutdatedInfo {
                name: name.to_string(),
                current: current.to_string(),
                wanted,
                latest,
                dep_type,
            });
        }
    }

    if outdated.is_empty() {
        println!("All packages are up to date.");
        return Ok(());
    }

    // Print table
    let name_w = outdated
        .iter()
        .map(|o| o.name.len())
        .max()
        .unwrap_or(7)
        .max(7);
    let curr_w = outdated
        .iter()
        .map(|o| o.current.len())
        .max()
        .unwrap_or(7)
        .max(7);
    let want_w = outdated
        .iter()
        .map(|o| o.wanted.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let lat_w = outdated
        .iter()
        .map(|o| o.latest.len())
        .max()
        .unwrap_or(6)
        .max(6);

    println!(
        "{:<name_w$}  {:<curr_w$}  {:<want_w$}  {:<lat_w$}  Type",
        "Package", "Current", "Wanted", "Latest",
    );
    println!(
        "{:-<name_w$}  {:-<curr_w$}  {:-<want_w$}  {:-<lat_w$}  ----",
        "", "", "", "",
    );

    for o in &outdated {
        println!(
            "{:<name_w$}  {:<curr_w$}  {:<want_w$}  {:<lat_w$}  {}",
            o.name, o.current, o.wanted, o.latest, o.dep_type,
        );
    }

    Ok(())
}

/// Find the highest version from a list that satisfies a semver specifier.
fn find_wanted(versions: &[String], specifier: &str) -> Option<String> {
    let req = match node_semver::Range::parse(specifier) {
        Ok(r) => r,
        Err(_) => return None,
    };

    let mut best: Option<node_semver::Version> = None;
    for ver_str in versions {
        if let Ok(ver) = node_semver::Version::parse(ver_str) {
            if req.satisfies(&ver) && best.as_ref().is_none_or(|b| ver > *b) {
                best = Some(ver);
            }
        }
    }

    best.map(|v| v.to_string())
}
