use std::path::Path;

use serde::Deserialize;
use tracing::{debug, info, warn};

use alchemy_core::resolver::ResolutionResult;

/// Minimal package.json representation for reading lifecycle scripts.
#[derive(Debug, Deserialize)]
struct PackageJson {
    #[serde(default)]
    scripts: std::collections::HashMap<String, String>,
}

const LIFECYCLE_SCRIPTS: &[&str] = &["preinstall", "install", "postinstall"];

/// Run lifecycle scripts (preinstall, install, postinstall) for all resolved packages.
pub fn run_lifecycle_scripts(
    project_dir: &Path,
    resolution: &ResolutionResult,
) -> anyhow::Result<()> {
    let pnpm_dir = project_dir.join("node_modules").join(".pnpm");
    let bin_dir = project_dir.join("node_modules").join(".bin");

    let mut scripts_run = 0u32;

    for id in resolution.packages.keys() {
        let pkg_dir = pnpm_dir
            .join(id.pnpm_dir_name())
            .join("node_modules")
            .join(&id.name);

        let pkg_json_path = pkg_dir.join("package.json");
        if !pkg_json_path.exists() {
            continue;
        }

        let pkg_json: PackageJson = match std::fs::read_to_string(&pkg_json_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(p) => p,
            None => continue,
        };

        for script_name in LIFECYCLE_SCRIPTS {
            let script = match pkg_json.scripts.get(*script_name) {
                Some(s) => s,
                None => continue,
            };

            debug!("Running {} for {}", script_name, id);
            scripts_run += 1;

            // Build PATH with node_modules/.bin prepended
            let path_env = {
                let existing = std::env::var("PATH").unwrap_or_default();
                format!("{}:{}", bin_dir.display(), existing)
            };

            let result = std::process::Command::new("sh")
                .arg("-c")
                .arg(script)
                .current_dir(&pkg_dir)
                .env("PATH", &path_env)
                .output();

            match result {
                Ok(output) if output.status.success() => {
                    debug!("{} {} completed successfully", id, script_name);
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    warn!(
                        "{} {} exited with {}\nstdout: {}\nstderr: {}",
                        id,
                        script_name,
                        output.status,
                        stdout.trim(),
                        stderr.trim()
                    );
                }
                Err(e) => {
                    warn!("Failed to execute {} for {}: {}", script_name, id, e);
                }
            }
        }
    }

    if scripts_run > 0 {
        info!("Ran {} lifecycle scripts", scripts_run);
    }

    Ok(())
}
