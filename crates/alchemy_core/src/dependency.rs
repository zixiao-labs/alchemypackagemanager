use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Unique identifier for a resolved package
#[derive(Debug, Clone, Hash, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct PackageId {
    pub name: String,
    pub version: String,
}

impl PackageId {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }

    /// Format as "name@version" used in .pnpm directory layout
    pub fn pnpm_dir_name(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DepKind {
    Normal,
    Dev,
    Peer,
    Optional,
}

/// A dependency entry from package.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub version_req: String,
    pub kind: DepKind,
}

/// Resolved package with all metadata needed for installation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedPackage {
    pub id: PackageId,
    pub tarball_url: String,
    pub integrity: Option<String>,
    pub dependencies: BTreeMap<String, String>,
    pub bin: Option<crate::manifest::BinField>,
}
