//! Project-level cache for incremental compilation.
//!
//! This module provides caching of compilation artifacts (IR, bytecode)
//! to enable fast incremental rebuilds. The cache is stored in
//! `target/.arth-cache/` within the project directory.
//!
//! Cache structure:
//! ```text
//! target/.arth-cache/
//!     index.json                 # CacheIndex
//!     vm/                        # Per-backend cache
//!         packages/
//!             app.http/
//!                 <fingerprint>.ir.bin
//!                 <fingerprint>.bc.abc
//!     llvm/
//!         packages/
//!             ...
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::fingerprint::{CompileInputs, Fingerprint, FingerprintKind};

/// Current cache format version.
/// Increment this when the cache format changes.
pub const CACHE_FORMAT_VERSION: u32 = 1;

/// Default maximum age for cache entries in days.
pub const DEFAULT_MAX_AGE_DAYS: u32 = 30;

/// Error type for cache operations.
#[derive(Debug)]
pub enum CacheError {
    /// IO error reading/writing cache files
    Io(std::io::Error),
    /// JSON serialization/deserialization error
    Json(serde_json::Error),
    /// Cache format version mismatch
    VersionMismatch { expected: u32, found: u32 },
    /// Cache directory not found or not accessible
    CacheNotFound(PathBuf),
    /// Corrupt cache entry
    CorruptEntry { package: String, reason: String },
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheError::Io(e) => write!(f, "cache IO error: {}", e),
            CacheError::Json(e) => write!(f, "cache JSON error: {}", e),
            CacheError::VersionMismatch { expected, found } => {
                write!(
                    f,
                    "cache version mismatch: expected {}, found {}",
                    expected, found
                )
            }
            CacheError::CacheNotFound(path) => {
                write!(f, "cache directory not found: {}", path.display())
            }
            CacheError::CorruptEntry { package, reason } => {
                write!(f, "corrupt cache entry for '{}': {}", package, reason)
            }
        }
    }
}

impl std::error::Error for CacheError {}

impl From<std::io::Error> for CacheError {
    fn from(e: std::io::Error) -> Self {
        CacheError::Io(e)
    }
}

impl From<serde_json::Error> for CacheError {
    fn from(e: serde_json::Error) -> Self {
        CacheError::Json(e)
    }
}

/// Index of cached compilation artifacts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CacheIndex {
    /// Cache format version
    pub version: u32,
    /// When this index was last updated (Unix timestamp)
    pub updated_at: u64,
    /// Fingerprint of build configuration
    pub config_fingerprint: String,
    /// Cached packages
    pub packages: BTreeMap<String, CachedPackage>,
}

impl Default for CacheIndex {
    fn default() -> Self {
        Self {
            version: CACHE_FORMAT_VERSION,
            updated_at: current_timestamp(),
            config_fingerprint: String::new(),
            packages: BTreeMap::new(),
        }
    }
}

impl CacheIndex {
    /// Create a new empty index with the given config fingerprint.
    pub fn new(config_fingerprint: &str) -> Self {
        Self {
            config_fingerprint: config_fingerprint.to_string(),
            ..Default::default()
        }
    }

    /// Check if the index is compatible with current format.
    pub fn is_compatible(&self) -> bool {
        self.version == CACHE_FORMAT_VERSION
    }

    /// Update the timestamp.
    pub fn touch(&mut self) {
        self.updated_at = current_timestamp();
    }
}

/// Cached compilation result for a package.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CachedPackage {
    /// Package name
    pub name: String,
    /// Fingerprint used to identify this cache entry
    pub input_fingerprint: Fingerprint,
    /// Interface fingerprint for dependency tracking
    pub interface_fingerprint: Fingerprint,
    /// When this entry was created (Unix timestamp)
    pub created_at: u64,
    /// Artifact file paths relative to cache directory
    pub artifacts: CachedArtifacts,
}

/// Paths to cached artifact files.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CachedArtifacts {
    /// IR representation (relative path)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ir: Option<String>,
    /// Bytecode (relative path)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytecode: Option<String>,
}

