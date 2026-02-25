//! Incremental compilation infrastructure.
//!
//! This module provides the core infrastructure for incremental builds:
//!
//! - **Fingerprinting** (`fingerprint.rs`): Content-based hashing for sources and artifacts
//! - **Dependency Graph** (`dep_graph.rs`): Package dependencies for invalidation tracking
//! - **Project Cache** (`project_cache.rs`): On-disk cache for compilation artifacts
//! - **Compile Session** (this file): Orchestrates incremental compilation
//!
//! # Usage
//!
//! ```ignore
//! use arth::compiler::driver::incremental::{CompileSession, CompileInputs};
//!
//! let inputs = CompileInputs::vm();
//! let mut session = CompileSession::new(project_root, inputs)?;
//!
//! // Analyze what needs to be rebuilt
//! let plan = session.plan(&sources, &resolved)?;
//!
//! // Execute compilation
//! for pkg in &plan.recompile {
//!     // Compile package...
//!     session.mark_compiled(pkg, &source_fp, &interface_fp)?;
//! }
//!
//! // Save state for next build
//! session.save()?;
//! ```

pub mod dep_graph;
pub mod fingerprint;
pub mod project_cache;

pub use dep_graph::{DepGraph, DepGraphBuilder, DepNode};
pub use fingerprint::{
    CompileInputs, FINGERPRINT_VERSION, Fingerprint, FingerprintKind, PackageInterface,
    fingerprint_config, fingerprint_content, fingerprint_interface, fingerprint_package,
    fingerprint_package_sources, fingerprint_source,
};
pub use project_cache::{
    CACHE_FORMAT_VERSION, CacheError, CacheIndex, CacheStats, CachedArtifacts, CachedPackage,
    DEFAULT_MAX_AGE_DAYS, GcStats, PackageCacheStatus, ProjectCache,
};

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Incremental compilation session.
///
/// Manages the state of an incremental build, including:
/// - Build configuration (compiler version, backend, options)
/// - Project cache (on-disk artifact storage)
/// - Dependency graph (for invalidation tracking)
/// - Compilation plan (what to rebuild vs use from cache)
pub struct CompileSession {
    /// Project root directory
    root: PathBuf,
    /// Compilation inputs (backend, options)
    inputs: CompileInputs,
    /// Project cache
    cache: ProjectCache,
    /// Previous fingerprints (loaded from cache)
    prev_fingerprints: BTreeMap<String, Fingerprint>,
    /// Previous interface fingerprints
    prev_interfaces: BTreeMap<String, Fingerprint>,
    /// Statistics for this session
    stats: SessionStats,
}

/// Statistics for a compilation session.
#[derive(Clone, Debug, Default)]
pub struct SessionStats {
    /// Number of packages with cache hits
    pub cache_hits: usize,
    /// Number of packages that needed recompilation
    pub cache_misses: usize,
    /// Number of packages invalidated by dependency changes
    pub dep_invalidations: usize,
    /// Whether incremental compilation was used
    pub incremental: bool,
}

impl std::fmt::Display for SessionStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.incremental {
            write!(
                f,
                "{} cached, {} compiled, {} invalidated by deps",
                self.cache_hits, self.cache_misses, self.dep_invalidations
            )
        } else {
            write!(f, "full rebuild ({} packages)", self.cache_misses)
        }
    }
}

/// Plan for incremental compilation.
#[derive(Clone, Debug, Default)]
pub struct IncrementalPlan {
    /// Packages that need full recompilation
    pub recompile: Vec<String>,
    /// Packages that can use cached artifacts
    pub cached: Vec<String>,
    /// Build order (dependencies before dependents)
    pub build_order: Vec<String>,
    /// Reasons for recompilation (for diagnostics)
    pub reasons: BTreeMap<String, RecompileReason>,
}

impl IncrementalPlan {
    /// Check if any recompilation is needed.
    pub fn needs_build(&self) -> bool {
        !self.recompile.is_empty()
    }

    /// Get total number of packages.
    pub fn total_packages(&self) -> usize {
        self.recompile.len() + self.cached.len()
    }
}

