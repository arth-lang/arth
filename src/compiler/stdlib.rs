//! Stdlib Index
//!
//! This module provides a centralized index of stdlib packages, modules, types, and functions
//! by parsing the `.arth` files in `stdlib/src/`. This replaces the manual symbol seeding
//! in `resolve/stdlib/` and `typeck/seed_stdlib_signatures`.
//!
//! # Design
//!
//! The stdlib loader follows these principles:
//! 1. **Single source of truth**: `stdlib/src/**/*.arth` defines all stdlib APIs
//! 2. **Parsed at startup**: Compiler loads stdlib index once at initialization
//! 3. **Intrinsic mapping**: Functions with `@intrinsic("name")` are tracked for lowering
//!
//! # Usage
//!
//! ```ignore
//! let stdlib = StdlibIndex::load("stdlib/src")?;
//! if let Some(pkg) = stdlib.get_package("math") {
//!     // ... use package info
//! }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::compiler::ast::{
    Attr, Decl, EnumDecl, EnumVariant, FileAst, FuncSig, InterfaceDecl, InterfaceMethod,
    ModuleDecl, NamePath, StructDecl, StructField, Visibility,
};
use crate::compiler::diagnostics::Reporter;
use crate::compiler::intrinsics;
use crate::compiler::parser::parse_file;
use crate::compiler::source::SourceFile;

/// Error type for stdlib loading failures.
#[derive(Debug)]
pub enum StdlibError {
    /// Failed to read a file
    Io(std::io::Error),
    /// File has no package declaration
    MissingPackage(PathBuf),
    /// Parse errors in stdlib file (should not happen with valid stdlib)
    ParseErrors(PathBuf, Vec<String>),
}

impl std::fmt::Display for StdlibError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StdlibError::Io(e) => write!(f, "I/O error: {}", e),
            StdlibError::MissingPackage(p) => {
                write!(
                    f,
                    "stdlib file missing package declaration: {}",
                    p.display()
                )
            }
            StdlibError::ParseErrors(p, errs) => {
                write!(f, "parse errors in {}: {:?}", p.display(), errs)
            }
        }
    }
}

impl std::error::Error for StdlibError {}

impl From<std::io::Error> for StdlibError {
    fn from(e: std::io::Error) -> Self {
        StdlibError::Io(e)
    }
}

/// Information about a function in the stdlib.
#[derive(Clone, Debug)]
pub struct StdlibFunc {
    /// Function name
    pub name: String,
    /// Function signature (from AST)
    pub sig: FuncSig,
    /// Intrinsic name if this function has @intrinsic attribute
    pub intrinsic: Option<String>,
}

/// Information about a struct in the stdlib.
#[derive(Clone, Debug)]
pub struct StdlibStruct {
    /// Struct name
    pub name: String,
    /// Fields
    pub fields: Vec<StructField>,
    /// Generic parameters
    pub generics: Vec<String>,
    /// Documentation
    pub doc: Option<String>,
}

/// Information about an enum in the stdlib.
#[derive(Clone, Debug)]
pub struct StdlibEnum {
    /// Enum name
    pub name: String,
    /// Variants
    pub variants: Vec<StdlibEnumVariant>,
    /// Generic parameters
    pub generics: Vec<String>,
    /// Documentation
    pub doc: Option<String>,
}

/// Enum variant info
#[derive(Clone, Debug)]
pub enum StdlibEnumVariant {
    Unit(String),
    Tuple(String, Vec<String>), // name, type names
}

/// Information about an interface in the stdlib.
#[derive(Clone, Debug)]
pub struct StdlibInterface {
    /// Interface name
    pub name: String,
    /// Method declarations (including default methods)
    pub methods: Vec<InterfaceMethod>,
    /// Extended interfaces
    pub extends: Vec<String>,
    /// Generic parameters
    pub generics: Vec<String>,
    /// Documentation
    pub doc: Option<String>,
}

/// Information about a module in the stdlib.
#[derive(Clone, Debug)]
pub struct StdlibModule {
    /// Module name
    pub name: String,
    /// Functions in this module
    pub functions: Vec<StdlibFunc>,
    /// Interfaces this module implements (target type inferred from method signatures)
    pub implements: Vec<String>,
    /// Whether this module is exported
    pub is_exported: bool,
    /// Documentation
    pub doc: Option<String>,
}