impl CachedArtifacts {
    /// Check if any artifacts are cached.
    pub fn is_empty(&self) -> bool {
        self.ir.is_none() && self.bytecode.is_none()
    }
}

/// Statistics from garbage collection.
#[derive(Clone, Debug, Default)]
pub struct GcStats {
    /// Number of entries removed
    pub entries_removed: usize,
    /// Bytes freed
    pub bytes_freed: u64,
    /// Number of entries kept
    pub entries_kept: usize,
}

/// Cache status for a package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageCacheStatus {
    /// Cache hit - fingerprint matches, artifacts exist
    Hit,
    /// Cache miss - no entry for this package
    Miss,
    /// Cache stale - entry exists but fingerprint doesn't match
    Stale,
    /// Cache corrupt - entry exists but artifacts missing/invalid
    Corrupt,
}

/// Project-level compilation cache.
pub struct ProjectCache {
    /// Root directory of the cache (e.g., target/.arth-cache/vm/)
    root: PathBuf,
    /// In-memory cache index
    index: CacheIndex,
    /// Whether the index has unsaved changes
    dirty: bool,
}

impl ProjectCache {
    /// Open or create a cache for the given project and backend.
    ///
    /// The cache is stored at `project_root/target/.arth-cache/<backend>/`.
    pub fn open(project_root: &Path, inputs: &CompileInputs) -> Result<Self, CacheError> {
        let cache_root = project_root
            .join("target")
            .join(".arth-cache")
            .join(&inputs.backend);

        // Ensure cache directory exists
        fs::create_dir_all(&cache_root)?;
        fs::create_dir_all(cache_root.join("packages"))?;

        // Load or create index
        let index_path = cache_root.join("index.json");
        let config_fingerprint = inputs.fingerprint().hash;

        let index = if index_path.exists() {
            let contents = fs::read_to_string(&index_path)?;
            let mut index: CacheIndex = serde_json::from_str(&contents)?;

            // Invalidate if version or config changed
            if !index.is_compatible() || index.config_fingerprint != config_fingerprint {
                CacheIndex::new(&config_fingerprint)
            } else {
                index.touch();
                index
            }
        } else {
            CacheIndex::new(&config_fingerprint)
        };

        Ok(Self {
            root: cache_root,
            index,
            dirty: false,
        })
    }

    /// Get the cache root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Get the packages directory.
    fn packages_dir(&self) -> PathBuf {
        self.root.join("packages")
    }

    /// Get the directory for a specific package.
    fn package_dir(&self, package: &str) -> PathBuf {
        self.packages_dir().join(package.replace('.', "/"))
    }

    /// Check the cache status for a package.
    pub fn status(&self, package: &str, fingerprint: &Fingerprint) -> PackageCacheStatus {
        match self.index.packages.get(package) {
            None => PackageCacheStatus::Miss,
            Some(entry) => {
                if entry.input_fingerprint.hash != fingerprint.hash {
                    PackageCacheStatus::Stale
                } else if !self.artifacts_exist(package, entry) {
                    PackageCacheStatus::Corrupt
                } else {
                    PackageCacheStatus::Hit
                }
            }
        }
    }

    /// Check if all artifacts for an entry exist on disk.
    fn artifacts_exist(&self, _package: &str, entry: &CachedPackage) -> bool {
        if let Some(ir_path) = &entry.artifacts.ir {
            if !self.root.join(ir_path).exists() {
                return false;
            }
        }
        if let Some(bc_path) = &entry.artifacts.bytecode {
            if !self.root.join(bc_path).exists() {
                return false;
            }
        }
        true
    }

    /// Get a cached package entry.
    pub fn get(&self, package: &str) -> Option<&CachedPackage> {
        self.index.packages.get(package)
    }

