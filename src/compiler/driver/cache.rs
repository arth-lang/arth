//! Stdlib caching infrastructure.
//!
//! This module provides caching for stdlib parsing and HIR lowering to speed up
//! compilation. The cache is invalidated when:
//! - Stdlib source files change (content hash mismatch)
//! - Compiler version changes
//!
//! # Cache Location
//!
//! The cache is stored in:
//! 1. `$ARTH_CACHE_DIR/stdlib/` if set
//! 2. `~/.arth/cache/stdlib/` otherwise
//!
//! # Cache Format
//!
//! The cache stores:
//! - `metadata.json` - version info and source hashes
//! - `hir_cache.bin` - serialized HIR files (future)

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Current stdlib cache format version.
/// Increment this when the cache format changes.
pub const CACHE_FORMAT_VERSION: u32 = 1;

/// Compiler version for cache compatibility.
pub const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Stdlib version constant.
/// This should match the version in stdlib sources.
pub const STDLIB_VERSION: &str = "0.1.0";

/// Metadata for the stdlib cache.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheMetadata {
    /// Cache format version
    pub format_version: u32,
    /// Compiler version that created this cache
    pub compiler_version: String,
    /// Stdlib version
    pub stdlib_version: String,
    /// Hash of all stdlib source files
    pub source_hash: String,
    /// Timestamp of cache creation
    pub created_at: u64,
    /// Map of file paths to their individual hashes
    pub file_hashes: BTreeMap<String, String>,
}

impl CacheMetadata {
    /// Create new metadata for the current stdlib state.
    pub fn new(source_hash: String, file_hashes: BTreeMap<String, String>) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self {
            format_version: CACHE_FORMAT_VERSION,
            compiler_version: COMPILER_VERSION.to_string(),
            stdlib_version: STDLIB_VERSION.to_string(),
            source_hash,
            created_at,
            file_hashes,
        }
    }

    /// Check if this metadata is compatible with the current compiler.
    pub fn is_compatible(&self) -> bool {
        self.format_version == CACHE_FORMAT_VERSION
            && self.compiler_version == COMPILER_VERSION
            && self.stdlib_version == STDLIB_VERSION
    }
}

/// Get the cache directory path.
pub fn get_cache_dir() -> Option<PathBuf> {
    // Check environment variable first
    if let Ok(dir) = std::env::var("ARTH_CACHE_DIR") {
        let path = PathBuf::from(dir).join("stdlib");
        return Some(path);
    }

    // Fall back to ~/.arth/cache/stdlib
    dirs::home_dir().map(|h| h.join(".arth").join("cache").join("stdlib"))
}