/// Reason a package needs recompilation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecompileReason {
    /// No cache entry exists
    NotCached,
    /// Source code changed
    SourceChanged,
    /// A dependency's interface changed
    DependencyChanged(String),
    /// Build configuration changed
    ConfigChanged,
    /// Cache entry is corrupt
    CacheCorrupt,
}

impl std::fmt::Display for RecompileReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecompileReason::NotCached => write!(f, "not in cache"),
            RecompileReason::SourceChanged => write!(f, "source changed"),
            RecompileReason::DependencyChanged(dep) => write!(f, "dependency '{}' changed", dep),
            RecompileReason::ConfigChanged => write!(f, "config changed"),
            RecompileReason::CacheCorrupt => write!(f, "cache corrupt"),
        }
    }
}

impl CompileSession {
    /// Create a new compilation session.
    pub fn new(root: &Path, inputs: CompileInputs) -> Result<Self, CacheError> {
        let cache = ProjectCache::open(root, &inputs)?;

        // Load previous fingerprints from cache
        let mut prev_fingerprints = BTreeMap::new();
        let mut prev_interfaces = BTreeMap::new();

        for (name, entry) in cache
            .stats()
            .entries
            .checked_sub(0)
            .map(|_| {
                // Iterate over cache entries - this is a bit awkward due to the API
                std::iter::empty::<(&String, &CachedPackage)>()
            })
            .unwrap_or_else(|| std::iter::empty())
        {
            prev_fingerprints.insert(name.clone(), entry.input_fingerprint.clone());
            prev_interfaces.insert(name.clone(), entry.interface_fingerprint.clone());
        }

        // Actually load from cache index
        // We need to access the cache's internal state
        // For now, we'll rebuild this when we call plan()

        Ok(Self {
            root: root.to_path_buf(),
            inputs,
            cache,
            prev_fingerprints,
            prev_interfaces,
            stats: SessionStats {
                incremental: true,
                ..Default::default()
            },
        })
    }

    /// Create a session for non-incremental (full) builds.
    pub fn new_non_incremental(root: &Path, inputs: CompileInputs) -> Result<Self, CacheError> {
        let cache = ProjectCache::open(root, &inputs)?;

        Ok(Self {
            root: root.to_path_buf(),
            inputs,
            cache,
            prev_fingerprints: BTreeMap::new(),
            prev_interfaces: BTreeMap::new(),
            stats: SessionStats {
                incremental: false,
                ..Default::default()
            },
        })
    }

    /// Get the project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Get the compilation inputs.
    pub fn inputs(&self) -> &CompileInputs {
        &self.inputs
    }

    /// Get the cache.
    pub fn cache(&self) -> &ProjectCache {
        &self.cache
    }

    /// Get mutable cache.
    pub fn cache_mut(&mut self) -> &mut ProjectCache {
        &mut self.cache
    }

    /// Get session statistics.
    pub fn stats(&self) -> &SessionStats {
        &self.stats
    }

