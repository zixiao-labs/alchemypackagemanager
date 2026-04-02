use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::dependency::{PackageId, PeerDepMeta, ResolvedPackage};
use crate::error::{AlchemyError, AlchemyResult};
use crate::graph::{DepEdge, DependencyGraph};
use crate::platform::Platform;
use crate::specifier::{self, DependencySpecifier};

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

/// Trait for resolving non-registry (git, tarball URL) dependencies.
/// Implemented by `alchemy_registry::RegistryClient`.
#[async_trait::async_trait]
pub trait NonRegistryFetcher: Send + Sync {
    /// Clone a git repository and return its resolved package metadata.
    async fn fetch_git(
        &self,
        name: &str,
        url: &str,
        commitish: Option<&str>,
    ) -> AlchemyResult<ResolvedPackage>;

    /// Download a tarball from a URL and return its resolved package metadata.
    async fn fetch_url(&self, name: &str, url: &str) -> AlchemyResult<ResolvedPackage>;
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
    /// Optional handler for git/url non-registry dependencies
    non_registry: Option<Box<dyn NonRegistryFetcher>>,
    /// Cache: package name → all versions (fetched once)
    version_cache: tokio::sync::Mutex<HashMap<String, Vec<VersionInfo>>>,
    /// Whether to auto-install missing peer dependencies
    auto_install_peers: bool,
    /// Base directory for resolving file: relative paths
    project_dir: Option<PathBuf>,
}

impl<F: MetadataFetcher> Resolver<F> {
    pub fn new(fetcher: F) -> Self {
        Self {
            fetcher,
            non_registry: None,
            version_cache: tokio::sync::Mutex::new(HashMap::new()),
            auto_install_peers: true,
            project_dir: None,
        }
    }

    pub fn with_auto_install_peers(mut self, auto_install: bool) -> Self {
        self.auto_install_peers = auto_install;
        self
    }

    pub fn with_non_registry(mut self, nr: Box<dyn NonRegistryFetcher>) -> Self {
        self.non_registry = Some(nr);
        self
    }

