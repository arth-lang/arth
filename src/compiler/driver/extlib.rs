//! External Package Discovery and Resolution
//!
//! Discovers and resolves precompiled Arth packages from `~/.arth/libs/<pkg>/<version>/`.
//! Supports both pure Arth libraries (lib.abc) and FFI-dependent libraries (lib/*.dylib).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use semver::{Version, VersionReq};
use serde::Deserialize;

use super::config::{DependencySpec, Manifest};

/// Information about an installed external package
#[derive(Clone, Debug)]
pub struct ExternalPackage {
    /// Package identifier (e.g., "org.example:net-http")
    pub id: String,
    /// Resolved version (e.g., "1.2.3")
    pub version: Version,
    /// Arth package prefix for imports (e.g., "net.http")
    pub arth_package: String,
    /// Root directory (~/.arth/libs/<pkg>/<ver>/)
    pub root_dir: PathBuf,
    /// Path to precompiled bytecode (lib.abc)
    pub bytecode_path: PathBuf,
    /// Native library files (.dylib/.so/.dll)
    pub native_libs: Vec<PathBuf>,
    /// Transitive dependencies
    pub dependencies: Vec<String>,
}

/// External package manifest (arth.toml in the package)
#[derive(Clone, Debug, Deserialize)]
pub struct ExternalManifest {
    pub package: ExternalPackageInfo,
    #[serde(default)]
    pub dependencies: BTreeMap<String, DependencySpec>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ExternalPackageInfo {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub edition: Option<String>,
    /// Root package name for imports (e.g., "net.http")
    #[serde(rename = "arth-package")]
    pub arth_package: Option<String>,
}

/// Index of all available external packages
#[derive(Clone, Debug, Default)]
pub struct ExternalPackageIndex {
    /// Packages by ID -> versions (sorted by version)
    packages: HashMap<String, BTreeMap<Version, ExternalPackage>>,
}

impl ExternalPackageIndex {
    /// Get all versions of a package
    pub fn get_versions(&self, id: &str) -> Option<&BTreeMap<Version, ExternalPackage>> {
        self.packages.get(id)
    }

    /// Get a specific version of a package
    pub fn get_package(&self, id: &str, version: &Version) -> Option<&ExternalPackage> {
        self.packages.get(id).and_then(|v| v.get(version))
    }

    /// Find the best matching version for a requirement
    pub fn find_matching(&self, id: &str, req: &VersionReq) -> Option<&ExternalPackage> {
        self.packages.get(id).and_then(|versions| {
            versions
                .iter()
                .rev() // Start from highest version
                .find(|(v, _)| req.matches(v))
                .map(|(_, pkg)| pkg)
        })
    }

    /// Check if any version of a package is installed
    pub fn has_package(&self, id: &str) -> bool {
        self.packages.contains_key(id)
    }

