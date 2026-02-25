//! Content-based fingerprinting for incremental compilation.
//!
//! This module provides deterministic content hashing for:
//! - Source files (content hash)
//! - Package sources (combined hash of all files in a package)
//! - Interfaces (public symbols for dependency tracking)
//! - Build configuration (compiler version, backend, options)
//!
//! All hashing uses SHA-256 and is designed to be deterministic:
//! - Inputs are sorted before hashing
//! - BTreeMap/BTreeSet are used for ordered iteration
//! - Stable serialization with sorted keys

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Current fingerprint format version.
/// Increment this when the fingerprinting algorithm changes.
pub const FINGERPRINT_VERSION: u32 = 1;

/// A content-addressable fingerprint for compilation artifacts.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fingerprint {
    /// SHA-256 hash as 64-character hex string
    pub hash: String,
    /// What this fingerprint represents
    pub kind: FingerprintKind,
}

impl Fingerprint {
    /// Create a new fingerprint from a hash and kind.
    pub fn new(hash: String, kind: FingerprintKind) -> Self {
        Self { hash, kind }
    }

    /// Get a short representation of the hash (first 16 chars).
    pub fn short(&self) -> &str {
        if self.hash.len() >= 16 {
            &self.hash[..16]
        } else {
            &self.hash
        }
    }
}

impl std::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.kind, self.short())
    }
}

/// The type of content a fingerprint represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FingerprintKind {
    /// Source file content hash
    Source,
    /// Package interface (public symbols only)
    Interface,
    /// Full package compilation (source + deps + config)
    Package,
    /// Build configuration
    Config,
}

impl std::fmt::Display for FingerprintKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FingerprintKind::Source => write!(f, "src"),
            FingerprintKind::Interface => write!(f, "iface"),
            FingerprintKind::Package => write!(f, "pkg"),
            FingerprintKind::Config => write!(f, "cfg"),
        }
    }
}

/// Compilation inputs that affect the build.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileInputs {
    /// Compiler version (from CARGO_PKG_VERSION)
    pub compiler_version: String,
    /// Backend: "vm", "llvm", or "cranelift"
    pub backend: String,
    /// Optimization level (0-3)
    pub opt_level: u8,
    /// Feature flags enabled
    pub features: BTreeSet<String>,
    /// Debug info enabled
    pub debug_info: bool,
}

impl Default for CompileInputs {
    fn default() -> Self {
        Self {
            compiler_version: env!("CARGO_PKG_VERSION").to_string(),
            backend: "vm".to_string(),
            opt_level: 0,
            features: BTreeSet::new(),
            debug_info: true,
        }
    }
}

impl CompileInputs {
    /// Create inputs for VM backend.
    pub fn vm() -> Self {
        Self {
            backend: "vm".to_string(),
            ..Default::default()
        }
    }

    /// Create inputs for LLVM backend.
    pub fn llvm() -> Self {
        Self {
            backend: "llvm".to_string(),
            ..Default::default()
        }
    }

    /// Create inputs for Cranelift backend.
    pub fn cranelift() -> Self {
        Self {
            backend: "cranelift".to_string(),
            ..Default::default()
        }
    }

    /// Set optimization level.
    pub fn with_opt_level(mut self, level: u8) -> Self {
        self.opt_level = level.min(3);
        self
    }

    /// Set debug info.
    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug_info = debug;
        self
    }

    /// Add a feature.
    pub fn with_feature(mut self, feature: impl Into<String>) -> Self {
        self.features.insert(feature.into());
        self
    }

    /// Compute fingerprint for these inputs.
    pub fn fingerprint(&self) -> Fingerprint {
        fingerprint_config(self)
    }
}

/// Compute SHA-256 hash of raw bytes.
fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Compute fingerprint for a single source file.
///
/// The fingerprint is based on the file path and content.
pub fn fingerprint_source(path: &Path, contents: &str) -> Fingerprint {
    let mut hasher = Sha256::new();

    // Include path for uniqueness
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(b"\0");

    // Include content
    hasher.update(contents.as_bytes());

    Fingerprint::new(hex::encode(hasher.finalize()), FingerprintKind::Source)
}

/// Compute fingerprint for a source file's content only (no path).
///
/// Use this when comparing content across different paths.
pub fn fingerprint_content(contents: &str) -> String {
    hash_bytes(contents.as_bytes())
}