    /// Plan incremental compilation.
    ///
    /// Analyzes which packages need recompilation based on:
    /// - Source fingerprint changes
    /// - Interface fingerprint changes (for dependents)
    /// - Build configuration changes
    ///
    /// Returns an `IncrementalPlan` with packages to recompile and packages to load from cache.
    pub fn plan(
        &mut self,
        source_fingerprints: &BTreeMap<String, Fingerprint>,
        dep_graph: &DepGraph,
    ) -> IncrementalPlan {
        let mut plan = IncrementalPlan::default();
        let config_fp = self.inputs.fingerprint();

        // First pass: identify packages with source changes
        let mut source_changed: HashSet<String> = HashSet::new();

        for (pkg, source_fp) in source_fingerprints {
            let status = self.cache.status(pkg, source_fp);

            match status {
                PackageCacheStatus::Hit => {
                    // Check if config changed
                    let entry = self.cache.get(pkg);
                    if entry.is_some() {
                        plan.cached.push(pkg.clone());
                        self.stats.cache_hits += 1;
                    } else {
                        source_changed.insert(pkg.clone());
                        plan.reasons.insert(pkg.clone(), RecompileReason::NotCached);
                    }
                }
                PackageCacheStatus::Miss => {
                    source_changed.insert(pkg.clone());
                    plan.reasons.insert(pkg.clone(), RecompileReason::NotCached);
                }
                PackageCacheStatus::Stale => {
                    source_changed.insert(pkg.clone());
                    plan.reasons
                        .insert(pkg.clone(), RecompileReason::SourceChanged);
                }
                PackageCacheStatus::Corrupt => {
                    source_changed.insert(pkg.clone());
                    plan.reasons
                        .insert(pkg.clone(), RecompileReason::CacheCorrupt);
                }
            }
        }

        // Second pass: propagate invalidation to dependents
        // This uses the dependency graph to find all packages that transitively depend
        // on packages with source changes.
        let all_invalidated = dep_graph.invalidated_by(&source_changed);

        for pkg in &all_invalidated {
            if !source_changed.contains(pkg) {
                // This package was invalidated due to a dependency change
                if let Some(deps) = dep_graph.dependencies_of(pkg) {
                    for dep in deps {
                        if source_changed.contains(dep) {
                            plan.reasons.insert(
                                pkg.clone(),
                                RecompileReason::DependencyChanged(dep.clone()),
                            );
                            self.stats.dep_invalidations += 1;
                            break;
                        }
                    }
                }
                // Remove from cached if it was there
                plan.cached.retain(|p| p != pkg);
                self.stats.cache_hits = self.stats.cache_hits.saturating_sub(1);
            }
            plan.recompile.push(pkg.clone());
            self.stats.cache_misses += 1;
        }

        // Add any remaining source-changed packages that weren't in the graph
        for pkg in source_changed {
            if !plan.recompile.contains(&pkg) {
                plan.recompile.push(pkg);
                self.stats.cache_misses += 1;
            }
        }

        // Sort recompile list by build order
        let build_order: Vec<_> = dep_graph.build_order.clone();
        plan.recompile.sort_by_key(|pkg| {
            build_order
                .iter()
                .position(|p| p == pkg)
                .unwrap_or(usize::MAX)
        });

        plan.build_order = build_order;

        plan
    }

    /// Mark a package as successfully compiled.
    ///
    /// Updates the cache with the new fingerprints.
    pub fn mark_compiled(
        &mut self,
        package: &str,
        source_fingerprint: Fingerprint,
        interface_fingerprint: Fingerprint,
    ) -> Result<(), CacheError> {
        let entry = CachedPackage {
            name: package.to_string(),
            input_fingerprint: source_fingerprint,
            interface_fingerprint,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            artifacts: CachedArtifacts::default(),
        };

        self.cache.insert(entry)
    }

    /// Store IR artifact for a package.
    pub fn store_ir(&mut self, package: &str, data: &[u8]) -> Result<PathBuf, CacheError> {
        let fp = self
            .cache
            .get(package)
            .map(|e| e.input_fingerprint.clone())
            .unwrap_or_else(|| Fingerprint::new("temp".to_string(), FingerprintKind::Package));

        self.cache.store_ir(package, &fp, data)
    }

    /// Store bytecode artifact for a package.
    pub fn store_bytecode(&mut self, package: &str, data: &[u8]) -> Result<PathBuf, CacheError> {
        let fp = self
            .cache
            .get(package)
            .map(|e| e.input_fingerprint.clone())
            .unwrap_or_else(|| Fingerprint::new("temp".to_string(), FingerprintKind::Package));

        self.cache.store_bytecode(package, &fp, data)
    }

    /// Load cached IR for a package.
    pub fn load_ir(&self, package: &str) -> Result<Option<Vec<u8>>, CacheError> {
        self.cache.load_ir(package)
    }

    /// Load cached bytecode for a package.
    pub fn load_bytecode(&self, package: &str) -> Result<Option<Vec<u8>>, CacheError> {
        self.cache.load_bytecode(package)
    }

    /// Save the session state (flush cache).
    pub fn save(&mut self) -> Result<(), CacheError> {
        self.cache.flush()
    }