/// Information about a package in the stdlib.
#[derive(Clone, Debug, Default)]
pub struct StdlibPackage {
    /// Package name (e.g., "math", "log", "net.http")
    pub name: String,
    /// Modules in this package
    pub modules: HashMap<String, StdlibModule>,
    /// Structs in this package
    pub structs: HashMap<String, StdlibStruct>,
    /// Enums in this package
    pub enums: HashMap<String, StdlibEnum>,
    /// Interfaces in this package
    pub interfaces: HashMap<String, StdlibInterface>,
    /// Source files that define this package
    pub source_files: Vec<PathBuf>,
}

/// Index of all stdlib packages, modules, types, and functions.
///
/// This is the single source of truth for stdlib symbols, built by parsing
/// the `.arth` files in `stdlib/src/`.
#[derive(Clone, Debug, Default)]
pub struct StdlibIndex {
    /// Package name -> PackageInfo
    packages: HashMap<String, StdlibPackage>,
    /// Intrinsic name -> (package, module, function) for quick lookup
    intrinsic_to_func: HashMap<String, (String, String, String)>,
}

impl StdlibIndex {
    /// Create an empty stdlib index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load stdlib from a directory containing `.arth` files.
    ///
    /// This walks the directory tree, parses each `.arth` file, and builds
    /// an index of all packages, modules, types, and functions.
    pub fn load(stdlib_root: &Path) -> Result<Self, StdlibError> {
        let mut index = StdlibIndex::new();

        // Find all .arth files recursively
        let arth_files = find_arth_files(stdlib_root)?;

        // Parse each file and extract declarations
        for path in arth_files {
            index.load_file(&path)?;
        }

        Ok(index)
    }

    /// Load a single .arth file into the index.
    ///
    /// If `strict` is true, parse errors will cause the load to fail.
    /// If `strict` is false, parse errors will be logged but loading will continue
    /// with whatever was successfully parsed (useful for stdlib files with
    /// experimental syntax).
    fn load_file(&mut self, path: &Path) -> Result<(), StdlibError> {
        self.load_file_impl(path, false)
    }