/// Compute combined fingerprint for all source files in a package.
///
/// Files are sorted by path for deterministic hashing.
pub fn fingerprint_package_sources(files: &[(PathBuf, String)]) -> Fingerprint {
    let mut hasher = Sha256::new();

    // Include fingerprint version for forward compatibility
    hasher.update(FINGERPRINT_VERSION.to_le_bytes());

    // Sort by path for determinism
    let mut sorted: Vec<_> = files.iter().collect();
    sorted.sort_by_key(|(p, _)| p);

    for (path, contents) in sorted {
        // Hash path:content_hash for each file
        let path_str = path.to_string_lossy();
        let content_hash = fingerprint_content(contents);

        hasher.update(path_str.as_bytes());
        hasher.update(b":");
        hasher.update(content_hash.as_bytes());
        hasher.update(b"\n");
    }

    Fingerprint::new(hex::encode(hasher.finalize()), FingerprintKind::Source)
}

/// Compute fingerprint for package interface (public symbols).
///
/// This is used for dependency tracking - if interface changes,
/// dependents need recompilation.
pub fn fingerprint_interface(interface: &PackageInterface) -> Fingerprint {
    let mut hasher = Sha256::new();

    // Include fingerprint version
    hasher.update(FINGERPRINT_VERSION.to_le_bytes());

    // Hash structs
    for (name, fields) in &interface.structs {
        hasher.update(b"struct:");
        hasher.update(name.as_bytes());
        hasher.update(b":");
        for (field_name, field_type) in fields {
            hasher.update(field_name.as_bytes());
            hasher.update(b":");
            hasher.update(field_type.as_bytes());
            hasher.update(b",");
        }
        hasher.update(b"\n");
    }

    // Hash enums
    for (name, variants) in &interface.enums {
        hasher.update(b"enum:");
        hasher.update(name.as_bytes());
        hasher.update(b":");
        for (variant_name, payload) in variants {
            hasher.update(variant_name.as_bytes());
            if let Some(p) = payload {
                hasher.update(b"(");
                hasher.update(p.as_bytes());
                hasher.update(b")");
            }
            hasher.update(b",");
        }
        hasher.update(b"\n");
    }

    // Hash interfaces
    for (name, methods) in &interface.interfaces {
        hasher.update(b"interface:");
        hasher.update(name.as_bytes());
        hasher.update(b":");
        for (method_name, signature) in methods {
            hasher.update(method_name.as_bytes());
            hasher.update(b":");
            hasher.update(signature.as_bytes());
            hasher.update(b",");
        }
        hasher.update(b"\n");
    }

    // Hash modules
    for (name, funcs) in &interface.modules {
        hasher.update(b"module:");
        hasher.update(name.as_bytes());
        hasher.update(b":");
        for (func_name, signature) in funcs {
            hasher.update(func_name.as_bytes());
            hasher.update(b":");
            hasher.update(signature.as_bytes());
            hasher.update(b",");
        }
        hasher.update(b"\n");
    }

    // Hash type aliases
    for (name, target) in &interface.type_aliases {
        hasher.update(b"typealias:");
        hasher.update(name.as_bytes());
        hasher.update(b"=");
        hasher.update(target.as_bytes());
        hasher.update(b"\n");
    }

    Fingerprint::new(hex::encode(hasher.finalize()), FingerprintKind::Interface)
}

/// Compute fingerprint for build configuration.
pub fn fingerprint_config(inputs: &CompileInputs) -> Fingerprint {
    let mut hasher = Sha256::new();

    // Include fingerprint version
    hasher.update(FINGERPRINT_VERSION.to_le_bytes());

    hasher.update(b"compiler:");
    hasher.update(inputs.compiler_version.as_bytes());
    hasher.update(b"\n");

    hasher.update(b"backend:");
    hasher.update(inputs.backend.as_bytes());
    hasher.update(b"\n");

    hasher.update(b"opt:");
    hasher.update([inputs.opt_level]);
    hasher.update(b"\n");

    hasher.update(b"debug:");
    hasher.update([inputs.debug_info as u8]);
    hasher.update(b"\n");

    hasher.update(b"features:");
    for feature in &inputs.features {
        hasher.update(feature.as_bytes());
        hasher.update(b",");
    }
    hasher.update(b"\n");

    Fingerprint::new(hex::encode(hasher.finalize()), FingerprintKind::Config)
}

