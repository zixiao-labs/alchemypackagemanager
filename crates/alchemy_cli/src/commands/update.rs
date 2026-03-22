use alchemy_core::config::AlchemyConfig;
use alchemy_registry::cache::MetadataCache;

/// Update packages to their latest versions within the semver range.
/// If specific package names are provided, only update those.
/// Otherwise, update all packages.
pub async fn run(packages: &[String]) -> anyhow::Result<()> {
    let project_dir = std::env::current_dir()?;
    let config = AlchemyConfig::load_from_dir(&project_dir)?;

    // Invalidate metadata cache for targeted packages (or all)
    let cache = MetadataCache::new(
        &config.store_dir,
        std::time::Duration::from_secs(config.metadata_cache_ttl),
    );

    if packages.is_empty() {
        println!("Clearing metadata cache and re-resolving all packages...");
        cache.clear();
    } else {
        for name in packages {
            println!("Invalidating cache for {}...", name);
            cache.invalidate(name);
        }
    }

    // Re-run full install (which will re-resolve with fresh metadata)
    super::install::run().await?;

    Ok(())
}
