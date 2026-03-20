use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;

use crate::dependency::PackageId;

/// Dependency graph built during resolution
pub struct DependencyGraph {
    pub graph: DiGraph<PackageId, ()>,
    pub index_map: HashMap<PackageId, NodeIndex>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            index_map: HashMap::new(),
        }
    }

    /// Add a package node, returning its index. Idempotent.
    pub fn add_package(&mut self, id: PackageId) -> NodeIndex {
        if let Some(&idx) = self.index_map.get(&id) {
            return idx;
        }
        let idx = self.graph.add_node(id.clone());
        self.index_map.insert(id, idx);
        idx
    }

    /// Add a dependency edge from `parent` to `child`.
    pub fn add_dependency(&mut self, parent: &PackageId, child: &PackageId) {
        let parent_idx = self.index_map[parent];
        let child_idx = self.index_map[child];
        self.graph.add_edge(parent_idx, child_idx, ());
    }

    /// Get all direct dependencies of a package
    pub fn dependencies_of(&self, id: &PackageId) -> Vec<&PackageId> {
        if let Some(&idx) = self.index_map.get(id) {
            self.graph
                .neighbors(idx)
                .map(|n| &self.graph[n])
                .collect()
        } else {
            vec![]
        }
    }

    /// All packages in the graph
    pub fn all_packages(&self) -> Vec<&PackageId> {
        self.graph.node_weights().collect()
    }
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}