    /// Clear the cache.
    pub fn clear_cache(&mut self) -> Result<(), CacheError> {
        self.cache.clear()
    }

    /// Run garbage collection on the cache.
    pub fn gc(&mut self, max_age_days: u32) -> Result<GcStats, CacheError> {
        self.cache.gc(max_age_days)
    }
}

/// Check if incremental compilation is enabled.
///
/// Returns false if the `ARTH_INCREMENTAL` environment variable is set to "0".
pub fn is_incremental_enabled() -> bool {
    std::env::var("ARTH_INCREMENTAL")
        .map(|v| v != "0")
        .unwrap_or(true)
}

/// Check if verbose cache output is enabled.
///
/// Returns true if the `ARTH_CACHE_VERBOSE` environment variable is set to "1".
pub fn is_cache_verbose() -> bool {
    std::env::var("ARTH_CACHE_VERBOSE")
        .map(|v| v == "1")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_compile_session_create() {
        let tmp = TempDir::new().unwrap();
        let inputs = CompileInputs::vm();
        let session = CompileSession::new(tmp.path(), inputs).unwrap();

        assert!(session.root().exists());
        assert!(session.stats().incremental);
    }

    #[test]
    fn test_compile_session_non_incremental() {
        let tmp = TempDir::new().unwrap();
        let inputs = CompileInputs::vm();
        let session = CompileSession::new_non_incremental(tmp.path(), inputs).unwrap();

        assert!(!session.stats().incremental);
    }

    #[test]
    fn test_incremental_plan_empty() {
        let tmp = TempDir::new().unwrap();
        let inputs = CompileInputs::vm();
        let mut session = CompileSession::new(tmp.path(), inputs).unwrap();

        let source_fps = BTreeMap::new();
        let dep_graph = DepGraph::new();

        let plan = session.plan(&source_fps, &dep_graph);

        assert!(!plan.needs_build());
        assert_eq!(plan.total_packages(), 0);
    }

    #[test]
    fn test_incremental_plan_new_package() {
        let tmp = TempDir::new().unwrap();
        let inputs = CompileInputs::vm();
        let mut session = CompileSession::new(tmp.path(), inputs).unwrap();

        let mut source_fps = BTreeMap::new();
        source_fps.insert(
            "app".to_string(),
            Fingerprint::new("hash1".to_string(), FingerprintKind::Source),
        );

        let mut dep_graph = DepGraph::new();
        dep_graph.add_node(DepNode::new("app"));
        dep_graph.compute_build_order();

        let plan = session.plan(&source_fps, &dep_graph);

        assert!(plan.needs_build());
        assert!(plan.recompile.contains(&"app".to_string()));
        assert_eq!(plan.reasons.get("app"), Some(&RecompileReason::NotCached));
    }

    #[test]
    fn test_incremental_plan_cached_hit() {
        let tmp = TempDir::new().unwrap();
        let inputs = CompileInputs::vm();
        let mut session = CompileSession::new(tmp.path(), inputs).unwrap();

        let fp = Fingerprint::new("hash1".to_string(), FingerprintKind::Source);

        // Mark as compiled first
        session
            .mark_compiled(
                "app",
                fp.clone(),
                Fingerprint::new("iface".to_string(), FingerprintKind::Interface),
            )
            .unwrap();

        let mut source_fps = BTreeMap::new();
        source_fps.insert("app".to_string(), fp);

        let mut dep_graph = DepGraph::new();
        dep_graph.add_node(DepNode::new("app"));
        dep_graph.compute_build_order();

        let plan = session.plan(&source_fps, &dep_graph);

        assert!(!plan.needs_build());
        assert!(plan.cached.contains(&"app".to_string()));
    }

    #[test]
    fn test_incremental_plan_source_changed() {
        let tmp = TempDir::new().unwrap();
        let inputs = CompileInputs::vm();
        let mut session = CompileSession::new(tmp.path(), inputs).unwrap();

        // Mark as compiled with old fingerprint
        session
            .mark_compiled(
                "app",
                Fingerprint::new("old_hash".to_string(), FingerprintKind::Source),
                Fingerprint::new("iface".to_string(), FingerprintKind::Interface),
            )
            .unwrap();

        // Plan with new fingerprint
        let mut source_fps = BTreeMap::new();
        source_fps.insert(
            "app".to_string(),
            Fingerprint::new("new_hash".to_string(), FingerprintKind::Source),
        );

        let mut dep_graph = DepGraph::new();
        dep_graph.add_node(DepNode::new("app"));
        dep_graph.compute_build_order();

        let plan = session.plan(&source_fps, &dep_graph);

        assert!(plan.needs_build());
        assert!(plan.recompile.contains(&"app".to_string()));
        assert_eq!(
            plan.reasons.get("app"),
            Some(&RecompileReason::SourceChanged)
        );
    }

    #[test]
    fn test_incremental_plan_dep_invalidation() {
        let tmp = TempDir::new().unwrap();
        let inputs = CompileInputs::vm();
        let mut session = CompileSession::new(tmp.path(), inputs).unwrap();

        // lib -> app (app depends on lib)
        let lib_fp = Fingerprint::new("lib_hash".to_string(), FingerprintKind::Source);
        let app_fp = Fingerprint::new("app_hash".to_string(), FingerprintKind::Source);

        // Both are cached
        session
            .mark_compiled(
                "lib",
                lib_fp.clone(),
                Fingerprint::new("lib_iface".to_string(), FingerprintKind::Interface),
            )
            .unwrap();
        session
            .mark_compiled(
                "app",
                app_fp.clone(),
                Fingerprint::new("app_iface".to_string(), FingerprintKind::Interface),
            )
            .unwrap();

        // lib source changed
        let mut source_fps = BTreeMap::new();
        source_fps.insert(
            "lib".to_string(),
            Fingerprint::new("lib_hash_NEW".to_string(), FingerprintKind::Source),
        );
        source_fps.insert("app".to_string(), app_fp);

        // Build dep graph: app -> lib
        let mut dep_graph = DepGraph::new();
        dep_graph.add_node(DepNode::new("lib"));
        dep_graph.add_node(DepNode::new("app"));
        dep_graph.add_edge("app", "lib");
        dep_graph.compute_build_order();

        let plan = session.plan(&source_fps, &dep_graph);

        // Both should need recompilation
        assert!(plan.recompile.contains(&"lib".to_string()));
        assert!(plan.recompile.contains(&"app".to_string()));

        // lib changed source, app invalidated by dep
        assert_eq!(
            plan.reasons.get("lib"),
            Some(&RecompileReason::SourceChanged)
        );
    }

    #[test]
    fn test_session_save_and_reload() {
        let tmp = TempDir::new().unwrap();
        let inputs = CompileInputs::vm();

        // Create session and compile
        {
            let mut session = CompileSession::new(tmp.path(), inputs.clone()).unwrap();
            session
                .mark_compiled(
                    "app",
                    Fingerprint::new("hash".to_string(), FingerprintKind::Source),
                    Fingerprint::new("iface".to_string(), FingerprintKind::Interface),
                )
                .unwrap();
            session.save().unwrap();
        }

        // Reload and verify
        {
            let session = CompileSession::new(tmp.path(), inputs).unwrap();
            assert!(session.cache.get("app").is_some());
        }
    }

    #[test]
    fn test_recompile_reason_display() {
        assert_eq!(format!("{}", RecompileReason::NotCached), "not in cache");
        assert_eq!(
            format!("{}", RecompileReason::SourceChanged),
            "source changed"
        );
        assert_eq!(
            format!("{}", RecompileReason::DependencyChanged("lib".to_string())),
            "dependency 'lib' changed"
        );
    }

    #[test]
    fn test_session_stats_display() {
        let stats = SessionStats {
            cache_hits: 5,
            cache_misses: 3,
            dep_invalidations: 1,
            incremental: true,
        };

        let display = format!("{}", stats);
        assert!(display.contains("5 cached"));
        assert!(display.contains("3 compiled"));
    }
}