/// Ensure the cache directory exists.
pub fn ensure_cache_dir() -> Option<PathBuf> {
    let dir = get_cache_dir()?;
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Compute SHA-256 hash of file contents.
pub fn hash_file_contents(contents: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(contents.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

/// Compute combined hash of all stdlib source files.
pub fn compute_stdlib_hash(sources: &[(PathBuf, String)]) -> (String, BTreeMap<String, String>) {
    let mut hasher = Sha256::new();
    let mut file_hashes = BTreeMap::new();

    // Sort by path for deterministic hashing
    let mut sorted: Vec<_> = sources.iter().collect();
    sorted.sort_by_key(|(p, _)| p);

    for (path, contents) in sorted {
        let file_hash = hash_file_contents(contents);
        let path_str = path.to_string_lossy().to_string();
        hasher.update(path_str.as_bytes());
        hasher.update(b":");
        hasher.update(file_hash.as_bytes());
        hasher.update(b"\n");
        file_hashes.insert(path_str, file_hash);
    }

    let combined = hex::encode(hasher.finalize());
    (combined, file_hashes)
}

/// Load stdlib sources and compute their hash.
pub fn load_stdlib_with_hash(
    stdlib_path: &Path,
) -> std::io::Result<(Vec<(PathBuf, String)>, String, BTreeMap<String, String>)> {
    let mut sources = Vec::new();
    let mut stack = vec![stdlib_path.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "arth") {
                let contents = fs::read_to_string(&path)?;
                sources.push((path, contents));
            }
        }
    }

    let (hash, file_hashes) = compute_stdlib_hash(&sources);
    Ok((sources, hash, file_hashes))
}

/// Load cache metadata from disk.
pub fn load_cache_metadata() -> Option<CacheMetadata> {
    let cache_dir = get_cache_dir()?;
    let metadata_path = cache_dir.join("metadata.json");
    let contents = fs::read_to_string(&metadata_path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Save cache metadata to disk.
pub fn save_cache_metadata(metadata: &CacheMetadata) -> Option<()> {
    let cache_dir = ensure_cache_dir()?;
    let metadata_path = cache_dir.join("metadata.json");
    let contents = serde_json::to_string_pretty(metadata).ok()?;
    fs::write(&metadata_path, contents).ok()
}

/// Check if the cache is valid for the current stdlib.
pub fn is_cache_valid(current_hash: &str) -> bool {
    let Some(metadata) = load_cache_metadata() else {
        return false;
    };

    metadata.is_compatible() && metadata.source_hash == current_hash
}

/// Invalidate the cache by removing all cached files.
pub fn invalidate_cache() -> Option<()> {
    let cache_dir = get_cache_dir()?;
    if cache_dir.exists() {
        fs::remove_dir_all(&cache_dir).ok()?;
    }
    Some(())
}

/// Cache status for reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    /// Cache hit - metadata valid and hashes match
    Hit,
    /// Cache miss - no cache exists
    Miss,
    /// Cache stale - cache exists but hashes don't match
    Stale,
    /// Cache incompatible - format/compiler version mismatch
    Incompatible,
}

/// Check cache status for the current stdlib.
pub fn check_cache_status(current_hash: &str) -> CacheStatus {
    let Some(metadata) = load_cache_metadata() else {
        return CacheStatus::Miss;
    };

    if !metadata.is_compatible() {
        return CacheStatus::Incompatible;
    }

    if metadata.source_hash != current_hash {
        return CacheStatus::Stale;
    }

    CacheStatus::Hit
}

/// Print cache info for debugging.
pub fn print_cache_info() {
    println!("Arth Stdlib Cache Info:");
    println!("  Compiler version: {}", COMPILER_VERSION);
    println!("  Stdlib version: {}", STDLIB_VERSION);
    println!("  Cache format: v{}", CACHE_FORMAT_VERSION);

    if let Some(cache_dir) = get_cache_dir() {
        println!("  Cache directory: {}", cache_dir.display());
        if cache_dir.exists() {
            if let Some(metadata) = load_cache_metadata() {
                println!("  Cache status: exists");
                println!("    Created: {}", metadata.created_at);
                println!("    Compiler: {}", metadata.compiler_version);
                println!("    Stdlib: {}", metadata.stdlib_version);
                println!("    Source hash: {}...", &metadata.source_hash[..16]);
                println!("    Files cached: {}", metadata.file_hashes.len());
            } else {
                println!("  Cache status: corrupted");
            }
        } else {
            println!("  Cache status: not found");
        }
    } else {
        println!("  Cache directory: not available");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_file_contents() {
        let hash1 = hash_file_contents("hello world");
        let hash2 = hash_file_contents("hello world");
        let hash3 = hash_file_contents("hello world!");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 64); // SHA-256 produces 64 hex chars
    }

    #[test]
    fn test_compute_stdlib_hash() {
        let sources = vec![
            (PathBuf::from("a.arth"), "package a;".to_string()),
            (PathBuf::from("b.arth"), "package b;".to_string()),
        ];

        let (hash1, files1) = compute_stdlib_hash(&sources);

        // Same content should produce same hash
        let (hash2, _) = compute_stdlib_hash(&sources);
        assert_eq!(hash1, hash2);

        // Different content should produce different hash
        let sources2 = vec![
            (
                PathBuf::from("a.arth"),
                "package a; // modified".to_string(),
            ),
            (PathBuf::from("b.arth"), "package b;".to_string()),
        ];
        let (hash3, _) = compute_stdlib_hash(&sources2);
        assert_ne!(hash1, hash3);

        // Individual file hashes should be present
        assert_eq!(files1.len(), 2);
    }

    #[test]
    fn test_cache_metadata_compatibility() {
        let mut meta = CacheMetadata::new("hash".to_string(), Default::default());
        assert!(meta.is_compatible());

        // Wrong format version
        meta.format_version = 999;
        assert!(!meta.is_compatible());
        meta.format_version = CACHE_FORMAT_VERSION;

        // Wrong compiler version
        meta.compiler_version = "0.0.0".to_string();
        assert!(!meta.is_compatible());
    }
}
