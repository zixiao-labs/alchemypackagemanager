use std::collections::{BTreeMap, HashMap, HashSet};

use crate::dependency::{PackageId, PeerDepMeta, ResolvedPackage};
use crate::error::{AlchemyError, AlchemyResult};
use crate::graph::{DepEdge, DependencyGraph};
use crate::platform::Platform;

/// Warning about a peer dependency that couldn't be satisfied
#[derive(Debug, Clone)]
pub struct PeerWarning {
    pub package: PackageId,
    pub peer_name: String,
    pub required: String,
    pub found: Option<String>,
}

/// Result of dependency resolution
pub struct ResolutionResult {
    pub graph: DependencyGraph,
    pub packages: HashMap<PackageId, ResolvedPackage>,
    /// Direct dependencies of the root project
    pub direct_deps: Vec<PackageId>,
    /// Warnings about peer dependency issues
    pub peer_warnings: Vec<PeerWarning>,
}

/// Result of workspace (multi-root) dependency resolution.
pub struct WorkspaceResolutionResult {
    pub graph: DependencyGraph,
    pub packages: HashMap<PackageId, ResolvedPackage>,
    /// Direct deps per importer (importer key → list of direct dep PackageIds)
    pub importers: BTreeMap<String, Vec<PackageId>>,
    pub peer_warnings: Vec<PeerWarning>,
}

/// Trait for fetching registry metadata — allows mocking in tests
#[async_trait::async_trait]
pub trait MetadataFetcher: Send + Sync {
    /// Fetch all available versions for a package with their metadata
    async fn fetch_versions(&self, name: &str) -> AlchemyResult<Vec<VersionInfo>>;
}

/// Info about a single version of a package from the registry
#[derive(Debug, Clone)]
pub struct VersionInfo {
    pub version: String,
    pub dependencies: BTreeMap<String, String>,
    pub peer_dependencies: BTreeMap<String, String>,
    pub optional_dependencies: BTreeMap<String, String>,
    pub peer_dependencies_meta: BTreeMap<String, PeerDepMeta>,
    pub tarball_url: String,
    pub integrity: Option<String>,
    pub bin: Option<crate::manifest::BinField>,
    pub os: Option<Vec<String>>,
    pub cpu: Option<Vec<String>>,
    pub engines: Option<BTreeMap<String, String>>,
}

/// Greedy DFS resolver: for each dependency, pick the highest version that
/// satisfies the constraint, then recursively resolve its dependencies.
pub struct Resolver<F: MetadataFetcher> {
    fetcher: F,
    /// Cache: package name → all versions (fetched once)
    version_cache: tokio::sync::Mutex<HashMap<String, Vec<VersionInfo>>>,
    /// Whether to auto-install missing peer dependencies
    auto_install_peers: bool,
}

impl<F: MetadataFetcher> Resolver<F> {
    pub fn new(fetcher: F) -> Self {
        Self {
            fetcher,
            version_cache: tokio::sync::Mutex::new(HashMap::new()),
            auto_install_peers: true,
        }
    }

    pub fn with_auto_install_peers(mut self, auto_install: bool) -> Self {
        self.auto_install_peers = auto_install;
        self
    }

    pub async fn resolve(
        &self,
        root_deps: &BTreeMap<String, String>,
    ) -> AlchemyResult<ResolutionResult> {
        let mut graph = DependencyGraph::new();
        let mut packages: HashMap<PackageId, ResolvedPackage> = HashMap::new();
        let mut resolved_versions: HashMap<String, String> = HashMap::new();
        let mut peer_warnings: Vec<PeerWarning> = Vec::new();
        let mut direct_deps = Vec::new();

        // Resolve each direct dependency
        for (name, version_req) in root_deps {
            let id = self
                .resolve_package(
                    name,
                    version_req,
                    &mut graph,
                    &mut packages,
                    &mut resolved_versions,
                    &mut HashSet::new(),
                    &mut peer_warnings,
                )
                .await?;
            direct_deps.push(id);
        }

        Ok(ResolutionResult {
            graph,
            packages,
            direct_deps,
            peer_warnings,
        })
    }

