use alchemy_core::import;

/// Import a lockfile from another package manager.
pub fn run(source: &str) -> anyhow::Result<()> {
    let project_dir = std::env::current_dir()?;

    let lockfile = match source {
        "npm" => {
            let lock_path = project_dir.join("package-lock.json");
            if !lock_path.exists() {
                anyhow::bail!("No package-lock.json found in {}", project_dir.display());
            }
            println!("Importing from package-lock.json...");
            import::import_npm_lockfile(&lock_path)?
        }
        other => {
            anyhow::bail!("Unsupported lockfile format: '{}'. Supported: npm", other);
        }
    };

    let output_path = project_dir.join("alchemy-lock.yaml");
    lockfile.write_to_file(&output_path)?;

    println!(
        "Imported {} packages to alchemy-lock.yaml",
        lockfile.packages.len()
    );

    Ok(())
}
