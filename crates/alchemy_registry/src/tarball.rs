use std::path::Path;

use anyhow::Context;
use flate2::read::GzDecoder;
use tar::Archive;

/// Extract a .tgz tarball to the given directory.
/// npm tarballs contain a `package/` prefix which is stripped.
pub fn extract_tarball(tarball_bytes: &[u8], dest: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest)?;

    let gz = GzDecoder::new(tarball_bytes);
    let mut archive = Archive::new(gz);

    for entry in archive.entries().context("failed to read tarball entries")? {
        let mut entry = entry.context("failed to read tarball entry")?;
        let path = entry.path().context("failed to get entry path")?.into_owned();

        // Strip the leading `package/` directory that npm tarballs use
        let relative = path
            .components()
            .skip(1) // skip "package"
            .collect::<std::path::PathBuf>();

        if relative.as_os_str().is_empty() {
            continue;
        }

        let target = dest.join(&relative);

        // Create parent directories
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }

        entry.unpack(&target).with_context(|| {
            format!("failed to unpack {}", relative.display())
        })?;
    }

    Ok(())
}
