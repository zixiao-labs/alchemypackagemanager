/// Remove a package from dependencies
pub async fn run(package: &str) -> anyhow::Result<()> {
    let project_dir = std::env::current_dir()?;
    let manifest_path = project_dir.join("package.json");

    if !manifest_path.exists() {
        anyhow::bail!("No package.json found.");
    }

    let content = std::fs::read_to_string(&manifest_path)?;
    let mut json: serde_json::Value = serde_json::from_str(&content)?;

    let mut found = false;

    for key in &["dependencies", "devDependencies", "optionalDependencies"] {
        if let Some(deps) = json.get_mut(key) {
            if let Some(obj) = deps.as_object_mut() {
                if obj.remove(package).is_some() {
                    found = true;
                    println!("Removed {} from {}", package, key);
                }
            }
        }
    }

    if !found {
        anyhow::bail!("Package '{}' not found in any dependency group", package);
    }

    let updated = serde_json::to_string_pretty(&json)?;
    std::fs::write(&manifest_path, updated + "\n")?;

    // Re-install to update node_modules
    super::install::run().await?;

    Ok(())
}
