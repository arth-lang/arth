//! Symbol table and export table for TypeScript → Arth HIR.
//!
//! This module provides:
//! - `SymbolTable`: tracks all symbols in a TS module (types, functions, providers)
//! - `ExportTable`: tracks all exports with fully-qualified names and arity
//! - Cross-file symbol resolution support
//! - Arth import statement generation

use std::collections::{BTreeMap, HashMap};

use arth::compiler::hir::{HirDecl, HirFile, HirFunc};
use serde::{Deserialize, Serialize};

/// Kind of symbol in the symbol table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    /// Function definition
    Function,
    /// Struct/class type
    Struct,
    /// Enum type
    Enum,
    /// Interface type
    Interface,
    /// Type alias
    TypeAlias,
    /// Provider (state container)
    Provider,
    /// Imported symbol from another module
    Import,
}

/// A symbol in the symbol table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    /// Local name of the symbol
    pub name: String,
    /// Kind of symbol
    pub kind: SymbolKind,
    /// Fully-qualified name (Module.name)
    pub qualified_name: String,
    /// Whether this symbol is exported
    pub is_exported: bool,
    /// Arity for functions (None for non-functions)
    pub arity: Option<u8>,
    /// Source module for imports (None for local symbols)
    pub source_module: Option<String>,
}

/// Symbol table for a TypeScript module.
///
/// Tracks all declarations in the module, supporting:
/// - Type lookups for resolution
/// - Export collection
/// - Import tracking
#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    /// Module name (derived from file path or explicit)
    pub module_name: String,
    /// All symbols indexed by local name
    symbols: HashMap<String, Symbol>,
    /// Import bindings: local name → (source, original name)
    imports: HashMap<String, (String, String)>,
}

impl SymbolTable {
    /// Create a new symbol table for a module.
    pub fn new(module_name: String) -> Self {
        Self {
            module_name,
            symbols: HashMap::new(),
            imports: HashMap::new(),
        }
    }

    /// Build a symbol table from an HIR file.
    pub fn from_hir(hir: &HirFile) -> Self {
        let module_name = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m.name.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "Main".to_string());

        let mut table = Self::new(module_name.clone());

        for decl in &hir.decls {
            match decl {
                HirDecl::Module(m) => {
                    for func in &m.funcs {
                        let is_exported = func.sig.attrs.iter().any(|a| a.name == "export");
                        table.add_function(&func.sig.name, is_exported, &m.name, func);
                    }
                }
                HirDecl::Struct(s) => {
                    table.add_symbol(Symbol {
                        name: s.name.clone(),
                        kind: SymbolKind::Struct,
                        qualified_name: format!("{}.{}", module_name, s.name),
                        is_exported: true, // Structs lowered from TS are typically exported
                        arity: None,
                        source_module: None,
                    });
                }
                HirDecl::Enum(e) => {
                    table.add_symbol(Symbol {
                        name: e.name.clone(),
                        kind: SymbolKind::Enum,
                        qualified_name: format!("{}.{}", module_name, e.name),
                        is_exported: true,
                        arity: None,
                        source_module: None,
                    });
                }
                HirDecl::Interface(i) => {
                    table.add_symbol(Symbol {
                        name: i.name.clone(),
                        kind: SymbolKind::Interface,
                        qualified_name: format!("{}.{}", module_name, i.name),
                        is_exported: true,
                        arity: None,
                        source_module: None,
                    });
                }
                HirDecl::Provider(p) => {
                    table.add_symbol(Symbol {
                        name: p.name.clone(),
                        kind: SymbolKind::Provider,
                        qualified_name: format!("{}.{}", module_name, p.name),
                        is_exported: true,
                        arity: None,
                        source_module: None,
                    });
                }
                _ => {}
            }
        }

        // Parse import notes to track imports
        for note in &hir.notes {
            if let Some((local, source, _canonical)) = parse_import_note(&note.message) {
                table.add_import(local, source.clone(), source);
            }
        }

