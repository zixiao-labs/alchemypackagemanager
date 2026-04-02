use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use alchemy_core::config::AlchemyConfig;
use alchemy_core::dependency::{PackageId, PeerDepMeta, ResolvedPackage};
use alchemy_core::error::{AlchemyError, AlchemyResult};
use alchemy_core::integrity::verify_integrity;
use alchemy_core::resolver::{MetadataFetcher, NonRegistryFetcher, VersionInfo};

use crate::cache::MetadataCache;
use crate::metadata::PackageMetadata;
use crate::tarball;

const MAX_CONCURRENT_REQUESTS: usize = 32;
/// Timeout for individual HTTP requests (connect + response body).
const REQUEST_TIMEOUT_SECS: u64 = 60;
/// Maximum retry attempts for transient network errors.
const MAX_RETRIES: u32 = 3;

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
        let mut builder = Client::builder()
            .use_rustls_tls()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS));

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
        let with_slash = format!("{}/", normalized);
        if let Some(token) = self.auth_tokens.get(&with_slash) {
            return Some(token);
        }
        None
    }

    /// Fetch abbreviated metadata for a package, with retry on transient failures.
    pub async fn fetch_package_metadata(&self, name: &str) -> AlchemyResult<PackageMetadata> {
        // Check disk cache first
        if let Some(ref cache) = self.cache {
            if let Some(cached) = cache.get(name) {
                return Ok(cached);
            }
        }

        let _permit = self
            .semaphore
            .acquire()
            .await
            .expect("semaphore should never be closed");

        let registry = self.registry_for_package(name);

        // Encode scoped package names: @scope/name → @scope%2fname
        let encoded_name = if name.starts_with('@') {
            name.replacen('/', "%2f", 1)
        } else {
            name.to_string()
        };

        let url = format!("{}/{}", registry.trim_end_matches('/'), encoded_name);
        debug!("Fetching metadata: {}", url);

        let mut delay = Duration::from_millis(500);
        let mut last_err = AlchemyError::Network(String::new());

        for attempt in 0..MAX_RETRIES {
            let mut request = self
                .client
                .get(&url)
                .header("Accept", "application/vnd.npm.install-v1+json");

            if let Some(token) = self.auth_for_registry(registry) {
                if token.starts_with("Basic ") {
                    request = request.header("Authorization", token);
                } else {
                    request = request.header("Authorization", format!("Bearer {}", token));
                }
            }

            match request.send().await {
                Err(e) => {
                    last_err = AlchemyError::Network(format!("{name}: {e}"));
                    if attempt + 1 < MAX_RETRIES {
                        warn!(
                            "Fetch {name} failed (attempt {}), retrying in {:?}: {e}",
                            attempt + 1,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                        continue;
                    }
                }
                Ok(response) => {
                    if response.status() == reqwest::StatusCode::NOT_FOUND {
                        return Err(AlchemyError::PackageNotFound(name.to_string()));
                    }

                    if response.status().is_server_error() && attempt + 1 < MAX_RETRIES {
                        last_err =
                            AlchemyError::Network(format!("{name}: HTTP {}", response.status()));
                        warn!(
                            "Fetch {name} got {} (attempt {}), retrying in {:?}",
                            response.status(),
                            attempt + 1,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                        continue;
                    }

                    if !response.status().is_success() {
                        return Err(AlchemyError::Network(format!(
                            "{name}: HTTP {}",
                            response.status()
                        )));
                    }

                    let metadata: PackageMetadata = response
                        .json()
                        .await
                        .map_err(|e| AlchemyError::Network(format!("{name}: {e}")))?;

                    if let Some(ref cache) = self.cache {
                        cache.set(name, &metadata);
                    }

                    return Ok(metadata);
                }
            }
        }

        Err(last_err)
    }

    /// Download a tarball, verify its integrity, and extract it to the given directory.
    ///
    /// If `expected_integrity` is provided, the download is verified against the SRI hash
    /// before extraction. Mismatches return `AlchemyError::IntegrityError`.
    pub async fn download_and_extract(
        &self,
        id: &PackageId,
        tarball_url: &str,
        expected_integrity: Option<&str>,
        dest: &Path,
    ) -> anyhow::Result<()> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .expect("semaphore should never be closed");

        info!("Downloading {}...", id);

        let mut delay = Duration::from_millis(500);

        for attempt in 0..MAX_RETRIES {
            let mut request = self.client.get(tarball_url);

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

            match request.send().await {
                Err(e) => {
                    if attempt + 1 < MAX_RETRIES {
                        warn!(
                            "Download {} failed (attempt {}), retrying in {:?}: {e}",
                            id,
                            attempt + 1,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                        continue;
                    }
                    return Err(e.into());
                }
                Ok(response) => {
                    if response.status().is_server_error() && attempt + 1 < MAX_RETRIES {
                        warn!(
                            "Download {} got {} (attempt {}), retrying in {:?}",
                            id,
                            response.status(),
                            attempt + 1,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                        continue;
                    }

                    if !response.status().is_success() {
                        anyhow::bail!("download {id}: HTTP {}", response.status());
                    }

                    let bytes = response.bytes().await?;

                    // Verify integrity before extracting
                    if let Some(expected) = expected_integrity {
                        if !verify_integrity(&bytes, expected) {
                            let actual = alchemy_core::integrity::compute_integrity(&bytes);
                            anyhow::bail!(
                                "integrity check failed for {id}: expected {expected}, got {actual}"
                            );
                        }
                    }

                    tarball::extract_tarball(&bytes, dest)?;
                    return Ok(());
                }
            }
        }

        anyhow::bail!("download {id}: all retry attempts failed")
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

#[async_trait::async_trait]
impl NonRegistryFetcher for RegistryClient {
    /// Clone a git repository and derive a ResolvedPackage from its package.json.
    async fn fetch_git(
        &self,
        name: &str,
        url: &str,
        commitish: Option<&str>,
    ) -> AlchemyResult<ResolvedPackage> {
        let url = url.to_string();
        let commitish = commitish.map(|s| s.to_string());
        let temp_dir = tempfile::tempdir().map_err(AlchemyError::Io)?;
        let dest = temp_dir.path().to_owned();

        // Run blocking git operations on a thread pool
        tokio::task::spawn_blocking(move || {
            crate::git::clone_repo(&url, commitish.as_deref(), &dest)
        })
        .await
        .map_err(|e| AlchemyError::Other(e.to_string()))?
        .map_err(|e| AlchemyError::Other(e.to_string()))?;

        let sha = crate::git::get_head_sha(temp_dir.path())
            .map_err(|e| AlchemyError::Other(e.to_string()))?;

        let manifest =
            alchemy_core::manifest::Manifest::from_path(&temp_dir.path().join("package.json"))?;
        let version = manifest
            .version
            .as_deref()
            .unwrap_or(&sha[..8.min(sha.len())])
            .to_string();

        let id = PackageId::new(name, &version);

        // Copy the cloned directory to the content store via a temp path;
        // the store will be used later in the download phase.
        // Here we return source_path pointing to the temp dir clone.
        // Since TempDir will be dropped after this function, we persist it by
        // leaking its path — the install command will clean up after linking.
        let persisted_path = temp_dir.keep();

        Ok(ResolvedPackage {
            id,
            tarball_url: String::new(),
            integrity: None,
            source_path: Some(persisted_path),
            dependencies: manifest.dependencies,
            peer_dependencies: manifest.peer_dependencies,
            optional_dependencies: manifest.optional_dependencies,
            bin: manifest.bin,
            os: None,
            cpu: None,
            engines: None,
        })
    }

    /// Download a tarball URL and derive a ResolvedPackage from it.
    async fn fetch_url(&self, name: &str, url: &str) -> AlchemyResult<ResolvedPackage> {
        let url_str = url.to_string();
        let temp_dir = tempfile::tempdir().map_err(AlchemyError::Io)?;
        let dest = temp_dir.path().to_owned();

        // Run blocking download on thread pool
        tokio::task::spawn_blocking(move || crate::git::download_and_extract_url(&url_str, &dest))
            .await
            .map_err(|e| AlchemyError::Other(e.to_string()))?
            .map_err(|e| AlchemyError::Other(e.to_string()))?;

        let manifest =
            alchemy_core::manifest::Manifest::from_path(&temp_dir.path().join("package.json"))?;
        let version = manifest.version.as_deref().unwrap_or("0.0.0").to_string();
        let id = PackageId::new(name, &version);

        let persisted_path = temp_dir.keep();

        Ok(ResolvedPackage {
            id,
            tarball_url: url.to_string(),
            integrity: None,
            source_path: Some(persisted_path),
            dependencies: manifest.dependencies,
            peer_dependencies: manifest.peer_dependencies,
            optional_dependencies: manifest.optional_dependencies,
            bin: manifest.bin,
            os: None,
            cpu: None,
            engines: None,
        })
    }
}