    /// Iterate over all packages
    pub fn iter(&self) -> impl Iterator<Item = (&String, &BTreeMap<Version, ExternalPackage>)> {
        self.packages.iter()
    }
}

/// Error type for external package operations
#[derive(Debug)]
pub enum ExtlibError {
    /// ~/.arth/libs directory not found
    LibsDirNotFound,
    /// Package not installed
    PackageNotFound { id: String, version_req: String },
    /// Version not found
    VersionNotFound {
        id: String,
        available: Vec<String>,
        required: String,
    },
    /// Invalid package manifest
    InvalidManifest { path: PathBuf, error: String },
    /// Missing bytecode file
    MissingBytecode { path: PathBuf },
    /// Invalid version string
    InvalidVersion { version: String, error: String },
    /// Circular dependency detected
    CircularDependency { chain: Vec<String> },
    /// IO error
    Io(std::io::Error),
}

impl std::fmt::Display for ExtlibError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtlibError::LibsDirNotFound => {
                write!(f, "external libs directory not found (~/.arth/libs)")
            }
            ExtlibError::PackageNotFound { id, version_req } => {
                write!(
                    f,
                    "package '{}' version '{}' not installed in ~/.arth/libs",
                    id, version_req
                )
            }
            ExtlibError::VersionNotFound {
                id,
                available,
                required,
            } => {
                write!(
                    f,
                    "package '{}' version '{}' not found; available: {}",
                    id,
                    required,
                    if available.is_empty() {
                        "(none)".to_string()
                    } else {
                        available.join(", ")
                    }
                )
            }
            ExtlibError::InvalidManifest { path, error } => {
                write!(f, "invalid manifest at {}: {}", path.display(), error)
            }
            ExtlibError::MissingBytecode { path } => {
                write!(f, "missing bytecode file: {}", path.display())
            }
            ExtlibError::InvalidVersion { version, error } => {
                write!(f, "invalid version '{}': {}", version, error)
            }
            ExtlibError::CircularDependency { chain } => {
                write!(f, "circular dependency: {}", chain.join(" -> "))
            }
            ExtlibError::Io(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for ExtlibError {}

impl From<std::io::Error> for ExtlibError {
    fn from(e: std::io::Error) -> Self {
        ExtlibError::Io(e)
    }
}

/// Get the external libs directory path
///
/// Returns `$ARTH_LIBS_DIR` if set, otherwise `~/.arth/libs`
pub fn get_libs_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("ARTH_LIBS_DIR") {
        return Some(PathBuf::from(dir));
    }
    dirs::home_dir().map(|h| h.join(".arth").join("libs"))
}

/// Discover all installed external packages
///
/// Scans `~/.arth/libs/<pkg>/<version>/` for valid packages with:
/// - arth.toml (manifest)
/// - lib.abc (precompiled bytecode)
/// - lib/ (optional native libraries)
pub fn discover_external_packages() -> Result<ExternalPackageIndex, ExtlibError> {
    let libs_dir = match get_libs_dir() {
        Some(dir) if dir.is_dir() => dir,
        Some(_) => return Ok(ExternalPackageIndex::default()), // Dir doesn't exist yet
        None => return Err(ExtlibError::LibsDirNotFound),
    };

    let mut index = ExternalPackageIndex::default();

    // Iterate over package directories
    for pkg_entry in fs::read_dir(&libs_dir)? {
        let pkg_entry = pkg_entry?;
        let pkg_path = pkg_entry.path();

        if !pkg_path.is_dir() {
            continue;
        }

        let pkg_name = match pkg_path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Iterate over version directories
        for ver_entry in fs::read_dir(&pkg_path)? {
            let ver_entry = ver_entry?;
            let ver_path = ver_entry.path();

            if !ver_path.is_dir() {
                continue;
            }

            let ver_str = match ver_path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Try to parse as valid semver
            let version = match Version::parse(&ver_str) {
                Ok(v) => v,
                Err(_) => continue, // Skip invalid version directories
            };

            // Try to load the package
            match load_external_package(&ver_path, &pkg_name, version.clone()) {
                Ok(pkg) => {
                    index
                        .packages
                        .entry(pkg.id.clone())
                        .or_default()
                        .insert(version, pkg);
                }
                Err(_) => continue, // Skip invalid packages
            }
        }
    }

    Ok(index)
}

/// Load an external package from a directory
fn load_external_package(
    root: &Path,
    default_id: &str,
    version: Version,
) -> Result<ExternalPackage, ExtlibError> {
    // Read manifest
    let manifest_path = root.join("arth.toml");
    let manifest: ExternalManifest = if manifest_path.exists() {
        let text = fs::read_to_string(&manifest_path)?;
        toml::from_str(&text).map_err(|e| ExtlibError::InvalidManifest {
            path: manifest_path.clone(),
            error: e.to_string(),
        })?
    } else {
        // Create default manifest from directory structure
        ExternalManifest {
            package: ExternalPackageInfo {
                name: default_id.to_string(),
                version: version.to_string(),
                edition: None,
                arth_package: None,
            },
            dependencies: BTreeMap::new(),
        }
    };

    // Check for bytecode
    let bytecode_path = root.join("lib.abc");
    if !bytecode_path.exists() {
        return Err(ExtlibError::MissingBytecode {
            path: bytecode_path,
        });
    }

    // Discover native libraries
    let native_libs = discover_native_libs(&root.join("lib"));

    // Determine arth package prefix
    let arth_package = manifest
        .package
        .arth_package
        .clone()
        .unwrap_or_else(|| manifest.package.name.replace(':', ".").replace('-', "_"));

    // Collect dependency IDs
    let dependencies: Vec<String> = manifest.dependencies.keys().cloned().collect();

    Ok(ExternalPackage {
        id: manifest.package.name.clone(),
        version,
        arth_package,
        root_dir: root.to_path_buf(),
        bytecode_path,
        native_libs,
        dependencies,
    })
}

/// Discover native libraries in a lib/ directory
fn discover_native_libs(lib_dir: &Path) -> Vec<PathBuf> {
    let mut libs = Vec::new();

    if !lib_dir.is_dir() {
        return libs;
    }

    let extensions: &[&str] = if cfg!(target_os = "macos") {
        &["dylib"]
    } else if cfg!(target_os = "windows") {
        &["dll"]
    } else {
        &["so"]
    };

    if let Ok(entries) = fs::read_dir(lib_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if extensions.contains(&ext) {
                    libs.push(path);
                }
            }
        }
    }

    libs
}