    pub fn with_project_dir(mut self, dir: PathBuf) -> Self {
        self.project_dir = Some(dir);
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

        for (name, version_req) in root_deps {
            let id = self
                .resolve_specifier(
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
                    .resolve_specifier(
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

    /// Dispatch a dependency specifier to the appropriate resolver.
    #[allow(clippy::too_many_arguments)]
    fn resolve_specifier<'a>(
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
            match specifier::parse(version_req) {
                DependencySpecifier::Registry(semver) => {
                    self.resolve_package(
                        name,
                        &semver,
                        graph,
                        packages,
                        resolved_versions,
                        visiting,
                        peer_warnings,
                    )
                    .await
                }
                DependencySpecifier::Path(rel_path) => {
                    self.resolve_path_dep(
                        name,
                        &rel_path,
                        graph,
                        packages,
                        resolved_versions,
                        peer_warnings,
                    )
                    .await
                }
                DependencySpecifier::Git(git_spec) => {
                    self.resolve_git_dep(
                        name,
                        &git_spec.url,
                        git_spec.commitish.as_deref(),
                        graph,
                        packages,
                        resolved_versions,
                    )
                    .await
                }
                DependencySpecifier::Url(url) => {
                    self.resolve_url_dep(name, &url, graph, packages, resolved_versions)
                        .await
                }
                DependencySpecifier::Alias { package, spec } => {
                    // npm: alias — resolve the aliased package, register under `name`
                    let alias_req = specifier_to_version_req(&spec);
                    let aliased_id = self
                        .resolve_package(
                            &package,
                            &alias_req,
                            graph,
                            packages,
                            resolved_versions,
                            visiting,
                            peer_warnings,
                        )
                        .await?;
                    // Register the alias name → same version
                    resolved_versions.insert(name.to_string(), aliased_id.version.clone());
                    Ok(aliased_id)
                }
                DependencySpecifier::Workspace(_) => {
                    // workspace: deps are handled by the workspace layer before resolution
                    Err(AlchemyError::ResolutionFailed(format!(
                        "workspace: specifier for '{name}' should be handled by workspace resolver"
                    )))
                }
            }
        })
    }

    /// Resolve a local file: dependency by reading its package.json.
    async fn resolve_path_dep(
        &self,
        name: &str,
        rel_path: &Path,
        graph: &mut DependencyGraph,
        packages: &mut HashMap<PackageId, ResolvedPackage>,
        resolved_versions: &mut HashMap<String, String>,
        peer_warnings: &mut Vec<PeerWarning>,
    ) -> AlchemyResult<PackageId> {
        let abs_path = if rel_path.is_absolute() {
            rel_path.to_owned()
        } else {
            let base = self
                .project_dir
                .as_deref()
                .unwrap_or_else(|| Path::new("."));
            base.join(rel_path)
        };

        let manifest_path = abs_path.join("package.json");
        let manifest = crate::manifest::Manifest::from_path(&manifest_path)?;
        let version = manifest.version.as_deref().unwrap_or("0.0.0").to_string();
        let id = PackageId::new(name, &version);

        if packages.contains_key(&id) {
            return Ok(id);
        }

        graph.add_package(id.clone());
        resolved_versions.insert(name.to_string(), version.clone());

        let resolved = ResolvedPackage {
            id: id.clone(),
            tarball_url: String::new(),
            integrity: None,
            source_path: Some(abs_path.clone()),
            dependencies: manifest.dependencies.clone(),
            peer_dependencies: manifest.peer_dependencies.clone(),
            optional_dependencies: manifest.optional_dependencies.clone(),
            bin: manifest.bin.clone(),
            os: None,
            cpu: None,
            engines: None,
        };
        packages.insert(id.clone(), resolved);

        // Recursively resolve the local package's own deps
        let deps = manifest.dependencies.clone();
        for (dep_name, dep_req) in &deps {
            let child_id = self
                .resolve_specifier(
                    dep_name,
                    dep_req,
                    graph,
                    packages,
                    resolved_versions,
                    &mut HashSet::new(),
                    peer_warnings,
                )
                .await?;
            graph.add_package(child_id.clone());
            graph.add_dependency(&id, &child_id, DepEdge::Normal);
        }

        Ok(id)
    }

    /// Resolve a git: dependency by cloning via NonRegistryFetcher.
    async fn resolve_git_dep(
        &self,
        name: &str,
        url: &str,
        commitish: Option<&str>,
        graph: &mut DependencyGraph,
        packages: &mut HashMap<PackageId, ResolvedPackage>,
        resolved_versions: &mut HashMap<String, String>,
    ) -> AlchemyResult<PackageId> {
        let nr = self.non_registry.as_ref().ok_or_else(|| {
            AlchemyError::ResolutionFailed(format!(
                "cannot resolve git dependency '{name}': no NonRegistryFetcher configured"
            ))
        })?;

        let resolved = nr.fetch_git(name, url, commitish).await?;
        let id = resolved.id.clone();

        if !packages.contains_key(&id) {
            graph.add_package(id.clone());
            resolved_versions.insert(name.to_string(), id.version.clone());
            packages.insert(id.clone(), resolved);
        }

        Ok(id)
    }

    /// Resolve a url: (tarball) dependency via NonRegistryFetcher.
    async fn resolve_url_dep(
        &self,
        name: &str,
        url: &str,
        graph: &mut DependencyGraph,
        packages: &mut HashMap<PackageId, ResolvedPackage>,
        resolved_versions: &mut HashMap<String, String>,
    ) -> AlchemyResult<PackageId> {
        let nr = self.non_registry.as_ref().ok_or_else(|| {
            AlchemyError::ResolutionFailed(format!(
                "cannot resolve url dependency '{name}': no NonRegistryFetcher configured"
            ))
        })?;

        let resolved = nr.fetch_url(name, url).await?;
        let id = resolved.id.clone();

        if !packages.contains_key(&id) {
            graph.add_package(id.clone());
            resolved_versions.insert(name.to_string(), id.version.clone());
            packages.insert(id.clone(), resolved);
        }

        Ok(id)
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
                // If we already resolved this package in the current pass, reuse it
                if let Some(ver) = resolved_versions.get(name) {
                    return Ok(PackageId::new(name, ver));
                }
                // Unresolvable cycle: package depends on itself before any version was chosen
                return Err(AlchemyError::ResolutionFailed(format!(
                    "circular dependency detected for '{name}'"
                )));
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
                source_path: None,
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
                        let peer_id = PackageId::new(peer_name, existing_ver);
                        if graph.index_map.contains_key(&peer_id) {
                            graph.add_dependency(&id, &peer_id, DepEdge::Peer);
                        }
                    } else {
                        peer_warnings.push(PeerWarning {
                            package: id.clone(),
                            peer_name: peer_name.clone(),
                            required: peer_req.clone(),
                            found: Some(existing_ver.clone()),
                        });
                    }
                } else if self.auto_install_peers && !is_optional {
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
                    peer_warnings.push(PeerWarning {
                        package: id.clone(),
                        peer_name: peer_name.clone(),
                        required: peer_req.clone(),
                        found: None,
                    });
                }
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

/// Convert a parsed DependencySpecifier back to a version requirement string
/// for registry resolution.
fn specifier_to_version_req(spec: &DependencySpecifier) -> String {
    match spec {
        DependencySpecifier::Registry(s) => s.clone(),
        _ => "*".to_string(),
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
        match (
            node_semver::Version::parse(&a.version),
            node_semver::Version::parse(&b.version),
        ) {
            (Ok(va), Ok(vb)) => vb.cmp(&va),
            _ => std::cmp::Ordering::Equal,
        }
    });