        table
    }

    /// Add a function symbol.
    fn add_function(&mut self, name: &str, is_exported: bool, module_name: &str, func: &HirFunc) {
        let arity = func.sig.params.len() as u8;
        self.add_symbol(Symbol {
            name: name.to_string(),
            kind: SymbolKind::Function,
            qualified_name: format!("{}.{}", module_name, name),
            is_exported,
            arity: Some(arity),
            source_module: None,
        });
    }

    /// Add a symbol to the table.
    pub fn add_symbol(&mut self, symbol: Symbol) {
        self.symbols.insert(symbol.name.clone(), symbol);
    }

    /// Add an import binding.
    pub fn add_import(&mut self, local: String, source: String, original: String) {
        self.imports
            .insert(local.clone(), (source.clone(), original.clone()));
        self.add_symbol(Symbol {
            name: local,
            kind: SymbolKind::Import,
            qualified_name: format!("{}.{}", source, original),
            is_exported: false,
            arity: None,
            source_module: Some(source),
        });
    }

    /// Look up a symbol by name.
    pub fn get(&self, name: &str) -> Option<&Symbol> {
        self.symbols.get(name)
    }

    /// Check if a name is defined in this table.
    pub fn contains(&self, name: &str) -> bool {
        self.symbols.contains_key(name)
    }

    /// Get all exported symbols.
    pub fn exports(&self) -> impl Iterator<Item = &Symbol> {
        self.symbols.values().filter(|s| s.is_exported)
    }

    /// Get all import bindings.
    pub fn import_bindings(&self) -> &HashMap<String, (String, String)> {
        &self.imports
    }

    /// Resolve a type reference to its fully-qualified name.
    /// Returns None if the type is not found.
    pub fn resolve_type(&self, name: &str) -> Option<String> {
        if let Some(symbol) = self.symbols.get(name) {
            match symbol.kind {
                SymbolKind::Struct
                | SymbolKind::Enum
                | SymbolKind::Interface
                | SymbolKind::TypeAlias
                | SymbolKind::Provider => Some(symbol.qualified_name.clone()),
                SymbolKind::Import => Some(symbol.qualified_name.clone()),
                _ => None,
            }
        } else {
            None
        }
    }

    /// Resolve a function reference to its fully-qualified name.
    pub fn resolve_function(&self, name: &str) -> Option<String> {
        if let Some(symbol) = self.symbols.get(name) {
            match symbol.kind {
                SymbolKind::Function | SymbolKind::Import => Some(symbol.qualified_name.clone()),
                _ => None,
            }
        } else {
            None
        }
    }
}

/// Entry in the export table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportEntry {
    /// Local name of the export
    pub name: String,
    /// Fully-qualified name (Module.name)
    pub qualified_name: String,
    /// Kind of export
    pub kind: SymbolKind,
    /// Arity for functions (0 for non-functions)
    pub arity: u8,
}

/// Export table for a TypeScript module.
///
/// Tracks all exports with:
/// - Fully-qualified names
/// - Export kinds (function, type, provider)
/// - Function arity
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExportTable {
    /// Module name
    pub module_name: String,
    /// Exports ordered by name
    pub entries: Vec<ExportEntry>,
    /// Quick lookup by name
    #[serde(skip)]
    by_name: BTreeMap<String, usize>,
}

impl ExportTable {
    /// Create a new export table for a module.
    pub fn new(module_name: String) -> Self {
        Self {
            module_name,
            entries: Vec::new(),
            by_name: BTreeMap::new(),
        }
    }

    /// Build an export table from HIR files.
    pub fn from_hirs(hirs: &[HirFile]) -> Self {
        let module_name = hirs
            .iter()
            .flat_map(|h| h.decls.iter())
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m.name.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "Main".to_string());

        let mut table = Self::new(module_name.clone());

        for hir in hirs {
            for decl in &hir.decls {
                match decl {
                    HirDecl::Module(m) => {
                        for func in &m.funcs {
                            let is_exported = func.sig.attrs.iter().any(|a| a.name == "export");
                            if is_exported {
                                table.add_function(&func.sig.name, &m.name, func);
                            }
                        }
                    }
                    HirDecl::Struct(s) => {
                        table.add_type(&s.name, SymbolKind::Struct);
                    }
                    HirDecl::Enum(e) => {
                        table.add_type(&e.name, SymbolKind::Enum);
                    }
                    HirDecl::Interface(i) => {
                        table.add_type(&i.name, SymbolKind::Interface);
                    }
                    HirDecl::Provider(p) => {
                        table.add_type(&p.name, SymbolKind::Provider);
                    }
                    _ => {}
                }
            }
        }