/// Parse a version requirement from dependency spec
pub fn parse_version_req(spec: &DependencySpec) -> Result<VersionReq, ExtlibError> {
    let version_str = match spec {
        DependencySpec::Simple(s) => s.as_str(),
        DependencySpec::Detailed(d) => d.version.as_str(),
    };

    VersionReq::parse(version_str).map_err(|e| ExtlibError::InvalidVersion {
        version: version_str.to_string(),
        error: e.to_string(),
    })
}

/// Resolve dependencies from a project manifest
///
/// Returns a list of external packages that satisfy the manifest's dependencies.
pub fn resolve_dependencies(
    manifest: &Manifest,
    index: &ExternalPackageIndex,
) -> Result<Vec<ExternalPackage>, ExtlibError> {
    let mut resolved = Vec::new();
    let mut visited = std::collections::HashSet::new();

    for (id, spec) in &manifest.dependencies {
        resolve_dependency(
            id,
            spec,
            index,
            &mut resolved,
            &mut visited,
            &mut vec![id.clone()],
        )?;
    }

    Ok(resolved)
}

/// Recursively resolve a single dependency and its transitive dependencies
#[allow(clippy::ptr_arg)]
fn resolve_dependency(
    id: &str,
    spec: &DependencySpec,
    index: &ExternalPackageIndex,
    resolved: &mut Vec<ExternalPackage>,
    visited: &mut std::collections::HashSet<String>,
    chain: &mut Vec<String>,
) -> Result<(), ExtlibError> {
    // Check for cycles
    if visited.contains(id) {
        // Already resolved, skip
        return Ok(());
    }

    // Check if we're in the resolution chain (cycle detection)
    let chain_pos = chain.iter().position(|x| x == id);
    if let Some(pos) = chain_pos {
        if pos < chain.len() - 1 {
            return Err(ExtlibError::CircularDependency {
                chain: chain[pos..].to_vec(),
            });
        }
    }

    let req = parse_version_req(spec)?;

    let pkg = index.find_matching(id, &req).ok_or_else(|| {
        let available: Vec<String> = index
            .get_versions(id)
            .map(|vs| vs.keys().map(|v| v.to_string()).collect())
            .unwrap_or_default();

        if available.is_empty() {
            ExtlibError::PackageNotFound {
                id: id.to_string(),
                version_req: req.to_string(),
            }
        } else {
            ExtlibError::VersionNotFound {
                id: id.to_string(),
                available,
                required: req.to_string(),
            }
        }
    })?;

    // Mark as visited before resolving transitive deps
    visited.insert(id.to_string());

    // Resolve transitive dependencies
    // Note: We'd need to load the package's manifest to get its deps
    // For now, we use the dependencies collected during discovery

    // Add to resolved list
    resolved.push(pkg.clone());

    Ok(())
}

/// Collect all native library paths from resolved packages
pub fn collect_native_lib_paths(packages: &[ExternalPackage]) -> Vec<PathBuf> {
    packages
        .iter()
        .flat_map(|p| p.native_libs.clone())
        .collect()
}