    candidates.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A mock fetcher backed by a static map of name → versions.
    struct MockFetcher(HashMap<String, Vec<VersionInfo>>);

    impl MockFetcher {
        fn new() -> Self {
            Self(HashMap::new())
        }

        fn with_package(
            mut self,
            name: &str,
            versions: Vec<(&str, BTreeMap<String, String>)>,
        ) -> Self {
            let vis: Vec<VersionInfo> = versions
                .into_iter()
                .map(|(ver, deps)| VersionInfo {
                    version: ver.to_string(),
                    dependencies: deps,
                    peer_dependencies: BTreeMap::new(),
                    optional_dependencies: BTreeMap::new(),
                    peer_dependencies_meta: BTreeMap::new(),
                    tarball_url: format!("https://example.com/{}-{}.tgz", name, ver),
                    integrity: None,
                    bin: None,
                    os: None,
                    cpu: None,
                    engines: None,
                })
                .collect();
            self.0.insert(name.to_string(), vis);
            self
        }
    }

    #[async_trait::async_trait]
    impl MetadataFetcher for MockFetcher {
        async fn fetch_versions(&self, name: &str) -> AlchemyResult<Vec<VersionInfo>> {
            self.0
                .get(name)
                .cloned()
                .ok_or_else(|| AlchemyError::PackageNotFound(name.to_string()))
        }
    }