        table
    }

    /// Add a function export.
    fn add_function(&mut self, name: &str, module_name: &str, func: &HirFunc) {
        let arity = func.sig.params.len() as u8;
        self.add_entry(ExportEntry {
            name: name.to_string(),
            qualified_name: format!("{}.{}", module_name, name),
            kind: SymbolKind::Function,
            arity,
        });
    }

    /// Add a type export.
    fn add_type(&mut self, name: &str, kind: SymbolKind) {
        self.add_entry(ExportEntry {
            name: name.to_string(),
            qualified_name: format!("{}.{}", self.module_name, name),
            kind,
            arity: 0,
        });
    }

    /// Add an entry to the table.
    pub fn add_entry(&mut self, entry: ExportEntry) {
        let idx = self.entries.len();
        self.by_name.insert(entry.name.clone(), idx);
        self.entries.push(entry);
    }

    /// Look up an export by name.
    pub fn get(&self, name: &str) -> Option<&ExportEntry> {
        self.by_name.get(name).map(|&idx| &self.entries[idx])
    }

    /// Get the entry point function (conventionally "main").
    pub fn entry_point(&self) -> Option<&ExportEntry> {
        self.get("main")
    }

    /// Iterate over all exports.
    pub fn iter(&self) -> impl Iterator<Item = &ExportEntry> {
        self.entries.iter()
    }

    /// Get function exports only.
    pub fn functions(&self) -> impl Iterator<Item = &ExportEntry> {
        self.entries
            .iter()
            .filter(|e| e.kind == SymbolKind::Function)
    }

    /// Get type exports only.
    pub fn types(&self) -> impl Iterator<Item = &ExportEntry> {
        self.entries.iter().filter(|e| {
            matches!(
                e.kind,
                SymbolKind::Struct
                    | SymbolKind::Enum
                    | SymbolKind::Interface
                    | SymbolKind::TypeAlias
            )
        })
    }
}

/// Arth import statement representation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArthImport {
    /// Package path (e.g., "arth.log")
    pub package: String,
    /// Imported items (e.g., ["Logger", "info"])
    pub items: Vec<String>,
}

/// Generate Arth import statements from TS imports recorded in HIR notes.
///
/// Converts TS import notes to Arth import syntax:
/// - `import { Logger } from "arth:log"` → `import arth.log.Logger;`
/// - `import * as log from "arth:log"` → `import arth.log.*;`
pub fn generate_arth_imports(hir: &HirFile) -> Vec<ArthImport> {
    let mut imports_by_package: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for note in &hir.notes {
        if let Some((local, source, canonical)) = parse_import_note(&note.message) {
            // Only process arth:* imports for Arth import generation
            if source.starts_with("arth:") {
                let package = source.replace("arth:", "arth.");
                let item = if canonical.contains('.') {
                    // Use the last part of the canonical name
                    canonical.rsplit('.').next().unwrap_or(&local).to_string()
                } else {
                    local
                };
                imports_by_package.entry(package).or_default().push(item);
            }
        }
    }

    imports_by_package
        .into_iter()
        .map(|(package, items)| ArthImport { package, items })
        .collect()
}

/// Parse an import note message.
/// Returns (local_name, source, canonical_name) if valid.
fn parse_import_note(message: &str) -> Option<(String, String, String)> {
    // Expected shapes:
    // ts-import:named local=foo source=arth:log canonical=arth.log.Logger
    // ts-import:default local=Foo source=arth:log canonical=arth.log
    // ts-import:namespace local=log source=arth:log canonical=arth.log
    let rest = message.strip_prefix("ts-import:")?;
    let mut parts = rest.split_whitespace();
    let _kind = parts.next()?; // named/default/namespace (currently unused)
    let mut local = None;
    let mut source = None;
    let mut canonical = None;
    for part in parts {
        if let Some((k, v)) = part.split_once('=') {
            match k {
                "local" => local = Some(v.to_string()),
                "source" => source = Some(v.to_string()),
                "canonical" => canonical = Some(v.to_string()),
                _ => {}
            }
        }
    }
    let local = local?;
    let source = source?;
    let canonical = canonical?;
    Some((local, source, canonical))
}

