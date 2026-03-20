/// Initialize a new package.json
pub fn run() -> anyhow::Result<()> {
    let project_dir = std::env::current_dir()?;
    let manifest_path = project_dir.join("package.json");

    if manifest_path.exists() {
        anyhow::bail!("package.json already exists");
    }

    let dir_name = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("my-project");

    let package_json = serde_json::json!({
        "name": dir_name,
        "version": "1.0.0",
        "description": "",
        "main": "index.js",
        "scripts": {
            "test": "echo \"Error: no test specified\" && exit 1"
        },
        "keywords": [],
        "license": "ISC"
    });

    let content = serde_json::to_string_pretty(&package_json)?;
    std::fs::write(&manifest_path, content + "\n")?;

    println!("Created package.json");
    Ok(())
}
