//! Package Registry - Authoritative package/module resolution
//!
//! Provides a canonical mapping between source files, packages, and symbols.
//! This is the single source of truth for package identity in the compiler.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::compiler::ast::FileAst;
use crate::compiler::diagnostics::{Diagnostic, Reporter};
use crate::compiler::source::SourceFile;

/// Canonical package information
#[derive(Clone, Debug)]
pub struct PackageInfo {
    /// Fully-qualified package name (e.g., "app.http")
    pub name: String,
    /// Root directory for this package's files (relative to source root)
    pub root_dir: PathBuf,
    /// All files belonging to this package
    pub files: Vec<PathBuf>,
    /// Whether this is a stdlib package (no source files)
    pub is_stdlib: bool,
}

/// Central registry of all packages in the project
///
/// The registry provides authoritative file-to-package mapping and
/// supports lookups in both directions.
#[derive(Clone, Debug, Default)]
pub struct PackageRegistry {
    /// All known packages by name
    packages: HashMap<String, PackageInfo>,
    /// File path -> package name mapping
    file_to_package: HashMap<PathBuf, String>,
    /// Project root directory
    project_root: PathBuf,
    /// Source root (usually project_root/src or project_root for single files)
    source_root: PathBuf,
}

impl PackageRegistry {
    /// Build a package registry from source files and their ASTs.
    ///
    /// This validates that each file's package declaration matches its
    /// directory location and builds the authoritative mapping.
    pub fn build(root: &Path, files: &[(SourceFile, FileAst)], reporter: &mut Reporter) -> Self {
        let mut registry = PackageRegistry::default();

        // Determine project and source roots
        registry.project_root = if root.extension().is_some_and(|e| e == "arth") {
            root.parent().unwrap_or(root).to_path_buf()
        } else {
            root.to_path_buf()
        };

        // If arth.toml exists with src/ subdirectory, use src/ as source root
        let cfg_path = registry.project_root.join("arth.toml");
        if cfg_path.exists() {
            let src = registry.project_root.join("src");
            if src.is_dir() {
                registry.source_root = src;
            } else {
                registry.source_root = registry.project_root.clone();
            }
        } else {
            registry.source_root = registry.project_root.clone();
        }

        // Single-file mode has relaxed validation
        let single_file_mode = root.extension().is_some_and(|e| e == "arth");

        // Process each file
        for (sf, ast) in files {
            let pkg_name = match extract_package_name(ast) {
                Some(name) => name,
                None => {
                    reporter.emit(
                        Diagnostic::error("file is missing a package declaration")
                            .with_file(sf.path.clone()),
                    );
                    continue;
                }
            };

            // Validate directory mapping (skip in single-file mode)
            if !single_file_mode {
                if let Some(err) =
                    validate_package_dir_mapping(&registry.source_root, &sf.path, &pkg_name)
                {
                    reporter.emit(Diagnostic::error(err).with_file(sf.path.clone()));
                }
            }

            // Register file -> package mapping
            registry
                .file_to_package
                .insert(sf.path.clone(), pkg_name.clone());

            // Add/update package info
            let info = registry
                .packages
                .entry(pkg_name.clone())
                .or_insert_with(|| {
                    let root_dir = compute_package_dir(&registry.source_root, &pkg_name);
                    PackageInfo {
                        name: pkg_name.clone(),
                        root_dir,
                        files: Vec::new(),
                        is_stdlib: false,
                    }
                });
            info.files.push(sf.path.clone());
        }

        registry
    }

    /// Register a stdlib package (no source files)
    pub fn register_stdlib_package(&mut self, name: &str) {
        if !self.packages.contains_key(name) {
            self.packages.insert(
                name.to_string(),
                PackageInfo {
                    name: name.to_string(),
                    root_dir: PathBuf::new(),
                    files: Vec::new(),
                    is_stdlib: true,
                },
            );
        }
    }

    /// Register an external package (no source files, symbols from bytecode)
    pub fn register_external_package(&mut self, name: &str) {
        if !self.packages.contains_key(name) {
            self.packages.insert(
                name.to_string(),
                PackageInfo {
                    name: name.to_string(),
                    root_dir: PathBuf::from("<external>"),
                    files: Vec::new(),
                    is_stdlib: false, // Not stdlib, but precompiled
                },
            );
        }
    }

    /// Get package info by name
    pub fn get_package(&self, name: &str) -> Option<&PackageInfo> {
        self.packages.get(name)
    }

