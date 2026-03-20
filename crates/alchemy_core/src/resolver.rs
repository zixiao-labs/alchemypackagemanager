use std::collections::{BTreeMap, HashMap, HashSet};

use crate::dependency::{PackageId, ResolvedPackage};
use crate::error::{AlchemyError, AlchemyResult};
use crate::graph::DependencyGraph;

/// Result of dependency resolution
pub struct ResolutionResult {
    pub graph: DependencyGraph,
    pub packages: HashMap<PackageId, ResolvedPackage>,
    /// Direct dependencies of the root project
    pub direct_deps: Vec<PackageId>,
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
    pub tarball_url: String,
    pub integrity: Option<String>,
    pub bin: Option<crate::manifest::BinField>,
}

/// Greedy DFS resolver: for each dependency, pick the highest version that
/// satisfies the constraint, then recursively resolve its dependencies.
pub struct Resolver<F: MetadataFetcher> {
    fetcher: F,
    /// Cache: package name → all versions (fetched once)
    version_cache: tokio::sync::Mutex<HashMap<String, Vec<VersionInfo>>>,
}

impl<F: MetadataFetcher> Resolver<F> {
    pub fn new(fetcher: F) -> Self {
        Self {
            fetcher,
            version_cache: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub async fn resolve(
        &self,
        root_deps: &BTreeMap<String, String>,
    ) -> AlchemyResult<ResolutionResult> {
        let mut graph = DependencyGraph::new();
        let mut packages: HashMap<PackageId, ResolvedPackage> = HashMap::new();
        let mut resolved_versions: HashMap<String, String> = HashMap::new();
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
                )
                .await?;
            direct_deps.push(id);
        }

        Ok(ResolutionResult {
            graph,
            packages,
            direct_deps,
        })
    }

    fn resolve_package<'a>(
        &'a self,
        name: &'a str,
        version_req: &'a str,
        graph: &'a mut DependencyGraph,
        packages: &'a mut HashMap<PackageId, ResolvedPackage>,
        resolved_versions: &'a mut HashMap<String, String>,
        visiting: &'a mut HashSet<String>,
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

            let id = PackageId::new(name, &chosen.version);
            graph.add_package(id.clone());
            resolved_versions.insert(name.to_string(), chosen.version.clone());

            let resolved = ResolvedPackage {
                id: id.clone(),
                tarball_url: chosen.tarball_url.clone(),
                integrity: chosen.integrity.clone(),
                dependencies: chosen.dependencies.clone(),
                bin: chosen.bin.clone(),
            };
            packages.insert(id.clone(), resolved);

            // Recursively resolve transitive deps
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
                    )
                    .await?;
                graph.add_package(child_id.clone());
                graph.add_dependency(&id, &child_id);
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