/// Compute combined fingerprint for a package compilation.
///
/// This combines:
/// - Source fingerprint
/// - Config fingerprint
/// - Dependency interface fingerprints (sorted)
pub fn fingerprint_package(
    source_fingerprint: &Fingerprint,
    config_fingerprint: &Fingerprint,
    dep_interfaces: &BTreeMap<String, Fingerprint>,
) -> Fingerprint {
    let mut hasher = Sha256::new();

    // Include fingerprint version
    hasher.update(FINGERPRINT_VERSION.to_le_bytes());

    hasher.update(b"source:");
    hasher.update(source_fingerprint.hash.as_bytes());
    hasher.update(b"\n");

    hasher.update(b"config:");
    hasher.update(config_fingerprint.hash.as_bytes());
    hasher.update(b"\n");

    hasher.update(b"deps:");
    for (dep_name, dep_iface) in dep_interfaces {
        hasher.update(dep_name.as_bytes());
        hasher.update(b":");
        hasher.update(dep_iface.hash.as_bytes());
        hasher.update(b",");
    }
    hasher.update(b"\n");

    Fingerprint::new(hex::encode(hasher.finalize()), FingerprintKind::Package)
}

/// Package interface for dependency tracking.
///
/// Contains only public symbols that can be referenced from other packages.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageInterface {
    /// Public structs: name -> [(field_name, type_string)]
    pub structs: BTreeMap<String, Vec<(String, String)>>,
    /// Public enums: name -> [(variant_name, Optional<payload_type>)]
    pub enums: BTreeMap<String, Vec<(String, Option<String>)>>,
    /// Public interfaces: name -> [(method_name, signature_string)]
    pub interfaces: BTreeMap<String, Vec<(String, String)>>,
    /// Public modules: name -> [(func_name, signature_string)]
    pub modules: BTreeMap<String, Vec<(String, String)>>,
    /// Public type aliases: name -> target_type
    pub type_aliases: BTreeMap<String, String>,
}

impl PackageInterface {
    /// Create a new empty interface.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a struct with its fields.
    pub fn add_struct(&mut self, name: impl Into<String>, fields: Vec<(String, String)>) {
        self.structs.insert(name.into(), fields);
    }

    /// Add an enum with its variants.
    pub fn add_enum(&mut self, name: impl Into<String>, variants: Vec<(String, Option<String>)>) {
        self.enums.insert(name.into(), variants);
    }

    /// Add an interface with its methods.
    pub fn add_interface(&mut self, name: impl Into<String>, methods: Vec<(String, String)>) {
        self.interfaces.insert(name.into(), methods);
    }

    /// Add a module with its functions.
    pub fn add_module(&mut self, name: impl Into<String>, funcs: Vec<(String, String)>) {
        self.modules.insert(name.into(), funcs);
    }

    /// Add a type alias.
    pub fn add_type_alias(&mut self, name: impl Into<String>, target: impl Into<String>) {
        self.type_aliases.insert(name.into(), target.into());
    }

    /// Check if the interface is empty.
    pub fn is_empty(&self) -> bool {
        self.structs.is_empty()
            && self.enums.is_empty()
            && self.interfaces.is_empty()
            && self.modules.is_empty()
            && self.type_aliases.is_empty()
    }

