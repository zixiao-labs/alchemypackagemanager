use std::path::Path;
use std::sync::Arc;

use reqwest::Client;
use tokio::sync::Semaphore;
use tracing::{debug, info};

use alchemy_core::dependency::PackageId;
use alchemy_core::error::{AlchemyError, AlchemyResult};
use alchemy_core::resolver::{MetadataFetcher, VersionInfo};

use crate::metadata::PackageMetadata;
use crate::tarball;

const NPM_REGISTRY: &str = "https://registry.npmjs.org";
const MAX_CONCURRENT_REQUESTS: usize = 32;

/// HTTP/2 npm registry client
pub struct RegistryClient {
    client: Client,
    semaphore: Arc<Semaphore>,
    registry_url: String,
}

impl RegistryClient {
    pub fn new() -> anyhow::Result<Self> {
        let client = Client::builder().use_rustls_tls().build()?;

        Ok(Self {
            client,
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
            registry_url: NPM_REGISTRY.to_string(),
        })
    }

    /// Fetch abbreviated metadata for a package
    pub async fn fetch_package_metadata(&self, name: &str) -> AlchemyResult<PackageMetadata> {
        let _permit = self.semaphore.acquire().await.unwrap();

        // Encode scoped package names: @scope/name → @scope%2fname
        let encoded_name = if name.starts_with('@') {
            name.replacen('/', "%2f", 1)
        } else {
            name.to_string()
        };

        let url = format!("{}/{}", self.registry_url, encoded_name);
        debug!("Fetching metadata: {}", url);

        let response = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.npm.install-v1+json")
            .send()
            .await
            .map_err(|e| AlchemyError::Network(format!("{}: {}", name, e)))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AlchemyError::PackageNotFound(name.to_string()));
        }

        if !response.status().is_success() {
            return Err(AlchemyError::Network(format!(
                "{}: HTTP {}",
                name,
                response.status()
            )));
        }

        let metadata: PackageMetadata = response
            .json()
            .await
            .map_err(|e| AlchemyError::Network(format!("{}: {}", name, e)))?;

        Ok(metadata)
    }

    /// Download a tarball and extract it to the given directory
    pub async fn download_and_extract(
        &self,
        id: &PackageId,
        tarball_url: &str,
        dest: &Path,
    ) -> anyhow::Result<()> {
        let _permit = self.semaphore.acquire().await.unwrap();

        info!("Downloading {}...", id);

        let bytes = self.client.get(tarball_url).send().await?.bytes().await?;

        tarball::extract_tarball(&bytes, dest)?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl MetadataFetcher for RegistryClient {
    async fn fetch_versions(&self, name: &str) -> AlchemyResult<Vec<VersionInfo>> {
        let metadata = self.fetch_package_metadata(name).await?;

        let versions: Vec<VersionInfo> = metadata
            .versions
            .into_iter()
            .map(|(version, meta)| {
                // Merge optional deps into regular deps
                let mut deps = meta.dependencies;
                for (k, v) in meta.optional_dependencies {
                    deps.entry(k).or_insert(v);
                }

                VersionInfo {
                    version,
                    dependencies: deps,
                    tarball_url: meta.dist.tarball,
                    integrity: meta.dist.integrity,
                    bin: meta.bin,
                }
            })
            .collect();

        Ok(versions)
    }
}