    /// Get a cached package if the fingerprint matches.
    pub fn get_if_valid(&self, package: &str, fingerprint: &Fingerprint) -> Option<&CachedPackage> {
        match self.status(package, fingerprint) {
            PackageCacheStatus::Hit => self.index.packages.get(package),
            _ => None,
        }
    }

    /// Insert a new cache entry.
    pub fn insert(&mut self, entry: CachedPackage) -> Result<(), CacheError> {
        // Ensure package directory exists
        let pkg_dir = self.package_dir(&entry.name);
        fs::create_dir_all(&pkg_dir)?;

        self.index.packages.insert(entry.name.clone(), entry);
        self.index.touch();
        self.dirty = true;
        Ok(())
    }

    /// Store IR artifact for a package.
    pub fn store_ir(
        &mut self,
        package: &str,
        fingerprint: &Fingerprint,
        data: &[u8],
    ) -> Result<PathBuf, CacheError> {
        let pkg_dir = self.package_dir(package);
        fs::create_dir_all(&pkg_dir)?;

        let filename = format!("{}.ir.bin", fingerprint.short());
        let path = pkg_dir.join(&filename);

        let mut file = fs::File::create(&path)?;
        file.write_all(data)?;

        // Update entry
        if let Some(entry) = self.index.packages.get_mut(package) {
            let rel_path = path.strip_prefix(&self.root).unwrap_or(&path);
            entry.artifacts.ir = Some(rel_path.to_string_lossy().to_string());
            self.dirty = true;
        }

        Ok(path)
    }

    /// Store bytecode artifact for a package.
    pub fn store_bytecode(
        &mut self,
        package: &str,
        fingerprint: &Fingerprint,
        data: &[u8],
    ) -> Result<PathBuf, CacheError> {
        let pkg_dir = self.package_dir(package);
        fs::create_dir_all(&pkg_dir)?;

        let filename = format!("{}.bc.abc", fingerprint.short());
        let path = pkg_dir.join(&filename);

        let mut file = fs::File::create(&path)?;
        file.write_all(data)?;

        // Update entry
        if let Some(entry) = self.index.packages.get_mut(package) {
            let rel_path = path.strip_prefix(&self.root).unwrap_or(&path);
            entry.artifacts.bytecode = Some(rel_path.to_string_lossy().to_string());
            self.dirty = true;
        }

        Ok(path)
    }

