use alchemy_core::manifest::Manifest;

/// Run a script defined in package.json, returning the exit code.
pub fn run(script: &str, args: &[String]) -> anyhow::Result<i32> {
    let project_dir = std::env::current_dir()?;
    let manifest_path = project_dir.join("package.json");

    if !manifest_path.exists() {
        anyhow::bail!("No package.json found in {}", project_dir.display());
    }

    let manifest = Manifest::from_path(&manifest_path)?;

    // Check if script exists in package.json scripts
    let script_cmd = manifest.scripts.get(script);

    if script_cmd.is_none() {
        // Check if there's a binary in node_modules/.bin
        let bin_path = project_dir.join("node_modules").join(".bin").join(script);
        if bin_path.exists() || bin_path.with_extension("cmd").exists() {
            return run_bin(&project_dir, &bin_path, args);
        }

        // List available scripts
        eprintln!("Script \"{}\" not found in package.json.", script);
        if !manifest.scripts.is_empty() {
            eprintln!("\nAvailable scripts:");
            for name in manifest.scripts.keys() {
                eprintln!("  - {}", name);
            }
        }
        return Ok(1);
    }

    let script_cmd = script_cmd.unwrap();

    // Run pre<script> if it exists
    if let Some(pre_cmd) = manifest.scripts.get(&format!("pre{}", script)) {
        let code = execute_script(&project_dir, pre_cmd, &[])?;
        if code != 0 {
            return Ok(code);
        }
    }

    // Run the script itself
    let code = execute_script(&project_dir, script_cmd, args)?;
    if code != 0 {
        return Ok(code);
    }

    // Run post<script> if it exists
    if let Some(post_cmd) = manifest.scripts.get(&format!("post{}", script)) {
        let code = execute_script(&project_dir, post_cmd, &[])?;
        if code != 0 {
            return Ok(code);
        }
    }

    Ok(0)
}

fn execute_script(
    project_dir: &std::path::Path,
    script: &str,
    args: &[String],
) -> anyhow::Result<i32> {
    let bin_dir = project_dir.join("node_modules").join(".bin");

    // Build PATH with node_modules/.bin prepended
    let path_env = {
        let existing = std::env::var("PATH").unwrap_or_default();
        if bin_dir.exists() {
            format!("{}:{}", bin_dir.display(), existing)
        } else {
            existing
        }
    };

    // Build the full command with args appended
    let full_cmd = if args.is_empty() {
        script.to_string()
    } else {
        format!("{} {}", script, args.join(" "))
    };

    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(&full_cmd)
        .current_dir(project_dir)
        .env("PATH", &path_env)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()?;

    Ok(status.code().unwrap_or(1))
}

fn run_bin(
    project_dir: &std::path::Path,
    bin_path: &std::path::Path,
    args: &[String],
) -> anyhow::Result<i32> {
    let bin_dir = project_dir.join("node_modules").join(".bin");
    let path_env = {
        let existing = std::env::var("PATH").unwrap_or_default();
        format!("{}:{}", bin_dir.display(), existing)
    };

    let status = std::process::Command::new(bin_path)
        .args(args)
        .current_dir(project_dir)
        .env("PATH", &path_env)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()?;

    Ok(status.code().unwrap_or(1))
}
