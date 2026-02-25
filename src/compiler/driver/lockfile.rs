//! Lockfile Generation
//!
//! Generates `arth.lock.json` from `arth.toml` manifest and resolved dependencies.
//! Includes SHA-256 checksums for bytecode verification.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::config::{
    DependencySpec, DetailedDependency, LockDependency, LockMetadata, LockPackage, Lockfile,
    Manifest,
};
use super::extlib::{
    ExternalPackage, ExtlibError, discover_external_packages, resolve_dependencies,
};

// ============================================================================
// Lockfile Generation
// ============================================================================

/// Current lockfile format version
pub const LOCKFILE_VERSION: i64 = 1;

/// Error type for lockfile generation
#[derive(Debug)]
pub enum LockfileGenError {
    /// Error resolving dependencies
    Resolution(ExtlibError),
    /// Error computing checksum
    Checksum { path: String, error: String },
    /// IO error
    Io(std::io::Error),
}

impl std::fmt::Display for LockfileGenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockfileGenError::Resolution(e) => write!(f, "dependency resolution error: {}", e),
            LockfileGenError::Checksum { path, error } => {
                write!(f, "checksum error for '{}': {}", path, error)
            }
            LockfileGenError::Io(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for LockfileGenError {}

impl From<ExtlibError> for LockfileGenError {
    fn from(e: ExtlibError) -> Self {
        LockfileGenError::Resolution(e)
    }
}

impl From<std::io::Error> for LockfileGenError {
    fn from(e: std::io::Error) -> Self {
        LockfileGenError::Io(e)
    }
}

/// Options for lockfile generation
#[derive(Debug, Clone, Default)]
pub struct LockfileOptions {
    /// Include checksums in the lockfile (default: true)
    pub include_checksums: bool,
    /// Include source registry URLs (default: true)
    pub include_sources: bool,
    /// Include Arth compiler version in metadata (default: true)
    pub include_arth_version: bool,
}

impl LockfileOptions {
    /// Create options with all features enabled
    pub fn full() -> Self {
        Self {
            include_checksums: true,
            include_sources: true,
            include_arth_version: true,
        }
    }

    /// Create minimal options (no checksums or sources)
    pub fn minimal() -> Self {
        Self {
            include_checksums: false,
            include_sources: false,
            include_arth_version: false,
        }
    }
}

/// Generate a lockfile from a manifest
///
/// This resolves all dependencies and creates a lockfile with:
/// - Exact resolved versions for all dependencies
/// - SHA-256 checksums of bytecode files
/// - Source registry information
/// - Transitive dependency information
pub fn generate_lockfile(
    manifest: &Manifest,
    options: &LockfileOptions,
) -> Result<Lockfile, LockfileGenError> {
    // Discover available packages
    let index = discover_external_packages()?;

    // Resolve dependencies
    let resolved = resolve_dependencies(manifest, &index)?;

    // Build dependency map
    let mut dependencies = BTreeMap::new();

    for pkg in &resolved {
        let checksum = if options.include_checksums {
            compute_checksum(&pkg.bytecode_path)?
        } else {
            None
        };

        let source = if options.include_sources {
            Some(get_package_source(pkg))
        } else {
            None
        };

        // Get features from manifest if this is a direct dependency
        let features = get_dependency_features(&manifest.dependencies, &pkg.id);

        let lock_dep = LockDependency {
            version: pkg.version.to_string(),
            checksum,
            source,
            features,
            dependencies: pkg.dependencies.clone(),
        };

        dependencies.insert(pkg.id.clone(), lock_dep);
    }

    // Build metadata
    let metadata = LockMetadata {
        lockfile_version: Some(LOCKFILE_VERSION),
        resolved_at: Some(get_timestamp()),
        arth_version: if options.include_arth_version {
            Some(get_arth_version())
        } else {
            None
        },
    };

    Ok(Lockfile {
        package: LockPackage {
            name: manifest.package.name.clone(),
            version: manifest.package.version.clone(),
        },
        dependencies,
        source_fingerprints: BTreeMap::new(), // Populated during build
        metadata,
    })
}

/// Generate a lockfile and write it to the project directory
pub fn generate_and_write_lockfile(
    project_dir: &Path,
    manifest: &Manifest,
    options: &LockfileOptions,
) -> Result<Lockfile, LockfileGenError> {
    let lockfile = generate_lockfile(manifest, options)?;
    let lockfile_path = project_dir.join("arth.lock.json");

    let json = serde_json::to_string_pretty(&lockfile)
        .map_err(|e| LockfileGenError::Io(std::io::Error::other(e.to_string())))?;

    fs::write(&lockfile_path, json)?;

    Ok(lockfile)
}

// ============================================================================
// Checksum Computation
// ============================================================================

/// Compute SHA-256 checksum of a file
///
/// Returns the checksum in the format "sha256-<hex>"
pub fn compute_checksum(path: &Path) -> Result<Option<String>, LockfileGenError> {
    if !path.exists() {
        return Ok(None);
    }

    let mut file = fs::File::open(path).map_err(|e| LockfileGenError::Checksum {
        path: path.display().to_string(),
        error: e.to_string(),
    })?;

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = file
            .read(&mut buffer)
            .map_err(|e| LockfileGenError::Checksum {
                path: path.display().to_string(),
                error: e.to_string(),
            })?;

        if bytes_read == 0 {
            break;
        }

        hasher.update(&buffer[..bytes_read]);
    }

    let hash = hasher.finalize();
    Ok(Some(format!("sha256-{}", hex::encode(hash))))
}

