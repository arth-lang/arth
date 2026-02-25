//! Dependency graph for incremental compilation.
//!
//! This module builds and queries a dependency graph between packages,
//! enabling efficient invalidation when sources change.
//!
//! Key concepts:
//! - Each package is a node with source fingerprint and interface fingerprint
//! - Edges represent import dependencies between packages
//! - When a package's interface changes, all dependents must be recompiled
//! - When only the implementation changes (same interface), dependents are unaffected

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::fingerprint::{Fingerprint, FingerprintKind};

/// A node in the dependency graph representing a package.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DepNode {
    /// Package name (e.g., "app.http")
    pub package: String,
    /// Source files in this package
    pub files: Vec<PathBuf>,
    /// Fingerprint of all source files combined
    pub source_fingerprint: Fingerprint,
    /// Fingerprint of public interface (None if not yet computed)
    pub interface_fingerprint: Option<Fingerprint>,
    /// Packages this package depends on (imports from)
    pub dependencies: BTreeSet<String>,
    /// Packages that depend on this package (import this)
    pub dependents: BTreeSet<String>,
}

impl DepNode {
    /// Create a new node with the given package name.
    pub fn new(package: impl Into<String>) -> Self {
        Self {
            package: package.into(),
            files: Vec::new(),
            source_fingerprint: Fingerprint::new(String::new(), FingerprintKind::Source),
            interface_fingerprint: None,
            dependencies: BTreeSet::new(),
            dependents: BTreeSet::new(),
        }
    }

    /// Set the source fingerprint.
    pub fn with_source_fingerprint(mut self, fp: Fingerprint) -> Self {
        self.source_fingerprint = fp;
        self
    }

    /// Set the interface fingerprint.
    pub fn with_interface_fingerprint(mut self, fp: Fingerprint) -> Self {
        self.interface_fingerprint = Some(fp);
        self
    }

    /// Add a file to this package.
    pub fn add_file(&mut self, path: PathBuf) {
        self.files.push(path);
    }

    /// Add a dependency (package this imports from).
    pub fn add_dependency(&mut self, pkg: impl Into<String>) {
        self.dependencies.insert(pkg.into());
    }

    /// Add a dependent (package that imports this).
    pub fn add_dependent(&mut self, pkg: impl Into<String>) {
        self.dependents.insert(pkg.into());
    }
}

/// Dependency graph for a project.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DepGraph {
    /// All packages in the graph, keyed by package name
    pub nodes: BTreeMap<String, DepNode>,
    /// Topologically sorted build order (dependencies before dependents)
    pub build_order: Vec<String>,
}

impl DepGraph {
    /// Create a new empty dependency graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node to the graph.
    pub fn add_node(&mut self, node: DepNode) {
        self.nodes.insert(node.package.clone(), node);
    }

    /// Get a node by package name.
    pub fn get(&self, package: &str) -> Option<&DepNode> {
        self.nodes.get(package)
    }

    /// Get a mutable node by package name.
    pub fn get_mut(&mut self, package: &str) -> Option<&mut DepNode> {
        self.nodes.get_mut(package)
    }

    /// Add an edge from `from` to `to` (from imports to).
    pub fn add_edge(&mut self, from: &str, to: &str) {
        if let Some(from_node) = self.nodes.get_mut(from) {
            from_node.add_dependency(to);
        }
        if let Some(to_node) = self.nodes.get_mut(to) {
            to_node.add_dependent(from);
        }
    }

    /// Get all packages in the graph.
    pub fn packages(&self) -> impl Iterator<Item = &str> {
        self.nodes.keys().map(|s| s.as_str())
    }

    /// Get the number of packages.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Check if the graph is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Compute topological sort using Kahn's algorithm.
    ///
    /// Returns packages in dependency order (dependencies first).
    /// Returns None if there's a cycle.
    pub fn topological_sort(&self) -> Option<Vec<String>> {
        let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
        let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();

        // Initialize
        for (pkg, node) in &self.nodes {
            in_degree.entry(pkg.as_str()).or_insert(0);
            adj.entry(pkg.as_str()).or_insert_with(Vec::new);

            for dep in &node.dependencies {
                if self.nodes.contains_key(dep) {
                    adj.entry(dep.as_str())
                        .or_insert_with(Vec::new)
                        .push(pkg.as_str());
                    *in_degree.entry(pkg.as_str()).or_insert(0) += 1;
                }
            }
        }

        // Find nodes with no incoming edges
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(&pkg, _)| pkg)
            .collect();

