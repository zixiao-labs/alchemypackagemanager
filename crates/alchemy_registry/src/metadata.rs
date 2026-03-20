use serde::Deserialize;
use std::collections::BTreeMap;

/// Abbreviated registry metadata (npm install-v1 format)
#[derive(Debug, Deserialize)]
pub struct PackageMetadata {
    pub name: String,
    pub versions: BTreeMap<String, VersionMetadata>,
    #[serde(default, rename = "dist-tags")]
    pub dist_tags: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct VersionMetadata {
    pub version: String,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "devDependencies")]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "peerDependencies")]
    pub peer_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies")]
    pub optional_dependencies: BTreeMap<String, String>,
    pub dist: DistInfo,
    #[serde(default)]
    pub bin: Option<alchemy_core::manifest::BinField>,
}

#[derive(Debug, Deserialize)]
pub struct DistInfo {
    pub tarball: String,
    #[serde(default)]
    pub integrity: Option<String>,
    #[serde(default)]
    pub shasum: Option<String>,
}