    /// Load IR artifact for a package.
    pub fn load_ir(&self, package: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let entry = match self.index.packages.get(package) {
            Some(e) => e,
            None => return Ok(None),
        };

        let ir_path = match &entry.artifacts.ir {
            Some(p) => self.root.join(p),
            None => return Ok(None),
        };

        if !ir_path.exists() {
            return Ok(None);
        }

        let mut file = fs::File::open(&ir_path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        Ok(Some(data))
    }

    /// Load bytecode artifact for a package.
    pub fn load_bytecode(&self, package: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let entry = match self.index.packages.get(package) {
            Some(e) => e,
            None => return Ok(None),
        };

        let bc_path = match &entry.artifacts.bytecode {
            Some(p) => self.root.join(p),
            None => return Ok(None),
        };

        if !bc_path.exists() {
            return Ok(None);
        }

        let mut file = fs::File::open(&bc_path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        Ok(Some(data))
    }

    /// Invalidate cache entries for the given packages.
    pub fn invalidate(&mut self, packages: &[String]) -> Result<(), CacheError> {
        for pkg in packages {
            if let Some(entry) = self.index.packages.remove(pkg) {
                // Remove artifact files
                if let Some(ir) = &entry.artifacts.ir {
                    let _ = fs::remove_file(self.root.join(ir));
                }
                if let Some(bc) = &entry.artifacts.bytecode {
                    let _ = fs::remove_file(self.root.join(bc));
                }
            }
        }
        self.index.touch();
        self.dirty = true;
        Ok(())
    }

    /// Remove all cache entries.
    pub fn clear(&mut self) -> Result<(), CacheError> {
        // Remove all package directories
        let pkg_dir = self.packages_dir();
        if pkg_dir.exists() {
            fs::remove_dir_all(&pkg_dir)?;
            fs::create_dir_all(&pkg_dir)?;
        }

        self.index.packages.clear();
        self.index.touch();
        self.dirty = true;
        Ok(())
    }

    /// Garbage collect old cache entries.
    pub fn gc(&mut self, max_age_days: u32) -> Result<GcStats, CacheError> {
        let now = current_timestamp();
        let max_age_secs = max_age_days as u64 * 24 * 60 * 60;
        let cutoff = now.saturating_sub(max_age_secs);

        let mut stats = GcStats::default();
        let mut to_remove = Vec::new();

        for (name, entry) in &self.index.packages {
            if entry.created_at < cutoff {
                // Calculate size of artifacts to be removed
                if let Some(ir) = &entry.artifacts.ir {
                    if let Ok(meta) = fs::metadata(self.root.join(ir)) {
                        stats.bytes_freed += meta.len();
                    }
                }
                if let Some(bc) = &entry.artifacts.bytecode {
                    if let Ok(meta) = fs::metadata(self.root.join(bc)) {
                        stats.bytes_freed += meta.len();
                    }
                }
                to_remove.push(name.clone());
            } else {
                stats.entries_kept += 1;
            }
        }

        stats.entries_removed = to_remove.len();
        self.invalidate(&to_remove)?;

        Ok(stats)
    }

    /// Get cache statistics.
    pub fn stats(&self) -> CacheStats {
        let mut total_size = 0u64;

        for entry in self.index.packages.values() {
            if let Some(ir) = &entry.artifacts.ir {
                if let Ok(meta) = fs::metadata(self.root.join(ir)) {
                    total_size += meta.len();
                }
            }
            if let Some(bc) = &entry.artifacts.bytecode {
                if let Ok(meta) = fs::metadata(self.root.join(bc)) {
                    total_size += meta.len();
                }
            }
        }

        CacheStats {
            entries: self.index.packages.len(),
            total_size,
            config_fingerprint: self.index.config_fingerprint.clone(),
            last_updated: self.index.updated_at,
        }
    }

    /// Flush the cache index to disk.
    pub fn flush(&mut self) -> Result<(), CacheError> {
        if !self.dirty {
            return Ok(());
        }

        let index_path = self.root.join("index.json");
        let json = serde_json::to_string_pretty(&self.index)?;
        fs::write(&index_path, json)?;
        self.dirty = false;
        Ok(())
    }

    /// Check if there are unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
}

impl Drop for ProjectCache {
    fn drop(&mut self) {
        // Best-effort flush on drop
        let _ = self.flush();
    }
}

/// Cache statistics.
#[derive(Clone, Debug)]
pub struct CacheStats {
    /// Number of cached entries
    pub entries: usize,
    /// Total size in bytes
    pub total_size: u64,
    /// Config fingerprint
    pub config_fingerprint: String,
    /// Last update timestamp
    pub last_updated: u64,
}

impl std::fmt::Display for CacheStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let size_str = if self.total_size >= 1024 * 1024 {
            format!("{:.1} MB", self.total_size as f64 / (1024.0 * 1024.0))
        } else if self.total_size >= 1024 {
            format!("{:.1} KB", self.total_size as f64 / 1024.0)
        } else {
            format!("{} bytes", self.total_size)
        };

        write!(f, "{} entries, {} total", self.entries, size_str)
    }
}