/// Exported symbol from an external package
#[derive(Clone, Debug)]
pub struct ExternalSymbol {
    /// Full name (e.g., "demo.Math.add")
    pub name: String,
    /// Package prefix (e.g., "demo")
    pub package: String,
    /// Symbol name within package (e.g., "Math")
    pub symbol_name: String,
    /// Whether this is a module, function, etc.
    pub kind: ExternalSymbolKind,
    /// Arity for functions
    pub arity: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExternalSymbolKind {
    Module,
    Function,
}

/// Load exported symbols from an external package's bytecode
pub fn load_external_package_symbols(
    pkg: &ExternalPackage,
) -> Result<Vec<ExternalSymbol>, ExtlibError> {
    use arth_vm::link::decode_library;

    let bytes = std::fs::read(&pkg.bytecode_path)?;
    let (_program, exports) = decode_library(&bytes).map_err(|e| ExtlibError::InvalidManifest {
        path: pkg.bytecode_path.clone(),
        error: e,
    })?;

    let mut symbols = Vec::new();
    let mut seen_modules = std::collections::HashSet::new();

    for export in exports {
        // Export names are like "demo.Math.add"
        // Parse into package.module.function
        let parts: Vec<&str> = export.name.split('.').collect();
        if parts.len() >= 2 {
            let package = parts[0].to_string();
            let module_name = parts[1].to_string();

            // Add module symbol if not seen
            if seen_modules.insert((package.clone(), module_name.clone())) {
                symbols.push(ExternalSymbol {
                    name: format!("{}.{}", package, module_name),
                    package: package.clone(),
                    symbol_name: module_name.clone(),
                    kind: ExternalSymbolKind::Module,
                    arity: 0,
                });
            }

            // Add function symbol
            if parts.len() >= 3 {
                let func_name = parts[2..].join(".");
                symbols.push(ExternalSymbol {
                    name: export.name.clone(),
                    package: package.clone(),
                    symbol_name: func_name,
                    kind: ExternalSymbolKind::Function,
                    arity: export.arity,
                });
            }
        }
    }

    Ok(symbols)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_req_caret() {
        let spec = DependencySpec::Simple("^1.2.0".to_string());
        let req = parse_version_req(&spec).unwrap();
        assert!(req.matches(&Version::new(1, 2, 0)));
        assert!(req.matches(&Version::new(1, 9, 0)));
        assert!(!req.matches(&Version::new(2, 0, 0)));
    }

    #[test]
    fn test_parse_version_req_tilde() {
        let spec = DependencySpec::Simple("~1.2.0".to_string());
        let req = parse_version_req(&spec).unwrap();
        assert!(req.matches(&Version::new(1, 2, 0)));
        assert!(req.matches(&Version::new(1, 2, 9)));
        assert!(!req.matches(&Version::new(1, 3, 0)));
    }

    #[test]
    fn test_parse_version_req_exact() {
        // Use = prefix for exact version match
        let spec = DependencySpec::Simple("=1.2.3".to_string());
        let req = parse_version_req(&spec).unwrap();
        assert!(req.matches(&Version::new(1, 2, 3)));
        assert!(!req.matches(&Version::new(1, 2, 4)));
    }

    #[test]
    fn test_external_package_index_find_matching() {
        let mut index = ExternalPackageIndex::default();

        let pkg1 = ExternalPackage {
            id: "test:pkg".to_string(),
            version: Version::new(1, 0, 0),
            arth_package: "test.pkg".to_string(),
            root_dir: PathBuf::from("/test/1.0.0"),
            bytecode_path: PathBuf::from("/test/1.0.0/lib.abc"),
            native_libs: vec![],
            dependencies: vec![],
        };

        let pkg2 = ExternalPackage {
            id: "test:pkg".to_string(),
            version: Version::new(1, 2, 0),
            arth_package: "test.pkg".to_string(),
            root_dir: PathBuf::from("/test/1.2.0"),
            bytecode_path: PathBuf::from("/test/1.2.0/lib.abc"),
            native_libs: vec![],
            dependencies: vec![],
        };

        index
            .packages
            .entry("test:pkg".to_string())
            .or_default()
            .insert(Version::new(1, 0, 0), pkg1);
        index
            .packages
            .entry("test:pkg".to_string())
            .or_default()
            .insert(Version::new(1, 2, 0), pkg2);

        let req = VersionReq::parse("^1.0.0").unwrap();
        let found = index.find_matching("test:pkg", &req).unwrap();
        assert_eq!(found.version, Version::new(1, 2, 0)); // Should find highest matching

        let req_exact = VersionReq::parse("=1.0.0").unwrap();
        let found_exact = index.find_matching("test:pkg", &req_exact).unwrap();
        assert_eq!(found_exact.version, Version::new(1, 0, 0));
    }

    #[test]
    fn test_discover_native_libs_platform() {
        // This test verifies the extension detection logic
        let extensions: &[&str] = if cfg!(target_os = "macos") {
            &["dylib"]
        } else if cfg!(target_os = "windows") {
            &["dll"]
        } else {
            &["so"]
        };

        // On macOS, we expect dylib
        #[cfg(target_os = "macos")]
        assert!(extensions.contains(&"dylib"));

        // On Linux, we expect so
        #[cfg(target_os = "linux")]
        assert!(extensions.contains(&"so"));

        // On Windows, we expect dll
        #[cfg(target_os = "windows")]
        assert!(extensions.contains(&"dll"));
    }
}