/// Cross-file symbol resolver.
///
/// Resolves type references across multiple HIR files by
/// maintaining a global symbol table from all files.
#[derive(Debug, Default)]
pub struct CrossFileResolver {
    /// Symbol tables indexed by module name
    tables: HashMap<String, SymbolTable>,
}

impl CrossFileResolver {
    /// Create a new cross-file resolver.
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
        }
    }

    /// Build a resolver from multiple HIR files.
    pub fn from_hirs(hirs: &[HirFile]) -> Self {
        let mut resolver = Self::new();
        for hir in hirs {
            let table = SymbolTable::from_hir(hir);
            resolver.tables.insert(table.module_name.clone(), table);
        }
        resolver
    }

    /// Resolve a type reference.
    /// Tries local resolution first, then checks imported modules.
    pub fn resolve_type(&self, current_module: &str, name: &str) -> Option<String> {
        // Try current module first
        if let Some(table) = self.tables.get(current_module) {
            if let Some(qualified) = table.resolve_type(name) {
                return Some(qualified);
            }

            // Check imports
            if let Some((source, _original)) = table.import_bindings().get(name)
                && let Some(imported_table) = self.tables.get(source)
            {
                return imported_table.resolve_type(name);
            }
        }

        None
    }

    /// Resolve a function reference.
    pub fn resolve_function(&self, current_module: &str, name: &str) -> Option<String> {
        if let Some(table) = self.tables.get(current_module) {
            if let Some(qualified) = table.resolve_function(name) {
                return Some(qualified);
            }

            if let Some((source, _original)) = table.import_bindings().get(name)
                && let Some(imported_table) = self.tables.get(source)
            {
                return imported_table.resolve_function(name);
            }
        }

        None
    }

    /// Get all symbol tables.
    pub fn tables(&self) -> &HashMap<String, SymbolTable> {
        &self.tables
    }

    /// Get a symbol table by module name.
    pub fn get_table(&self, module_name: &str) -> Option<&SymbolTable> {
        self.tables.get(module_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_table_add_and_get() {
        let mut table = SymbolTable::new("TestModule".to_string());

        table.add_symbol(Symbol {
            name: "foo".to_string(),
            kind: SymbolKind::Function,
            qualified_name: "TestModule.foo".to_string(),
            is_exported: true,
            arity: Some(2),
            source_module: None,
        });

        let symbol = table.get("foo").unwrap();
        assert_eq!(symbol.name, "foo");
        assert_eq!(symbol.kind, SymbolKind::Function);
        assert_eq!(symbol.qualified_name, "TestModule.foo");
        assert!(symbol.is_exported);
        assert_eq!(symbol.arity, Some(2));
    }

    #[test]
    fn test_symbol_table_exports() {
        let mut table = SymbolTable::new("TestModule".to_string());

        table.add_symbol(Symbol {
            name: "publicFn".to_string(),
            kind: SymbolKind::Function,
            qualified_name: "TestModule.publicFn".to_string(),
            is_exported: true,
            arity: Some(0),
            source_module: None,
        });

        table.add_symbol(Symbol {
            name: "privateFn".to_string(),
            kind: SymbolKind::Function,
            qualified_name: "TestModule.privateFn".to_string(),
            is_exported: false,
            arity: Some(1),
            source_module: None,
        });

        let exports: Vec<_> = table.exports().collect();
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].name, "publicFn");
    }

    #[test]
    fn test_symbol_table_imports() {
        let mut table = SymbolTable::new("TestModule".to_string());

        table.add_import(
            "Logger".to_string(),
            "arth.log".to_string(),
            "Logger".to_string(),
        );

        assert!(table.contains("Logger"));
        let symbol = table.get("Logger").unwrap();
        assert_eq!(symbol.kind, SymbolKind::Import);
        assert_eq!(symbol.qualified_name, "arth.log.Logger");
        assert_eq!(symbol.source_module, Some("arth.log".to_string()));
    }

    #[test]
    fn test_symbol_table_resolve_type() {
        let mut table = SymbolTable::new("TestModule".to_string());

        table.add_symbol(Symbol {
            name: "User".to_string(),
            kind: SymbolKind::Struct,
            qualified_name: "TestModule.User".to_string(),
            is_exported: true,
            arity: None,
            source_module: None,
        });

        assert_eq!(
            table.resolve_type("User"),
            Some("TestModule.User".to_string())
        );
        assert_eq!(table.resolve_type("Unknown"), None);
    }

    #[test]
    fn test_export_table_functions() {
        let mut table = ExportTable::new("TestModule".to_string());

        table.add_entry(ExportEntry {
            name: "add".to_string(),
            qualified_name: "TestModule.add".to_string(),
            kind: SymbolKind::Function,
            arity: 2,
        });

        table.add_entry(ExportEntry {
            name: "User".to_string(),
            qualified_name: "TestModule.User".to_string(),
            kind: SymbolKind::Struct,
            arity: 0,
        });

        let functions: Vec<_> = table.functions().collect();
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].name, "add");
        assert_eq!(functions[0].arity, 2);

        let types: Vec<_> = table.types().collect();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].name, "User");
    }

    #[test]
    fn test_export_table_entry_point() {
        let mut table = ExportTable::new("TestModule".to_string());

        table.add_entry(ExportEntry {
            name: "main".to_string(),
            qualified_name: "TestModule.main".to_string(),
            kind: SymbolKind::Function,
            arity: 0,
        });

        let entry = table.entry_point().unwrap();
        assert_eq!(entry.name, "main");
        assert_eq!(entry.qualified_name, "TestModule.main");
    }

    #[test]
    fn test_parse_import_note() {
        let note = "ts-import:named local=Logger source=arth:log canonical=arth.log.Logger";
        let (local, source, canonical) = parse_import_note(note).unwrap();
        assert_eq!(local, "Logger");
        assert_eq!(source, "arth:log");
        assert_eq!(canonical, "arth.log.Logger");
    }

    #[test]
    fn test_parse_import_note_default() {
        let note = "ts-import:default local=Foo source=arth:log canonical=arth.log";
        let (local, source, canonical) = parse_import_note(note).unwrap();
        assert_eq!(local, "Foo");
        assert_eq!(source, "arth:log");
        assert_eq!(canonical, "arth.log");
    }

    #[test]
    fn test_cross_file_resolver() {
        let mut resolver = CrossFileResolver::new();

        let mut table1 = SymbolTable::new("ModuleA".to_string());
        table1.add_symbol(Symbol {
            name: "TypeA".to_string(),
            kind: SymbolKind::Struct,
            qualified_name: "ModuleA.TypeA".to_string(),
            is_exported: true,
            arity: None,
            source_module: None,
        });
        resolver.tables.insert("ModuleA".to_string(), table1);

        let mut table2 = SymbolTable::new("ModuleB".to_string());
        table2.add_symbol(Symbol {
            name: "TypeB".to_string(),
            kind: SymbolKind::Struct,
            qualified_name: "ModuleB.TypeB".to_string(),
            is_exported: true,
            arity: None,
            source_module: None,
        });
        resolver.tables.insert("ModuleB".to_string(), table2);

        // Resolve from current module
        assert_eq!(
            resolver.resolve_type("ModuleA", "TypeA"),
            Some("ModuleA.TypeA".to_string())
        );

        // Cannot resolve type from different module without import
        assert_eq!(resolver.resolve_type("ModuleA", "TypeB"), None);
    }

    #[test]
    fn test_arth_import_generation() {
        use arth::compiler::hir::{HirFile, LoweringNote};
        use std::path::PathBuf;

        let hir = HirFile {
            path: PathBuf::from("test.ts"),
            package: None,
            decls: Vec::new(),
            notes: vec![
                LoweringNote {
                    span: Some(arth::compiler::hir::core::Span {
                        file: std::sync::Arc::new(PathBuf::from("test.ts")),
                        start: 0,
                        end: 0,
                    }),
                    message:
                        "ts-import:named local=Logger source=arth:log canonical=arth.log.Logger"
                            .to_string(),
                },
                LoweringNote {
                    span: Some(arth::compiler::hir::core::Span {
                        file: std::sync::Arc::new(PathBuf::from("test.ts")),
                        start: 0,
                        end: 0,
                    }),
                    message: "ts-import:named local=info source=arth:log canonical=arth.log.info"
                        .to_string(),
                },
            ],
            source_language: None,
            is_guest: true,
        };

        let imports = generate_arth_imports(&hir);
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].package, "arth.log");
        assert!(imports[0].items.contains(&"Logger".to_string()));
        assert!(imports[0].items.contains(&"info".to_string()));
    }
}