    fn deps(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[tokio::test]
    async fn test_resolve_simple_package() {
        let fetcher = MockFetcher::new().with_package(
            "lodash",
            vec![("4.17.21", BTreeMap::new()), ("4.17.20", BTreeMap::new())],
        );
        let resolver = Resolver::new(fetcher);
        let root_deps = deps(&[("lodash", "^4.17.0")]);
        let result = resolver.resolve(&root_deps).await.unwrap();
        assert_eq!(result.packages.len(), 1);
        let id = result.direct_deps.first().unwrap();
        assert_eq!(id.name, "lodash");
        assert_eq!(id.version, "4.17.21"); // picks highest
    }

    #[tokio::test]
    async fn test_resolve_picks_highest_satisfying() {
        let fetcher = MockFetcher::new().with_package(
            "semver",
            vec![
                ("7.6.0", BTreeMap::new()),
                ("7.5.4", BTreeMap::new()),
                ("6.3.1", BTreeMap::new()),
            ],
        );
        let resolver = Resolver::new(fetcher);
        let result = resolver
            .resolve(&deps(&[("semver", "^7.0.0")]))
            .await
            .unwrap();
        let id = result.direct_deps.first().unwrap();
        assert_eq!(id.version, "7.6.0");
    }

    #[tokio::test]
    async fn test_resolve_transitive_deps() {
        let fetcher = MockFetcher::new()
            .with_package(
                "express",
                vec![("4.18.0", deps(&[("body-parser", "^1.20.0")]))],
            )
            .with_package(
                "body-parser",
                vec![("1.20.2", BTreeMap::new()), ("1.20.0", BTreeMap::new())],
            );
        let resolver = Resolver::new(fetcher);
        let result = resolver
            .resolve(&deps(&[("express", "^4.0.0")]))
            .await
            .unwrap();
        // Both express and body-parser should be in packages
        assert_eq!(result.packages.len(), 2);
        assert!(result
            .packages
            .keys()
            .any(|id| id.name == "body-parser" && id.version == "1.20.2"));
    }

    #[tokio::test]
    async fn test_resolve_deduplicates_shared_dep() {
        // Both A and B depend on lodash — only one lodash should be resolved
        let fetcher = MockFetcher::new()
            .with_package("pkg-a", vec![("1.0.0", deps(&[("lodash", "^4.0.0")]))])
            .with_package("pkg-b", vec![("1.0.0", deps(&[("lodash", "^4.0.0")]))])
            .with_package("lodash", vec![("4.17.21", BTreeMap::new())]);
        let resolver = Resolver::new(fetcher);
        let result = resolver
            .resolve(&deps(&[("pkg-a", "^1.0.0"), ("pkg-b", "^1.0.0")]))
            .await
            .unwrap();
        let lodash_count = result
            .packages
            .keys()
            .filter(|id| id.name == "lodash")
            .count();
        assert_eq!(lodash_count, 1);
    }

    #[tokio::test]
    async fn test_resolve_package_not_found_returns_error() {
        let fetcher = MockFetcher::new();
        let resolver = Resolver::new(fetcher);
        let result = resolver.resolve(&deps(&[("nonexistent", "^1.0.0")])).await;
        assert!(result.is_err());
        assert!(matches!(
            result.err().unwrap(),
            AlchemyError::PackageNotFound(_)
        ));
    }

    #[tokio::test]
    async fn test_resolve_no_matching_version_returns_error() {
        let fetcher = MockFetcher::new().with_package("old-pkg", vec![("0.5.0", BTreeMap::new())]);
        let resolver = Resolver::new(fetcher);
        // Require >=1.0.0 but only 0.5.0 exists
        let result = resolver.resolve(&deps(&[("old-pkg", "^1.0.0")])).await;
        assert!(result.is_err());
        assert!(matches!(
            result.err().unwrap(),
            AlchemyError::VersionNotFound(_, _)
        ));
    }

    #[tokio::test]
    async fn test_cycle_detection_does_not_produce_fake_version() {
        // A depends on B, B depends on A — circular, but A is already resolved
        // when B tries to resolve it, so it reuses A's version correctly.
        let fetcher = MockFetcher::new()
            .with_package("pkg-a", vec![("1.0.0", deps(&[("pkg-b", "^1.0.0")]))])
            .with_package("pkg-b", vec![("1.0.0", deps(&[("pkg-a", "^1.0.0")]))]);
        let resolver = Resolver::new(fetcher);
        let result = resolver.resolve(&deps(&[("pkg-a", "^1.0.0")])).await;
        // Cycle is handled gracefully (A was already resolved when B loops back)
        // The critical invariant: no fake 0.0.0 version is emitted
        let has_fake = match result {
            Ok(r) => r.packages.keys().any(|id| id.version == "0.0.0"),
            Err(_) => false, // error is also acceptable
        };
        assert!(
            !has_fake,
            "cycle detection must never produce fake 0.0.0 versions"
        );
    }

    #[tokio::test]
    async fn test_file_dep_resolved_with_source_path() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let pkg_dir = dir.path().join("local-pkg");
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"local-pkg","version":"2.3.4","dependencies":{}}"#,
        )
        .unwrap();

        let fetcher = MockFetcher::new();
        let resolver = Resolver::new(fetcher).with_project_dir(dir.path().to_owned());

        let root_deps = deps(&[("local-pkg", "file:./local-pkg")]);
        let result = resolver.resolve(&root_deps).await.unwrap();

        assert_eq!(result.packages.len(), 1);
        let (id, pkg) = result.packages.iter().next().unwrap();
        assert_eq!(id.name, "local-pkg");
        assert_eq!(id.version, "2.3.4");
        assert!(pkg.source_path.is_some());
    }

    #[tokio::test]
    async fn test_npm_alias_resolves_to_aliased_package() {
        let fetcher = MockFetcher::new().with_package(
            "react",
            vec![("18.2.0", BTreeMap::new()), ("17.0.2", BTreeMap::new())],
        );
        let resolver = Resolver::new(fetcher);
        // npm:react@^17 should resolve to react@17.0.2
        let root_deps = deps(&[("my-react", "npm:react@^17.0.0")]);
        let result = resolver.resolve(&root_deps).await.unwrap();
        // The resolved package should be react@17.0.2
        assert!(result
            .packages
            .keys()
            .any(|id| id.name == "react" && id.version == "17.0.2"));
    }
}
