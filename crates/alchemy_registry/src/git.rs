use std::path::Path;

use tracing::debug;

use crate::tarball;

/// Clone a git repository to a destination directory.
/// Uses `git` CLI (same approach as npm/pnpm).
pub fn clone_repo(url: &str, commitish: Option<&str>, dest: &Path) -> anyhow::Result<()> {
    debug!("Cloning {} to {}", url, dest.display());

    let mut cmd = std::process::Command::new("git");
    cmd.arg("clone").arg("--depth").arg("1");

    if let Some(ref_spec) = commitish {
        cmd.arg("--branch").arg(ref_spec);
    }

    cmd.arg(url).arg(dest);

    let output = cmd.output()?;

    if !output.status.success() {
        // If shallow clone with branch fails (e.g., it's a commit hash),
        // try a full clone
        if commitish.is_some() {
            debug!("Shallow clone failed, trying full clone");
            let output = std::process::Command::new("git")
                .arg("clone")
                .arg(url)
                .arg(dest)
                .output()?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("git clone failed for {}: {}", url, stderr.trim());
            }

            // Checkout the specific commitish
            if let Some(ref_spec) = commitish {
                let output = std::process::Command::new("git")
                    .arg("checkout")
                    .arg(ref_spec)
                    .current_dir(dest)
                    .output()?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!(
                        "git checkout {} failed for {}: {}",
                        ref_spec,
                        url,
                        stderr.trim()
                    );
                }
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git clone failed for {}: {}", url, stderr.trim());
        }
    }

    Ok(())
}

/// Get the current HEAD SHA of a git repository
pub fn get_head_sha(repo_dir: &Path) -> anyhow::Result<String> {
    let output = std::process::Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(repo_dir)
        .output()?;

    if !output.status.success() {
        anyhow::bail!("git rev-parse HEAD failed");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Download a tarball URL and extract it
pub fn download_and_extract_url(url: &str, dest: &Path) -> anyhow::Result<()> {
    debug!("Downloading tarball from {}", url);

    // Use a blocking reqwest client for simplicity in this sync context
    let bytes = reqwest::blocking::get(url)?.bytes()?;
    tarball::extract_tarball(&bytes, dest)?;

    Ok(())
}