    /// Load a single .arth file with configurable strictness.
    fn load_file_impl(&mut self, path: &Path, strict: bool) -> Result<(), StdlibError> {
        // Read file content
        let content = std::fs::read_to_string(path)?;

        // Create a SourceFile for parsing
        let sf = SourceFile {
            path: path.to_path_buf(),
            text: content,
        };

        // Parse the file (using a dummy reporter to collect errors)
        let mut reporter = Reporter::new();
        let ast = parse_file(&sf, &mut reporter);

        // Check for parse errors
        if reporter.has_errors() {
            if strict {
                let errors: Vec<String> = reporter
                    .diagnostics()
                    .iter()
                    .map(|d| d.message.clone())
                    .collect();
                return Err(StdlibError::ParseErrors(path.to_path_buf(), errors));
            }
            // In non-strict mode, continue with whatever was parsed
            // Some stdlib files may use experimental syntax not yet supported
        }

        // Extract package name
        let pkg_name = match &ast.package {
            Some(p) => {
                p.0.iter()
                    .map(|id| id.0.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
            }
            None => {
                if strict {
                    return Err(StdlibError::MissingPackage(path.to_path_buf()));
                }
                // In non-strict mode, skip files without package declarations
                return Ok(());
            }
        };

        // Get or create package entry
        let pkg = self
            .packages
            .entry(pkg_name.clone())
            .or_insert_with(|| StdlibPackage {
                name: pkg_name.clone(),
                ..Default::default()
            });

        pkg.source_files.push(path.to_path_buf());

        // Process declarations
        self.process_declarations(&pkg_name, &ast);

        Ok(())
    }

    /// Process declarations from a parsed AST.
    fn process_declarations(&mut self, pkg_name: &str, ast: &FileAst) {
        for decl in &ast.decls {
            match decl {
                Decl::Module(m) => self.process_module(pkg_name, m),
                Decl::Struct(s) => self.process_struct(pkg_name, s),
                Decl::Enum(e) => self.process_enum(pkg_name, e),
                Decl::Interface(i) => self.process_interface(pkg_name, i),
                _ => {} // Skip other declaration types for now
            }
        }
    }

    /// Process a module declaration.
    fn process_module(&mut self, pkg_name: &str, module: &ModuleDecl) {
        let mod_name = module.name.0.clone();
        let mut functions = Vec::new();

        for func in &module.items {
            let intrinsic = extract_intrinsic_name(&func.sig.attrs);

            // Register intrinsic mapping
            if let Some(ref intr_name) = intrinsic {
                self.intrinsic_to_func.insert(
                    intr_name.clone(),
                    (
                        pkg_name.to_string(),
                        mod_name.clone(),
                        func.sig.name.0.clone(),
                    ),
                );
            }

            functions.push(StdlibFunc {
                name: func.sig.name.0.clone(),
                sig: func.sig.clone(),
                intrinsic,
            });
        }

        let stdlib_module = StdlibModule {
            name: mod_name.clone(),
            functions,
            implements: module
                .implements
                .iter()
                .map(|np| namepath_to_string(np))
                .collect(),
            is_exported: module.is_exported,
            doc: module.doc.clone(),
        };

        if let Some(pkg) = self.packages.get_mut(pkg_name) {
            pkg.modules.insert(mod_name, stdlib_module);
        }
    }

    /// Process a struct declaration.
    fn process_struct(&mut self, pkg_name: &str, s: &StructDecl) {
        let stdlib_struct = StdlibStruct {
            name: s.name.0.clone(),
            fields: s.fields.clone(),
            generics: s.generics.iter().map(|g| g.name.0.clone()).collect(),
            doc: s.doc.clone(),
        };

        if let Some(pkg) = self.packages.get_mut(pkg_name) {
            pkg.structs.insert(s.name.0.clone(), stdlib_struct);
        }
    }

    /// Process an enum declaration.
    fn process_enum(&mut self, pkg_name: &str, e: &EnumDecl) {
        let variants = e
            .variants
            .iter()
            .map(|v| match v {
                EnumVariant::Unit { name, .. } => StdlibEnumVariant::Unit(name.0.clone()),
                EnumVariant::Tuple { name, types, .. } => StdlibEnumVariant::Tuple(
                    name.0.clone(),
                    types.iter().map(|t| namepath_to_string(t)).collect(),
                ),
            })
            .collect();

        let stdlib_enum = StdlibEnum {
            name: e.name.0.clone(),
            variants,
            generics: e.generics.iter().map(|g| g.name.0.clone()).collect(),
            doc: e.doc.clone(),
        };

        if let Some(pkg) = self.packages.get_mut(pkg_name) {
            pkg.enums.insert(e.name.0.clone(), stdlib_enum);
        }
    }

    /// Process an interface declaration.
    fn process_interface(&mut self, pkg_name: &str, i: &InterfaceDecl) {
        let stdlib_interface = StdlibInterface {
            name: i.name.0.clone(),
            methods: i.methods.clone(),
            extends: i.extends.iter().map(|np| namepath_to_string(np)).collect(),
            generics: i.generics.iter().map(|g| g.name.0.clone()).collect(),
            doc: i.doc.clone(),
        };

        if let Some(pkg) = self.packages.get_mut(pkg_name) {
            pkg.interfaces.insert(i.name.0.clone(), stdlib_interface);
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Public query API
    // ─────────────────────────────────────────────────────────────────────────

    /// Get a package by name.
    pub fn get_package(&self, name: &str) -> Option<&StdlibPackage> {
        self.packages.get(name)
    }

    /// Get all packages.
    pub fn packages(&self) -> impl Iterator<Item = &StdlibPackage> {
        self.packages.values()
    }

    /// Get all package names.
    pub fn package_names(&self) -> impl Iterator<Item = &str> {
        self.packages.keys().map(|s| s.as_str())
    }

    /// Check if a package exists.
    pub fn has_package(&self, name: &str) -> bool {
        self.packages.contains_key(name)
    }

    /// Get a module by package and module name.
    pub fn get_module(&self, pkg: &str, module: &str) -> Option<&StdlibModule> {
        self.packages.get(pkg).and_then(|p| p.modules.get(module))
    }

    /// Get a function signature by package, module, and function name.
    pub fn get_func_sig(&self, pkg: &str, module: &str, func: &str) -> Option<&FuncSig> {
        self.get_module(pkg, module)
            .and_then(|m| m.functions.iter().find(|f| f.name == func))
            .map(|f| &f.sig)
    }

    /// Get a function by intrinsic name.
    pub fn get_func_by_intrinsic(&self, intrinsic_name: &str) -> Option<&StdlibFunc> {
        self.intrinsic_to_func
            .get(intrinsic_name)
            .and_then(|(pkg, module, func)| {
                self.get_module(pkg, module)
                    .and_then(|m| m.functions.iter().find(|f| f.name == *func))
            })
    }

    /// Look up intrinsic info by intrinsic name.
    pub fn get_intrinsic(&self, name: &str) -> Option<&'static intrinsics::Intrinsic> {
        intrinsics::lookup(name)
    }

    /// Get a struct by package and struct name.
    pub fn get_struct(&self, pkg: &str, name: &str) -> Option<&StdlibStruct> {
        self.packages.get(pkg).and_then(|p| p.structs.get(name))
    }

    /// Get an enum by package and enum name.
    pub fn get_enum(&self, pkg: &str, name: &str) -> Option<&StdlibEnum> {
        self.packages.get(pkg).and_then(|p| p.enums.get(name))
    }

    /// Get an interface by package and interface name.
    pub fn get_interface(&self, pkg: &str, name: &str) -> Option<&StdlibInterface> {
        self.packages.get(pkg).and_then(|p| p.interfaces.get(name))
    }

    /// Get total count of packages.
    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    /// Get total count of modules across all packages.
    pub fn module_count(&self) -> usize {
        self.packages.values().map(|p| p.modules.len()).sum()
    }

    /// Get total count of functions across all modules.
    pub fn function_count(&self) -> usize {
        self.packages
            .values()
            .flat_map(|p| p.modules.values())
            .map(|m| m.functions.len())
            .sum()
    }

    /// Get count of intrinsic-backed functions.
    pub fn intrinsic_count(&self) -> usize {
        self.intrinsic_to_func.len()
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Integration with resolve phase
    // ─────────────────────────────────────────────────────────────────────────

    /// Get all symbols from a package as (name, kind, visibility) tuples.
    /// This is used by the resolver to populate PackageSymbols.
    pub fn get_package_symbols(
        &self,
        pkg_name: &str,
    ) -> Vec<(String, StdlibSymbolKind, Visibility)> {
        let Some(pkg) = self.packages.get(pkg_name) else {
            return Vec::new();
        };

        let mut symbols = Vec::new();

        // Add modules
        for (name, m) in &pkg.modules {
            let vis = if m.is_exported {
                Visibility::Public
            } else {
                Visibility::Default
            };
            symbols.push((name.clone(), StdlibSymbolKind::Module, vis));
        }

        // Add structs
        for (name, _s) in &pkg.structs {
            symbols.push((name.clone(), StdlibSymbolKind::Struct, Visibility::Public));
        }

        // Add enums
        for (name, _e) in &pkg.enums {
            symbols.push((name.clone(), StdlibSymbolKind::Enum, Visibility::Public));
        }

        // Add interfaces
        for (name, _i) in &pkg.interfaces {
            symbols.push((
                name.clone(),
                StdlibSymbolKind::Interface,
                Visibility::Public,
            ));
        }

        symbols
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Integration with typeck phase
    // ─────────────────────────────────────────────────────────────────────────

    /// Get all function signatures for a module.
    /// Returns (function_name, FuncSig) pairs.
    pub fn get_module_functions(&self, pkg: &str, module: &str) -> Vec<(&str, &FuncSig)> {
        self.get_module(pkg, module)
            .map(|m| {
                m.functions
                    .iter()
                    .map(|f| (f.name.as_str(), &f.sig))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all module function signatures across the entire stdlib.
    /// Returns ((package, module, function), FuncSig) tuples for seeding typeck.
    pub fn all_module_functions(&self) -> Vec<((String, String, String), &FuncSig)> {
        let mut result = Vec::new();

        for (pkg_name, pkg) in &self.packages {
            for (mod_name, module) in &pkg.modules {
                for func in &module.functions {
                    result.push((
                        (pkg_name.clone(), mod_name.clone(), func.name.clone()),
                        &func.sig,
                    ));
                }
            }
        }

        result
    }
}

/// Symbol kinds for stdlib entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdlibSymbolKind {
    Module,
    Struct,
    Enum,
    Interface,
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper functions
// ─────────────────────────────────────────────────────────────────────────────

/// Find all .arth files recursively in a directory.
fn find_arth_files(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    find_arth_files_recursive(dir, &mut files)?;
    Ok(files)
}

fn find_arth_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    if !dir.is_dir() {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            find_arth_files_recursive(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "arth") {
            files.push(path);
        }
    }

    Ok(())
}

/// Convert a NamePath to a dot-separated string.
fn namepath_to_string(np: &NamePath) -> String {
    np.path
        .iter()
        .map(|id| id.0.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

/// Extract the intrinsic name from function attributes.
fn extract_intrinsic_name(attrs: &[Attr]) -> Option<String> {
    for attr in attrs {
        // Check if attribute name is "intrinsic"
        if attr.name.path.len() == 1 && attr.name.path[0].0 == "intrinsic" {
            if let Some(args) = &attr.args {
                // Parse the argument string - expected format: "name"
                if let Some(name) = intrinsics::parse_intrinsic_attr(args) {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_intrinsic_name() {
        use crate::compiler::ast::{Attr, Ident, NamePath};

        let attrs = vec![Attr {
            name: NamePath::new(vec![Ident("intrinsic".to_string())]),
            args: Some(r#""math.sqrt""#.to_string()),
        }];

        assert_eq!(
            extract_intrinsic_name(&attrs),
            Some("math.sqrt".to_string())
        );
    }

    #[test]
    fn test_extract_intrinsic_name_no_attr() {
        let attrs: Vec<Attr> = vec![];
        assert_eq!(extract_intrinsic_name(&attrs), None);
    }

    #[test]
    fn test_namepath_to_string() {
        use crate::compiler::ast::{Ident, NamePath};

        let np = NamePath::new(vec![Ident("math".to_string()), Ident("Math".to_string())]);
        assert_eq!(namepath_to_string(&np), "math.Math");
    }

    #[test]
    fn test_stdlib_index_new() {
        let index = StdlibIndex::new();
        assert_eq!(index.package_count(), 0);
    }

    #[test]
    fn test_load_stdlib_from_project() {
        // This test requires the actual stdlib directory to exist
        let stdlib_path = Path::new("stdlib/src");
        if !stdlib_path.exists() {
            return; // Skip if stdlib doesn't exist (e.g., in CI without full checkout)
        }

        let index = StdlibIndex::load(stdlib_path).expect("Failed to load stdlib");

        // Verify we loaded something
        assert!(
            index.package_count() > 0,
            "Should have loaded at least one package"
        );

        // Verify math package exists
        assert!(index.has_package("math"), "Should have math package");

        // Verify Math module exists in math package
        let math_module = index.get_module("math", "Math");
        assert!(
            math_module.is_some(),
            "Should have Math module in math package"
        );

        // Verify sqrt function exists
        let sqrt = index.get_func_sig("math", "Math", "sqrt");
        assert!(sqrt.is_some(), "Should have sqrt function");

        // Verify intrinsic mapping
        let sqrt_func = index.get_func_by_intrinsic("math.sqrt");
        assert!(
            sqrt_func.is_some(),
            "Should be able to look up sqrt by intrinsic name"
        );
    }

    #[test]
    fn test_stdlib_intrinsic_count() {
        let stdlib_path = Path::new("stdlib/src");
        if !stdlib_path.exists() {
            return;
        }

        let index = StdlibIndex::load(stdlib_path).expect("Failed to load stdlib");

        // We should have some intrinsic-backed functions (at least Math functions)
        assert!(
            index.intrinsic_count() > 0,
            "Should have at least some intrinsic functions"
        );
    }

    #[test]
    fn test_stdlib_index_summary() {
        let stdlib_path = Path::new("stdlib/src");
        if !stdlib_path.exists() {
            return;
        }

        let index = StdlibIndex::load(stdlib_path).expect("Failed to load stdlib");

        // Print summary for debugging
        eprintln!("StdlibIndex summary:");
        eprintln!("  Packages: {}", index.package_count());
        eprintln!("  Modules: {}", index.module_count());
        eprintln!("  Functions: {}", index.function_count());
        eprintln!("  Intrinsics: {}", index.intrinsic_count());

        // Verify specific expected packages
        for pkg_name in ["math", "log", "concurrent", "time", "arth"] {
            if index.has_package(pkg_name) {
                let pkg = index.get_package(pkg_name).unwrap();
                eprintln!("  Package '{}': {} modules", pkg_name, pkg.modules.len());
            }
        }

        // Verify math intrinsics are present
        let math_sqrt = index.get_func_by_intrinsic("math.sqrt");
        assert!(math_sqrt.is_some(), "math.sqrt intrinsic should be present");
    }

    #[test]
    fn test_db_stdlib_parsing() {
        let stdlib_path = Path::new("stdlib/src");
        if !stdlib_path.exists() {
            return;
        }

        let index = StdlibIndex::load(stdlib_path).expect("Failed to load stdlib");

        // Verify db package exists
        assert!(index.has_package("db"), "Should have db package");

        // Verify db.sqlite package exists
        assert!(
            index.has_package("db.sqlite"),
            "Should have db.sqlite package"
        );

        // Verify db.postgres package exists
        assert!(
            index.has_package("db.postgres"),
            "Should have db.postgres package"
        );

        // Verify core db types exist
        let db_pkg = index.get_package("db").expect("db package should exist");

        // Check structs
        assert!(
            db_pkg.structs.contains_key("ValueBox"),
            "db should have ValueBox struct"
        );
        assert!(
            db_pkg.structs.contains_key("Param"),
            "db should have Param struct"
        );
        assert!(
            db_pkg.structs.contains_key("Column"),
            "db should have Column struct"
        );
        assert!(
            db_pkg.structs.contains_key("Row"),
            "db should have Row struct"
        );
        assert!(
            db_pkg.structs.contains_key("Connection"),
            "db should have Connection struct"
        );
        assert!(
            db_pkg.structs.contains_key("Statement"),
            "db should have Statement struct"
        );

        // Check enums
        assert!(
            db_pkg.enums.contains_key("Value"),
            "db should have Value enum"
        );
        assert!(
            db_pkg.enums.contains_key("ValueType"),
            "db should have ValueType enum"
        );
        assert!(
            db_pkg.enums.contains_key("DriverType"),
            "db should have DriverType enum"
        );
        assert!(
            db_pkg.enums.contains_key("ConnectionState"),
            "db should have ConnectionState enum"
        );
        assert!(
            db_pkg.enums.contains_key("ParamStyle"),
            "db should have ParamStyle enum"
        );

        // Check exception structs
        assert!(
            db_pkg.structs.contains_key("ConnectionError"),
            "db should have ConnectionError struct"
        );
        assert!(
            db_pkg.structs.contains_key("QueryError"),
            "db should have QueryError struct"
        );
        assert!(
            db_pkg.structs.contains_key("PrepareError"),
            "db should have PrepareError struct"
        );
        assert!(
            db_pkg.structs.contains_key("BindError"),
            "db should have BindError struct"
        );
        assert!(
            db_pkg.structs.contains_key("TransactionError"),
            "db should have TransactionError struct"
        );
        assert!(
            db_pkg.structs.contains_key("PoolExhaustedError"),
            "db should have PoolExhaustedError struct"
        );
        assert!(
            db_pkg.structs.contains_key("TimeoutError"),
            "db should have TimeoutError struct"
        );
        assert!(
            db_pkg.structs.contains_key("TypeMismatchError"),
            "db should have TypeMismatchError struct"
        );

        // Check interfaces
        assert!(
            db_pkg.interfaces.contains_key("Driver"),
            "db should have Driver interface"
        );
        assert!(
            db_pkg.interfaces.contains_key("DbError"),
            "db should have DbError interface"
        );

        // Check modules
        // Note: Module is named ValueBoxFns to avoid conflict with ValueBox struct
        assert!(
            db_pkg.modules.contains_key("ValueBoxFns"),
            "db should have ValueBoxFns module"
        );
        assert!(
            db_pkg.modules.contains_key("Param"),
            "db should have Param module"
        );
        assert!(
            db_pkg.modules.contains_key("Row"),
            "db should have Row module"
        );
        assert!(
            db_pkg.modules.contains_key("Column"),
            "db should have Column module"
        );
        assert!(
            db_pkg.modules.contains_key("Connection"),
            "db should have Connection module"
        );
        assert!(
            db_pkg.modules.contains_key("Statement"),
            "db should have Statement module"
        );

        // Verify SQLite module
        let sqlite_pkg = index
            .get_package("db.sqlite")
            .expect("db.sqlite package should exist");
        assert!(
            sqlite_pkg.modules.contains_key("Sqlite"),
            "db.sqlite should have Sqlite module"
        );

        // Verify PostgreSQL module
        let pg_pkg = index
            .get_package("db.postgres")
            .expect("db.postgres package should exist");
        assert!(
            pg_pkg.modules.contains_key("Postgres"),
            "db.postgres should have Postgres module"
        );

        // Print db package summary
        eprintln!("db package summary:");
        eprintln!("  Structs: {}", db_pkg.structs.len());
        eprintln!("  Enums: {}", db_pkg.enums.len());
        eprintln!("  Interfaces: {}", db_pkg.interfaces.len());
        eprintln!("  Modules: {}", db_pkg.modules.len());
    }

    #[test]
    fn test_mail_stdlib_parsing() {
        let stdlib_path = Path::new("stdlib/src");
        if !stdlib_path.exists() {
            return;
        }

        let index = StdlibIndex::load(stdlib_path).expect("Failed to load stdlib");

        // Verify mail package exists
        assert!(index.has_package("mail"), "Should have mail package");

        // Verify protocol-specific packages exist
        assert!(
            index.has_package("mail.smtp"),
            "Should have mail.smtp package"
        );
        assert!(
            index.has_package("mail.imap"),
            "Should have mail.imap package"
        );
        assert!(
            index.has_package("mail.pop3"),
            "Should have mail.pop3 package"
        );

        // Verify core mail types exist
        let mail_pkg = index
            .get_package("mail")
            .expect("mail package should exist");

        // Check interfaces
        assert!(
            mail_pkg.interfaces.contains_key("MailError"),
            "mail should have MailError interface"
        );
        assert!(
            mail_pkg.interfaces.contains_key("Address"),
            "mail should have Address interface"
        );
        assert!(
            mail_pkg.interfaces.contains_key("Part"),
            "mail should have Part interface"
        );
        assert!(
            mail_pkg.interfaces.contains_key("Message"),
            "mail should have Message interface"
        );

        // Check structs
        assert!(
            mail_pkg.structs.contains_key("InternetAddress"),
            "mail should have InternetAddress struct"
        );
        assert!(
            mail_pkg.structs.contains_key("GroupAddress"),
            "mail should have GroupAddress struct"
        );
        assert!(
            mail_pkg.structs.contains_key("ContentType"),
            "mail should have ContentType struct"
        );
        assert!(
            mail_pkg.structs.contains_key("Headers"),
            "mail should have Headers struct"
        );
        assert!(
            mail_pkg.structs.contains_key("MimeMessage"),
            "mail should have MimeMessage struct"
        );
        assert!(
            mail_pkg.structs.contains_key("Session"),
            "mail should have Session struct"
        );
        assert!(
            mail_pkg.structs.contains_key("SendResult"),
            "mail should have SendResult struct"
        );
        assert!(
            mail_pkg.structs.contains_key("MimePart"),
            "mail should have MimePart struct"
        );

        // Check exception structs
        assert!(
            mail_pkg.structs.contains_key("AuthenticationError"),
            "mail should have AuthenticationError struct"
        );
        assert!(
            mail_pkg.structs.contains_key("ConnectionError"),
            "mail should have ConnectionError struct"
        );
        assert!(
            mail_pkg.structs.contains_key("TransportError"),
            "mail should have TransportError struct"
        );
        assert!(
            mail_pkg.structs.contains_key("AddressError"),
            "mail should have AddressError struct"
        );

        // Check enums
        assert!(
            mail_pkg.enums.contains_key("Protocol"),
            "mail should have Protocol enum"
        );
        assert!(
            mail_pkg.enums.contains_key("AuthType"),
            "mail should have AuthType enum"
        );
        assert!(
            mail_pkg.enums.contains_key("TransferEncoding"),
            "mail should have TransferEncoding enum"
        );
        assert!(
            mail_pkg.enums.contains_key("TransportState"),
            "mail should have TransportState enum"
        );
        assert!(
            mail_pkg.enums.contains_key("StoreState"),
            "mail should have StoreState enum"
        );
        assert!(
            mail_pkg.enums.contains_key("FolderMode"),
            "mail should have FolderMode enum"
        );

        // Check modules
        assert!(
            mail_pkg.modules.contains_key("InternetAddress"),
            "mail should have InternetAddress module"
        );
        assert!(
            mail_pkg.modules.contains_key("ContentType"),
            "mail should have ContentType module"
        );
        assert!(
            mail_pkg.modules.contains_key("Headers"),
            "mail should have Headers module"
        );
        assert!(
            mail_pkg.modules.contains_key("MessageBuilder"),
            "mail should have MessageBuilder module"
        );
        assert!(
            mail_pkg.modules.contains_key("SessionBuilder"),
            "mail should have SessionBuilder module"
        );
        assert!(
            mail_pkg.modules.contains_key("Transport"),
            "mail should have Transport module"
        );
        assert!(
            mail_pkg.modules.contains_key("Store"),
            "mail should have Store module"
        );
        assert!(
            mail_pkg.modules.contains_key("Folder"),
            "mail should have Folder module"
        );
        assert!(
            mail_pkg.modules.contains_key("Mail"),
            "mail should have Mail module"
        );
        assert!(
            mail_pkg.modules.contains_key("SearchTerm"),
            "mail should have SearchTerm module"
        );

        // Check encoding modules
        assert!(
            mail_pkg.modules.contains_key("Base64"),
            "mail should have Base64 module"
        );
        assert!(
            mail_pkg.modules.contains_key("QuotedPrintable"),
            "mail should have QuotedPrintable module"
        );
        assert!(
            mail_pkg.modules.contains_key("Charset"),
            "mail should have Charset module"
        );
        assert!(
            mail_pkg.modules.contains_key("EncodedWord"),
            "mail should have EncodedWord module"
        );

        // Verify SMTP module
        let smtp_pkg = index
            .get_package("mail.smtp")
            .expect("mail.smtp package should exist");
        assert!(
            smtp_pkg.modules.contains_key("Smtp"),
            "mail.smtp should have Smtp module"
        );

        // Verify IMAP module
        let imap_pkg = index
            .get_package("mail.imap")
            .expect("mail.imap package should exist");
        assert!(
            imap_pkg.modules.contains_key("Imap"),
            "mail.imap should have Imap module"
        );

        // Verify POP3 module
        let pop3_pkg = index
            .get_package("mail.pop3")
            .expect("mail.pop3 package should exist");
        assert!(
            pop3_pkg.modules.contains_key("Pop3"),
            "mail.pop3 should have Pop3 module"
        );

        // Print mail package summary
        eprintln!("mail package summary:");
        eprintln!("  Structs: {}", mail_pkg.structs.len());
        eprintln!("  Enums: {}", mail_pkg.enums.len());
        eprintln!("  Interfaces: {}", mail_pkg.interfaces.len());
        eprintln!("  Modules: {}", mail_pkg.modules.len());
    }
}