        let mut result = Vec::new();

        while let Some(pkg) = queue.pop_front() {
            result.push(pkg.to_string());

            if let Some(neighbors) = adj.get(pkg) {
                for &neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(neighbor);
                        }
                    }
                }
            }
        }

        if result.len() == self.nodes.len() {
            Some(result)
        } else {
            None // Cycle detected
        }
    }

    /// Compute and store the build order.
    ///
    /// Returns false if there's a cycle.
    pub fn compute_build_order(&mut self) -> bool {
        if let Some(order) = self.topological_sort() {
            self.build_order = order;
            true
        } else {
            false
        }
    }

    /// Get packages that need recompilation given a set of changed packages.
    ///
    /// This propagates invalidation through the dependency graph:
    /// - All changed packages need recompilation
    /// - All transitive dependents of changed packages need recompilation
    pub fn invalidated_by(&self, changed: &HashSet<String>) -> HashSet<String> {
        let mut invalidated = changed.clone();
        let mut queue: VecDeque<&str> = changed.iter().map(|s| s.as_str()).collect();

        while let Some(pkg) = queue.pop_front() {
            if let Some(node) = self.nodes.get(pkg) {
                for dependent in &node.dependents {
                    if !invalidated.contains(dependent) {
                        invalidated.insert(dependent.clone());
                        queue.push_back(dependent.as_str());
                    }
                }
            }
        }

        invalidated
    }

    /// Get packages that need recompilation due to interface changes.
    ///
    /// This is more precise than `invalidated_by`:
    /// - Only propagates to dependents if the interface changed
    /// - Implementation-only changes don't trigger dependent recompilation
    pub fn invalidated_by_interface_change(
        &self,
        interface_changed: &HashSet<String>,
    ) -> HashSet<String> {
        self.invalidated_by(interface_changed)
    }

    /// Get the immediate dependencies of a package.
    pub fn dependencies_of(&self, package: &str) -> Option<&BTreeSet<String>> {
        self.nodes.get(package).map(|n| &n.dependencies)
    }

    /// Get the immediate dependents of a package.
    pub fn dependents_of(&self, package: &str) -> Option<&BTreeSet<String>> {
        self.nodes.get(package).map(|n| &n.dependents)
    }

    /// Get all transitive dependencies of a package.
    pub fn transitive_dependencies(&self, package: &str) -> HashSet<String> {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        if let Some(node) = self.nodes.get(package) {
            for dep in &node.dependencies {
                queue.push_back(dep.as_str());
            }
        }

        while let Some(pkg) = queue.pop_front() {
            if visited.insert(pkg.to_string()) {
                if let Some(node) = self.nodes.get(pkg) {
                    for dep in &node.dependencies {
                        if !visited.contains(dep.as_str()) {
                            queue.push_back(dep.as_str());
                        }
                    }
                }
            }
        }

        visited
    }

    /// Detect cycles in the dependency graph.
    ///
    /// Returns Some with the cycle path if a cycle exists.
    pub fn detect_cycle(&self) -> Option<Vec<String>> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum State {
            Unvisited,
            InProgress,
            Done,
        }

        let mut state: BTreeMap<&str, State> = self
            .nodes
            .keys()
            .map(|k| (k.as_str(), State::Unvisited))
            .collect();
        let mut path = Vec::new();

        fn dfs<'a>(
            graph: &'a DepGraph,
            pkg: &'a str,
            state: &mut BTreeMap<&'a str, State>,
            path: &mut Vec<String>,
        ) -> Option<Vec<String>> {
            state.insert(pkg, State::InProgress);
            path.push(pkg.to_string());

            if let Some(node) = graph.nodes.get(pkg) {
                for dep in &node.dependencies {
                    if !graph.nodes.contains_key(dep) {
                        continue; // Skip external deps
                    }

                    match state.get(dep.as_str()) {
                        Some(State::InProgress) => {
                            // Found a cycle
                            path.push(dep.clone());
                            return Some(path.clone());
                        }
                        Some(State::Unvisited) => {
                            if let Some(cycle) = dfs(graph, dep, state, path) {
                                return Some(cycle);
                            }
                        }
                        _ => {}
                    }
                }
            }

            state.insert(pkg, State::Done);
            path.pop();
            None
        }

        for pkg in self.nodes.keys() {
            if state.get(pkg.as_str()) == Some(&State::Unvisited) {
                if let Some(cycle) = dfs(self, pkg, &mut state, &mut path) {
                    return Some(cycle);
                }
            }
        }

        None
    }
}

