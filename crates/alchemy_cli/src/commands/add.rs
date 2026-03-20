/// Add a package to dependencies and install
pub async fn run(package: &str, dev: bool) -> anyhow::Result<()> {
    let project_dir = std::env::current_dir()?;
    let manifest_path = project_dir.join("package.json");

    if !manifest_path.exists() {
        anyhow::bail!("No package.json found. Run `alchemy init` first.");
    }

    // Parse package spec: "pkg" or "pkg@version"
    let (name, version_req) = if let Some(at_pos) = package.rfind('@') {
        if at_pos == 0 {
            // Scoped package without version: @scope/pkg
            (package.to_string(), "latest".to_string())
        } else {
            (
                package[..at_pos].to_string(),
                package[at_pos + 1..].to_string(),
            )
        }
    } else {
        (package.to_string(), "latest".to_string())
    };

    // Resolve "latest" to actual version range
    let version_spec = if version_req == "latest" {
        let client = alchemy_registry::RegistryClient::new()?;
        let metadata = client.fetch_package_metadata(&name).await?;
        let latest = metadata
            .dist_tags
            .get("latest")
            .ok_or_else(|| anyhow::anyhow!("No 'latest' tag for {}", name))?;
        format!("^{}", latest)
    } else {
        version_req
    };

    // Update package.json
    let content = std::fs::read_to_string(&manifest_path)?;
    let mut json: serde_json::Value = serde_json::from_str(&content)?;

    let dep_key = if dev {
        "devDependencies"
    } else {
        "dependencies"
    };

    if json.get(dep_key).is_none() {
        json[dep_key] = serde_json::json!({});
    }
    json[dep_key][&name] = serde_json::Value::String(version_spec.clone());

    let updated = serde_json::to_string_pretty(&json)?;
    std::fs::write(&manifest_path, updated + "\n")?;

    println!("Added {}@{} to {}", name, version_spec, dep_key);

    // Run install
    super::install::run().await?;

    Ok(())
}
