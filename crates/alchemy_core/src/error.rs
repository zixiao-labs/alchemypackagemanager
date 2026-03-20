use thiserror::Error;

#[derive(Error, Debug)]
pub enum AlchemyError {
    #[error("Failed to parse package.json: {0}")]
    ManifestParse(String),

    #[error("Package not found: {0}")]
    PackageNotFound(String),

    #[error("Version not found: {0}@{1}")]
    VersionNotFound(String, String),

    #[error("Resolution failed: {0}")]
    ResolutionFailed(String),

    #[error("Integrity check failed for {0}: expected {1}, got {2}")]
    IntegrityError(String, String, String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("{0}")]
    Other(String),
}

pub type AlchemyResult<T> = Result<T, AlchemyError>;