    /// Resolve multiple importers (workspace packages) together.
    /// All share the same resolution state, ensuring dependency deduplication.
    pub async fn resolve_workspace(
        &self,
        importers: &BTreeMap<String, BTreeMap<String, String>>,
    ) -> AlchemyResult<WorkspaceResolutionResult> {
        let mut graph = DependencyGraph::new();
        let mut packages: HashMap<PackageId, ResolvedPackage> = HashMap::new();
        let mut resolved_versions: HashMap<String, String> = HashMap::new();
        let mut peer_warnings: Vec<PeerWarning> = Vec::new();
        let mut importer_direct_deps: BTreeMap<String, Vec<PackageId>> = BTreeMap::new();

        for (importer_key, deps) in importers {
            let mut direct_deps = Vec::new();
            for (name, version_req) in deps {
                let id = self
                    .resolve_package(
                        name,
                        version_req,
                        &mut graph,
                        &mut packages,
                        &mut resolved_versions,
                        &mut HashSet::new(),
                        &mut peer_warnings,
                    )
                    .await?;
                direct_deps.push(id);
            }
            importer_direct_deps.insert(importer_key.clone(), direct_deps);
        }

        Ok(WorkspaceResolutionResult {
            graph,
            packages,
            importers: importer_direct_deps,
            peer_warnings,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_package<'a>(
        &'a self,
        name: &'a str,
        version_req: &'a str,
        graph: &'a mut DependencyGraph,
        packages: &'a mut HashMap<PackageId, ResolvedPackage>,
        resolved_versions: &'a mut HashMap<String, String>,
        visiting: &'a mut HashSet<String>,
        peer_warnings: &'a mut Vec<PeerWarning>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AlchemyResult<PackageId>> + Send + 'a>>
    {
        Box::pin(async move {
            // If already resolved and satisfies, reuse
            if let Some(existing_version) = resolved_versions.get(name) {
                if version_satisfies(existing_version, version_req) {
                    return Ok(PackageId::new(name, existing_version));
                }
            }

            // Cycle detection
            if visiting.contains(name) {
                let id = resolved_versions
                    .get(name)
                    .map(|v| PackageId::new(name, v))
                    .unwrap_or_else(|| PackageId::new(name, "0.0.0"));
                return Ok(id);
            }
            visiting.insert(name.to_string());

            // Fetch versions
            let versions = self.get_versions(name).await?;

            // Pick highest version satisfying the constraint
            let chosen = pick_best_version(&versions, version_req).ok_or_else(|| {
                AlchemyError::VersionNotFound(name.to_string(), version_req.to_string())
            })?;

            // Platform compatibility check
            let platform = Platform::current();
            if !platform.is_compatible(chosen.os.as_ref(), chosen.cpu.as_ref()) {
                tracing::debug!(
                    "Skipping {} — incompatible platform (os: {:?}, cpu: {:?})",
                    name,
                    chosen.os,
                    chosen.cpu
                );
                return Err(AlchemyError::Other(format!(
                    "{} is not compatible with the current platform ({}/{})",
                    name, platform.os, platform.cpu
                )));
            }

            let id = PackageId::new(name, &chosen.version);
            graph.add_package(id.clone());
            resolved_versions.insert(name.to_string(), chosen.version.clone());

            let resolved = ResolvedPackage {
                id: id.clone(),
                tarball_url: chosen.tarball_url.clone(),
                integrity: chosen.integrity.clone(),
                dependencies: chosen.dependencies.clone(),
                peer_dependencies: chosen.peer_dependencies.clone(),
                optional_dependencies: chosen.optional_dependencies.clone(),
                bin: chosen.bin.clone(),
                os: chosen.os.clone(),
                cpu: chosen.cpu.clone(),
                engines: chosen.engines.clone(),
            };
            packages.insert(id.clone(), resolved);

            // Recursively resolve normal dependencies
            let child_deps = chosen.dependencies.clone();
            for (dep_name, dep_req) in &child_deps {
                let child_id = self
                    .resolve_package(
                        dep_name,
                        dep_req,
                        graph,
                        packages,
                        resolved_versions,
                        visiting,
                        peer_warnings,
                    )
                    .await?;
                graph.add_package(child_id.clone());
                graph.add_dependency(&id, &child_id, DepEdge::Normal);
            }

            // Resolve optional dependencies (failures are non-fatal)
            let optional_deps = chosen.optional_dependencies.clone();
            for (dep_name, dep_req) in &optional_deps {
                match self
                    .resolve_package(
                        dep_name,
                        dep_req,
                        graph,
                        packages,
                        resolved_versions,
                        visiting,
                        peer_warnings,
                    )
                    .await
                {
                    Ok(child_id) => {
                        graph.add_package(child_id.clone());
                        graph.add_dependency(&id, &child_id, DepEdge::Optional);
                    }
                    Err(e) => {
                        tracing::debug!(
                            "Skipping optional dependency {} for {}: {}",
                            dep_name,
                            id,
                            e
                        );
                    }
                }
            }

            // Handle peer dependencies
            let peer_deps = chosen.peer_dependencies.clone();
            let peer_meta = chosen.peer_dependencies_meta.clone();
            for (peer_name, peer_req) in &peer_deps {
                let is_optional = peer_meta
                    .get(peer_name)
                    .map(|m| m.optional)
                    .unwrap_or(false);

                // Check if already resolved with a satisfying version
                if let Some(existing_ver) = resolved_versions.get(peer_name) {
                    if version_satisfies(existing_ver, peer_req) {
                        // Already satisfied — add a peer edge
                        let peer_id = PackageId::new(peer_name, existing_ver);
                        if graph.index_map.contains_key(&peer_id) {
                            graph.add_dependency(&id, &peer_id, DepEdge::Peer);
                        }
                    } else {
                        // Version conflict
                        peer_warnings.push(PeerWarning {
                            package: id.clone(),
                            peer_name: peer_name.clone(),
                            required: peer_req.clone(),
                            found: Some(existing_ver.clone()),
                        });
                    }
                } else if self.auto_install_peers && !is_optional {
                    // Auto-install the peer dep
                    match self
                        .resolve_package(
                            peer_name,
                            peer_req,
                            graph,
                            packages,
                            resolved_versions,
                            visiting,
                            peer_warnings,
                        )
                        .await
                    {
                        Ok(peer_id) => {
                            graph.add_package(peer_id.clone());
                            graph.add_dependency(&id, &peer_id, DepEdge::Peer);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to auto-install peer dependency {} for {}: {}",
                                peer_name,
                                id,
                                e
                            );
                            peer_warnings.push(PeerWarning {
                                package: id.clone(),
                                peer_name: peer_name.clone(),
                                required: peer_req.clone(),
                                found: None,
                            });
                        }
                    }
                } else if !is_optional {
                    // Not auto-installing — warn about missing peer
                    peer_warnings.push(PeerWarning {
                        package: id.clone(),
                        peer_name: peer_name.clone(),
                        required: peer_req.clone(),
                        found: None,
                    });
                }
                // Optional peers that aren't installed are silently skipped
            }

            visiting.remove(name);
            Ok(id)
        })
    }

    async fn get_versions(&self, name: &str) -> AlchemyResult<Vec<VersionInfo>> {
        let cache = self.version_cache.lock().await;
        if let Some(versions) = cache.get(name) {
            return Ok(versions.clone());
        }
        drop(cache);

        let versions = self.fetcher.fetch_versions(name).await?;

        let mut cache = self.version_cache.lock().await;
        cache.insert(name.to_string(), versions.clone());
        Ok(versions)
    }
}

/// Check if a concrete version satisfies a version requirement (npm semantics)
fn version_satisfies(version: &str, requirement: &str) -> bool {
    let req = match node_semver::Range::parse(requirement) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let ver = match node_semver::Version::parse(version) {
        Ok(v) => v,
        Err(_) => return false,
    };
    req.satisfies(&ver)
}

/// Pick the highest version satisfying the requirement
fn pick_best_version<'a>(
    versions: &'a [VersionInfo],
    requirement: &str,
) -> Option<&'a VersionInfo> {
    let req = node_semver::Range::parse(requirement).ok()?;

    let mut candidates: Vec<&VersionInfo> = versions
        .iter()
        .filter(|v| {
            node_semver::Version::parse(&v.version)
                .map(|sv| req.satisfies(&sv))
                .unwrap_or(false)
        })
        .collect();

    // Sort descending, pick highest
    candidates.sort_by(|a, b| {
        let va = node_semver::Version::parse(&a.version).unwrap();
        let vb = node_semver::Version::parse(&b.version).unwrap();
        vb.cmp(&va)
    });

    candidates.into_iter().next()
}