/// Builder for constructing a dependency graph.
#[derive(Debug, Default)]
pub struct DepGraphBuilder {
    nodes: BTreeMap<String, DepNode>,
    edges: Vec<(String, String)>,
}

impl DepGraphBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a package node.
    pub fn add_package(&mut self, name: impl Into<String>) -> &mut Self {
        let name = name.into();
        self.nodes
            .entry(name.clone())
            .or_insert_with(|| DepNode::new(name));
        self
    }

    /// Add a package with source fingerprint.
    pub fn add_package_with_fingerprint(
        &mut self,
        name: impl Into<String>,
        source_fp: Fingerprint,
    ) -> &mut Self {
        let name = name.into();
        let node = DepNode::new(name.clone()).with_source_fingerprint(source_fp);
        self.nodes.insert(name, node);
        self
    }

    /// Add a file to a package.
    pub fn add_file(&mut self, package: &str, path: PathBuf) -> &mut Self {
        if let Some(node) = self.nodes.get_mut(package) {
            node.add_file(path);
        }
        self
    }

    /// Add an import edge (from imports to).
    pub fn add_import(&mut self, from: impl Into<String>, to: impl Into<String>) -> &mut Self {
        self.edges.push((from.into(), to.into()));
        self
    }

    /// Build the dependency graph.
    pub fn build(mut self) -> DepGraph {
        let mut graph = DepGraph::new();

        // Add all nodes
        for (_, node) in std::mem::take(&mut self.nodes) {
            graph.add_node(node);
        }

        // Add all edges
        for (from, to) in self.edges {
            graph.add_edge(&from, &to);
        }

        // Compute build order
        graph.compute_build_order();

        graph
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dep_graph_empty() {
        let graph = DepGraph::new();
        assert!(graph.is_empty());
        assert_eq!(graph.len(), 0);
    }

    #[test]
    fn test_dep_graph_add_node() {
        let mut graph = DepGraph::new();
        graph.add_node(DepNode::new("app"));

        assert!(!graph.is_empty());
        assert_eq!(graph.len(), 1);
        assert!(graph.get("app").is_some());
    }

    #[test]
    fn test_dep_graph_add_edge() {
        let mut graph = DepGraph::new();
        graph.add_node(DepNode::new("app"));
        graph.add_node(DepNode::new("lib"));
        graph.add_edge("app", "lib");

        let app = graph.get("app").unwrap();
        assert!(app.dependencies.contains("lib"));

        let lib = graph.get("lib").unwrap();
        assert!(lib.dependents.contains("app"));
    }

    #[test]
    fn test_topological_sort_simple() {
        let mut graph = DepGraph::new();
        graph.add_node(DepNode::new("a"));
        graph.add_node(DepNode::new("b"));
        graph.add_node(DepNode::new("c"));
        // c -> b -> a
        graph.add_edge("c", "b");
        graph.add_edge("b", "a");

        let order = graph.topological_sort().unwrap();

        // a should come before b, b before c
        let pos_a = order.iter().position(|x| x == "a").unwrap();
        let pos_b = order.iter().position(|x| x == "b").unwrap();
        let pos_c = order.iter().position(|x| x == "c").unwrap();

        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    #[test]
    fn test_topological_sort_diamond() {
        // Diamond: d -> (b, c) -> a
        let mut graph = DepGraph::new();
        graph.add_node(DepNode::new("a"));
        graph.add_node(DepNode::new("b"));
        graph.add_node(DepNode::new("c"));
        graph.add_node(DepNode::new("d"));
        graph.add_edge("d", "b");
        graph.add_edge("d", "c");
        graph.add_edge("b", "a");
        graph.add_edge("c", "a");

        let order = graph.topological_sort().unwrap();
        assert_eq!(order.len(), 4);

        let pos_a = order.iter().position(|x| x == "a").unwrap();
        let pos_b = order.iter().position(|x| x == "b").unwrap();
        let pos_c = order.iter().position(|x| x == "c").unwrap();
        let pos_d = order.iter().position(|x| x == "d").unwrap();

        assert!(pos_a < pos_b);
        assert!(pos_a < pos_c);
        assert!(pos_b < pos_d);
        assert!(pos_c < pos_d);
    }

    #[test]
    fn test_topological_sort_cycle() {
        let mut graph = DepGraph::new();
        graph.add_node(DepNode::new("a"));
        graph.add_node(DepNode::new("b"));
        graph.add_edge("a", "b");
        graph.add_edge("b", "a");

        assert!(graph.topological_sort().is_none());
    }

    #[test]
    fn test_detect_cycle() {
        let mut graph = DepGraph::new();
        graph.add_node(DepNode::new("a"));
        graph.add_node(DepNode::new("b"));
        graph.add_node(DepNode::new("c"));
        graph.add_edge("a", "b");
        graph.add_edge("b", "c");
        graph.add_edge("c", "a");

        let cycle = graph.detect_cycle();
        assert!(cycle.is_some());
    }

    #[test]
    fn test_detect_no_cycle() {
        let mut graph = DepGraph::new();
        graph.add_node(DepNode::new("a"));
        graph.add_node(DepNode::new("b"));
        graph.add_node(DepNode::new("c"));
        graph.add_edge("c", "b");
        graph.add_edge("b", "a");

        assert!(graph.detect_cycle().is_none());
    }

    #[test]
    fn test_invalidated_by() {
        // c -> b -> a
        let mut graph = DepGraph::new();
        graph.add_node(DepNode::new("a"));
        graph.add_node(DepNode::new("b"));
        graph.add_node(DepNode::new("c"));
        graph.add_edge("c", "b");
        graph.add_edge("b", "a");

        // If a changes, b and c should be invalidated
        let changed = HashSet::from(["a".to_string()]);
        let invalidated = graph.invalidated_by(&changed);

        assert!(invalidated.contains("a"));
        assert!(invalidated.contains("b"));
        assert!(invalidated.contains("c"));
    }

    #[test]
    fn test_invalidated_by_leaf() {
        // c -> b -> a
        let mut graph = DepGraph::new();
        graph.add_node(DepNode::new("a"));
        graph.add_node(DepNode::new("b"));
        graph.add_node(DepNode::new("c"));
        graph.add_edge("c", "b");
        graph.add_edge("b", "a");

        // If c changes, only c should be invalidated (no dependents)
        let changed = HashSet::from(["c".to_string()]);
        let invalidated = graph.invalidated_by(&changed);

        assert!(invalidated.contains("c"));
        assert!(!invalidated.contains("b"));
        assert!(!invalidated.contains("a"));
    }

    #[test]
    fn test_transitive_dependencies() {
        // d -> c -> b -> a
        let mut graph = DepGraph::new();
        graph.add_node(DepNode::new("a"));
        graph.add_node(DepNode::new("b"));
        graph.add_node(DepNode::new("c"));
        graph.add_node(DepNode::new("d"));
        graph.add_edge("d", "c");
        graph.add_edge("c", "b");
        graph.add_edge("b", "a");

        let deps = graph.transitive_dependencies("d");

        assert!(deps.contains("a"));
        assert!(deps.contains("b"));
        assert!(deps.contains("c"));
        assert!(!deps.contains("d"));
    }

    #[test]
    fn test_builder() {
        let mut builder = DepGraphBuilder::new();
        builder.add_package("app");
        builder.add_package("lib");
        builder.add_import("app", "lib");
        let graph = builder.build();

        assert_eq!(graph.len(), 2);
        assert!(graph.get("app").unwrap().dependencies.contains("lib"));

        // Build order should be computed
        assert!(!graph.build_order.is_empty());
    }

    #[test]
    fn test_builder_with_fingerprint() {
        let fp = Fingerprint::new("test_hash".to_string(), FingerprintKind::Source);

        let mut builder = DepGraphBuilder::new();
        builder.add_package_with_fingerprint("app", fp.clone());
        let graph = builder.build();

        let node = graph.get("app").unwrap();
        assert_eq!(node.source_fingerprint, fp);
    }
}