/// Get the current Unix timestamp.
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Get the path to the global cache directory.
pub fn global_cache_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("ARTH_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    dirs::home_dir().map(|h| h.join(".arth").join("cache"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_inputs() -> CompileInputs {
        CompileInputs::vm()
    }

    #[test]
    fn test_cache_open_creates_dirs() {
        let tmp = TempDir::new().unwrap();
        let cache = ProjectCache::open(tmp.path(), &test_inputs()).unwrap();

        assert!(cache.root().exists());
        assert!(cache.root().join("packages").exists());
    }

    #[test]
    fn test_cache_insert_and_get() {
        let tmp = TempDir::new().unwrap();
        let mut cache = ProjectCache::open(tmp.path(), &test_inputs()).unwrap();

        let entry = CachedPackage {
            name: "test.pkg".to_string(),
            input_fingerprint: Fingerprint::new("test_hash".to_string(), FingerprintKind::Package),
            interface_fingerprint: Fingerprint::new(
                "iface_hash".to_string(),
                FingerprintKind::Interface,
            ),
            created_at: current_timestamp(),
            artifacts: CachedArtifacts::default(),
        };

        cache.insert(entry.clone()).unwrap();

        let retrieved = cache.get("test.pkg").unwrap();
        assert_eq!(retrieved.name, "test.pkg");
        assert_eq!(retrieved.input_fingerprint.hash, "test_hash");
    }

    #[test]
    fn test_cache_status() {
        let tmp = TempDir::new().unwrap();
        let mut cache = ProjectCache::open(tmp.path(), &test_inputs()).unwrap();

        let fp = Fingerprint::new("hash1".to_string(), FingerprintKind::Package);

        // Initially miss
        assert_eq!(cache.status("test.pkg", &fp), PackageCacheStatus::Miss);

        // Insert entry
        let entry = CachedPackage {
            name: "test.pkg".to_string(),
            input_fingerprint: fp.clone(),
            interface_fingerprint: Fingerprint::new(
                "iface".to_string(),
                FingerprintKind::Interface,
            ),
            created_at: current_timestamp(),
            artifacts: CachedArtifacts::default(),
        };
        cache.insert(entry).unwrap();

        // Now hit (no artifacts to check)
        assert_eq!(cache.status("test.pkg", &fp), PackageCacheStatus::Hit);

        // Different fingerprint = stale
        let fp2 = Fingerprint::new("hash2".to_string(), FingerprintKind::Package);
        assert_eq!(cache.status("test.pkg", &fp2), PackageCacheStatus::Stale);
    }

    #[test]
    fn test_cache_store_and_load() {
        let tmp = TempDir::new().unwrap();
        let mut cache = ProjectCache::open(tmp.path(), &test_inputs()).unwrap();

        let fp = Fingerprint::new("hash1".to_string(), FingerprintKind::Package);
        let entry = CachedPackage {
            name: "test.pkg".to_string(),
            input_fingerprint: fp.clone(),
            interface_fingerprint: Fingerprint::new(
                "iface".to_string(),
                FingerprintKind::Interface,
            ),
            created_at: current_timestamp(),
            artifacts: CachedArtifacts::default(),
        };
        cache.insert(entry).unwrap();

        // Store IR
        let ir_data = b"test IR data";
        cache.store_ir("test.pkg", &fp, ir_data).unwrap();

        // Store bytecode
        let bc_data = b"test bytecode";
        cache.store_bytecode("test.pkg", &fp, bc_data).unwrap();

        // Load and verify
        let loaded_ir = cache.load_ir("test.pkg").unwrap().unwrap();
        assert_eq!(loaded_ir, ir_data);

        let loaded_bc = cache.load_bytecode("test.pkg").unwrap().unwrap();
        assert_eq!(loaded_bc, bc_data);
    }

    #[test]
    fn test_cache_invalidate() {
        let tmp = TempDir::new().unwrap();
        let mut cache = ProjectCache::open(tmp.path(), &test_inputs()).unwrap();

        let entry = CachedPackage {
            name: "test.pkg".to_string(),
            input_fingerprint: Fingerprint::new("hash".to_string(), FingerprintKind::Package),
            interface_fingerprint: Fingerprint::new(
                "iface".to_string(),
                FingerprintKind::Interface,
            ),
            created_at: current_timestamp(),
            artifacts: CachedArtifacts::default(),
        };
        cache.insert(entry).unwrap();

        assert!(cache.get("test.pkg").is_some());

        cache.invalidate(&["test.pkg".to_string()]).unwrap();

        assert!(cache.get("test.pkg").is_none());
    }

    #[test]
    fn test_cache_clear() {
        let tmp = TempDir::new().unwrap();
        let mut cache = ProjectCache::open(tmp.path(), &test_inputs()).unwrap();

        for i in 0..3 {
            let entry = CachedPackage {
                name: format!("pkg{}", i),
                input_fingerprint: Fingerprint::new(format!("hash{}", i), FingerprintKind::Package),
                interface_fingerprint: Fingerprint::new(
                    "iface".to_string(),
                    FingerprintKind::Interface,
                ),
                created_at: current_timestamp(),
                artifacts: CachedArtifacts::default(),
            };
            cache.insert(entry).unwrap();
        }

        assert_eq!(cache.stats().entries, 3);

        cache.clear().unwrap();

        assert_eq!(cache.stats().entries, 0);
    }

    #[test]
    fn test_cache_flush_and_reload() {
        let tmp = TempDir::new().unwrap();
        let inputs = test_inputs();

        // Create and populate cache
        {
            let mut cache = ProjectCache::open(tmp.path(), &inputs).unwrap();
            let entry = CachedPackage {
                name: "test.pkg".to_string(),
                input_fingerprint: Fingerprint::new("hash".to_string(), FingerprintKind::Package),
                interface_fingerprint: Fingerprint::new(
                    "iface".to_string(),
                    FingerprintKind::Interface,
                ),
                created_at: current_timestamp(),
                artifacts: CachedArtifacts::default(),
            };
            cache.insert(entry).unwrap();
            cache.flush().unwrap();
        }

        // Reload and verify
        {
            let cache = ProjectCache::open(tmp.path(), &inputs).unwrap();
            assert!(cache.get("test.pkg").is_some());
        }
    }

    #[test]
    fn test_cache_stats() {
        let tmp = TempDir::new().unwrap();
        let mut cache = ProjectCache::open(tmp.path(), &test_inputs()).unwrap();

        let fp = Fingerprint::new("hash".to_string(), FingerprintKind::Package);
        let entry = CachedPackage {
            name: "test.pkg".to_string(),
            input_fingerprint: fp.clone(),
            interface_fingerprint: Fingerprint::new(
                "iface".to_string(),
                FingerprintKind::Interface,
            ),
            created_at: current_timestamp(),
            artifacts: CachedArtifacts::default(),
        };
        cache.insert(entry).unwrap();

        // Store some data
        cache.store_ir("test.pkg", &fp, b"test data").unwrap();

        let stats = cache.stats();
        assert_eq!(stats.entries, 1);
        assert!(stats.total_size > 0);
    }

    #[test]
    fn test_cache_index_serialization() {
        let index = CacheIndex::new("test_config");
        let json = serde_json::to_string_pretty(&index).unwrap();
        let parsed: CacheIndex = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.version, CACHE_FORMAT_VERSION);
        assert_eq!(parsed.config_fingerprint, "test_config");
    }

    #[test]
    fn test_config_change_invalidates_cache() {
        let tmp = TempDir::new().unwrap();

        // Create cache with VM backend
        {
            let mut cache = ProjectCache::open(tmp.path(), &CompileInputs::vm()).unwrap();
            let entry = CachedPackage {
                name: "test.pkg".to_string(),
                input_fingerprint: Fingerprint::new("hash".to_string(), FingerprintKind::Package),
                interface_fingerprint: Fingerprint::new(
                    "iface".to_string(),
                    FingerprintKind::Interface,
                ),
                created_at: current_timestamp(),
                artifacts: CachedArtifacts::default(),
            };
            cache.insert(entry).unwrap();
            cache.flush().unwrap();
        }

        // Open with different config (LLVM backend) - should get fresh cache
        // Note: LLVM uses different directory, so this is actually independent
        let cache = ProjectCache::open(tmp.path(), &CompileInputs::llvm()).unwrap();
        // Different backend = different directory, so empty cache
        assert!(cache.get("test.pkg").is_none());
    }
}