    /// Get the package name for a given file path
    pub fn get_package_for_file(&self, path: &Path) -> Option<&str> {
        self.file_to_package.get(path).map(|s| s.as_str())
    }

    /// Iterate over all registered packages
    pub fn all_packages(&self) -> impl Iterator<Item = &PackageInfo> {
        self.packages.values()
    }

    /// Check if a package exists
    pub fn has_package(&self, name: &str) -> bool {
        self.packages.contains_key(name)
    }

    /// Get the source root directory
    pub fn source_root(&self) -> &Path {
        &self.source_root
    }

    /// Get the project root directory
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Resolve a package name to its expected directory path
    ///
    /// For package "app.http", returns "{source_root}/app/http"
    pub fn resolve_package_path(&self, pkg_name: &str) -> PathBuf {
        compute_package_dir(&self.source_root, pkg_name)
    }

    /// Get all package names
    pub fn package_names(&self) -> impl Iterator<Item = &str> {
        self.packages.keys().map(|s| s.as_str())
    }
}

/// Extract package name from AST as a dot-separated string
fn extract_package_name(ast: &FileAst) -> Option<String> {
    ast.package.as_ref().map(|p| {
        p.0.iter()
            .map(|id| id.0.as_str())
            .collect::<Vec<_>>()
            .join(".")
    })
}

/// Compute expected directory for a package relative to source root
fn compute_package_dir(source_root: &Path, pkg_name: &str) -> PathBuf {
    let mut dir = source_root.to_path_buf();
    for segment in pkg_name.split('.') {
        if !segment.is_empty() {
            dir.push(segment);
        }
    }
    dir
}

/// Validate that a file's package declaration matches its directory location
///
/// Returns Some(error_message) if there's a mismatch, None if valid.
fn validate_package_dir_mapping(
    source_root: &Path,
    file_path: &Path,
    pkg_name: &str,
) -> Option<String> {
    let expected_segments: Vec<&str> = pkg_name.split('.').filter(|s| !s.is_empty()).collect();
    let actual_segments = extract_dir_segments(source_root, file_path);

    if expected_segments != actual_segments {
        let expected_path = if expected_segments.is_empty() {
            ".".to_string()
        } else {
            expected_segments.join("/")
        };
        let actual_path = if actual_segments.is_empty() {
            ".".to_string()
        } else {
            actual_segments.join("/")
        };
        Some(format!(
            "package '{}' must match directory path '{}', found '{}'",
            pkg_name, expected_path, actual_path
        ))
    } else {
        None
    }
}

/// Extract directory segments from a file path relative to source root
fn extract_dir_segments(source_root: &Path, file_path: &Path) -> Vec<String> {
    let rel = file_path.strip_prefix(source_root).unwrap_or(file_path);
    let dir = rel.parent().unwrap_or(Path::new(""));
    dir.components()
        .filter_map(|c| {
            let s = c.as_os_str().to_string_lossy().to_string();
            if s.is_empty() { None } else { Some(s) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_package_name() {
        use crate::compiler::ast::{FileAst, Ident, PackageName};

        let ast = FileAst {
            package: Some(PackageName(vec![
                Ident("app".to_string()),
                Ident("http".to_string()),
            ])),
            imports: vec![],
            decls: vec![],
        };
        assert_eq!(extract_package_name(&ast), Some("app.http".to_string()));
    }

    #[test]
    fn test_compute_package_dir() {
        let root = PathBuf::from("/project/src");
        assert_eq!(
            compute_package_dir(&root, "app.http"),
            PathBuf::from("/project/src/app/http")
        );
        assert_eq!(
            compute_package_dir(&root, "demo"),
            PathBuf::from("/project/src/demo")
        );
    }

    #[test]
    fn test_validate_package_dir_mapping_valid() {
        let root = PathBuf::from("/project/src");
        let file = PathBuf::from("/project/src/app/http/Client.arth");
        assert!(validate_package_dir_mapping(&root, &file, "app.http").is_none());
    }

    #[test]
    fn test_validate_package_dir_mapping_mismatch() {
        let root = PathBuf::from("/project/src");
        let file = PathBuf::from("/project/src/wrong/path/Client.arth");
        let err = validate_package_dir_mapping(&root, &file, "app.http");
        assert!(err.is_some());
        assert!(err.unwrap().contains("must match directory path"));
    }

    #[test]
    fn test_registry_stdlib_package() {
        let mut registry = PackageRegistry::default();
        registry.register_stdlib_package("log");
        assert!(registry.has_package("log"));
        assert!(registry.get_package("log").unwrap().is_stdlib);
    }
}