/// Verify a file's checksum matches the expected value
pub fn verify_checksum(path: &Path, expected: &str) -> Result<bool, LockfileGenError> {
    let actual = compute_checksum(path)?;
    Ok(actual.as_deref() == Some(expected))
}

// ============================================================================
// Lockfile Comparison
// ============================================================================

/// Result of comparing two lockfiles
#[derive(Debug, Clone, Default)]
pub struct LockfileDiff {
    /// Dependencies that were added
    pub added: Vec<String>,
    /// Dependencies that were removed
    pub removed: Vec<String>,
    /// Dependencies that changed version
    pub changed: Vec<LockfileChange>,
    /// Dependencies with checksum mismatches
    pub checksum_changed: Vec<String>,
}

/// A change in a dependency
#[derive(Debug, Clone)]
pub struct LockfileChange {
    pub id: String,
    pub old_version: String,
    pub new_version: String,
}

impl LockfileDiff {
    /// Check if the lockfiles are identical
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && self.checksum_changed.is_empty()
    }

    /// Check if there are any breaking changes (removed or changed dependencies)
    pub fn has_breaking_changes(&self) -> bool {
        !self.removed.is_empty() || !self.changed.is_empty()
    }
}

/// Compare two lockfiles and return the differences
pub fn diff_lockfiles(old: &Lockfile, new: &Lockfile) -> LockfileDiff {
    let mut diff = LockfileDiff::default();

    // Find added and changed dependencies
    for (id, new_dep) in &new.dependencies {
        match old.dependencies.get(id) {
            None => diff.added.push(id.clone()),
            Some(old_dep) => {
                if old_dep.version != new_dep.version {
                    diff.changed.push(LockfileChange {
                        id: id.clone(),
                        old_version: old_dep.version.clone(),
                        new_version: new_dep.version.clone(),
                    });
                } else if old_dep.checksum != new_dep.checksum {
                    diff.checksum_changed.push(id.clone());
                }
            }
        }
    }

    // Find removed dependencies
    for id in old.dependencies.keys() {
        if !new.dependencies.contains_key(id) {
            diff.removed.push(id.clone());
        }
    }

    diff
}

// ============================================================================
// Lockfile Validation
// ============================================================================

