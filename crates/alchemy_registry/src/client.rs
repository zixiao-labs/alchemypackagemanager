use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use reqwest::Client;
use tokio::sync::Semaphore;
use tracing::{debug, info};

use alchemy_core::config::AlchemyConfig;
use alchemy_core::dependency::{PackageId, PeerDepMeta};
use alchemy_core::error::{AlchemyError, AlchemyResult};
use alchemy_core::resolver::{MetadataFetcher, VersionInfo};

use crate::cache::MetadataCache;
use crate::metadata::PackageMetadata;
use crate::tarball;

const MAX_CONCURRENT_REQUESTS: usize = 32;

/// HTTP/2 npm registry client
pub struct RegistryClient {
    client: Client,
    semaphore: Arc<Semaphore>,
    registry_url: String,
    scoped_registries: BTreeMap<String, String>,
    auth_tokens: BTreeMap<String, String>,
    cache: Option<MetadataCache>,
}

impl RegistryClient {
    pub fn new(config: &AlchemyConfig) -> anyhow::Result<Self> {
        let mut builder = Client::builder().use_rustls_tls();

        // Configure proxy if specified
        if let Some(proxy_url) = &config.proxy {
            builder = builder.proxy(reqwest::Proxy::http(proxy_url)?);
        }
        if let Some(proxy_url) = &config.https_proxy {
            builder = builder.proxy(reqwest::Proxy::https(proxy_url)?);
        }

        let client = builder.build()?;

        Ok(Self {
            client,
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
            registry_url: config.default_registry.clone(),
            scoped_registries: config.scoped_registries.clone(),
            auth_tokens: config.auth_tokens.clone(),
            cache: None,
        })
    }

    /// Attach a metadata cache to this client.
    pub fn with_cache(mut self, cache: MetadataCache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Get the appropriate registry URL for a given package name.
    fn registry_for_package(&self, name: &str) -> &str {
        if let Some(scope) = name.strip_prefix('@') {
            if let Some(scope_name) = scope.split('/').next() {
                if let Some(url) = self.scoped_registries.get(scope_name) {
                    return url;
                }
            }
        }
        &self.registry_url
    }

    /// Get the auth token for a given registry URL, if any.
    fn auth_for_registry(&self, registry_url: &str) -> Option<&str> {
        if let Some(token) = self.auth_tokens.get(registry_url) {
            return Some(token);
        }
        let normalized = registry_url.trim_end_matches('/');
        if let Some(token) = self.auth_tokens.get(normalized) {
            return Some(token);
        }
        // Try matching with trailing slash
        let with_slash = format!("{}/", normalized);
        if let Some(token) = self.auth_tokens.get(&with_slash) {
            return Some(token);
        }
        None
    }

    /// Fetch abbreviated metadata for a package
    pub async fn fetch_package_metadata(&self, name: &str) -> AlchemyResult<PackageMetadata> {
        // Check disk cache first
        if let Some(ref cache) = self.cache {
            if let Some(cached) = cache.get(name) {
                return Ok(cached);
            }
        }

        let _permit = self.semaphore.acquire().await.unwrap();

        let registry = self.registry_for_package(name);

        // Encode scoped package names: @scope/name → @scope%2fname
        let encoded_name = if name.starts_with('@') {
            name.replacen('/', "%2f", 1)
        } else {
            name.to_string()
        };

        let url = format!("{}/{}", registry.trim_end_matches('/'), encoded_name);
        debug!("Fetching metadata: {}", url);

        let mut request = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.npm.install-v1+json");

        // Add auth header if available
        if let Some(token) = self.auth_for_registry(registry) {
            if token.starts_with("Basic ") {
                request = request.header("Authorization", token);
            } else {
                request = request.header("Authorization", format!("Bearer {}", token));
            }
        }

        let response = request
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

        // Write to disk cache
        if let Some(ref cache) = self.cache {
            cache.set(name, &metadata);
        }

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

        let mut request = self.client.get(tarball_url);

        // Add auth for tarball downloads too if the URL matches a known registry
        for (registry_url, token) in &self.auth_tokens {
            if tarball_url.starts_with(registry_url.as_str()) {
                if token.starts_with("Basic ") {
                    request = request.header("Authorization", token.as_str());
                } else {
                    request = request.header("Authorization", format!("Bearer {}", token));
                }
                break;
            }
        }

        let bytes = request.send().await?.bytes().await?;

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
            .map(|(version, meta)| VersionInfo {
                version,
                dependencies: meta.dependencies,
                peer_dependencies: meta.peer_dependencies,
                optional_dependencies: meta.optional_dependencies,
                peer_dependencies_meta: meta
                    .peer_dependencies_meta
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k,
                            PeerDepMeta {
                                optional: v.optional,
                            },
                        )
                    })
                    .collect(),
                tarball_url: meta.dist.tarball,
                integrity: meta.dist.integrity,
                bin: meta.bin,
                os: meta.os,
                cpu: meta.cpu,
                engines: meta.engines,
            })
            .collect();

        Ok(versions)
    }
}
