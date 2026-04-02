use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

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

    /// Format as "name@version" used in .pnpm directory layout.
    /// Scoped packages use `+` instead of `/` to keep a flat directory name.
    pub fn pnpm_dir_name(&self) -> String {
        let safe_name = self.name.replace('/', "+");
        format!("{}@{}", safe_name, self.version)
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

/// Metadata about a peer dependency (from peerDependenciesMeta field)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PeerDepMeta {
    #[serde(default)]
    pub optional: bool,
}

/// Source of a resolved package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PackageSource {
    Registry {
        tarball_url: String,
        integrity: Option<String>,
    },
    Git {
        url: String,
        commitish: Option<String>,
    },
    Path {
        path: PathBuf,
    },
    Url {
        url: String,
    },
}

/// Resolved package with all metadata needed for installation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedPackage {
    pub id: PackageId,
    pub tarball_url: String,
    pub integrity: Option<String>,
    /// For `file:` dependencies, the absolute path to the local package directory.
    /// When set, the linker copies/hardlinks from this path instead of the content store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<PathBuf>,
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub peer_dependencies: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub optional_dependencies: BTreeMap<String, String>,
    pub bin: Option<crate::manifest::BinField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engines: Option<BTreeMap<String, String>>,
}