    /// Compute fingerprint for this interface.
    pub fn fingerprint(&self) -> Fingerprint {
        fingerprint_interface(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprint_source_deterministic() {
        let path = PathBuf::from("test.arth");
        let contents = "package test;\nmodule Foo {}";

        let fp1 = fingerprint_source(&path, contents);
        let fp2 = fingerprint_source(&path, contents);

        assert_eq!(fp1, fp2);
        assert_eq!(fp1.kind, FingerprintKind::Source);
        assert_eq!(fp1.hash.len(), 64); // SHA-256 produces 64 hex chars
    }

    #[test]
    fn test_fingerprint_source_changes_with_content() {
        let path = PathBuf::from("test.arth");

        let fp1 = fingerprint_source(&path, "package test;");
        let fp2 = fingerprint_source(&path, "package test; // modified");

        assert_ne!(fp1, fp2);
    }

    #[test]
    fn test_fingerprint_source_changes_with_path() {
        let contents = "package test;";

        let fp1 = fingerprint_source(Path::new("a.arth"), contents);
        let fp2 = fingerprint_source(Path::new("b.arth"), contents);

        assert_ne!(fp1, fp2);
    }

    #[test]
    fn test_fingerprint_package_sources_deterministic() {
        let files = vec![
            (PathBuf::from("b.arth"), "package b;".to_string()),
            (PathBuf::from("a.arth"), "package a;".to_string()),
        ];

        let fp1 = fingerprint_package_sources(&files);

        // Reverse order should produce same hash (sorted internally)
        let files_reversed = vec![
            (PathBuf::from("a.arth"), "package a;".to_string()),
            (PathBuf::from("b.arth"), "package b;".to_string()),
        ];

        let fp2 = fingerprint_package_sources(&files_reversed);

        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_fingerprint_config_varies_with_backend() {
        let vm_inputs = CompileInputs::vm();
        let llvm_inputs = CompileInputs::llvm();

        let fp_vm = fingerprint_config(&vm_inputs);
        let fp_llvm = fingerprint_config(&llvm_inputs);

        assert_ne!(fp_vm, fp_llvm);
        assert_eq!(fp_vm.kind, FingerprintKind::Config);
    }

    #[test]
    fn test_fingerprint_config_varies_with_opt_level() {
        let inputs_0 = CompileInputs::vm().with_opt_level(0);
        let inputs_3 = CompileInputs::vm().with_opt_level(3);

        let fp_0 = fingerprint_config(&inputs_0);
        let fp_3 = fingerprint_config(&inputs_3);

        assert_ne!(fp_0, fp_3);
    }

    #[test]
    fn test_fingerprint_config_varies_with_features() {
        let inputs_none = CompileInputs::vm();
        let inputs_feat = CompileInputs::vm().with_feature("async-cps");

        let fp_none = fingerprint_config(&inputs_none);
        let fp_feat = fingerprint_config(&inputs_feat);

        assert_ne!(fp_none, fp_feat);
    }

    #[test]
    fn test_fingerprint_interface_empty() {
        let iface = PackageInterface::new();
        let fp = iface.fingerprint();

        assert_eq!(fp.kind, FingerprintKind::Interface);
        assert!(!fp.hash.is_empty());
    }

    #[test]
    fn test_fingerprint_interface_with_struct() {
        let mut iface1 = PackageInterface::new();
        iface1.add_struct(
            "User",
            vec![
                ("name".to_string(), "String".to_string()),
                ("age".to_string(), "Int".to_string()),
            ],
        );

        let mut iface2 = PackageInterface::new();
        iface2.add_struct(
            "User",
            vec![
                ("name".to_string(), "String".to_string()),
                ("age".to_string(), "Int".to_string()),
            ],
        );

        assert_eq!(iface1.fingerprint(), iface2.fingerprint());

        // Change field type
        let mut iface3 = PackageInterface::new();
        iface3.add_struct(
            "User",
            vec![
                ("name".to_string(), "String".to_string()),
                ("age".to_string(), "Float".to_string()), // Changed!
            ],
        );

        assert_ne!(iface1.fingerprint(), iface3.fingerprint());
    }

    #[test]
    fn test_fingerprint_interface_with_module() {
        let mut iface = PackageInterface::new();
        iface.add_module(
            "MathFns",
            vec![
                ("add".to_string(), "(Int, Int) -> Int".to_string()),
                ("sub".to_string(), "(Int, Int) -> Int".to_string()),
            ],
        );

        let fp = iface.fingerprint();
        assert_eq!(fp.kind, FingerprintKind::Interface);
    }

    #[test]
    fn test_fingerprint_package_combines_all() {
        let source_fp = Fingerprint::new("source_hash".to_string(), FingerprintKind::Source);
        let config_fp = Fingerprint::new("config_hash".to_string(), FingerprintKind::Config);

        let mut deps = BTreeMap::new();
        deps.insert(
            "dep1".to_string(),
            Fingerprint::new("dep1_iface".to_string(), FingerprintKind::Interface),
        );

        let pkg_fp = fingerprint_package(&source_fp, &config_fp, &deps);

        assert_eq!(pkg_fp.kind, FingerprintKind::Package);

        // Change a dependency and verify hash changes
        let mut deps2 = BTreeMap::new();
        deps2.insert(
            "dep1".to_string(),
            Fingerprint::new("dep1_iface_changed".to_string(), FingerprintKind::Interface),
        );

        let pkg_fp2 = fingerprint_package(&source_fp, &config_fp, &deps2);
        assert_ne!(pkg_fp, pkg_fp2);
    }

    #[test]
    fn test_fingerprint_short() {
        let fp = Fingerprint::new(
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_string(),
            FingerprintKind::Source,
        );

        assert_eq!(fp.short(), "abcdef0123456789");
    }

    #[test]
    fn test_fingerprint_display() {
        let fp = Fingerprint::new(
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_string(),
            FingerprintKind::Source,
        );

        assert_eq!(format!("{}", fp), "src:abcdef0123456789");
    }

    #[test]
    fn test_compile_inputs_default() {
        let inputs = CompileInputs::default();

        assert_eq!(inputs.backend, "vm");
        assert_eq!(inputs.opt_level, 0);
        assert!(inputs.debug_info);
        assert!(inputs.features.is_empty());
    }

    #[test]
    fn test_content_fingerprint_ignores_path() {
        let hash1 = fingerprint_content("hello world");
        let hash2 = fingerprint_content("hello world");

        assert_eq!(hash1, hash2);
    }
}
