use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Represents a parsed package.json file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub name: Option<String>,
    pub version: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "devDependencies")]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "peerDependencies")]
    pub peer_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies")]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub bin: Option<BinField>,
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
    #[serde(default)]
    pub workspaces: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BinField {
    Path(String),
    Map(BTreeMap<String, String>),
}

impl Manifest {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn from_path(path: &std::path::Path) -> crate::error::AlchemyResult<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_json(&content).map_err(|e| {
            crate::error::AlchemyError::ManifestParse(format!("{}: {}", path.display(), e))
        })
    }

    /// All production dependencies (dependencies + optionalDependencies)
    pub fn prod_dependencies(&self) -> BTreeMap<String, String> {
        let mut deps = self.dependencies.clone();
        deps.extend(self.optional_dependencies.clone());
        deps
    }

    /// All dependencies including devDependencies
    pub fn all_dependencies(&self) -> BTreeMap<String, String> {
        let mut deps = self.prod_dependencies();
        deps.extend(self.dev_dependencies.clone());
        deps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_manifest() {
        let json = r#"{
            "name": "test-pkg",
            "version": "1.0.0",
            "dependencies": {
                "express": "^4.17.0"
            },
            "devDependencies": {
                "jest": "^29.0.0"
            }
        }"#;
        let m = Manifest::from_json(json).unwrap();
        assert_eq!(m.name.as_deref(), Some("test-pkg"));
        assert_eq!(m.dependencies.get("express").unwrap(), "^4.17.0");
        assert_eq!(m.dev_dependencies.get("jest").unwrap(), "^29.0.0");
    }
}
