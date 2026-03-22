use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Abbreviated registry metadata (npm install-v1 format)
#[derive(Debug, Deserialize, Serialize)]
pub struct PackageMetadata {
    pub name: String,
    pub versions: BTreeMap<String, VersionMetadata>,
    #[serde(default, rename = "dist-tags")]
    pub dist_tags: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize)]
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
    #[serde(default, rename = "peerDependenciesMeta")]
    pub peer_dependencies_meta: BTreeMap<String, PeerDepMetaRaw>,
    pub dist: DistInfo,
    #[serde(default)]
    pub bin: Option<alchemy_core::manifest::BinField>,
    #[serde(default)]
    pub os: Option<Vec<String>>,
    #[serde(default)]
    pub cpu: Option<Vec<String>>,
    #[serde(default)]
    pub engines: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PeerDepMetaRaw {
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DistInfo {
    pub tarball: String,
    #[serde(default)]
    pub integrity: Option<String>,
    #[serde(default)]
    pub shasum: Option<String>,
}