/// Validate a lockfile against the actual installed packages
pub fn validate_lockfile(lockfile: &Lockfile) -> Result<Vec<ValidationIssue>, LockfileGenError> {
    let index = discover_external_packages()?;
    let mut issues = Vec::new();

    for (id, dep) in &lockfile.dependencies {
        // Check if package exists
        let version = match semver::Version::parse(&dep.version) {
            Ok(v) => v,
            Err(_) => {
                issues.push(ValidationIssue::InvalidVersion {
                    id: id.clone(),
                    version: dep.version.clone(),
                });
                continue;
            }
        };

        match index.get_package(id, &version) {
            None => {
                issues.push(ValidationIssue::MissingPackage {
                    id: id.clone(),
                    version: dep.version.clone(),
                });
            }
            Some(pkg) => {
                // Verify checksum if present
                if let Some(expected_checksum) = &dep.checksum {
                    match verify_checksum(&pkg.bytecode_path, expected_checksum) {
                        Ok(true) => {}
                        Ok(false) => {
                            issues.push(ValidationIssue::ChecksumMismatch {
                                id: id.clone(),
                                path: pkg.bytecode_path.display().to_string(),
                            });
                        }
                        Err(_) => {
                            issues.push(ValidationIssue::ChecksumError {
                                id: id.clone(),
                                path: pkg.bytecode_path.display().to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    Ok(issues)
}

/// Issue found during lockfile validation
#[derive(Debug, Clone)]
pub enum ValidationIssue {
    /// Package is not installed
    MissingPackage { id: String, version: String },
    /// Version string is invalid
    InvalidVersion { id: String, version: String },
    /// Checksum doesn't match
    ChecksumMismatch { id: String, path: String },
    /// Error computing checksum
    ChecksumError { id: String, path: String },
}

impl std::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationIssue::MissingPackage { id, version } => {
                write!(f, "package '{}@{}' is not installed", id, version)
            }
            ValidationIssue::InvalidVersion { id, version } => {
                write!(f, "invalid version '{}' for package '{}'", version, id)
            }
            ValidationIssue::ChecksumMismatch { id, path } => {
                write!(f, "checksum mismatch for '{}' at {}", id, path)
            }
            ValidationIssue::ChecksumError { id, path } => {
                write!(f, "error computing checksum for '{}' at {}", id, path)
            }
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Get the source URL for a package
fn get_package_source(pkg: &ExternalPackage) -> String {
    // For now, assume all packages come from the default registry
    // In the future, this could be read from package metadata
    format!("registry+https://registry.arth.dev/{}", pkg.id)
}

/// Get features from dependency spec
fn get_dependency_features(deps: &BTreeMap<String, DependencySpec>, id: &str) -> Vec<String> {
    match deps.get(id) {
        Some(DependencySpec::Detailed(DetailedDependency { features, .. })) => features.clone(),
        _ => Vec::new(),
    }
}

/// Get current timestamp in ISO 8601 format
fn get_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    // Format as ISO 8601 (simplified)
    let secs = now.as_secs();
    let days = secs / 86400;
    let remaining = secs % 86400;
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;
    let seconds = remaining % 60;

    // Calculate year/month/day from days since epoch (simplified)
    // This is a simplified calculation - for production, use chrono
    let year = 1970 + (days / 365);
    let day_of_year = days % 365;
    let month = (day_of_year / 30) + 1;
    let day = (day_of_year % 30) + 1;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

/// Get the current Arth compiler version
fn get_arth_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_compute_checksum() {
        // Create a temp file with known content
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"hello world").unwrap();
        file.flush().unwrap();

        let checksum = compute_checksum(file.path()).unwrap();
        assert!(checksum.is_some());

        let checksum = checksum.unwrap();
        assert!(checksum.starts_with("sha256-"));

        // Known SHA-256 of "hello world"
        assert_eq!(
            checksum,
            "sha256-b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_verify_checksum() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"test content").unwrap();
        file.flush().unwrap();

        let checksum = compute_checksum(file.path()).unwrap().unwrap();

        assert!(verify_checksum(file.path(), &checksum).unwrap());
        assert!(!verify_checksum(file.path(), "sha256-invalid").unwrap());
    }

    #[test]
    fn test_diff_lockfiles_empty() {
        let lock1 = Lockfile {
            package: LockPackage {
                name: "test".into(),
                version: "1.0.0".into(),
            },
            dependencies: BTreeMap::new(),
            source_fingerprints: BTreeMap::new(),
            metadata: LockMetadata::default(),
        };
        let lock2 = lock1.clone();

        let diff = diff_lockfiles(&lock1, &lock2);
        assert!(diff.is_empty());
    }

    #[test]
    fn test_diff_lockfiles_added() {
        let lock1 = Lockfile {
            package: LockPackage {
                name: "test".into(),
                version: "1.0.0".into(),
            },
            dependencies: BTreeMap::new(),
            source_fingerprints: BTreeMap::new(),
            metadata: LockMetadata::default(),
        };

        let mut lock2 = lock1.clone();
        lock2.dependencies.insert(
            "new-dep".into(),
            LockDependency {
                version: "1.0.0".into(),
                checksum: None,
                source: None,
                features: vec![],
                dependencies: vec![],
            },
        );

        let diff = diff_lockfiles(&lock1, &lock2);
        assert_eq!(diff.added, vec!["new-dep"]);
        assert!(diff.removed.is_empty());
        assert!(diff.changed.is_empty());
    }

    #[test]
    fn test_diff_lockfiles_removed() {
        let mut lock1 = Lockfile {
            package: LockPackage {
                name: "test".into(),
                version: "1.0.0".into(),
            },
            dependencies: BTreeMap::new(),
            source_fingerprints: BTreeMap::new(),
            metadata: LockMetadata::default(),
        };
        lock1.dependencies.insert(
            "old-dep".into(),
            LockDependency {
                version: "1.0.0".into(),
                checksum: None,
                source: None,
                features: vec![],
                dependencies: vec![],
            },
        );

        let lock2 = Lockfile {
            package: LockPackage {
                name: "test".into(),
                version: "1.0.0".into(),
            },
            dependencies: BTreeMap::new(),
            source_fingerprints: BTreeMap::new(),
            metadata: LockMetadata::default(),
        };

        let diff = diff_lockfiles(&lock1, &lock2);
        assert!(diff.added.is_empty());
        assert_eq!(diff.removed, vec!["old-dep"]);
        assert!(diff.changed.is_empty());
    }

    #[test]
    fn test_diff_lockfiles_changed() {
        let mut lock1 = Lockfile {
            package: LockPackage {
                name: "test".into(),
                version: "1.0.0".into(),
            },
            dependencies: BTreeMap::new(),
            source_fingerprints: BTreeMap::new(),
            metadata: LockMetadata::default(),
        };
        lock1.dependencies.insert(
            "dep".into(),
            LockDependency {
                version: "1.0.0".into(),
                checksum: None,
                source: None,
                features: vec![],
                dependencies: vec![],
            },
        );

        let mut lock2 = lock1.clone();
        lock2.dependencies.get_mut("dep").unwrap().version = "2.0.0".into();

        let diff = diff_lockfiles(&lock1, &lock2);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.changed[0].id, "dep");
        assert_eq!(diff.changed[0].old_version, "1.0.0");
        assert_eq!(diff.changed[0].new_version, "2.0.0");
    }

    #[test]
    fn test_lockfile_options() {
        let full = LockfileOptions::full();
        assert!(full.include_checksums);
        assert!(full.include_sources);
        assert!(full.include_arth_version);

        let minimal = LockfileOptions::minimal();
        assert!(!minimal.include_checksums);
        assert!(!minimal.include_sources);
        assert!(!minimal.include_arth_version);
    }

    #[test]
    fn test_get_timestamp() {
        let ts = get_timestamp();
        assert!(ts.contains('T'));
        assert!(ts.ends_with('Z'));
    }

    #[test]
    fn test_get_arth_version() {
        let version = get_arth_version();
        assert!(!version.is_empty());
    }
}
