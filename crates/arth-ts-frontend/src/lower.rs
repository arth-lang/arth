use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arth::compiler::ast::Visibility as AstVisibility;
use arth::compiler::hir::core::{HirId, Span as HirCoreSpan};
use arth::compiler::hir::{
    HirAttr, HirBlock, HirDecl, HirExpr, HirField, HirFile, HirFunc, HirFuncSig, HirInterface,
    HirInterfaceMethod, HirModule, HirParam, HirProvider, HirSourceLanguage, HirStmt, HirStruct,
    HirType, LoweringNote,
};
use swc_common::{FileName, SourceMap, Span as SwcSpan, Spanned, sync::Lrc};
use swc_ecma_ast as ast;
use swc_ecma_ast::EsVersion;
use swc_ecma_parser::{Parser, StringInput, Syntax, TsSyntax};

/// Options controlling TS subset validation and lowering behavior.
#[derive(Clone, Debug, Default)]
pub struct TsLoweringOptions {
    /// Optional explicit package name for the resulting `HirFile`.
    pub package: Option<String>,
}

/// Errors that can occur while parsing or lowering TS into Arth HIR.
#[derive(Debug)]
pub enum TsLoweringError {
    ParseError(String),

    Unsupported(&'static str),
}

impl std::fmt::Display for TsLoweringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TsLoweringError::ParseError(msg) => write!(f, "TypeScript parse error: {msg}"),
            TsLoweringError::Unsupported(msg) => write!(f, "unsupported TS construct: {msg}"),
        }
    }
}

impl std::error::Error for TsLoweringError {}

/// Context for resolving imported names to their qualified module.function paths.
/// Maps local binding names to qualified names (e.g., "add" → "Math.add").
#[derive(Default, Clone)]
struct ImportContext {
    /// Maps local name → qualified name (Module.function)
    bindings: HashMap<String, String>,
}

impl ImportContext {
    fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// Add a binding: local_name → module_name.original_name
    fn add_binding(&mut self, local: String, module_name: &str, original: &str) {
        let qualified = format!("{}.{}", module_name, original);
        self.bindings.insert(local, qualified);
    }

    /// Resolve a name: returns the qualified name if it's an import, otherwise None.
    fn resolve(&self, name: &str) -> Option<&String> {
        self.bindings.get(name)
    }
}

/// Context for tracking known provider type names.
/// Used to detect provider initialization in constructors.
#[derive(Default, Clone)]
struct ProviderContext {
    /// Set of known provider type names (from @provider decorated classes/interfaces)
    provider_names: HashSet<String>,
}

impl ProviderContext {
    fn new() -> Self {
        Self {
            provider_names: HashSet::new(),
        }
    }

    /// Register a provider type name.
    fn add_provider(&mut self, name: String) {
        self.provider_names.insert(name);
    }

    /// Check if a type name is a known provider.
    fn is_provider(&self, name: &str) -> bool {
        self.provider_names.contains(name)
    }
}

/// Derive a module name from an import source path.
/// E.g., "./math" → "Math", "../utils/helpers" → "Helpers"
fn module_name_from_import_source(src: &str) -> String {
    // Strip leading relative segments
    let mut s = src;
    while let Some(rest) = s.strip_prefix("./") {
        s = rest;
    }
    while let Some(rest) = s.strip_prefix("../") {
        s = rest;
    }

    // Get the last path component (basename)
    let basename = s.rsplit('/').next().unwrap_or(s);

    // Drop extensions
    let stem = basename
        .strip_suffix(".ts")
        .or_else(|| basename.strip_suffix(".tsx"))
        .or_else(|| basename.strip_suffix(".js"))
        .unwrap_or(basename);

    // Convert to PascalCase
    let mut out = String::new();
    for part in stem.split(['-', '_', '.']) {
        if part.is_empty() {
            continue;
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            for ch in chars {
                out.extend(ch.to_lowercase());
            }
        }
    }

    if out.is_empty() {
        "Module".to_string()
    } else {
        out
    }
}

fn canonical_module_name_from_path(path: &PathBuf) -> String {
    use std::path::Path;

    let stem = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Main");

    let mut out = String::new();
    for part in stem.split(['-', '_', '.']) {
        if part.is_empty() {
            continue;
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            for ch in chars {
                out.extend(ch.to_lowercase());
            }
        }
    }

    if out.is_empty() {
        "Main".to_string()
    } else {
        out
    }
}

fn canonical_import_path(raw: &str) -> String {
    // Strip leading relative segments like "./" or "../".
    let mut s = raw;
    while let Some(rest) = s.strip_prefix("./") {
        s = rest;
    }
    while let Some(rest) = s.strip_prefix("../") {
        s = rest;
    }

    // Drop common TS/JS extensions.
    let s = s
        .strip_suffix(".ts")
        .or_else(|| s.strip_suffix(".tsx"))
        .or_else(|| s.strip_suffix(".js"))
        .unwrap_or(s);

    let mut out = String::new();
    for ch in s.chars() {
        match ch {
            ':' | '/' | '\\' => out.push('.'),
            _ => out.push(ch),
        }
    }

    out
}

fn collect_import_notes(
    import: &ast::ImportDecl,
    file: &Arc<PathBuf>,
    notes: &mut Vec<LoweringNote>,
) {
    let src = import.src.value.as_str().unwrap_or("").to_string();
    let canonical_path = canonical_import_path(&src);
    let span = Some(span_from_swc(import.span, file));

    for spec in &import.specifiers {
        match spec {
            ast::ImportSpecifier::Named(n) => {
                let local = n.local.sym.to_string();
                let imported = match &n.imported {
                    Some(ast::ModuleExportName::Ident(id)) => id.sym.to_string(),
                    Some(ast::ModuleExportName::Str(s)) => {
                        s.value.as_str().unwrap_or("").to_string()
                    }
                    None => local.clone(),
                };
                let canonical = if canonical_path.is_empty() {
                    imported.clone()
                } else {
                    format!("{}.{}", canonical_path, imported)
                };
                notes.push(LoweringNote {
                    span: span.clone(),
                    message: format!(
                        "ts-import:named local={} source={} canonical={}",
                        local, src, canonical
                    ),
                });
            }
            ast::ImportSpecifier::Default(d) => {
                let local = d.local.sym.to_string();
                let canonical = if canonical_path.is_empty() {
                    src.clone()
                } else {
                    canonical_path.clone()
                };
                notes.push(LoweringNote {
                    span: span.clone(),
                    message: format!(
                        "ts-import:default local={} source={} canonical={}",
                        local, src, canonical
                    ),
                });
            }
            ast::ImportSpecifier::Namespace(ns) => {
                let local = ns.local.sym.to_string();
                let canonical = if canonical_path.is_empty() {
                    src.clone()
                } else {
                    canonical_path.clone()
                };
                notes.push(LoweringNote {
                    span: span.clone(),
                    message: format!(
                        "ts-import:namespace local={} source={} canonical={}",
                        local, src, canonical
                    ),
                });
            }
        }
    }
}

/// Collect import bindings for resolving imported names during expression lowering.
/// Only processes relative imports (./foo, ../bar); host imports (arth:*) are skipped.
fn collect_import_bindings(import: &ast::ImportDecl, ctx: &mut ImportContext) {
    let src = import.src.value.as_str().unwrap_or("").to_string();

    // Skip host capability imports
    if src.starts_with("arth:") {
        return;
    }

    // Only process relative imports
    if !src.starts_with("./") && !src.starts_with("../") {
        return;
    }

    let module_name = module_name_from_import_source(&src);

    for spec in &import.specifiers {
        match spec {
            ast::ImportSpecifier::Named(n) => {
                let local = n.local.sym.to_string();
                let original = match &n.imported {
                    Some(ast::ModuleExportName::Ident(id)) => id.sym.to_string(),
                    Some(ast::ModuleExportName::Str(s)) => {
                        s.value.as_str().unwrap_or("").to_string()
                    }
                    None => local.clone(),
                };
                ctx.add_binding(local, &module_name, &original);
            }
            ast::ImportSpecifier::Default(d) => {
                // Default import: `import Foo from "./foo"` → Foo.default or just Foo
                let local = d.local.sym.to_string();
                ctx.add_binding(local, &module_name, "default");
            }
            ast::ImportSpecifier::Namespace(_ns) => {
                // Namespace import: `import * as Foo from "./foo"`
                // The namespace itself becomes the module reference, handled differently
                // For now, skip - callers would use Foo.func() which is already qualified
            }
        }
    }
}

/// Collect provider type names from a module item.
/// Looks for @provider decorated classes and interfaces.
fn collect_provider_names(item: &ast::ModuleItem, ctx: &mut ProviderContext) {
    match item {
        ast::ModuleItem::ModuleDecl(decl) => match decl {
            ast::ModuleDecl::ExportDecl(ed) => match &ed.decl {
                ast::Decl::Class(class_decl) => {
                    if has_provider_decorator(&class_decl.class) {
                        ctx.add_provider(class_decl.ident.sym.to_string());
                    }
                }
                ast::Decl::TsInterface(iface_decl) => {
                    if has_provider_decorator_on_interface(iface_decl) {
                        ctx.add_provider(iface_decl.id.sym.to_string());
                    }
                }
                _ => {}
            },
            ast::ModuleDecl::ExportDefaultDecl(ed) => {
                if let ast::DefaultDecl::Class(class_expr) = &ed.decl
                    && has_provider_decorator(&class_expr.class)
                    && let Some(ident) = &class_expr.ident
                {
                    ctx.add_provider(ident.sym.to_string());
                }
            }
            _ => {}
        },
        ast::ModuleItem::Stmt(stmt) => {
            if let ast::Stmt::Decl(decl) = stmt {
                match decl {
                    ast::Decl::Class(class_decl) => {
                        if has_provider_decorator(&class_decl.class) {
                            ctx.add_provider(class_decl.ident.sym.to_string());
                        }
                    }
                    ast::Decl::TsInterface(iface_decl) => {
                        if has_provider_decorator_on_interface(iface_decl) {
                            ctx.add_provider(iface_decl.id.sym.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Check if a class has the @provider decorator.
fn has_provider_decorator(class: &ast::Class) -> bool {
    for dec in &class.decorators {
        if let Some(name) = decorator_name(&dec.expr)
            && name == "provider"
        {
            return true;
        }
    }
    false
}

/// Check if an interface has the @provider decorator.
fn has_provider_decorator_on_interface(iface: &ast::TsInterfaceDecl) -> bool {
    // TypeScript interfaces don't have decorators in the AST,
    // but we support a convention where a preceding @provider comment
    // or a @provider decorator syntax extension marks an interface.
    // For now, check the body for a special marker or just return false.
    // This will be extended when we add decorator support for interfaces.
    //
    // NOTE: The current validator and spec document shows @provider on interfaces,
    // but SWC doesn't parse decorators on interfaces. We'll detect provider
    // interfaces by name convention or by their usage context.
    _ = iface;
    false
}

/// Convenience entrypoint: parse TypeScript source from a string and
/// lower it into an Arth `HirFile`.
pub fn lower_ts_str_to_hir(
    source: &str,
    filename: &str,
    opts: TsLoweringOptions,
) -> Result<HirFile, TsLoweringError> {
    let cm: Lrc<SourceMap> = Default::default();
    let fname = FileName::Real(PathBuf::from(filename));
    let file = cm.new_source_file(fname.into(), source.to_string());

    let lexer = swc_ecma_parser::lexer::Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: false,
            decorators: true, // Enable @provider, @data decorators
            dts: false,
            no_early_errors: true,
            disallow_ambiguous_jsx_like: false,
        }),
        EsVersion::Es2022,
        StringInput::from(&*file),
        None,
    );

    let mut parser = Parser::new_from(lexer);
    let module = parser
        .parse_module()
        .map_err(|e| TsLoweringError::ParseError(format!("{e:?}")))?;

    // First, validate that the parsed TS module stays within the
    // Arth TS subset (see `docs/ts-subset.md`). This is a conservative
    // check: many constructs are currently rejected with
    // `TsLoweringError::Unsupported` and have clear TODOs in
    // `validate_ts_subset` for future implementation.
    crate::validate::validate_ts_subset(&module)?;

    // For now we do a very small amount of lowering:
    // - collect exported function and class declarations
    // - map their signatures and bodies into minimal HIR, grouped
    //   into a canonical module derived from the file name.

    let path = PathBuf::from(filename);
    let file_arc = Arc::new(path.clone());
    let mut id_gen = HirIdGen::default();

    let mut decls = Vec::new();
    let mut module_funcs: Vec<HirFunc> = Vec::new();
    let mut notes: Vec<LoweringNote> = Vec::new();

    // First pass: collect import bindings for resolving imported names
    let mut import_ctx = ImportContext::new();
    for item in &module.body {
        if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)) = item {
            collect_import_bindings(import, &mut import_ctx);
        }
    }

    // Second pass: collect provider type names from @provider decorated classes/interfaces
    let mut provider_ctx = ProviderContext::new();
    for item in &module.body {
        collect_provider_names(item, &mut provider_ctx);
    }

    for item in module.body {
        match item {
            ast::ModuleItem::ModuleDecl(decl) => match decl {
                ast::ModuleDecl::Import(import) => {
                    // Record a lowering note for each import with its
                    // canonical package/module path so that host tooling
                    // can reconstruct an explicit import table.
                    collect_import_notes(&import, &file_arc, &mut notes);
                }
                ast::ModuleDecl::ExportDecl(ed) => match ed.decl {
                    ast::Decl::Fn(fn_decl) => {
                        let func = lower_fn_decl_to_hir_exported(
                            &fn_decl,
                            &file_arc,
                            &mut id_gen,
                            &import_ctx,
                        )?;
                        module_funcs.push(func);
                    }
                    ast::Decl::Class(class_decl) => {
                        let class_decls = lower_class_decl_to_hir(
                            &class_decl,
                            &file_arc,
                            &mut id_gen,
                            &import_ctx,
                            &provider_ctx,
                            true, // is_exported
                        )?;
                        decls.extend(class_decls);
                    }
                    ast::Decl::TsTypeAlias(type_alias) => {
                        // Lower type alias to struct if it's an object type
                        if let Some(struct_decl) =
                            lower_type_alias_to_hir(&type_alias, &file_arc, &mut id_gen)?
                        {
                            decls.push(struct_decl);
                        }
                    }
                    ast::Decl::TsInterface(iface_decl) => {
                        // Lower interface to Arth interface if it has methods
                        if let Some(iface) =
                            lower_interface_decl_to_hir(&iface_decl, &file_arc, &mut id_gen)?
                        {
                            decls.push(iface);
                        }
                    }
                    _ => {
                        // Other declarations (enums, etc.) are ignored for now
                    }
                },
                ast::ModuleDecl::ExportDefaultDecl(ed) => match ed.decl {
                    ast::DefaultDecl::Class(class_expr) => {
                        let Some(ident) = class_expr.ident else {
                            return Err(TsLoweringError::Unsupported(
                                "anonymous default-exported classes are not lowered from TS to Arth HIR; name the class (e.g. `export default class Controller { ... }`)",
                            ));
                        };
                        let class_decl = ast::ClassDecl {
                            ident,
                            class: class_expr.class,
                            declare: false,
                        };
                        let class_decls = lower_class_decl_to_hir(
                            &class_decl,
                            &file_arc,
                            &mut id_gen,
                            &import_ctx,
                            &provider_ctx,
                            true, // is_exported (default export)
                        )?;
                        decls.extend(class_decls);
                    }
                    ast::DefaultDecl::Fn(fn_expr) => {
                        let Some(ident) = fn_expr.ident else {
                            return Err(TsLoweringError::Unsupported(
                                "anonymous default-exported functions are not lowered from TS to Arth HIR; name the function (e.g. `export default function main() { ... }`)",
                            ));
                        };
                        let fn_decl = ast::FnDecl {
                            ident,
                            function: fn_expr.function,
                            declare: false,
                        };
                        let func = lower_fn_decl_to_hir_exported(
                            &fn_decl,
                            &file_arc,
                            &mut id_gen,
                            &import_ctx,
                        )?;
                        module_funcs.push(func);
                    }
                    ast::DefaultDecl::TsInterfaceDecl(_) => {
                        // Types-only; ignored by lowering.
                    }
                },
                // Other export forms are rejected by the subset validator.
                _ => {}
            },
            // Also lower non-exported declarations so that internal
            // function calls work correctly and @provider/@data classes are handled.
            ast::ModuleItem::Stmt(stmt) => {
                if let ast::Stmt::Decl(decl) = stmt {
                    match decl {
                        ast::Decl::Fn(fn_decl) => {
                            let func = lower_fn_decl_to_hir(
                                &fn_decl,
                                &file_arc,
                                &mut id_gen,
                                &import_ctx,
                            )?;
                            module_funcs.push(func);
                        }
                        ast::Decl::Class(class_decl) => {
                            // Handle non-exported classes (especially @provider/@data decorated)
                            let class_decls = lower_class_decl_to_hir(
                                &class_decl,
                                &file_arc,
                                &mut id_gen,
                                &import_ctx,
                                &provider_ctx,
                                false, // is_exported
                            )?;
                            decls.extend(class_decls);
                        }
                        ast::Decl::TsTypeAlias(type_alias) => {
                            // Lower type alias to struct if it's an object type
                            if let Some(struct_decl) =
                                lower_type_alias_to_hir(&type_alias, &file_arc, &mut id_gen)?
                            {
                                decls.push(struct_decl);
                            }
                        }
                        ast::Decl::TsInterface(iface_decl) => {
                            // Lower interface to Arth interface if it has methods
                            if let Some(iface) =
                                lower_interface_decl_to_hir(&iface_decl, &file_arc, &mut id_gen)?
                            {
                                decls.push(iface);
                            }
                        }
                        _ => {
                            // Other declarations (vars, etc.) are ignored for now
                        }
                    }
                }
            }
        }
    }

    if !module_funcs.is_empty() {
        let module_name = canonical_module_name_from_path(&path);
        let module_span = module_funcs
            .first()
            .map(|f| f.span.clone())
            .unwrap_or_else(|| HirCoreSpan {
                file: file_arc.clone(),
                start: 0,
                end: 0,
            });

        decls.push(HirDecl::Module(HirModule {
            name: module_name,
            is_exported: true,
            funcs: module_funcs,
            implements: Vec::new(),
            doc: None,
            attrs: Vec::new(),
            id: id_gen.fresh(),
            span: module_span,
        }));
    }

    let hir = HirFile {
        path,
        package: opts.package.map(Into::into),
        decls,
        notes,
        source_language: Some(HirSourceLanguage::Ts),
        is_guest: true,
    };

    enforce_guest_mode(&hir)?;

    Ok(hir)
}

/// Resolve an import path relative to the importing file.
/// Returns `None` for host imports (e.g., `arth:log`) or if resolution fails.
fn resolve_import_path(import_source: &str, importing_file: &Path) -> Option<PathBuf> {
    // Host capability imports (arth:*) are not resolved to files
    if import_source.starts_with("arth:") {
        return None;
    }

    // Only resolve relative imports
    if !import_source.starts_with("./") && !import_source.starts_with("../") {
        return None;
    }

    let parent = importing_file.parent()?;
    let mut resolved = parent.to_path_buf();

    // Handle the relative path
    let import_path = import_source.strip_prefix("./").unwrap_or(import_source);

    resolved.push(import_path);

    // Try common extensions if not specified
    if resolved.extension().is_none() {
        for ext in &["ts", "tsx"] {
            let with_ext = resolved.with_extension(ext);
            if with_ext.exists() {
                return Some(with_ext);
            }
        }
        // Try index.ts in directory
        let index = resolved.join("index.ts");
        if index.exists() {
            return Some(index);
        }
    }

    if resolved.exists() {
        Some(resolved)
    } else {
        None
    }
}

/// Collect all relative import sources from a TS module.
fn collect_import_sources(module: &ast::Module) -> Vec<String> {
    let mut sources = Vec::new();
    for item in &module.body {
        if let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import)) = item {
            let src = import.src.value.as_str().unwrap_or("").to_string();
            // Only include relative imports (not arth:* host imports)
            if src.starts_with("./") || src.starts_with("../") {
                sources.push(src);
            }
        }
    }
    sources
}

/// Load and lower a multi-file TS project starting from an entry file.
/// Recursively resolves and loads all relative imports.
pub fn lower_ts_project_to_hir(
    entry_path: &Path,
    opts: TsLoweringOptions,
) -> Result<Vec<HirFile>, TsLoweringError> {
    let mut loaded: HashSet<PathBuf> = HashSet::new();
    let mut to_load: Vec<PathBuf> = vec![entry_path.canonicalize().map_err(|e| {
        TsLoweringError::ParseError(format!(
            "failed to resolve entry path {}: {e}",
            entry_path.display()
        ))
    })?];
    let mut hir_files: Vec<HirFile> = Vec::new();

    while let Some(path) = to_load.pop() {
        if loaded.contains(&path) {
            continue;
        }
        loaded.insert(path.clone());

        // Read and parse the file
        let source = std::fs::read_to_string(&path).map_err(|e| {
            TsLoweringError::ParseError(format!("failed to read {}: {e}", path.display()))
        })?;

        // Parse to get imports before full lowering
        let cm: Lrc<SourceMap> = Default::default();
        let fname = FileName::Real(path.clone());
        let file = cm.new_source_file(fname.into(), source.clone());

        let lexer = swc_ecma_parser::lexer::Lexer::new(
            Syntax::Typescript(TsSyntax {
                tsx: false,
                decorators: true, // Enable @provider, @data decorators
                dts: false,
                no_early_errors: true,
                disallow_ambiguous_jsx_like: false,
            }),
            EsVersion::Es2022,
            StringInput::from(&*file),
            None,
        );

        let mut parser = Parser::new_from(lexer);
        let module = parser
            .parse_module()
            .map_err(|e| TsLoweringError::ParseError(format!("{e:?}")))?;

        // Collect imports and queue them for loading
        for import_src in collect_import_sources(&module) {
            if let Some(resolved) = resolve_import_path(&import_src, &path)
                && let Ok(canonical) = resolved.canonicalize()
                && !loaded.contains(&canonical)
            {
                to_load.push(canonical);
            }
        }

        // Lower this file
        let hir =
            lower_ts_str_to_hir(&source, path.to_str().unwrap_or("unknown.ts"), opts.clone())?;
        hir_files.push(hir);
    }

    Ok(hir_files)
}

#[derive(Default)]
struct HirIdGen {
    next: u32,
}

impl HirIdGen {
    fn fresh(&mut self) -> HirId {
        self.next += 1;
        HirId(self.next)
    }
}

fn span_from_swc(span: SwcSpan, file: &Arc<PathBuf>) -> HirCoreSpan {
    HirCoreSpan {
        file: file.clone(),
        start: span.lo.0,
        end: span.hi.0,
    }
}

fn lower_fn_decl_to_hir(
    f: &ast::FnDecl,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<HirFunc, TsLoweringError> {
    lower_fn_decl_to_hir_with_export(f, file, id_gen, import_ctx, false)
}

fn lower_fn_decl_to_hir_exported(
    f: &ast::FnDecl,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<HirFunc, TsLoweringError> {
    lower_fn_decl_to_hir_with_export(f, file, id_gen, import_ctx, true)
}

fn lower_fn_decl_to_hir_with_export(
    f: &ast::FnDecl,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
    is_exported: bool,
) -> Result<HirFunc, TsLoweringError> {
    let name = f.ident.sym.to_string();
    let span = span_from_swc(f.function.span, file);

    // Lower function parameters with support for optional, default, and rest parameters
    let params = lower_function_params(&f.function.params, file)?;

    // Lower return type
    let ret_ty =
        extract_type_name_from_ts(&f.function.return_type).map(|path| HirType::Name { path });

    // Extract generic type parameters
    let generics = lower_type_params(&f.function.type_params);

    // Add @export attribute if this function is exported
    let attrs = if is_exported {
        vec![arth::compiler::hir::HirAttr {
            name: "export".to_string(),
            args: None,
        }]
    } else {
        Vec::new()
    };

    let sig = HirFuncSig {
        name,
        params,
        ret: ret_ty,
        is_async: f.function.is_async,
        is_unsafe: false,
        doc: None,
        attrs,
        span: Some(span.clone()),
        generics,
    };

    let body = match &f.function.body {
        Some(block_stmt) => {
            let block = lower_block_stmt(block_stmt, file, id_gen, import_ctx)?;
            Some(block)
        }
        None => None,
    };

    Ok(HirFunc {
        sig,
        id: id_gen.fresh(),
        span,
        body,
    })
}

/// Lower function parameters with support for optional, default, and rest parameters.
///
/// Parameter transformations:
/// - Simple `param: Type` → `HirParam { name, ty: Type }`
/// - Optional `param?: Type` → `HirParam { name, ty: Optional<Type> }`
/// - Default `param: Type = value` → `HirParam { name, ty: Optional<Type> }` (default handled in body)
/// - Rest `...param: Type[]` → `HirParam { name, ty: List<Type> }`
fn lower_function_params(
    params: &[ast::Param],
    _file: &Arc<PathBuf>,
) -> Result<Vec<arth::compiler::hir::HirParam>, TsLoweringError> {
    let mut result = Vec::new();

    for p in params {
        match &p.pat {
            ast::Pat::Ident(id) => {
                let pname = id.id.sym.to_string();
                let mut type_path = extract_type_name_from_ts(&id.type_ann)
                    .unwrap_or_else(|| vec!["Unknown".to_string()]);

                // Handle optional parameter (param?: Type) → Optional<Type>
                if id.optional {
                    let inner_type = type_path.join(".");
                    type_path = vec![format!("Optional<{}>", inner_type)];
                }

                let ty = HirType::Name { path: type_path };
                result.push(arth::compiler::hir::HirParam { name: pname, ty });
            }
            ast::Pat::Assign(assign) => {
                // Default parameter: param: Type = defaultValue
                // Extract the identifier and type from the left side
                if let ast::Pat::Ident(id) = assign.left.as_ref() {
                    let pname = id.id.sym.to_string();
                    let mut type_path = extract_type_name_from_ts(&id.type_ann)
                        .unwrap_or_else(|| vec!["Unknown".to_string()]);

                    // Default parameters are treated as Optional<T> since they may not be provided
                    let inner_type = type_path.join(".");
                    type_path = vec![format!("Optional<{}>", inner_type)];

                    let ty = HirType::Name { path: type_path };
                    result.push(arth::compiler::hir::HirParam { name: pname, ty });
                } else {
                    return Err(TsLoweringError::Unsupported(
                        "destructured default parameters are not yet supported",
                    ));
                }
            }
            ast::Pat::Rest(rest) => {
                // Rest parameter: ...args: Type[]
                // Note: The type annotation is on the RestPat itself, not the inner ident
                if let ast::Pat::Ident(id) = rest.arg.as_ref() {
                    let pname = id.id.sym.to_string();
                    // Extract type from rest.type_ann (e.g., `...args: number[]` has the type on rest)
                    let type_path = extract_type_name_from_ts(&rest.type_ann)
                        .unwrap_or_else(|| vec!["List<Unknown>".to_string()]);

                    // Rest parameters should already be array types, but ensure List<T> format
                    let ty = HirType::Name { path: type_path };
                    result.push(arth::compiler::hir::HirParam { name: pname, ty });
                } else {
                    return Err(TsLoweringError::Unsupported(
                        "destructured rest parameters are not yet supported",
                    ));
                }
            }
            _ => {
                return Err(TsLoweringError::Unsupported(
                    "only simple identifier, optional, default, and rest parameters are supported",
                ));
            }
        }
    }

    Ok(result)
}

/// Lower generic type parameters from a function/method declaration.
fn lower_type_params(
    type_params: &Option<Box<ast::TsTypeParamDecl>>,
) -> Vec<arth::compiler::hir::HirGenericParam> {
    let Some(params) = type_params else {
        return Vec::new();
    };

    params
        .params
        .iter()
        .map(|p| {
            let bound = p.constraint.as_ref().map(|c| HirType::Name {
                path: extract_type_name_from_ts_type(c),
            });

            arth::compiler::hir::HirGenericParam {
                name: p.name.sym.to_string(),
                bound,
            }
        })
        .collect()
}

/// Classification of class based on its decorators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClassKind {
    /// @provider decorator: lowered to HirProvider
    Provider,
    /// @data decorator: lowered to HirStruct only (no module)
    Data,
    /// No special decorator: lowered to HirStruct + HirModule
    Regular,
}

/// Determine the kind of class based on its decorators.
fn classify_class(class: &ast::Class) -> ClassKind {
    for dec in &class.decorators {
        if let Some(name) = decorator_name(&dec.expr) {
            match name {
                "provider" => return ClassKind::Provider,
                "data" => return ClassKind::Data,
                _ => {}
            }
        }
    }
    ClassKind::Regular
}

/// Extract the name from a decorator expression (@name or @name()).
fn decorator_name(expr: &ast::Expr) -> Option<&str> {
    match expr {
        ast::Expr::Ident(id) => Some(id.sym.as_ref()),
        ast::Expr::Call(call) => {
            if let ast::Callee::Expr(callee) = &call.callee
                && let ast::Expr::Ident(id) = callee.as_ref()
            {
                return Some(id.sym.as_ref());
            }
            None
        }
        _ => None,
    }
}

/// Lower a TypeScript `type` alias declaration to an Arth HIR struct.
///
/// Only object type literals are supported:
/// ```typescript
/// type User = {
///   name: string;
///   email: string;
/// };
/// ```
///
/// Maps to:
/// ```arth
/// struct User {
///   public String name;
///   public String email;
/// }
/// ```
fn lower_type_alias_to_hir(
    alias: &ast::TsTypeAliasDecl,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
) -> Result<Option<HirDecl>, TsLoweringError> {
    let type_name = alias.id.sym.to_string();
    let span = span_from_swc(alias.span, file);

    // Only handle object type literals for now
    let ast::TsType::TsTypeLit(type_lit) = alias.type_ann.as_ref() else {
        // Non-object types (primitives, unions, etc.) are type aliases,
        // not struct definitions. Skip them for now.
        return Ok(None);
    };

    let mut fields = Vec::new();

    for member in &type_lit.members {
        match member {
            ast::TsTypeElement::TsPropertySignature(prop) => {
                // Extract field name
                let field_name = match prop.key.as_ref() {
                    ast::Expr::Ident(id) => id.sym.to_string(),
                    ast::Expr::Lit(ast::Lit::Str(s)) => s.value.as_str().unwrap_or("").to_string(),
                    _ => {
                        return Err(TsLoweringError::Unsupported(
                            "type literal field keys must be identifiers or string literals",
                        ));
                    }
                };

                // Extract field type
                let ty = if let Some(type_ann) = &prop.type_ann {
                    HirType::Name {
                        path: extract_type_name_from_ts_type(&type_ann.type_ann),
                    }
                } else {
                    HirType::Name {
                        path: vec!["Unknown".to_string()],
                    }
                };

                let field_span = span_from_swc(prop.span, file);
                fields.push(HirField {
                    name: field_name,
                    ty,
                    doc: None,
                    attrs: Vec::new(),
                    span: Some(field_span),
                    is_shared: false,
                    is_final: prop.readonly,
                    vis: AstVisibility::Public,
                });
            }
            _ => {
                // Skip non-property members (methods, index signatures, etc.)
                // These don't map to struct fields
            }
        }
    }

    // Only create a struct if we have fields
    if fields.is_empty() {
        return Ok(None);
    }

    Ok(Some(HirDecl::Struct(HirStruct {
        name: type_name,
        fields,
        doc: None,
        attrs: Vec::new(),
        id: id_gen.fresh(),
        span,
        generics: Vec::new(),
    })))
}

/// Extract type path from a TsType node.
///
/// Primitive type mappings:
/// - `string` → `String`
/// - `number` → `Int` (default, use context for Float)
/// - `boolean` → `Bool`
/// - `void` → `Void`
/// - `null` / `undefined` → error (bare nullish types not allowed)
///
/// Generic type mappings:
/// - `T[]` → `List<T>`
/// - `T | null` / `T | undefined` → `Optional<T>`
/// - `Map<K, V>` → `Map<K, V>`
/// - `Set<T>` → `Set<T>`
/// - `Promise<T>` → `Task<T>` (async return type)
fn extract_type_name_from_ts_type(ty: &ast::TsType) -> Vec<String> {
    match ty {
        ast::TsType::TsKeywordType(kw) => {
            use ast::TsKeywordTypeKind as K;
            let name = match kw.kind {
                K::TsStringKeyword => "String",
                K::TsNumberKeyword => "Int",
                K::TsBooleanKeyword => "Bool",
                K::TsVoidKeyword => "Void",
                K::TsAnyKeyword => "Any",
                K::TsNeverKeyword => "Never",
                K::TsObjectKeyword => "Object",
                // Bare null/undefined should be caught by validation,
                // but if we get here, map to a sentinel
                K::TsNullKeyword | K::TsUndefinedKeyword => "Null",
                _ => "Unknown",
            };
            vec![name.to_string()]
        }
        ast::TsType::TsTypeRef(tr) => {
            if let ast::TsEntityName::Ident(id) = &tr.type_name {
                let type_name = id.sym.to_string();

                // Handle generic types: Map<K, V>, Set<T>, Promise<T>
                if let Some(type_params) = &tr.type_params {
                    let params: Vec<String> = type_params
                        .params
                        .iter()
                        .map(|p| extract_type_name_from_ts_type(p).join("."))
                        .collect();

                    match type_name.as_str() {
                        // Map<K, V> → Map<K, V> (preserved)
                        "Map" if params.len() == 2 => {
                            return vec![format!("Map<{}, {}>", params[0], params[1])];
                        }
                        // Set<T> → Set<T> (preserved)
                        "Set" if params.len() == 1 => {
                            return vec![format!("Set<{}>", params[0])];
                        }
                        // Promise<T> → Task<T> (async return type)
                        "Promise" if params.len() == 1 => {
                            return vec![format!("Task<{}>", params[0])];
                        }
                        // Array<T> → List<T>
                        "Array" if params.len() == 1 => {
                            return vec![format!("List<{}>", params[0])];
                        }
                        // Other generic types: preserve with mapped type params
                        _ => {
                            if params.is_empty() {
                                return vec![type_name];
                            }
                            return vec![format!("{}<{}>", type_name, params.join(", "))];
                        }
                    }
                }

                vec![type_name]
            } else {
                vec!["Unknown".to_string()]
            }
        }
        ast::TsType::TsArrayType(arr) => {
            let elem_type = extract_type_name_from_ts_type(&arr.elem_type);
            vec![format!("List<{}>", elem_type.join("."))]
        }
        ast::TsType::TsUnionOrIntersectionType(ui) => {
            // Check for T | null or T | undefined patterns → Optional<T>
            if let ast::TsUnionOrIntersectionType::TsUnionType(union) = ui {
                let types: Vec<_> = union.types.iter().collect();
                if types.len() == 2 {
                    // Check if one is null/undefined
                    let (nullable, other) = if is_null_or_undefined(types[0]) {
                        (true, types[1])
                    } else if is_null_or_undefined(types[1]) {
                        (true, types[0])
                    } else {
                        (false, types[0])
                    };

                    if nullable {
                        let inner = extract_type_name_from_ts_type(other);
                        return vec![format!("Optional<{}>", inner.join("."))];
                    }
                }
            }
            vec!["Unknown".to_string()]
        }
        ast::TsType::TsParenthesizedType(paren) => {
            // Unwrap parenthesized types: (T) → T
            extract_type_name_from_ts_type(&paren.type_ann)
        }
        ast::TsType::TsFnOrConstructorType(_) => {
            // Function types not directly supported, map to Unknown
            vec!["Unknown".to_string()]
        }
        ast::TsType::TsTupleType(tuple) => {
            // Tuples could be mapped to a Tuple<T1, T2, ...> type
            let elems: Vec<String> = tuple
                .elem_types
                .iter()
                .map(|e| extract_type_name_from_ts_type(&e.ty).join("."))
                .collect();
            vec![format!("Tuple<{}>", elems.join(", "))]
        }
        ast::TsType::TsLitType(_) => {
            // Literal types (e.g., "hello", 42) not directly mappable
            vec!["Unknown".to_string()]
        }
        _ => vec!["Unknown".to_string()],
    }
}

/// Check if a type is null or undefined.
fn is_null_or_undefined(ty: &ast::TsType) -> bool {
    if let ast::TsType::TsKeywordType(kw) = ty {
        matches!(
            kw.kind,
            ast::TsKeywordTypeKind::TsNullKeyword | ast::TsKeywordTypeKind::TsUndefinedKeyword
        )
    } else {
        false
    }
}

/// Lower a TypeScript `interface` declaration to Arth HIR.
///
/// - Interfaces with methods → `HirInterface` (behavior contracts)
/// - Interfaces with only properties → `HirStruct` (data shapes, user convenience)
///
/// Example with methods:
/// ```typescript
/// interface Serializable {
///   serialize(): string;
///   deserialize(data: string): void;
/// }
/// ```
/// Maps to:
/// ```arth
/// interface Serializable {
///   String serialize();
///   void deserialize(String data);
/// }
/// ```
///
/// Example with only properties:
/// ```typescript
/// interface User {
///   name: string;
///   email: string;
/// }
/// ```
/// Maps to:
/// ```arth
/// struct User {
///   public String name;
///   public String email;
/// }
/// ```
fn lower_interface_decl_to_hir(
    iface: &ast::TsInterfaceDecl,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
) -> Result<Option<HirDecl>, TsLoweringError> {
    let interface_name = iface.id.sym.to_string();
    let span = span_from_swc(iface.span, file);

    let mut methods = Vec::new();
    let mut fields = Vec::new();

    for member in &iface.body.body {
        match member {
            ast::TsTypeElement::TsMethodSignature(method) => {
                // Extract method name
                let method_name = match method.key.as_ref() {
                    ast::Expr::Ident(id) => id.sym.to_string(),
                    _ => {
                        return Err(TsLoweringError::Unsupported(
                            "interface method keys must be identifiers",
                        ));
                    }
                };

                // Extract parameters
                let params: Vec<HirParam> = method
                    .params
                    .iter()
                    .filter_map(|param| {
                        if let ast::TsFnParam::Ident(id) = param {
                            let param_name = id.id.sym.to_string();
                            let param_type = if let Some(type_ann) = &id.type_ann {
                                HirType::Name {
                                    path: extract_type_name_from_ts_type(&type_ann.type_ann),
                                }
                            } else {
                                HirType::Name {
                                    path: vec!["Unknown".to_string()],
                                }
                            };
                            Some(HirParam {
                                name: param_name,
                                ty: param_type,
                            })
                        } else {
                            None
                        }
                    })
                    .collect();

                // Extract return type
                let ret = method.type_ann.as_ref().map(|ta| HirType::Name {
                    path: extract_type_name_from_ts_type(&ta.type_ann),
                });

                let method_span = span_from_swc(method.span, file);

                methods.push(HirInterfaceMethod {
                    sig: HirFuncSig {
                        name: method_name,
                        generics: Vec::new(),
                        params,
                        ret,
                        is_async: false,
                        is_unsafe: false,
                        doc: None,
                        attrs: Vec::new(),
                        span: Some(method_span),
                    },
                    default_body: None,
                });
            }
            ast::TsTypeElement::TsPropertySignature(prop) => {
                // Extract property as a struct field
                let field_name = match prop.key.as_ref() {
                    ast::Expr::Ident(id) => id.sym.to_string(),
                    ast::Expr::Lit(ast::Lit::Str(s)) => s.value.as_str().unwrap_or("").to_string(),
                    _ => continue, // Skip non-identifier keys
                };

                let ty = if let Some(type_ann) = &prop.type_ann {
                    HirType::Name {
                        path: extract_type_name_from_ts_type(&type_ann.type_ann),
                    }
                } else {
                    HirType::Name {
                        path: vec!["Unknown".to_string()],
                    }
                };

                let field_span = span_from_swc(prop.span, file);
                fields.push(HirField {
                    name: field_name,
                    ty,
                    doc: None,
                    attrs: Vec::new(),
                    span: Some(field_span),
                    is_shared: false,
                    is_final: prop.readonly,
                    vis: AstVisibility::Public,
                });
            }
            _ => {
                // Skip other members (call signatures, index signatures, etc.)
            }
        }
    }

    // If interface has only properties (no methods), lower to struct
    if methods.is_empty() && !fields.is_empty() {
        return Ok(Some(HirDecl::Struct(HirStruct {
            name: interface_name,
            fields,
            doc: None,
            attrs: Vec::new(),
            id: id_gen.fresh(),
            span,
            generics: Vec::new(),
        })));
    }

    // If empty interface (no methods and no fields), skip it
    if methods.is_empty() {
        return Ok(None);
    }

    // Handle interface generics
    let generics = iface
        .type_params
        .as_ref()
        .map(|tp| {
            tp.params
                .iter()
                .map(|p| arth::compiler::hir::HirGenericParam {
                    name: p.name.sym.to_string(),
                    bound: None,
                })
                .collect()
        })
        .unwrap_or_default();

    // Handle extends clause
    let extends: Vec<Vec<String>> = iface
        .extends
        .iter()
        .filter_map(|ext| {
            // ext.expr is an Expr, usually an identifier for the extended interface
            if let ast::Expr::Ident(id) = ext.expr.as_ref() {
                Some(vec![id.sym.to_string()])
            } else {
                None
            }
        })
        .collect();

    Ok(Some(HirDecl::Interface(HirInterface {
        name: interface_name,
        generics,
        methods,
        extends,
        doc: None,
        attrs: Vec::new(),
        id: id_gen.fresh(),
        span,
    })))
}

/// Desugar a TS `class` declaration into canonical Arth HIR items.
///
/// The lowering depends on class decorators:
/// - `@provider`: lowered to `HirProvider` (provider declaration)
/// - `@data`: lowered to `HirStruct` only (data shape, no module)
/// - No decorator: lowered to `HirModule` only (behavior container, no struct)
///
/// For `@provider` and `@data` classes:
/// - Fields become HIR fields with appropriate modifiers
/// - `readonly` → `final`, non-readonly → `shared` (for providers)
///
/// For regular classes (no decorator):
/// - Lowered to `HirModule` containing only functions (methods)
/// - Class-level fields are NOT allowed (will error)
/// - `this` keyword usage is NOT allowed (will error)
/// - `implements` clause is preserved in the module
/// - Constructor with provider initialization is lowered to `constructor()` function
fn lower_class_decl_to_hir(
    c: &ast::ClassDecl,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
    provider_ctx: &ProviderContext,
    is_exported: bool,
) -> Result<Vec<HirDecl>, TsLoweringError> {
    let class_name = c.ident.sym.to_string();
    let class_span = span_from_swc(c.class.span, file);
    let class_kind = classify_class(&c.class);

    // For regular classes (no decorator), class-level fields are not allowed.
    // Check for fields and error if found.
    if class_kind == ClassKind::Regular {
        for member in &c.class.body {
            if let ast::ClassMember::ClassProp(prop) = member {
                let field_name = match &prop.key {
                    ast::PropName::Ident(id) => id.sym.to_string(),
                    ast::PropName::Str(s) => s.value.as_str().unwrap_or("<unknown>").to_string(),
                    _ => "<computed>".to_string(),
                };
                return Err(TsLoweringError::ParseError(format!(
                    "class '{}' has a field '{}' which is not allowed; \
                     use @data for data classes or @provider for stateful classes. \
                     Regular classes must be pure behavior (functions only).",
                    class_name, field_name
                )));
            }
        }
    }

    // Instance fields become HIR fields (for struct/provider).
    let mut fields = Vec::new();
    for member in &c.class.body {
        if let ast::ClassMember::ClassProp(prop) = member {
            if prop.is_static {
                // Validator should already reject this, but keep a guard here.
                return Err(TsLoweringError::Unsupported(
                    "static class fields are not yet lowered from TS to Arth HIR",
                ));
            }

            let field_name = match &prop.key {
                ast::PropName::Ident(id) => id.sym.to_string(),
                ast::PropName::Str(s) => s.value.as_str().unwrap_or("").to_string(),
                _ => {
                    return Err(TsLoweringError::Unsupported(
                        "only identifier and string-named fields are lowered from TS classes",
                    ));
                }
            };

            let ty = HirType::Name {
                path: extract_type_name_from_ts(&prop.type_ann)
                    .unwrap_or_else(|| vec!["Unknown".to_string()]),
            };

            let field_span = span_from_swc(prop.span, file);

            // For providers: non-readonly fields become `shared`
            let is_shared = class_kind == ClassKind::Provider && !prop.readonly;

            fields.push(HirField {
                name: field_name,
                ty,
                doc: None,
                attrs: Vec::new(),
                span: Some(field_span),
                is_shared,
                is_final: prop.readonly,
                vis: AstVisibility::Public,
            });
        }
    }

    // Handle @provider decorated classes: lower to HirProvider
    if class_kind == ClassKind::Provider {
        return Ok(vec![HirDecl::Provider(HirProvider {
            name: class_name,
            fields,
            doc: None,
            attrs: Vec::new(),
            id: id_gen.fresh(),
            span: class_span,
        })]);
    }

    // Handle @data decorated classes: lower to HirStruct only (no module)
    if class_kind == ClassKind::Data {
        return Ok(vec![HirDecl::Struct(HirStruct {
            name: class_name,
            fields,
            doc: None,
            attrs: Vec::new(),
            id: id_gen.fresh(),
            span: class_span,
            generics: Vec::new(),
        })]);
    }

    // Regular class: lower to HirModule only (no struct)
    // Regular classes are pure behavior containers (functions only).

    // Extract implements clause from the class
    // Each interface is a path (Vec<String>) for qualified names
    let implements: Vec<Vec<String>> = c
        .class
        .implements
        .iter()
        .filter_map(|impl_clause| {
            // Extract the interface name from the expression
            if let ast::Expr::Ident(id) = impl_clause.expr.as_ref() {
                Some(vec![id.sym.to_string()])
            } else {
                None
            }
        })
        .collect();

    // Methods become functions within the module.
    // Constructors with provider initialization are lowered to `constructor()` function.
    let mut funcs = Vec::new();

    for member in &c.class.body {
        match member {
            ast::ClassMember::Method(method) => {
                let mfunc =
                    lower_method_to_hir_stateless(method, file, id_gen, import_ctx, provider_ctx)?;
                funcs.push(mfunc);
            }
            ast::ClassMember::Constructor(ctor) => {
                // Check if constructor initializes a provider.
                // Pattern: `const state: ProviderType = { ... }`
                if let Some(body) = &ctor.body
                    && has_provider_init(body, provider_ctx)
                {
                    // Lower constructor with provider init to HirFunc
                    let ctor_func = lower_constructor_to_hir(ctor, file, id_gen, import_ctx)?;
                    funcs.push(ctor_func);
                }
                // If no provider init or no body, ignore the constructor (empty or no-op)
            }
            _ => {}
        }
    }

    let module_decl = HirDecl::Module(HirModule {
        name: class_name.clone(),
        is_exported,
        funcs,
        implements,
        doc: None,
        attrs: Vec::new(),
        id: id_gen.fresh(),
        span: class_span,
    });

    Ok(vec![module_decl])
}

/// Check if a constructor body contains provider initialization.
///
/// Looks for patterns like:
/// ```typescript
/// const state: ProviderType = { ... };
/// ```
/// where `ProviderType` is a known provider type from the context.
fn has_provider_init(body: &ast::BlockStmt, provider_ctx: &ProviderContext) -> bool {
    for stmt in &body.stmts {
        if let Some(type_name) = extract_provider_init_type(stmt)
            && provider_ctx.is_provider(&type_name)
        {
            return true;
        }
    }
    false
}

/// Extract the type name from a provider initialization statement.
///
/// Matches pattern: `const varName: TypeName = { ... }`
/// Returns the type name if found.
fn extract_provider_init_type(stmt: &ast::Stmt) -> Option<String> {
    // Look for: const varName: TypeName = { ... }
    let ast::Stmt::Decl(ast::Decl::Var(var_decl)) = stmt else {
        return None;
    };

    // Must be const declaration
    if var_decl.kind != ast::VarDeclKind::Const {
        return None;
    }

    // Check each declarator
    for decl in &var_decl.decls {
        // Must have an object literal initializer
        let Some(init) = &decl.init else {
            continue;
        };
        if !matches!(init.as_ref(), ast::Expr::Object(_)) {
            continue;
        }

        // Must be an identifier pattern with type annotation
        let ast::Pat::Ident(ident) = &decl.name else {
            continue;
        };

        // Extract the type name
        if let Some(type_ann) = &ident.type_ann
            && let ast::TsType::TsTypeRef(type_ref) = type_ann.type_ann.as_ref()
            && let ast::TsEntityName::Ident(type_ident) = &type_ref.type_name
        {
            return Some(type_ident.sym.to_string());
        }
    }

    None
}

/// Lower a constructor to HirFunc with name "constructor".
///
/// The constructor body is lowered to statements in the function body.
/// This is used for constructors that initialize providers.
fn lower_constructor_to_hir(
    ctor: &ast::Constructor,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<HirFunc, TsLoweringError> {
    let span = span_from_swc(ctor.span, file);

    // Constructor parameters (typically empty for controller pattern)
    let params = lower_constructor_params(&ctor.params, file)?;

    let sig = HirFuncSig {
        name: "constructor".to_string(),
        params,
        ret: Some(HirType::Name {
            path: vec!["void".to_string()],
        }),
        is_async: false,
        is_unsafe: false,
        doc: None,
        attrs: vec![HirAttr {
            name: "export".to_string(),
            args: None,
        }],
        span: Some(span.clone()),
        generics: Vec::new(),
    };

    let body = if let Some(block) = &ctor.body {
        lower_block_stmt(block, file, id_gen, import_ctx)?
    } else {
        HirBlock {
            id: id_gen.fresh(),
            span: span.clone(),
            stmts: Vec::new(),
        }
    };

    Ok(HirFunc {
        sig,
        id: id_gen.fresh(),
        span,
        body: Some(body),
    })
}

/// Lower constructor parameters to HirParams.
///
/// Constructor params use `ParamOrTsParamProp` which can be either
/// regular params or TypeScript parameter properties.
fn lower_constructor_params(
    params: &[ast::ParamOrTsParamProp],
    file: &Arc<PathBuf>,
) -> Result<Vec<HirParam>, TsLoweringError> {
    let mut result = Vec::new();

    for p in params {
        match p {
            ast::ParamOrTsParamProp::Param(param) => {
                // Regular parameter - reuse the existing param lowering
                let ast_params = vec![param.clone()];
                let lowered = lower_function_params(&ast_params, file)?;
                result.extend(lowered);
            }
            ast::ParamOrTsParamProp::TsParamProp(prop) => {
                // TypeScript parameter property (e.g., `constructor(public name: string)`)
                // Extract the parameter from the property
                match &prop.param {
                    ast::TsParamPropParam::Ident(id) => {
                        let pname = id.id.sym.to_string();
                        let type_path = extract_type_name_from_ts(&id.type_ann)
                            .unwrap_or_else(|| vec!["Unknown".to_string()]);
                        let ty = HirType::Name { path: type_path };
                        result.push(HirParam { name: pname, ty });
                    }
                    ast::TsParamPropParam::Assign(assign) => {
                        // Default parameter property
                        if let ast::Pat::Ident(id) = assign.left.as_ref() {
                            let pname = id.id.sym.to_string();
                            let mut type_path = extract_type_name_from_ts(&id.type_ann)
                                .unwrap_or_else(|| vec!["Unknown".to_string()]);
                            let inner_type = type_path.join(".");
                            type_path = vec![format!("Optional<{}>", inner_type)];
                            let ty = HirType::Name { path: type_path };
                            result.push(HirParam { name: pname, ty });
                        }
                    }
                }
            }
        }
    }

    Ok(result)
}

/// Lower a method to HIR for stateless regular classes (no `self` parameter).
///
/// Unlike `lower_method_to_hir` which adds a `self: ClassName` parameter,
/// this version treats methods as standalone functions in a module.
/// Regular classes cannot use `this` keyword since they have no state.
///
/// If the first parameter is a known provider type, it is renamed to `self`
/// following the Arth convention for provider-injected methods.
fn lower_method_to_hir_stateless(
    method: &ast::ClassMethod,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
    provider_ctx: &ProviderContext,
) -> Result<HirFunc, TsLoweringError> {
    use swc_ecma_ast::MethodKind;

    if method.kind != MethodKind::Method {
        return Err(TsLoweringError::Unsupported(
            "getters/setters are not yet lowered from TS classes",
        ));
    }

    let span = span_from_swc(method.span, file);

    let name = match &method.key {
        ast::PropName::Ident(id) => id.sym.to_string(),
        ast::PropName::Str(s) => s.value.as_str().unwrap_or("").to_string(),
        _ => {
            return Err(TsLoweringError::Unsupported(
                "only identifier and string-named methods are lowered from TS classes",
            ));
        }
    };

    // Check for `this` keyword usage in method body - not allowed in regular classes
    if let Some(block) = &method.function.body
        && block_contains_this(block)
    {
        return Err(TsLoweringError::ParseError(format!(
            "method '{}' uses 'this' keyword which is not allowed in regular classes; \
             regular classes are pure behavior containers. Use @provider for stateful \
             classes that need 'this' access, or pass state as function parameters.",
            name
        )));
    }

    // Lower parameters with support for optional, default, and rest parameters
    let mut params = lower_function_params(&method.function.params, file)?;

    // Provider parameter injection: if first param is a known provider type,
    // rename it to "self" following Arth convention.
    // Pattern: `increment(state: State)` → `increment(State self)`
    if let Some(first_param) = params.first_mut()
        && let HirType::Name { path } = &first_param.ty
        && let Some(type_name) = path.first()
        && provider_ctx.is_provider(type_name)
    {
        first_param.name = "self".to_string();
    }

    // Lower generic type parameters
    let generics = lower_type_params(&method.function.type_params);

    let ret_ty =
        extract_type_name_from_ts(&method.function.return_type).map(|path| HirType::Name { path });

    let sig = HirFuncSig {
        name,
        params,
        ret: ret_ty,
        is_async: method.function.is_async,
        is_unsafe: false,
        doc: None,
        attrs: vec![HirAttr {
            name: "export".to_string(),
            args: None,
        }],
        span: Some(span.clone()),
        generics,
    };

    let body = if let Some(block) = &method.function.body {
        lower_block_stmt(block, file, id_gen, import_ctx)?
    } else {
        HirBlock {
            id: id_gen.fresh(),
            span: span.clone(),
            stmts: Vec::new(),
        }
    };

    Ok(HirFunc {
        sig,
        id: id_gen.fresh(),
        span,
        body: Some(body),
    })
}

/// Check if a block statement contains any `this` keyword usage.
fn block_contains_this(block: &ast::BlockStmt) -> bool {
    for stmt in &block.stmts {
        if stmt_contains_this(stmt) {
            return true;
        }
    }
    false
}

/// Check if a statement contains any `this` keyword usage.
fn stmt_contains_this(stmt: &ast::Stmt) -> bool {
    match stmt {
        ast::Stmt::Expr(expr_stmt) => expr_contains_this(&expr_stmt.expr),
        ast::Stmt::Return(ret) => ret.arg.as_ref().is_some_and(|e| expr_contains_this(e)),
        ast::Stmt::If(if_stmt) => {
            expr_contains_this(&if_stmt.test)
                || stmt_contains_this(&if_stmt.cons)
                || if_stmt.alt.as_ref().is_some_and(|s| stmt_contains_this(s))
        }
        ast::Stmt::While(while_stmt) => {
            expr_contains_this(&while_stmt.test) || stmt_contains_this(&while_stmt.body)
        }
        ast::Stmt::For(for_stmt) => {
            for_stmt.init.as_ref().is_some_and(|init| match init {
                ast::VarDeclOrExpr::Expr(e) => expr_contains_this(e),
                ast::VarDeclOrExpr::VarDecl(v) => var_decl_contains_this(v),
            }) || for_stmt
                .test
                .as_ref()
                .is_some_and(|e| expr_contains_this(e))
                || for_stmt
                    .update
                    .as_ref()
                    .is_some_and(|e| expr_contains_this(e))
                || stmt_contains_this(&for_stmt.body)
        }
        ast::Stmt::ForOf(for_of) => {
            expr_contains_this(&for_of.right) || stmt_contains_this(&for_of.body)
        }
        ast::Stmt::ForIn(for_in) => {
            expr_contains_this(&for_in.right) || stmt_contains_this(&for_in.body)
        }
        ast::Stmt::Block(block) => block_contains_this(block),
        ast::Stmt::Decl(ast::Decl::Var(v)) => var_decl_contains_this(v),
        ast::Stmt::Try(try_stmt) => {
            block_contains_this(&try_stmt.block)
                || try_stmt
                    .handler
                    .as_ref()
                    .is_some_and(|h| block_contains_this(&h.body))
                || try_stmt.finalizer.as_ref().is_some_and(block_contains_this)
        }
        ast::Stmt::Throw(throw) => expr_contains_this(&throw.arg),
        _ => false,
    }
}

/// Check if a variable declaration contains any `this` keyword usage.
fn var_decl_contains_this(decl: &ast::VarDecl) -> bool {
    decl.decls
        .iter()
        .any(|d| d.init.as_ref().is_some_and(|e| expr_contains_this(e)))
}

/// Check if an expression contains any `this` keyword usage.
fn expr_contains_this(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::This(_) => true,
        ast::Expr::Member(member) => {
            expr_contains_this(&member.obj)
                || match &member.prop {
                    ast::MemberProp::Computed(c) => expr_contains_this(&c.expr),
                    _ => false,
                }
        }
        ast::Expr::Call(call) => {
            (match &call.callee {
                ast::Callee::Expr(e) => expr_contains_this(e),
                _ => false,
            }) || call.args.iter().any(|arg| expr_contains_this(&arg.expr))
        }
        ast::Expr::New(new) => {
            expr_contains_this(&new.callee)
                || new
                    .args
                    .as_ref()
                    .is_some_and(|args| args.iter().any(|a| expr_contains_this(&a.expr)))
        }
        ast::Expr::Bin(bin) => expr_contains_this(&bin.left) || expr_contains_this(&bin.right),
        ast::Expr::Unary(unary) => expr_contains_this(&unary.arg),
        ast::Expr::Update(update) => expr_contains_this(&update.arg),
        ast::Expr::Assign(assign) => {
            (match &assign.left {
                ast::AssignTarget::Simple(ast::SimpleAssignTarget::Member(m)) => {
                    expr_contains_this(&m.obj)
                }
                _ => false,
            }) || expr_contains_this(&assign.right)
        }
        ast::Expr::Cond(cond) => {
            expr_contains_this(&cond.test)
                || expr_contains_this(&cond.cons)
                || expr_contains_this(&cond.alt)
        }
        ast::Expr::Paren(paren) => expr_contains_this(&paren.expr),
        ast::Expr::Array(arr) => arr
            .elems
            .iter()
            .any(|el| el.as_ref().is_some_and(|e| expr_contains_this(&e.expr))),
        ast::Expr::Object(obj) => obj.props.iter().any(|prop| match prop {
            ast::PropOrSpread::Prop(p) => match p.as_ref() {
                ast::Prop::KeyValue(kv) => expr_contains_this(&kv.value),
                ast::Prop::Shorthand(id) => id.sym.as_ref() == "this",
                _ => false,
            },
            ast::PropOrSpread::Spread(s) => expr_contains_this(&s.expr),
        }),
        ast::Expr::Await(await_expr) => expr_contains_this(&await_expr.arg),
        ast::Expr::Seq(seq) => seq.exprs.iter().any(|e| expr_contains_this(e)),
        ast::Expr::Tpl(tpl) => tpl.exprs.iter().any(|e| expr_contains_this(e)),
        _ => false,
    }
}

/// Extract type path from an optional type annotation.
///
/// This is a wrapper around `extract_type_name_from_ts_type` that handles
/// the `Option<Box<TsTypeAnn>>` wrapper commonly used in function parameters
/// and return types.
fn extract_type_name_from_ts(type_ann: &Option<Box<ast::TsTypeAnn>>) -> Option<Vec<String>> {
    let ann = type_ann.as_ref()?;
    Some(extract_type_name_from_ts_type(&ann.type_ann))
}

fn lower_block_stmt(
    b: &ast::BlockStmt,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<HirBlock, TsLoweringError> {
    let span = span_from_swc(b.span, file);
    let mut stmts = Vec::new();

    for s in &b.stmts {
        if let Some(hir_stmt) = lower_stmt(s, file, id_gen, import_ctx)? {
            stmts.push(hir_stmt);
        }
    }

    Ok(HirBlock {
        id: id_gen.fresh(),
        span,
        stmts,
    })
}

fn lower_stmts_to_block(
    stmts: &[ast::Stmt],
    span: swc_common::Span,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<HirBlock, TsLoweringError> {
    let span = span_from_swc(span, file);
    let mut out = Vec::new();
    for s in stmts {
        if let Some(h) = lower_stmt(s, file, id_gen, import_ctx)? {
            out.push(h);
        }
    }
    Ok(HirBlock {
        id: id_gen.fresh(),
        span,
        stmts: out,
    })
}

fn lower_single_stmt_as_block(
    s: &ast::Stmt,
    parent_span: swc_common::Span,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<HirBlock, TsLoweringError> {
    lower_stmts_to_block(
        std::slice::from_ref(s),
        parent_span,
        file,
        id_gen,
        import_ctx,
    )
}

fn lower_stmt(
    s: &ast::Stmt,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<Option<HirStmt>, TsLoweringError> {
    match s {
        ast::Stmt::Return(ret) => lower_return_stmt(ret, file, id_gen, import_ctx),
        ast::Stmt::Expr(e) => {
            // Recognize assignment expression statements like `x = y;` or
            // `obj.field = y;` and lower them into dedicated assignment
            // statements instead of treating them as generic expressions.
            if let ast::Expr::Assign(a) = &*e.expr {
                lower_assign_expr_stmt(a, file, id_gen, import_ctx)
            } else if let ast::Expr::Update(u) = &*e.expr {
                lower_update_expr_stmt(u, file, id_gen, import_ctx)
            } else {
                lower_expr_stmt(e, file, id_gen, import_ctx)
            }
        }
        ast::Stmt::Decl(ast::Decl::Var(var)) => lower_var_decl_stmt(var, file, id_gen, import_ctx),
        ast::Stmt::If(if_stmt) => {
            let span = span_from_swc(if_stmt.span, file);
            let cond = lower_expr(&if_stmt.test, file, id_gen, import_ctx)?;
            let then_blk =
                lower_single_stmt_as_block(&if_stmt.cons, if_stmt.span, file, id_gen, import_ctx)?;
            let else_blk = if let Some(alt) = &if_stmt.alt {
                Some(lower_single_stmt_as_block(
                    alt,
                    if_stmt.span,
                    file,
                    id_gen,
                    import_ctx,
                )?)
            } else {
                None
            };
            Ok(Some(HirStmt::If {
                id: id_gen.fresh(),
                span,
                cond,
                then_blk,
                else_blk,
            }))
        }
        ast::Stmt::While(while_stmt) => {
            let span = span_from_swc(while_stmt.span, file);
            let cond = lower_expr(&while_stmt.test, file, id_gen, import_ctx)?;
            let body = lower_single_stmt_as_block(
                &while_stmt.body,
                while_stmt.span,
                file,
                id_gen,
                import_ctx,
            )?;
            Ok(Some(HirStmt::While {
                id: id_gen.fresh(),
                span,
                cond,
                body,
            }))
        }
        ast::Stmt::DoWhile(do_while) => {
            // Desugar `do body while (cond);` into:
            // { body; while (cond) { body; } }
            let span = span_from_swc(do_while.span, file);
            let mut stmts = Vec::new();
            if let Some(first) = lower_stmt(&do_while.body, file, id_gen, import_ctx)? {
                stmts.push(first);
            }
            let cond = lower_expr(&do_while.test, file, id_gen, import_ctx)?;
            let while_body = lower_single_stmt_as_block(
                &do_while.body,
                do_while.span,
                file,
                id_gen,
                import_ctx,
            )?;
            let while_stmt = HirStmt::While {
                id: id_gen.fresh(),
                span: span.clone(),
                cond,
                body: while_body,
            };
            stmts.push(while_stmt);
            Ok(Some(HirStmt::Block(HirBlock {
                id: id_gen.fresh(),
                span,
                stmts,
            })))
        }
        ast::Stmt::For(for_stmt) => lower_for_stmt(for_stmt, file, id_gen, import_ctx),
        ast::Stmt::ForIn(_) => Err(TsLoweringError::Unsupported(
            "`for..in` loops are not lowered from TS to Arth HIR; use `for (;;)` or `for..of` over arrays instead (see docs/ts-subset.md §3.3).",
        )),
        ast::Stmt::ForOf(for_of) => lower_for_of_stmt(for_of, file, id_gen, import_ctx),
        ast::Stmt::Switch(sw) => lower_switch_stmt(sw, file, id_gen, import_ctx),
        // TODO(ts-lowering:exceptions):
        //   Decide how (or if) TS `throw` / `try` / `catch` map onto Arth's
        //   exception model before adding lowering here.
        ast::Stmt::Throw(_) | ast::Stmt::Try(_) => Err(TsLoweringError::Unsupported(
            "exceptions are not yet lowered from TS to Arth HIR",
        )),
        ast::Stmt::Break(b) => {
            if b.label.is_some() {
                return Err(TsLoweringError::Unsupported(
                    "labeled `break` is not supported when lowering TS to Arth HIR; use unlabeled `break` inside structured loops instead (see docs/ts-subset.md §3.3).",
                ));
            }
            let span = span_from_swc(b.span, file);
            Ok(Some(HirStmt::Break {
                id: id_gen.fresh(),
                span,
                label: None,
            }))
        }
        ast::Stmt::Continue(c) => {
            if c.label.is_some() {
                return Err(TsLoweringError::Unsupported(
                    "labeled `continue` is not supported when lowering TS to Arth HIR; use unlabeled `continue` inside structured loops instead (see docs/ts-subset.md §3.3).",
                ));
            }
            let span = span_from_swc(c.span, file);
            Ok(Some(HirStmt::Continue {
                id: id_gen.fresh(),
                span,
                label: None,
            }))
        }
        ast::Stmt::Labeled(_) => Err(TsLoweringError::Unsupported(
            "labeled statements are not yet lowered from TS to Arth HIR; keep loops unlabeled in the TS guest subset (see docs/ts-subset.md §3.3).",
        )),
        ast::Stmt::Block(b) => {
            let blk = lower_block_stmt(b, file, id_gen, import_ctx)?;
            Ok(Some(HirStmt::Block(blk)))
        }
        ast::Stmt::Empty(_) => Ok(None),
        // Debugger/with and other statement kinds are outside the intended guest subset.
        _ => Err(TsLoweringError::Unsupported(
            "this statement kind is outside the current TS → Arth lowering subset; see docs/ts-subset.md §3.3 for allowed control flow.",
        )),
    }
}

fn lower_return_stmt(
    ret: &ast::ReturnStmt,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<Option<HirStmt>, TsLoweringError> {
    let span = span_from_swc(ret.span, file);
    let expr = match &ret.arg {
        Some(e) => Some(lower_expr(e, file, id_gen, import_ctx)?),
        None => None,
    };
    Ok(Some(HirStmt::Return {
        id: id_gen.fresh(),
        span,
        expr,
    }))
}

fn lower_expr_stmt(
    e: &ast::ExprStmt,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<Option<HirStmt>, TsLoweringError> {
    let span = span_from_swc(e.span, file);
    let expr = lower_expr(&e.expr, file, id_gen, import_ctx)?;
    Ok(Some(HirStmt::Expr {
        id: id_gen.fresh(),
        span,
        expr,
    }))
}

fn lower_assign_expr_stmt(
    a: &ast::AssignExpr,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<Option<HirStmt>, TsLoweringError> {
    use swc_ecma_ast::{AssignOp, AssignTarget, SimpleAssignTarget};

    let span = span_from_swc(a.span, file);

    match &a.left {
        AssignTarget::Simple(simple) => match simple {
            SimpleAssignTarget::Ident(bident) => {
                let name = bident.id.sym.to_string();
                match a.op {
                    AssignOp::Assign => {
                        let rhs = lower_expr(&a.right, file, id_gen, import_ctx)?;
                        Ok(Some(HirStmt::Assign {
                            id: id_gen.fresh(),
                            span,
                            name,
                            expr: rhs,
                        }))
                    }
                    AssignOp::AddAssign
                    | AssignOp::SubAssign
                    | AssignOp::MulAssign
                    | AssignOp::DivAssign
                    | AssignOp::ModAssign
                    | AssignOp::LShiftAssign
                    | AssignOp::RShiftAssign
                    | AssignOp::ZeroFillRShiftAssign
                    | AssignOp::BitAndAssign
                    | AssignOp::BitOrAssign
                    | AssignOp::BitXorAssign => {
                        use arth::compiler::hir::HirAssignOp as AO;
                        let op = match a.op {
                            AssignOp::AddAssign => AO::Add,
                            AssignOp::SubAssign => AO::Sub,
                            AssignOp::MulAssign => AO::Mul,
                            AssignOp::DivAssign => AO::Div,
                            AssignOp::ModAssign => AO::Mod,
                            AssignOp::LShiftAssign => AO::Shl,
                            AssignOp::RShiftAssign | AssignOp::ZeroFillRShiftAssign => AO::Shr,
                            AssignOp::BitAndAssign => AO::And,
                            AssignOp::BitOrAssign => AO::Or,
                            AssignOp::BitXorAssign => AO::Xor,
                            _ => unreachable!(),
                        };
                        let rhs = lower_expr(&a.right, file, id_gen, import_ctx)?;
                        Ok(Some(HirStmt::AssignOp {
                            id: id_gen.fresh(),
                            span,
                            name,
                            op,
                            expr: rhs,
                        }))
                    }
                    _ => Err(TsLoweringError::Unsupported(
                        "this assignment operator is not yet lowered from TS to Arth HIR; keep assignments to `=`, `+=`, `-=`, `*=`, `/=`, `%=`, `<<=`, `>>=`, `&=`, `|=`, `^=` in the TS guest subset",
                    )),
                }
            }
            SimpleAssignTarget::Member(m) => {
                // Map `obj.field = rhs` to a `FieldAssign` statement when the
                // field name is an identifier or string literal.
                let object_expr = lower_expr(&m.obj, file, id_gen, import_ctx)?;

                let field = match &m.prop {
                    ast::MemberProp::Ident(id) => id.sym.to_string(),
                    ast::MemberProp::Computed(comp) => {
                        if let ast::Expr::Lit(ast::Lit::Str(s)) = &*comp.expr {
                            s.value.as_str().unwrap_or("").to_string()
                        } else {
                            return Err(TsLoweringError::Unsupported(
                                "only identifier and string-literal property names are lowered for assignments in TS → Arth HIR; avoid computed keys in assignment targets (see docs/ts-subset.md §3.5).",
                            ));
                        }
                    }
                    ast::MemberProp::PrivateName(_) => {
                        return Err(TsLoweringError::Unsupported(
                            "private class fields (`#field`) are not lowered in the Arth TS guest subset; use public fields or helper methods instead (see docs/ts-subset.md §3.4–§3.5).",
                        ));
                    }
                };

                match a.op {
                    AssignOp::Assign => {
                        let rhs = lower_expr(&a.right, file, id_gen, import_ctx)?;
                        Ok(Some(HirStmt::FieldAssign {
                            id: id_gen.fresh(),
                            span,
                            object: object_expr,
                            field,
                            expr: rhs,
                        }))
                    }
                    AssignOp::AddAssign
                    | AssignOp::SubAssign
                    | AssignOp::MulAssign
                    | AssignOp::DivAssign
                    | AssignOp::ModAssign
                    | AssignOp::LShiftAssign
                    | AssignOp::RShiftAssign
                    | AssignOp::ZeroFillRShiftAssign
                    | AssignOp::BitAndAssign
                    | AssignOp::BitOrAssign
                    | AssignOp::BitXorAssign => {
                        use arth::compiler::hir::HirBinOp;
                        let op = match a.op {
                            AssignOp::AddAssign => HirBinOp::Add,
                            AssignOp::SubAssign => HirBinOp::Sub,
                            AssignOp::MulAssign => HirBinOp::Mul,
                            AssignOp::DivAssign => HirBinOp::Div,
                            AssignOp::ModAssign => HirBinOp::Mod,
                            AssignOp::LShiftAssign => HirBinOp::Shl,
                            AssignOp::RShiftAssign | AssignOp::ZeroFillRShiftAssign => {
                                HirBinOp::Shr
                            }
                            AssignOp::BitAndAssign => HirBinOp::BitAnd,
                            AssignOp::BitOrAssign => HirBinOp::BitOr,
                            AssignOp::BitXorAssign => HirBinOp::Xor,
                            _ => unreachable!(),
                        };

                        let current = HirExpr::Member {
                            id: id_gen.fresh(),
                            span: span.clone(),
                            object: Box::new(object_expr.clone()),
                            member: field.clone(),
                        };
                        let rhs = lower_expr(&a.right, file, id_gen, import_ctx)?;
                        let combined = HirExpr::Binary {
                            id: id_gen.fresh(),
                            span: span.clone(),
                            left: Box::new(current),
                            op,
                            right: Box::new(rhs),
                        };
                        Ok(Some(HirStmt::FieldAssign {
                            id: id_gen.fresh(),
                            span,
                            object: object_expr,
                            field,
                            expr: combined,
                        }))
                    }
                    _ => Err(TsLoweringError::Unsupported(
                        "this assignment operator is not yet lowered from TS to Arth HIR; keep assignments to `=`, `+=`, `-=`, `*=`, `/=`, `%=`, `<<=`, `>>=`, `&=`, `|=`, `^=` in the TS guest subset",
                    )),
                }
            }
            _ => Err(TsLoweringError::Unsupported(
                "assignment target is not yet lowered from TS to Arth HIR; assign only to simple locals or object fields (see docs/ts-subset.md §3.3–§3.5).",
            )),
        },
        AssignTarget::Pat(_) => Err(TsLoweringError::Unsupported(
            "destructuring assignments are not yet lowered from TS to Arth HIR; assign to simple identifiers instead (see docs/ts-subset.md §3.3).",
        )),
    }
}

fn lower_update_expr_stmt(
    u: &ast::UpdateExpr,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<Option<HirStmt>, TsLoweringError> {
    use swc_ecma_ast::UpdateOp;

    let span = span_from_swc(u.span, file);

    let (bin_op, assign_op) = match u.op {
        UpdateOp::PlusPlus => (
            arth::compiler::hir::HirBinOp::Add,
            arth::compiler::hir::HirAssignOp::Add,
        ),
        UpdateOp::MinusMinus => (
            arth::compiler::hir::HirBinOp::Sub,
            arth::compiler::hir::HirAssignOp::Sub,
        ),
    };

    let one = HirExpr::Float {
        id: id_gen.fresh(),
        span: span.clone(),
        value: 1.0,
    };

    match &*u.arg {
        ast::Expr::Ident(id) => Ok(Some(HirStmt::AssignOp {
            id: id_gen.fresh(),
            span,
            name: id.sym.to_string(),
            op: assign_op,
            expr: one,
        })),
        ast::Expr::Member(m) => {
            let object_expr = lower_expr(&m.obj, file, id_gen, import_ctx)?;

            let field = match &m.prop {
                ast::MemberProp::Ident(id) => id.sym.to_string(),
                ast::MemberProp::Computed(comp) => {
                    if let ast::Expr::Lit(ast::Lit::Str(s)) = &*comp.expr {
                        s.value.as_str().unwrap_or("").to_string()
                    } else {
                        return Err(TsLoweringError::Unsupported(
                            "only identifier and string-literal property names are lowered for `++`/`--` in TS → Arth HIR; avoid computed keys in update targets (see docs/ts-subset.md §3.5).",
                        ));
                    }
                }
                ast::MemberProp::PrivateName(_) => {
                    return Err(TsLoweringError::Unsupported(
                        "private class fields (`#field`) are not lowered in the Arth TS guest subset; use public fields or helper methods instead (see docs/ts-subset.md §3.4–§3.5).",
                    ));
                }
            };

            let current = HirExpr::Member {
                id: id_gen.fresh(),
                span: span.clone(),
                object: Box::new(object_expr.clone()),
                member: field.clone(),
            };
            let combined = HirExpr::Binary {
                id: id_gen.fresh(),
                span: span.clone(),
                left: Box::new(current),
                op: bin_op,
                right: Box::new(one),
            };

            Ok(Some(HirStmt::FieldAssign {
                id: id_gen.fresh(),
                span,
                object: object_expr,
                field,
                expr: combined,
            }))
        }
        _ => Err(TsLoweringError::Unsupported(
            "only identifier and `.field` update targets are lowered from TS to Arth HIR; rewrite complex update expressions as explicit assignments",
        )),
    }
}

fn lower_var_decl_stmt(
    var: &ast::VarDecl,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<Option<HirStmt>, TsLoweringError> {
    use swc_ecma_ast::VarDeclKind;

    if matches!(var.kind, VarDeclKind::Var) {
        return Err(TsLoweringError::Unsupported(
            "`var` declarations are not lowered from TS to Arth HIR; the TS guest subset only supports `let` / `const` (see docs/ts-subset.md §3.3).",
        ));
    }

    let span = span_from_swc(var.span, file);
    let mut stmts = Vec::new();

    for decl in &var.decls {
        let pat = &decl.name;
        let ident = match pat {
            ast::Pat::Ident(id) => id,
            _ => {
                return Err(TsLoweringError::Unsupported(
                    "only simple identifier variable declarations are lowered from TS to Arth HIR; destructure inside the statement body instead (see docs/ts-subset.md §3.3).",
                ));
            }
        };

        let name = ident.id.sym.to_string();
        let ty = HirType::Name {
            path: extract_type_name_from_ts(&ident.type_ann)
                .unwrap_or_else(|| vec!["Unknown".to_string()]),
        };

        let init_expr = if let Some(init) = &decl.init {
            Some(lower_expr(init, file, id_gen, import_ctx)?)
        } else {
            None
        };

        stmts.push(HirStmt::VarDecl {
            id: id_gen.fresh(),
            span: span.clone(),
            ty,
            name,
            init: init_expr,
            is_shared: false,
        });
    }

    if stmts.is_empty() {
        Ok(None)
    } else if stmts.len() == 1 {
        Ok(Some(stmts.remove(0)))
    } else {
        Ok(Some(HirStmt::Block(HirBlock {
            id: id_gen.fresh(),
            span,
            stmts,
        })))
    }
}

fn lower_expr(
    e: &ast::Expr,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<HirExpr, TsLoweringError> {
    use ast::Expr::*;

    Ok(match e {
        // Literals
        Lit(l) => match l {
            ast::Lit::Num(n) => HirExpr::Float {
                id: id_gen.fresh(),
                span: span_from_swc(n.span, file),
                value: n.value,
            },
            ast::Lit::Str(s) => {
                let text = s.value.as_str().unwrap_or("").to_string();
                HirExpr::Str {
                    id: id_gen.fresh(),
                    span: span_from_swc(s.span, file),
                    value: text,
                }
            }
            ast::Lit::Bool(b) => HirExpr::Bool {
                id: id_gen.fresh(),
                span: span_from_swc(b.span, file),
                value: b.value,
            },
            ast::Lit::Null(n) => {
                // `null` → `Optional.none()`
                let span = span_from_swc(n.span, file);
                let optional_module = HirExpr::Ident {
                    id: id_gen.fresh(),
                    span: span.clone(),
                    name: "Optional".to_string(),
                };
                let none_callee = HirExpr::Member {
                    id: id_gen.fresh(),
                    span: span.clone(),
                    object: Box::new(optional_module),
                    member: "none".to_string(),
                };
                HirExpr::Call {
                    id: id_gen.fresh(),
                    span,
                    callee: Box::new(none_callee),
                    args: Vec::new(),
                    borrow: None,
                }
            }
            _ => {
                return Err(TsLoweringError::Unsupported(
                    "this literal kind is outside the current TS → Arth expression subset; use number/boolean/string/null literals instead (see docs/ts-subset.md §3.2).",
                ));
            }
        },

        // Simple identifier
        Ident(id) => HirExpr::Ident {
            id: id_gen.fresh(),
            span: span_from_swc(id.span, file),
            name: id.sym.to_string(),
        },

        // Binary operators
        Bin(b) => {
            use arth::compiler::hir::HirBinOp;
            use swc_ecma_ast::BinaryOp as B;

            let op = match b.op {
                B::Add => HirBinOp::Add,
                B::Sub => HirBinOp::Sub,
                B::Mul => HirBinOp::Mul,
                B::Div => HirBinOp::Div,
                B::Mod => HirBinOp::Mod,
                B::Lt => HirBinOp::Lt,
                B::LtEq => HirBinOp::Le,
                B::Gt => HirBinOp::Gt,
                B::GtEq => HirBinOp::Ge,
                B::EqEq | B::EqEqEq => HirBinOp::Eq,
                B::NotEq | B::NotEqEq => HirBinOp::Ne,
                B::LogicalAnd => HirBinOp::And,
                B::LogicalOr => HirBinOp::Or,
                B::BitAnd => HirBinOp::BitAnd,
                B::BitOr => HirBinOp::BitOr,
                B::BitXor => HirBinOp::Xor,
                B::LShift => HirBinOp::Shl,
                B::RShift | B::ZeroFillRShift => HirBinOp::Shr,
                _ => {
                    return Err(TsLoweringError::Unsupported(
                        "this binary operator is outside the Arth TS guest subset; use the arithmetic/comparison/logical operators listed in docs/ts-subset.md §3.3.",
                    ));
                }
            };

            HirExpr::Binary {
                id: id_gen.fresh(),
                span: span_from_swc(b.span, file),
                left: Box::new(lower_expr(&b.left, file, id_gen, import_ctx)?),
                op,
                right: Box::new(lower_expr(&b.right, file, id_gen, import_ctx)?),
            }
        }

        // Unary operators
        Unary(u) => {
            use arth::compiler::hir::HirUnOp;
            use swc_ecma_ast::UnaryOp as U;

            let op = match u.op {
                U::Minus => HirUnOp::Neg,
                U::Bang => HirUnOp::Not,
                U::Plus | U::Tilde | U::TypeOf | U::Void | U::Delete => {
                    return Err(TsLoweringError::Unsupported(
                        "this unary operator is outside the current TS → Arth expression subset; use only `-` and `!` (see docs/ts-subset.md §3.3).",
                    ));
                }
            };

            HirExpr::Unary {
                id: id_gen.fresh(),
                span: span_from_swc(u.span, file),
                op,
                expr: Box::new(lower_expr(&u.arg, file, id_gen, import_ctx)?),
            }
        }

        // Function/closure calls
        Call(c) => {
            let span = span_from_swc(c.span, file);

            // Check for built-in global mappings (console.*, Date.*, etc.)
            if let Some(mapped) = try_map_builtin_call(c, file, id_gen, import_ctx)? {
                return Ok(mapped);
            }

            // Check if the callee is a simple identifier that needs import resolution.
            // If `import { add } from "./math"` then `add(x)` → `Math.add(x)`
            let callee_expr = match &c.callee {
                ast::Callee::Expr(expr) => {
                    if let ast::Expr::Ident(ident) = &**expr {
                        let name = ident.sym.to_string();
                        if let Some(qualified) = import_ctx.resolve(&name) {
                            // Convert "Module.function" into a Member expression
                            let ident_span = span_from_swc(ident.span, file);
                            if let Some(dot_pos) = qualified.find('.') {
                                let module_name = &qualified[..dot_pos];
                                let func_name = &qualified[dot_pos + 1..];
                                let module_expr = HirExpr::Ident {
                                    id: id_gen.fresh(),
                                    span: ident_span.clone(),
                                    name: module_name.to_string(),
                                };
                                HirExpr::Member {
                                    id: id_gen.fresh(),
                                    span: ident_span,
                                    object: Box::new(module_expr),
                                    member: func_name.to_string(),
                                }
                            } else {
                                // No dot, just use the qualified name as-is
                                HirExpr::Ident {
                                    id: id_gen.fresh(),
                                    span: ident_span,
                                    name: qualified.clone(),
                                }
                            }
                        } else {
                            // Not an imported name, lower normally
                            lower_expr(expr, file, id_gen, import_ctx)?
                        }
                    } else {
                        lower_expr(expr, file, id_gen, import_ctx)?
                    }
                }
                _ => {
                    return Err(TsLoweringError::Unsupported(
                        "only simple expression callees are lowered from TS to Arth HIR; avoid `super`/`import` call forms in the guest subset (see docs/ts-subset.md §3.3).",
                    ));
                }
            };

            let mut args = Vec::new();
            for arg in &c.args {
                if arg.spread.is_some() {
                    return Err(TsLoweringError::Unsupported(
                        "spread arguments (`f(...xs)`) are not yet lowered from TS to Arth HIR; expand the array explicitly before the call (see docs/ts-subset.md §3.5).",
                    ));
                }
                let e = lower_expr(&arg.expr, file, id_gen, import_ctx)?;
                args.push(e);
            }

            HirExpr::Call {
                id: id_gen.fresh(),
                span,
                callee: Box::new(callee_expr),
                args,
                borrow: None,
            }
        }

        // `new Ctor(args...)` → `Ctor.new(args...)`
        New(n) => {
            // Only simple identifier constructors are supported for now.
            let ctor_ident = match &*n.callee {
                ast::Expr::Ident(id) => id,
                _ => {
                    return Err(TsLoweringError::Unsupported(
                        "only `new` expressions of the form `new Ctor(...)` are lowered from TS to Arth HIR; avoid complex constructor expressions in the TS guest subset.",
                    ));
                }
            };

            let span = span_from_swc(n.span, file);
            let ctor_name = ctor_ident.sym.to_string();

            // Build `Ctor.new` callee as a member expression on the type name.
            let ctor_mod = HirExpr::Ident {
                id: id_gen.fresh(),
                span: span.clone(),
                name: ctor_name,
            };
            let ctor_member = HirExpr::Member {
                id: id_gen.fresh(),
                span: span.clone(),
                object: Box::new(ctor_mod),
                member: "new".to_string(),
            };

            let mut args = Vec::new();
            if let Some(nargs) = &n.args {
                for arg in nargs {
                    if arg.spread.is_some() {
                        return Err(TsLoweringError::Unsupported(
                            "spread arguments are not yet lowered from TS `new` expressions; expand them explicitly before the `new` call (see docs/ts-subset.md §3.5).",
                        ));
                    }
                    let e = lower_expr(&arg.expr, file, id_gen, import_ctx)?;
                    args.push(e);
                }
            }

            HirExpr::Call {
                id: id_gen.fresh(),
                span,
                callee: Box::new(ctor_member),
                args,
                borrow: None,
            }
        }

        // Ternary conditional: `cond ? then_expr : else_expr`
        Cond(c) => HirExpr::Conditional {
            id: id_gen.fresh(),
            span: span_from_swc(c.span, file),
            cond: Box::new(lower_expr(&c.test, file, id_gen, import_ctx)?),
            then_expr: Box::new(lower_expr(&c.cons, file, id_gen, import_ctx)?),
            else_expr: Box::new(lower_expr(&c.alt, file, id_gen, import_ctx)?),
        },

        // Member / index access: `obj.prop` or `obj[expr]`
        Member(m) => {
            let object = lower_expr(&m.obj, file, id_gen, import_ctx)?;
            match &m.prop {
                ast::MemberProp::Ident(name) => HirExpr::Member {
                    id: id_gen.fresh(),
                    span: span_from_swc(m.span, file),
                    object: Box::new(object),
                    member: name.sym.to_string(),
                },
                ast::MemberProp::Computed(comp) => {
                    let index = lower_expr(&comp.expr, file, id_gen, import_ctx)?;
                    HirExpr::Index {
                        id: id_gen.fresh(),
                        span: span_from_swc(m.span, file),
                        object: Box::new(object),
                        index: Box::new(index),
                    }
                }
                ast::MemberProp::PrivateName(_) => {
                    return Err(TsLoweringError::Unsupported(
                        "private class fields (`#field`) are not lowered in the Arth TS guest subset; use public fields or helper methods instead (see docs/ts-subset.md §3.4–§3.5).",
                    ));
                }
            }
        }

        // Array literals: `[e1, e2, ...]` → `ListLit`
        Array(arr) => {
            let mut elements = Vec::new();
            for elem in &arr.elems {
                let Some(elem) = elem else {
                    return Err(TsLoweringError::Unsupported(
                        "array holes (e.g. `[1,,2]`) are not lowered from TS to Arth HIR; use explicit elements instead (see docs/ts-subset.md §3.5).",
                    ));
                };

                if elem.spread.is_some() {
                    return Err(TsLoweringError::Unsupported(
                        "array spread (`[...xs]`) is not yet lowered from TS to Arth HIR; build arrays via concatenation or explicit loops instead (see docs/ts-subset.md §3.5).",
                    ));
                }

                let e = lower_expr(&elem.expr, file, id_gen, import_ctx)?;
                elements.push(e);
            }

            HirExpr::ListLit {
                id: id_gen.fresh(),
                span: span_from_swc(arr.span, file),
                elements,
            }
        }

        // Object literals → anonymous StructLit.
        //
        // WAID controller code uses JS-style dot property access (`obj.field`)
        // on object literals. In Arth HIR/IR, dot access is implemented via
        // `__arth_struct_get_named` / `__arth_struct_set_named`, so we model
        // TS object literals as runtime structs with named fields rather than
        // maps. This keeps `obj.field` working without requiring a full JS
        // object model in the VM.
        Object(obj) => {
            let mut fields = Vec::new();

            for prop in &obj.props {
                match prop {
                    ast::PropOrSpread::Spread(_) => {
                        return Err(TsLoweringError::Unsupported(
                            "object spread (`{ ...other }`) is not yet lowered from TS to Arth HIR; copy fields explicitly or via helpers instead (see docs/ts-subset.md §3.5).",
                        ));
                    }
                    ast::PropOrSpread::Prop(p) => match &**p {
                        ast::Prop::KeyValue(kv) => {
                            let field_name = match &kv.key {
                                ast::PropName::Ident(id) => id.sym.to_string(),
                                ast::PropName::Str(s) => s.value.as_str().unwrap_or("").to_string(),
                                _ => {
                                    return Err(TsLoweringError::Unsupported(
                                        "object literal keys must be identifiers or string literals for TS → Arth lowering; computed/numeric/symbol keys are outside the current subset (see docs/ts-subset.md §3.5).",
                                    ));
                                }
                            };

                            let value_expr = lower_expr(&kv.value, file, id_gen, import_ctx)?;
                            fields.push((field_name, value_expr));
                        }
                        ast::Prop::Shorthand(id) => {
                            let value_expr = HirExpr::Ident {
                                id: id_gen.fresh(),
                                span: span_from_swc(id.span, file),
                                name: id.sym.to_string(),
                            };
                            fields.push((id.sym.to_string(), value_expr));
                        }
                        ast::Prop::Method(_)
                        | ast::Prop::Getter(_)
                        | ast::Prop::Setter(_)
                        | ast::Prop::Assign(_) => {
                            return Err(TsLoweringError::Unsupported(
                                "object literal methods/getters/setters/assign props are not lowered from TS to Arth HIR; encode them as fields whose values are functions instead (see docs/ts-subset.md §3.5).",
                            ));
                        }
                    },
                }
            }

            HirExpr::StructLit {
                id: id_gen.fresh(),
                span: span_from_swc(obj.span, file),
                type_name: HirType::Name {
                    path: vec!["Object".to_string()],
                },
                fields,
                spread: None, // TypeScript frontend doesn't support spread syntax yet
            }
        }

        // Arrow function expression → lambda
        Arrow(a) => {
            // Parameters: only simple identifiers are supported (validator enforces this).
            let mut params = Vec::new();
            for p in &a.params {
                let ast::Pat::Ident(id) = p else {
                    return Err(TsLoweringError::Unsupported(
                        "arrow function parameters must be simple identifiers in TS → Arth lowering; destructure inside the body instead (see docs/ts-subset.md §3.3).",
                    ));
                };
                let pname = id.id.sym.to_string();
                let ty = HirType::Name {
                    path: extract_type_name_from_ts(&id.type_ann)
                        .unwrap_or_else(|| vec!["Unknown".to_string()]),
                };

                params.push(arth::compiler::hir::HirParam { name: pname, ty });
            }

            let ret_ty =
                extract_type_name_from_ts(&a.return_type).map(|path| HirType::Name { path });
            let span = span_from_swc(a.span, file);

            let body = match &*a.body {
                ast::BlockStmtOrExpr::BlockStmt(block) => {
                    lower_block_stmt(block, file, id_gen, import_ctx)?
                }
                ast::BlockStmtOrExpr::Expr(expr) => {
                    let expr_span = span_from_swc(expr.span(), file);
                    let expr_hir = lower_expr(expr, file, id_gen, import_ctx)?;
                    let ret_stmt = HirStmt::Return {
                        id: id_gen.fresh(),
                        span: expr_span.clone(),
                        expr: Some(expr_hir),
                    };
                    HirBlock {
                        id: id_gen.fresh(),
                        span: span.clone(),
                        stmts: vec![ret_stmt],
                    }
                }
            };

            HirExpr::Lambda {
                id: id_gen.fresh(),
                span,
                params,
                body,
                ret: ret_ty,
                // For TS guest code we currently do not perform capture
                // analysis; lambdas behave as capture-less functions.
                captures: Vec::new(),
            }
        }

        // `function () { ... }` expression → lambda
        Fn(f) => {
            if f.function.is_generator {
                return Err(TsLoweringError::Unsupported(
                    "generator functions (`function*`) are not lowered from TS to Arth HIR; rewrite using async/await or explicit state machines instead (see docs/ts-subset.md §3.4).",
                ));
            }

            let mut params = Vec::new();
            for p in &f.function.params {
                let ast::Pat::Ident(id) = &p.pat else {
                    return Err(TsLoweringError::Unsupported(
                        "function expression parameters must be simple identifiers in TS → Arth lowering; destructure inside the body instead (see docs/ts-subset.md §3.3).",
                    ));
                };
                let pname = id.id.sym.to_string();
                let ty = HirType::Name {
                    path: extract_type_name_from_ts(&id.type_ann)
                        .unwrap_or_else(|| vec!["Unknown".to_string()]),
                };
                params.push(arth::compiler::hir::HirParam { name: pname, ty });
            }

            let ret_ty = extract_type_name_from_ts(&f.function.return_type)
                .map(|path| HirType::Name { path });

            let span = span_from_swc(f.function.span, file);

            let body = if let Some(block) = &f.function.body {
                lower_block_stmt(block, file, id_gen, import_ctx)?
            } else {
                HirBlock {
                    id: id_gen.fresh(),
                    span: span.clone(),
                    stmts: Vec::new(),
                }
            };

            HirExpr::Lambda {
                id: id_gen.fresh(),
                span,
                params,
                body,
                ret: ret_ty,
                captures: Vec::new(),
            }
        }

        // Await expression
        Await(a) => HirExpr::Await {
            id: id_gen.fresh(),
            span: span_from_swc(a.span, file),
            expr: Box::new(lower_expr(&a.arg, file, id_gen, import_ctx)?),
        },

        // `this` → `self` identifier for methods
        This(t) => HirExpr::Ident {
            id: id_gen.fresh(),
            span: span_from_swc(t.span, file),
            name: "self".to_string(),
        },

        // Parenthesized expression: `(expr)` → `expr`
        Paren(p) => lower_expr(&p.expr, file, id_gen, import_ctx)?,

        // TS cast / assertion wrappers: erase at runtime.
        TsAs(a) => lower_expr(&a.expr, file, id_gen, import_ctx)?,
        TsConstAssertion(a) => lower_expr(&a.expr, file, id_gen, import_ctx)?,
        TsNonNull(nn) => lower_expr(&nn.expr, file, id_gen, import_ctx)?,
        TsTypeAssertion(a) => lower_expr(&a.expr, file, id_gen, import_ctx)?,
        TsInstantiation(inst) => lower_expr(&inst.expr, file, id_gen, import_ctx)?,

        // Template literals: `Hello ${name}!` → String concatenation
        Tpl(tpl) => lower_template_literal(tpl, file, id_gen, import_ctx)?,

        _ => {
            return Err(TsLoweringError::Unsupported(
                "this expression kind is not yet lowered from TS to Arth HIR; keep guest code to the subset in docs/ts-subset.md §3 or extend the lowerer.",
            ));
        }
    })
}

/// Try to map a built-in global call to its Arth equivalent.
///
/// Maps:
/// - `console.log/info/warn/error/debug/trace(...)` → `Log.info/info/warn/error/debug/trace(...)`
/// - `Date.now()` → `DateTime.nowMillis()`
/// - `Math.*()` → `Math.*()`
/// - `JSON.stringify(x)` → `Json.encode(x)`
/// - `JSON.parse(x)` → `Json.decode(x)`
fn try_map_builtin_call(
    c: &ast::CallExpr,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<Option<HirExpr>, TsLoweringError> {
    let ast::Callee::Expr(callee_expr) = &c.callee else {
        return Ok(None);
    };

    // Check for member expression pattern: `object.method(...)`
    let ast::Expr::Member(member) = callee_expr.as_ref() else {
        return Ok(None);
    };

    // Get the object name (must be an identifier like `console`, `Date`, `Math`, `JSON`)
    let ast::Expr::Ident(obj_ident) = member.obj.as_ref() else {
        return Ok(None);
    };
    let obj_name = obj_ident.sym.as_str();

    // Get the method name
    let ast::MemberProp::Ident(method_ident) = &member.prop else {
        return Ok(None);
    };
    let method_name = method_ident.sym.as_str();

    let span = span_from_swc(c.span, file);

    // Map the object and method to Arth equivalents
    let (arth_module, arth_method) = match (obj_name, method_name) {
        // console.* → Log.*
        ("console", "log") => ("Log", "info"),
        ("console", "info") => ("Log", "info"),
        ("console", "warn") => ("Log", "warn"),
        ("console", "error") => ("Log", "error"),
        ("console", "debug") => ("Log", "debug"),
        ("console", "trace") => ("Log", "trace"),

        // Date.now() → DateTime.nowMillis()
        ("Date", "now") => ("DateTime", "nowMillis"),

        // JSON.stringify(x) → Json.stringify(x)
        // JSON.parse(x) → Json.parse(x)
        ("JSON", "stringify") => ("Json", "stringify"),
        ("JSON", "parse") => ("Json", "parse"),

        // Math.* passes through (Arth has a Math module)
        ("Math", _) => ("Math", method_name),

        _ => return Ok(None),
    };

    // Build the Arth call expression
    let module_expr = HirExpr::Ident {
        id: id_gen.fresh(),
        span: span.clone(),
        name: arth_module.to_string(),
    };
    let callee = HirExpr::Member {
        id: id_gen.fresh(),
        span: span.clone(),
        object: Box::new(module_expr),
        member: arth_method.to_string(),
    };

    // Lower the arguments
    let mut args = Vec::new();
    for arg in &c.args {
        if arg.spread.is_some() {
            return Err(TsLoweringError::Unsupported(
                "spread arguments are not supported in built-in call mappings",
            ));
        }
        args.push(lower_expr(&arg.expr, file, id_gen, import_ctx)?);
    }

    Ok(Some(HirExpr::Call {
        id: id_gen.fresh(),
        span,
        callee: Box::new(callee),
        args,
        borrow: None,
    }))
}

/// Lower a template literal to string concatenation.
///
/// Converts `` `Hello ${name}!` `` to `"Hello " + name + "!"`.
fn lower_template_literal(
    tpl: &ast::Tpl,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<HirExpr, TsLoweringError> {
    let span = span_from_swc(tpl.span, file);

    // Template literals have alternating quasis (string parts) and expressions.
    // quasis: ["Hello ", "!"], exprs: [name]
    // Result: "Hello " + name + "!"

    let mut parts: Vec<HirExpr> = Vec::new();

    for (i, quasi) in tpl.quasis.iter().enumerate() {
        // Add the string part (quasi)
        // Use cooked value if available (processed escapes), otherwise use raw
        let text = quasi
            .cooked
            .as_ref()
            .map(|s| s.as_str().unwrap_or("").to_string())
            .unwrap_or_else(|| quasi.raw.to_string());

        if !text.is_empty() {
            parts.push(HirExpr::Str {
                id: id_gen.fresh(),
                span: span_from_swc(quasi.span, file),
                value: text,
            });
        }

        // Add the expression part (if not the last quasi)
        if i < tpl.exprs.len() {
            let expr = lower_expr(&tpl.exprs[i], file, id_gen, import_ctx)?;
            parts.push(expr);
        }
    }

    // If empty template, return empty string
    if parts.is_empty() {
        return Ok(HirExpr::Str {
            id: id_gen.fresh(),
            span,
            value: String::new(),
        });
    }

    // If single part, return it directly
    if parts.len() == 1 {
        return Ok(parts.remove(0));
    }

    // Build a chain of binary Add operations: part1 + part2 + part3 + ...
    let mut result = parts.remove(0);
    for part in parts {
        result = HirExpr::Binary {
            id: id_gen.fresh(),
            span: span.clone(),
            left: Box::new(result),
            op: arth::compiler::hir::HirBinOp::Add,
            right: Box::new(part),
        };
    }

    Ok(result)
}

fn lower_for_stmt(
    for_stmt: &ast::ForStmt,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<Option<HirStmt>, TsLoweringError> {
    let span = span_from_swc(for_stmt.span, file);
    let mut stmts: Vec<HirStmt> = Vec::new();

    // Lower init (if present) into a statement placed before the loop.
    if let Some(init) = &for_stmt.init {
        let init_stmt = match init {
            ast::VarDeclOrExpr::VarDecl(var) => lower_var_decl_stmt(var, file, id_gen, import_ctx)?,
            ast::VarDeclOrExpr::Expr(expr) => {
                let expr_span = span_from_swc(expr.span(), file);
                match &**expr {
                    ast::Expr::Assign(a) => lower_assign_expr_stmt(a, file, id_gen, import_ctx)?,
                    ast::Expr::Update(u) => lower_update_expr_stmt(u, file, id_gen, import_ctx)?,
                    _ => {
                        let e = lower_expr(expr, file, id_gen, import_ctx)?;
                        Some(HirStmt::Expr {
                            id: id_gen.fresh(),
                            span: expr_span,
                            expr: e,
                        })
                    }
                }
            }
        };
        if let Some(st) = init_stmt {
            stmts.push(st);
        }
    }

    // Condition: if absent, assume `true` (like Arth surface for-loops).
    let cond = if let Some(test) = &for_stmt.test {
        lower_expr(test, file, id_gen, import_ctx)?
    } else {
        HirExpr::Bool {
            id: id_gen.fresh(),
            span: span.clone(),
            value: true,
        }
    };

    // Body block.
    let mut body =
        lower_single_stmt_as_block(&for_stmt.body, for_stmt.span, file, id_gen, import_ctx)?;

    // Step is appended at the end of the loop body if present.
    if let Some(update) = &for_stmt.update {
        let update_span = span_from_swc(update.span(), file);
        let update_stmt = match update.as_ref() {
            ast::Expr::Assign(a) => lower_assign_expr_stmt(a, file, id_gen, import_ctx)?,
            ast::Expr::Update(u) => lower_update_expr_stmt(u, file, id_gen, import_ctx)?,
            _ => {
                let e = lower_expr(update, file, id_gen, import_ctx)?;
                Some(HirStmt::Expr {
                    id: id_gen.fresh(),
                    span: update_span,
                    expr: e,
                })
            }
        };
        if let Some(st) = update_stmt {
            body.stmts.push(st);
        }
    }

    let while_stmt = HirStmt::While {
        id: id_gen.fresh(),
        span: span.clone(),
        cond,
        body,
    };
    stmts.push(while_stmt);

    Ok(Some(HirStmt::Block(HirBlock {
        id: id_gen.fresh(),
        span,
        stmts,
    })))
}

fn lower_for_of_stmt(
    for_of: &ast::ForOfStmt,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<Option<HirStmt>, TsLoweringError> {
    use ast::ForHead;

    let span = span_from_swc(for_of.span, file);

    // Extract element variable name and (optional) type.
    let (elem_name, elem_type_ann) = match &for_of.left {
        ForHead::VarDecl(var) => {
            let var = &**var;
            if var.decls.len() != 1 {
                return Err(TsLoweringError::Unsupported(
                    "for-of lowering currently supports a single loop variable; split multiple declarations into separate loops.",
                ));
            }
            let decl = &var.decls[0];
            let ident = match &decl.name {
                ast::Pat::Ident(id) => id,
                _ => {
                    return Err(TsLoweringError::Unsupported(
                        "for-of lowering only supports simple identifier bindings; destructure inside the loop body instead (see docs/ts-subset.md §3.3).",
                    ));
                }
            };
            (ident.id.sym.to_string(), ident.type_ann.clone())
        }
        ForHead::Pat(pat) => {
            let ident = match &**pat {
                ast::Pat::Ident(id) => id,
                _ => {
                    return Err(TsLoweringError::Unsupported(
                        "for-of lowering only supports simple identifier bindings; destructure inside the loop body instead (see docs/ts-subset.md §3.3).",
                    ));
                }
            };
            (ident.id.sym.to_string(), ident.type_ann.clone())
        }
        ForHead::UsingDecl(_) => {
            return Err(TsLoweringError::Unsupported(
                "`using` declarations are not supported in TS guest for-of loops; declare locals explicitly instead.",
            ));
        }
    };

    // Synthesize temporary names for the array and index.
    let arr_id = id_gen.fresh();
    let arr_name = format!("__ts_for_of_arr_{}", arr_id.0);
    let idx_id = id_gen.fresh();
    let idx_name = format!("__ts_for_of_idx_{}", idx_id.0);

    // Arr variable: `let __ts_for_of_arr_N = <right>;`
    let arr_init = lower_expr(&for_of.right, file, id_gen, import_ctx)?;
    let arr_decl = HirStmt::VarDecl {
        id: arr_id,
        span: span.clone(),
        ty: HirType::Name {
            path: vec!["Unknown".to_string()],
        },
        name: arr_name.clone(),
        init: Some(arr_init),
        is_shared: false,
    };

    // Index variable: `let __ts_for_of_idx_N: Int = 0;`
    let idx_decl = HirStmt::VarDecl {
        id: idx_id,
        span: span.clone(),
        ty: HirType::Name {
            path: vec!["Int".to_string()],
        },
        name: idx_name.clone(),
        init: Some(HirExpr::Int {
            id: id_gen.fresh(),
            span: span.clone(),
            value: 0,
        }),
        is_shared: false,
    };

    // Element variable: `let elem: T;` (initialized per-iteration).
    let elem_ty = HirType::Name {
        path: extract_type_name_from_ts(&elem_type_ann)
            .unwrap_or_else(|| vec!["Unknown".to_string()]),
    };
    let elem_decl = HirStmt::VarDecl {
        id: id_gen.fresh(),
        span: span.clone(),
        ty: elem_ty,
        name: elem_name.clone(),
        init: None,
        is_shared: false,
    };

    // Build `List.len(__arr)`
    let list_ident = HirExpr::Ident {
        id: id_gen.fresh(),
        span: span.clone(),
        name: "List".to_string(),
    };
    let list_len_callee = HirExpr::Member {
        id: id_gen.fresh(),
        span: span.clone(),
        object: Box::new(list_ident),
        member: "len".to_string(),
    };
    let arr_ident_for_len = HirExpr::Ident {
        id: id_gen.fresh(),
        span: span.clone(),
        name: arr_name.clone(),
    };
    let len_call = HirExpr::Call {
        id: id_gen.fresh(),
        span: span.clone(),
        callee: Box::new(list_len_callee),
        args: vec![arr_ident_for_len],
        borrow: None,
    };

    // Condition: `__idx < List.len(__arr)`
    let idx_ident_for_cond = HirExpr::Ident {
        id: id_gen.fresh(),
        span: span.clone(),
        name: idx_name.clone(),
    };
    let cond = HirExpr::Binary {
        id: id_gen.fresh(),
        span: span.clone(),
        left: Box::new(idx_ident_for_cond),
        op: arth::compiler::hir::HirBinOp::Lt,
        right: Box::new(len_call),
    };

    // Element assignment at loop head: `elem = List.get(__arr, __idx);`
    let list_ident_get = HirExpr::Ident {
        id: id_gen.fresh(),
        span: span.clone(),
        name: "List".to_string(),
    };
    let list_get_callee = HirExpr::Member {
        id: id_gen.fresh(),
        span: span.clone(),
        object: Box::new(list_ident_get),
        member: "get".to_string(),
    };
    let arr_ident_for_get = HirExpr::Ident {
        id: id_gen.fresh(),
        span: span.clone(),
        name: arr_name.clone(),
    };
    let idx_ident_for_get = HirExpr::Ident {
        id: id_gen.fresh(),
        span: span.clone(),
        name: idx_name.clone(),
    };
    let get_call = HirExpr::Call {
        id: id_gen.fresh(),
        span: span.clone(),
        callee: Box::new(list_get_callee),
        args: vec![arr_ident_for_get, idx_ident_for_get],
        borrow: None,
    };
    let elem_assign = HirStmt::Assign {
        id: id_gen.fresh(),
        span: span.clone(),
        name: elem_name.clone(),
        expr: get_call,
    };

    // User body, lowered to a block.
    let mut body_block =
        lower_single_stmt_as_block(&for_of.body, for_of.span, file, id_gen, import_ctx)?;
    // Prepend element assignment.
    body_block.stmts.insert(0, elem_assign);

    // Index increment at end of body: `__idx = __idx + 1;`
    let idx_ident_lhs = idx_name.clone();
    let idx_ident_for_rhs = HirExpr::Ident {
        id: id_gen.fresh(),
        span: span.clone(),
        name: idx_name.clone(),
    };
    let one_expr = HirExpr::Int {
        id: id_gen.fresh(),
        span: span.clone(),
        value: 1,
    };
    let idx_plus_one = HirExpr::Binary {
        id: id_gen.fresh(),
        span: span.clone(),
        left: Box::new(idx_ident_for_rhs),
        op: arth::compiler::hir::HirBinOp::Add,
        right: Box::new(one_expr),
    };
    let idx_inc = HirStmt::Assign {
        id: id_gen.fresh(),
        span: span.clone(),
        name: idx_ident_lhs,
        expr: idx_plus_one,
    };
    body_block.stmts.push(idx_inc);

    let while_stmt = HirStmt::While {
        id: id_gen.fresh(),
        span: span.clone(),
        cond,
        body: body_block,
    };

    let stmts = vec![arr_decl, idx_decl, elem_decl, while_stmt];
    Ok(Some(HirStmt::Block(HirBlock {
        id: id_gen.fresh(),
        span,
        stmts,
    })))
}

fn lower_switch_stmt(
    sw: &ast::SwitchStmt,
    file: &Arc<PathBuf>,
    id_gen: &mut HirIdGen,
    import_ctx: &ImportContext,
) -> Result<Option<HirStmt>, TsLoweringError> {
    let span = span_from_swc(sw.span, file);
    let expr = lower_expr(&sw.discriminant, file, id_gen, import_ctx)?;

    let mut cases: Vec<(HirExpr, HirBlock)> = Vec::new();
    let mut default_blk: Option<HirBlock> = None;

    for case in &sw.cases {
        let body_block = lower_stmts_to_block(&case.cons, sw.span, file, id_gen, import_ctx)?;
        if let Some(test) = &case.test {
            let test_expr = lower_expr(test, file, id_gen, import_ctx)?;
            cases.push((test_expr, body_block));
        } else {
            default_blk = Some(body_block);
        }
    }

    Ok(Some(HirStmt::Switch {
        id: id_gen.fresh(),
        span,
        expr,
        cases,
        default: default_blk,
        pattern_cases: Vec::new(),
    }))
}

fn enforce_guest_mode(hir: &HirFile) -> Result<(), TsLoweringError> {
    if !hir.is_guest {
        return Ok(());
    }

    for decl in &hir.decls {
        match decl {
            HirDecl::ExternFunc(_) => {
                return Err(TsLoweringError::Unsupported(
                    "extern functions and FFI declarations are not allowed in TS guest modules; define FFI in Arth and expose safe wrappers instead (see docs/ts-subset.md §6.1).",
                ));
            }
            HirDecl::Function(func) => {
                enforce_guest_func(func)?;
            }
            HirDecl::Module(module) => {
                for func in &module.funcs {
                    enforce_guest_func(func)?;
                }
            }
            _ => {}
        }
    }

    Ok(())
}

fn enforce_guest_func(func: &HirFunc) -> Result<(), TsLoweringError> {
    if func.sig.is_unsafe {
        return Err(TsLoweringError::Unsupported(
            "unsafe functions are not allowed in TS guest modules; keep TS guest code in the safe subset (see docs/ts-subset.md §6.1).",
        ));
    }

    if let Some(body) = &func.body {
        enforce_guest_block(body)?;
    }

    Ok(())
}

fn enforce_guest_block(block: &HirBlock) -> Result<(), TsLoweringError> {
    for stmt in &block.stmts {
        enforce_guest_stmt(stmt)?;
    }
    Ok(())
}

fn enforce_guest_stmt(stmt: &HirStmt) -> Result<(), TsLoweringError> {
    match stmt {
        HirStmt::Unsafe { .. } => {
            return Err(TsLoweringError::Unsupported(
                "unsafe blocks are not allowed in TS guest modules; use safe primitives or host capabilities instead (see docs/ts-subset.md §6.1).",
            ));
        }
        HirStmt::If {
            then_blk, else_blk, ..
        } => {
            enforce_guest_block(then_blk)?;
            if let Some(else_block) = else_blk {
                enforce_guest_block(else_block)?;
            }
        }
        HirStmt::While { body, .. } => {
            enforce_guest_block(body)?;
        }
        HirStmt::Labeled { stmt: inner, .. } => {
            enforce_guest_stmt(inner)?;
        }
        HirStmt::For { body, .. } => {
            enforce_guest_block(body)?;
        }
        HirStmt::Switch { cases, default, .. } => {
            for (_, case_block) in cases {
                enforce_guest_block(case_block)?;
            }
            if let Some(default_block) = default {
                enforce_guest_block(default_block)?;
            }
        }
        HirStmt::Try {
            try_blk,
            catches,
            finally_blk,
            ..
        } => {
            enforce_guest_block(try_blk)?;
            for catch_clause in catches {
                enforce_guest_block(&catch_clause.block)?;
            }
            if let Some(finally_block) = finally_blk {
                enforce_guest_block(finally_block)?;
            }
        }
        HirStmt::Block(block) => {
            enforce_guest_block(block)?;
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_import_context_binding() {
        let mut ctx = ImportContext::new();
        ctx.add_binding("add".to_string(), "Math", "add");
        ctx.add_binding("multiply".to_string(), "Math", "multiply");
        ctx.add_binding("foo".to_string(), "Utils", "bar");

        assert_eq!(ctx.resolve("add"), Some(&"Math.add".to_string()));
        assert_eq!(ctx.resolve("multiply"), Some(&"Math.multiply".to_string()));
        assert_eq!(ctx.resolve("foo"), Some(&"Utils.bar".to_string()));
        assert_eq!(ctx.resolve("unknown"), None);
    }

    #[test]
    fn test_module_name_from_import_source() {
        assert_eq!(module_name_from_import_source("./math"), "Math");
        assert_eq!(module_name_from_import_source("./math.ts"), "Math");
        assert_eq!(
            module_name_from_import_source("../utils/helpers"),
            "Helpers"
        );
        // PascalCase: each word separated by - or _ gets capitalized
        assert_eq!(module_name_from_import_source("./my-module"), "MyModule");
        assert_eq!(module_name_from_import_source("./my_module"), "MyModule");
    }

    #[test]
    fn test_import_resolution_in_lowering() {
        // Test that function calls to imported names are resolved.
        let source = r#"
import { add } from "./math";

export function main(): number {
  let x = add(5, 3);
  return x;
}
"#;
        let hir = lower_ts_str_to_hir(source, "main.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        // Find the module and the main function
        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        let main_func = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "main")
            .expect("should have main function");

        // Get the body
        let body = main_func.body.as_ref().expect("main should have body");
        // First statement should be VarDecl with init expr being a Call with Math.add
        let var_decl = body.stmts.first().expect("should have statements");
        if let HirStmt::VarDecl {
            init: Some(init_expr),
            ..
        } = var_decl
        {
            if let HirExpr::Call { callee, .. } = init_expr {
                // The callee should be a Member expression: Math.add
                if let HirExpr::Member { object, member, .. } = &**callee {
                    if let HirExpr::Ident { name, .. } = &**object {
                        assert_eq!(name, "Math", "import should resolve to Math module");
                        assert_eq!(member, "add", "function name should be add");
                    } else {
                        panic!("Expected Ident for object, got {:?}", object);
                    }
                } else {
                    panic!(
                        "Expected Member expr for callee after import resolution, got {:?}",
                        callee
                    );
                }
            } else {
                panic!("Expected Call expr for init, got {:?}", init_expr);
            }
        } else {
            panic!("Expected VarDecl with init, got {:?}", var_decl);
        }
    }

    #[test]
    fn test_provider_decorator_lowers_to_hir_provider() {
        // Test that @provider decorated class is lowered to HirProvider.
        let source = r#"
@provider
class State {
    count: number = 0;
    readonly name: string = "default";
}
"#;
        let hir = lower_ts_str_to_hir(source, "state.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        // Should have exactly one declaration: HirProvider
        assert_eq!(hir.decls.len(), 1, "should have exactly one decl");

        let provider = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Provider(p) = d {
                    Some(p)
                } else {
                    None
                }
            })
            .expect("should have a provider decl");

        assert_eq!(provider.name, "State");
        assert_eq!(provider.fields.len(), 2, "should have 2 fields");

        // Check count field
        let count_field = provider
            .fields
            .iter()
            .find(|f| f.name == "count")
            .expect("should have count field");
        assert!(!count_field.is_final, "count should not be final");

        // Check name field (readonly → final)
        let name_field = provider
            .fields
            .iter()
            .find(|f| f.name == "name")
            .expect("should have name field");
        assert!(name_field.is_final, "name should be final (readonly)");
    }

    #[test]
    fn test_data_decorator_lowers_to_hir_struct_only() {
        // Test that @data decorated class is lowered to HirStruct only (no module).
        let source = r#"
@data
class User {
    id: number;
    name: string;
}
"#;
        let hir = lower_ts_str_to_hir(source, "user.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        // Should have exactly one declaration: HirStruct (no HirModule)
        assert_eq!(hir.decls.len(), 1, "should have exactly one decl");

        let struct_decl = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Struct(s) = d {
                    Some(s)
                } else {
                    None
                }
            })
            .expect("should have a struct decl");

        assert_eq!(struct_decl.name, "User");
        assert_eq!(struct_decl.fields.len(), 2, "should have 2 fields");

        // Ensure no module was created
        let has_module = hir.decls.iter().any(|d| matches!(d, HirDecl::Module(_)));
        assert!(!has_module, "should not have a module for @data class");
    }

    #[test]
    fn test_regular_class_lowers_to_module_only() {
        // Test that a regular class (no decorator) is lowered to HirModule only (no struct).
        // Regular classes are pure behavior containers.
        let source = r#"
export default class Controller {
    getUser(user: User): User {
        return user;
    }

    validateEmail(email: string): boolean {
        return email.includes("@");
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "controller.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        // Should have HirModule only (no struct for regular classes)
        let has_struct = hir.decls.iter().any(|d| matches!(d, HirDecl::Struct(_)));
        let has_module = hir.decls.iter().any(|d| matches!(d, HirDecl::Module(_)));

        assert!(!has_struct, "regular class should NOT have a struct decl");
        assert!(has_module, "regular class should have a module decl");

        // Verify module properties
        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        assert_eq!(module.name, "Controller");
        assert!(module.is_exported, "module should be exported");
        assert_eq!(module.funcs.len(), 2, "should have 2 methods");
    }

    #[test]
    fn test_regular_class_with_fields_errors() {
        // Test that regular classes with fields produce an error.
        let source = r#"
export default class Controller {
    state = { count: 0 };

    increment() {
        this.state.count = this.state.count + 1;
    }
}
"#;
        let result = lower_ts_str_to_hir(source, "controller.ts", TsLoweringOptions::default());
        assert!(result.is_err(), "should error on class with fields");

        let err = result.unwrap_err();
        let err_msg = format!("{}", err);
        assert!(
            err_msg.contains("field") && err_msg.contains("state"),
            "error should mention the field name"
        );
    }

    #[test]
    fn test_regular_class_with_implements() {
        // Test that a regular class with implements clause is properly lowered.
        let source = r#"
interface Greeter {
    greet(name: string): string;
}

export class FriendlyGreeter implements Greeter {
    greet(name: string): string {
        return "Hello, " + name + "!";
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "greeter.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        // Should have HirInterface and HirModule
        let has_interface = hir.decls.iter().any(|d| matches!(d, HirDecl::Interface(_)));
        let has_module = hir.decls.iter().any(|d| matches!(d, HirDecl::Module(_)));

        assert!(has_interface, "should have an interface decl");
        assert!(has_module, "should have a module decl");

        // Verify module implements the interface
        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        assert_eq!(module.name, "FriendlyGreeter");
        assert!(module.is_exported, "module should be exported");
        assert_eq!(module.implements.len(), 1, "should implement 1 interface");
        assert_eq!(module.implements[0], vec!["Greeter"]);
    }

    #[test]
    fn test_regular_class_with_this_keyword_errors() {
        // Test that regular classes using `this` keyword produce an error.
        let source = r#"
export class Counter {
    increment(): void {
        this.count = 1;
    }
}
"#;
        let result = lower_ts_str_to_hir(source, "counter.ts", TsLoweringOptions::default());
        assert!(result.is_err(), "should error on class using 'this'");

        let err = result.unwrap_err();
        let err_msg = format!("{}", err);
        assert!(
            err_msg.contains("this") && err_msg.contains("increment"),
            "error should mention 'this' and the method name: {}",
            err_msg
        );
    }

    #[test]
    fn test_regular_class_without_this_passes() {
        // Test that regular classes NOT using `this` pass validation.
        let source = r#"
export class Utils {
    add(a: number, b: number): number {
        return a + b;
    }

    greet(name: string): string {
        return "Hello, " + name;
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "utils.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        assert_eq!(module.name, "Utils");
        assert_eq!(module.funcs.len(), 2, "should have 2 methods");
    }

    #[test]
    fn test_provider_decorator_call_form() {
        // Test that @provider() (call form) also works.
        let source = r#"
@provider()
class AppState {
    counter: number = 0;
}
"#;
        let hir = lower_ts_str_to_hir(source, "app.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let provider = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Provider(p) = d {
                    Some(p)
                } else {
                    None
                }
            })
            .expect("should have a provider decl");

        assert_eq!(provider.name, "AppState");
    }

    #[test]
    fn test_type_alias_lowers_to_struct() {
        // Test that a type alias with object literal lowers to HirStruct.
        let source = r#"
type User = {
    name: string;
    email: string;
};
"#;
        let hir = lower_ts_str_to_hir(source, "user.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        // Should have exactly one declaration: HirStruct
        assert_eq!(hir.decls.len(), 1, "should have exactly one decl");

        let struct_decl = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Struct(s) = d {
                    Some(s)
                } else {
                    None
                }
            })
            .expect("should have a struct decl");

        assert_eq!(struct_decl.name, "User");
        assert_eq!(struct_decl.fields.len(), 2, "should have 2 fields");

        // Check field names
        let name_field = struct_decl
            .fields
            .iter()
            .find(|f| f.name == "name")
            .expect("should have name field");
        assert!(!name_field.is_final);

        let email_field = struct_decl
            .fields
            .iter()
            .find(|f| f.name == "email")
            .expect("should have email field");
        assert!(!email_field.is_final);
    }

    #[test]
    fn test_type_alias_with_readonly_fields() {
        // Test that readonly fields in type alias become final fields.
        let source = r#"
type Config = {
    readonly apiUrl: string;
    readonly timeout: number;
    mutable: boolean;
};
"#;
        let hir = lower_ts_str_to_hir(source, "config.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let struct_decl = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Struct(s) = d {
                    Some(s)
                } else {
                    None
                }
            })
            .expect("should have a struct decl");

        assert_eq!(struct_decl.name, "Config");
        assert_eq!(struct_decl.fields.len(), 3, "should have 3 fields");

        // Check readonly → final
        let api_url_field = struct_decl
            .fields
            .iter()
            .find(|f| f.name == "apiUrl")
            .expect("should have apiUrl field");
        assert!(api_url_field.is_final, "apiUrl should be final (readonly)");

        let timeout_field = struct_decl
            .fields
            .iter()
            .find(|f| f.name == "timeout")
            .expect("should have timeout field");
        assert!(timeout_field.is_final, "timeout should be final (readonly)");

        let mutable_field = struct_decl
            .fields
            .iter()
            .find(|f| f.name == "mutable")
            .expect("should have mutable field");
        assert!(!mutable_field.is_final, "mutable should not be final");
    }

    #[test]
    fn test_type_alias_with_array_and_optional() {
        // Test that T[] → List<T> and T | null → Optional<T>.
        let source = r#"
type Order = {
    items: string[];
    user: User | null;
};
"#;
        let hir = lower_ts_str_to_hir(source, "order.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let struct_decl = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Struct(s) = d {
                    Some(s)
                } else {
                    None
                }
            })
            .expect("should have a struct decl");

        assert_eq!(struct_decl.name, "Order");
        assert_eq!(struct_decl.fields.len(), 2, "should have 2 fields");

        // Check items field type is List<String>
        let items_field = struct_decl
            .fields
            .iter()
            .find(|f| f.name == "items")
            .expect("should have items field");
        if let HirType::Name { path } = &items_field.ty {
            assert_eq!(path, &vec!["List<String>".to_string()]);
        } else {
            panic!("items field should have Name type");
        }

        // Check user field type is Optional<User>
        let user_field = struct_decl
            .fields
            .iter()
            .find(|f| f.name == "user")
            .expect("should have user field");
        if let HirType::Name { path } = &user_field.ty {
            assert_eq!(path, &vec!["Optional<User>".to_string()]);
        } else {
            panic!("user field should have Name type");
        }
    }

    #[test]
    fn test_type_mapping_map_and_set() {
        // Test that Map<K, V> → Map<K, V> and Set<T> → Set<T>.
        let source = r#"
type Cache = {
    entries: Map<string, number>;
    tags: Set<string>;
};
"#;
        let hir = lower_ts_str_to_hir(source, "cache.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let struct_decl = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Struct(s) = d {
                    Some(s)
                } else {
                    None
                }
            })
            .expect("should have a struct decl");

        assert_eq!(struct_decl.name, "Cache");

        // Check entries field type is Map<String, Int>
        let entries_field = struct_decl
            .fields
            .iter()
            .find(|f| f.name == "entries")
            .expect("should have entries field");
        if let HirType::Name { path } = &entries_field.ty {
            assert_eq!(path, &vec!["Map<String, Int>".to_string()]);
        } else {
            panic!("entries field should have Name type");
        }

        // Check tags field type is Set<String>
        let tags_field = struct_decl
            .fields
            .iter()
            .find(|f| f.name == "tags")
            .expect("should have tags field");
        if let HirType::Name { path } = &tags_field.ty {
            assert_eq!(path, &vec!["Set<String>".to_string()]);
        } else {
            panic!("tags field should have Name type");
        }
    }

    #[test]
    fn test_type_mapping_promise() {
        // Test that Promise<T> → Task<T>.
        let source = r#"
type AsyncResult = {
    pending: Promise<string>;
};
"#;
        let hir = lower_ts_str_to_hir(source, "async.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let struct_decl = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Struct(s) = d {
                    Some(s)
                } else {
                    None
                }
            })
            .expect("should have a struct decl");

        // Check pending field type is Task<String>
        let pending_field = struct_decl
            .fields
            .iter()
            .find(|f| f.name == "pending")
            .expect("should have pending field");
        if let HirType::Name { path } = &pending_field.ty {
            assert_eq!(path, &vec!["Task<String>".to_string()]);
        } else {
            panic!("pending field should have Name type");
        }
    }

    #[test]
    fn test_type_mapping_nested_generics() {
        // Test nested generic types like Map<string, string[]>.
        let source = r#"
type MultiIndex = {
    index: Map<string, string[]>;
};
"#;
        let hir = lower_ts_str_to_hir(source, "index.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let struct_decl = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Struct(s) = d {
                    Some(s)
                } else {
                    None
                }
            })
            .expect("should have a struct decl");

        // Check index field type is Map<String, List<String>>
        let index_field = struct_decl
            .fields
            .iter()
            .find(|f| f.name == "index")
            .expect("should have index field");
        if let HirType::Name { path } = &index_field.ty {
            assert_eq!(path, &vec!["Map<String, List<String>>".to_string()]);
        } else {
            panic!("index field should have Name type");
        }
    }

    #[test]
    fn test_type_mapping_array_generic_form() {
        // Test that Array<T> → List<T> (generic form of T[]).
        let source = r#"
type Container = {
    items: Array<number>;
};
"#;
        let hir = lower_ts_str_to_hir(source, "container.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let struct_decl = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Struct(s) = d {
                    Some(s)
                } else {
                    None
                }
            })
            .expect("should have a struct decl");

        // Check items field type is List<Int>
        let items_field = struct_decl
            .fields
            .iter()
            .find(|f| f.name == "items")
            .expect("should have items field");
        if let HirType::Name { path } = &items_field.ty {
            assert_eq!(path, &vec!["List<Int>".to_string()]);
        } else {
            panic!("items field should have Name type");
        }
    }

    #[test]
    fn test_exported_type_alias_lowers_to_struct() {
        // Test that exported type alias lowers correctly.
        let source = r#"
export type Product = {
    id: string;
    price: number;
};
"#;
        let hir = lower_ts_str_to_hir(source, "product.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let struct_decl = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Struct(s) = d {
                    Some(s)
                } else {
                    None
                }
            })
            .expect("should have a struct decl");

        assert_eq!(struct_decl.name, "Product");
        assert_eq!(struct_decl.fields.len(), 2, "should have 2 fields");
    }

    #[test]
    fn test_non_object_type_alias_is_skipped() {
        // Test that non-object type aliases (primitives, unions) are skipped.
        let source = r#"
type UserId = string;
type Count = number;
"#;
        let hir = lower_ts_str_to_hir(source, "aliases.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        // Should have no declarations (primitive type aliases are skipped)
        assert_eq!(
            hir.decls.len(),
            0,
            "should have no decls for primitive aliases"
        );
    }

    #[test]
    fn test_interface_with_methods_lowers_to_hir_interface() {
        // Test that an interface with methods lowers to HirInterface.
        let source = r#"
interface Serializable {
    serialize(): string;
    deserialize(data: string): void;
}
"#;
        let hir = lower_ts_str_to_hir(source, "serializable.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        // Should have exactly one declaration: HirInterface
        assert_eq!(hir.decls.len(), 1, "should have exactly one decl");

        let interface = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Interface(i) = d {
                    Some(i)
                } else {
                    None
                }
            })
            .expect("should have an interface decl");

        assert_eq!(interface.name, "Serializable");
        assert_eq!(interface.methods.len(), 2, "should have 2 methods");

        // Check serialize method
        let serialize = interface
            .methods
            .iter()
            .find(|m| m.sig.name == "serialize")
            .expect("should have serialize method");
        assert_eq!(serialize.sig.params.len(), 0);
        assert!(serialize.sig.ret.is_some());

        // Check deserialize method
        let deserialize = interface
            .methods
            .iter()
            .find(|m| m.sig.name == "deserialize")
            .expect("should have deserialize method");
        assert_eq!(deserialize.sig.params.len(), 1);
        assert_eq!(deserialize.sig.params[0].name, "data");
    }

    #[test]
    fn test_interface_with_generics() {
        // Test that an interface with generics is handled.
        let source = r#"
interface Repository<T> {
    findById(id: string): T;
    save(entity: T): void;
}
"#;
        let hir = lower_ts_str_to_hir(source, "repository.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let interface = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Interface(i) = d {
                    Some(i)
                } else {
                    None
                }
            })
            .expect("should have an interface decl");

        assert_eq!(interface.name, "Repository");
        assert_eq!(interface.generics.len(), 1, "should have 1 generic param");
        assert_eq!(interface.generics[0].name, "T");
        assert_eq!(interface.methods.len(), 2, "should have 2 methods");
    }

    #[test]
    fn test_interface_with_extends() {
        // Test that an interface with extends clause is handled.
        let source = r#"
interface BaseLogger {
    log(msg: string): void;
}

interface AdvancedLogger extends BaseLogger {
    error(msg: string): void;
    warn(msg: string): void;
}
"#;
        let hir = lower_ts_str_to_hir(source, "logger.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        // Should have two interfaces
        let interfaces: Vec<_> = hir
            .decls
            .iter()
            .filter_map(|d| {
                if let HirDecl::Interface(i) = d {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(interfaces.len(), 2, "should have 2 interfaces");

        let advanced = interfaces
            .iter()
            .find(|i| i.name == "AdvancedLogger")
            .expect("should have AdvancedLogger");

        assert_eq!(advanced.extends.len(), 1, "should extend 1 interface");
        assert_eq!(advanced.extends[0], vec!["BaseLogger".to_string()]);
    }

    #[test]
    fn test_interface_with_only_properties_lowers_to_struct() {
        // Test that an interface with only properties (no methods) lowers to struct.
        // This provides user convenience - they can use either `type` or `interface`.
        let source = r#"
interface User {
    name: string;
    email: string;
}
"#;
        let hir = lower_ts_str_to_hir(source, "user.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        // Should have one struct declaration
        let struct_decl = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Struct(s) = d {
                    Some(s)
                } else {
                    None
                }
            })
            .expect("property-only interface should lower to struct");

        assert_eq!(struct_decl.name, "User");
        assert_eq!(struct_decl.fields.len(), 2, "should have 2 fields");

        // Verify no interface was created
        let has_interface = hir.decls.iter().any(|d| matches!(d, HirDecl::Interface(_)));
        assert!(!has_interface, "should not have an interface decl");
    }

    #[test]
    fn test_exported_interface_lowers_correctly() {
        // Test that exported interface lowers correctly.
        let source = r#"
export interface Comparable {
    compareTo(other: Comparable): number;
}
"#;
        let hir = lower_ts_str_to_hir(source, "comparable.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let interface = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Interface(i) = d {
                    Some(i)
                } else {
                    None
                }
            })
            .expect("should have an interface decl");

        assert_eq!(interface.name, "Comparable");
        assert_eq!(interface.methods.len(), 1);
    }

    // =========================================================================
    // Phase 3: Function Transformation Tests
    // =========================================================================

    #[test]
    fn test_function_optional_parameter() {
        // Test that optional param?: Type → Optional<Type>
        let source = r#"
export function greet(name?: string): string {
    return "Hello";
}
"#;
        let hir = lower_ts_str_to_hir(source, "greet.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let func = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    m.funcs.iter().find(|f| f.sig.name == "greet")
                } else {
                    None
                }
            })
            .expect("should have a greet function");

        assert_eq!(func.sig.params.len(), 1);
        let param = &func.sig.params[0];
        assert_eq!(param.name, "name");
        if let HirType::Name { path } = &param.ty {
            assert_eq!(path, &vec!["Optional<String>".to_string()]);
        } else {
            panic!("param type should be Name");
        }
    }

    #[test]
    fn test_function_default_parameter() {
        // Test that param: Type = value → Optional<Type>
        let source = r#"
export function greet(name: string = "World"): string {
    return "Hello " + name;
}
"#;
        let hir = lower_ts_str_to_hir(source, "greet.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let func = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    m.funcs.iter().find(|f| f.sig.name == "greet")
                } else {
                    None
                }
            })
            .expect("should have a greet function");

        assert_eq!(func.sig.params.len(), 1);
        let param = &func.sig.params[0];
        assert_eq!(param.name, "name");
        if let HirType::Name { path } = &param.ty {
            assert_eq!(path, &vec!["Optional<String>".to_string()]);
        } else {
            panic!("param type should be Name");
        }
    }

    #[test]
    fn test_function_rest_parameter() {
        // Test that ...args: Type[] → List<Type>
        let source = r#"
export function sum(...numbers: number[]): number {
    return 0;
}
"#;
        let hir = lower_ts_str_to_hir(source, "sum.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let func = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    m.funcs.iter().find(|f| f.sig.name == "sum")
                } else {
                    None
                }
            })
            .expect("should have a sum function");

        assert_eq!(func.sig.params.len(), 1);
        let param = &func.sig.params[0];
        assert_eq!(param.name, "numbers");
        if let HirType::Name { path } = &param.ty {
            assert_eq!(path, &vec!["List<Int>".to_string()]);
        } else {
            panic!("param type should be Name");
        }
    }

    #[test]
    fn test_function_generic_type_param() {
        // Test that generic functions have type parameters
        let source = r#"
export function identity<T>(value: T): T {
    return value;
}
"#;
        let hir = lower_ts_str_to_hir(source, "identity.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let func = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    m.funcs.iter().find(|f| f.sig.name == "identity")
                } else {
                    None
                }
            })
            .expect("should have an identity function");

        assert_eq!(func.sig.generics.len(), 1);
        assert_eq!(func.sig.generics[0].name, "T");
        assert!(func.sig.generics[0].bound.is_none());
    }

    #[test]
    fn test_function_generic_with_constraint() {
        // Test that generic constraints are preserved
        let source = r#"
export function process<T extends Comparable>(value: T): T {
    return value;
}
"#;
        let hir = lower_ts_str_to_hir(source, "process.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let func = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    m.funcs.iter().find(|f| f.sig.name == "process")
                } else {
                    None
                }
            })
            .expect("should have a process function");

        assert_eq!(func.sig.generics.len(), 1);
        assert_eq!(func.sig.generics[0].name, "T");
        assert!(func.sig.generics[0].bound.is_some());
        if let Some(HirType::Name { path }) = &func.sig.generics[0].bound {
            assert_eq!(path, &vec!["Comparable".to_string()]);
        } else {
            panic!("bound should be Comparable");
        }
    }

    #[test]
    fn test_method_optional_parameter() {
        // Test that class methods handle optional params correctly
        let source = r#"
export default class UserService {
    findUser(id: string, includeDeleted?: boolean): string {
        return id;
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "user.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let func = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    m.funcs.iter().find(|f| f.sig.name == "findUser")
                } else {
                    None
                }
            })
            .expect("should have a findUser method");

        assert_eq!(func.sig.params.len(), 2);

        let id_param = &func.sig.params[0];
        assert_eq!(id_param.name, "id");
        if let HirType::Name { path } = &id_param.ty {
            assert_eq!(path, &vec!["String".to_string()]);
        }

        let include_deleted_param = &func.sig.params[1];
        assert_eq!(include_deleted_param.name, "includeDeleted");
        if let HirType::Name { path } = &include_deleted_param.ty {
            assert_eq!(path, &vec!["Optional<Bool>".to_string()]);
        }
    }

    #[test]
    fn test_method_generic_type_param() {
        // Test that class methods handle generic type parameters
        let source = r#"
export default class Container {
    wrap<T>(value: T): T {
        return value;
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "container.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let func = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    m.funcs.iter().find(|f| f.sig.name == "wrap")
                } else {
                    None
                }
            })
            .expect("should have a wrap method");

        assert_eq!(func.sig.generics.len(), 1);
        assert_eq!(func.sig.generics[0].name, "T");
    }

    #[test]
    fn test_async_function_return_type() {
        // Test that async functions are marked as async
        let source = r#"
export async function fetchData(url: string): Promise<string> {
    return "";
}
"#;
        let hir = lower_ts_str_to_hir(source, "fetch.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let func = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    m.funcs.iter().find(|f| f.sig.name == "fetchData")
                } else {
                    None
                }
            })
            .expect("should have a fetchData function");

        assert!(func.sig.is_async, "function should be marked as async");

        // Return type should be Task<String>
        if let Some(HirType::Name { path }) = &func.sig.ret {
            assert_eq!(path, &vec!["Task<String>".to_string()]);
        } else {
            panic!("return type should be Task<String>");
        }
    }

    #[test]
    fn test_mixed_parameter_types() {
        // Test function with regular, optional, and default params
        let source = r#"
export function formatMessage(
    template: string,
    name?: string,
    count: number = 1
): string {
    return template;
}
"#;
        let hir = lower_ts_str_to_hir(source, "format.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let func = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    m.funcs.iter().find(|f| f.sig.name == "formatMessage")
                } else {
                    None
                }
            })
            .expect("should have a formatMessage function");

        assert_eq!(func.sig.params.len(), 3);

        // Regular param: String
        let template_param = &func.sig.params[0];
        assert_eq!(template_param.name, "template");
        if let HirType::Name { path } = &template_param.ty {
            assert_eq!(path, &vec!["String".to_string()]);
        }

        // Optional param: Optional<String>
        let name_param = &func.sig.params[1];
        assert_eq!(name_param.name, "name");
        if let HirType::Name { path } = &name_param.ty {
            assert_eq!(path, &vec!["Optional<String>".to_string()]);
        }

        // Default param: Optional<Int>
        let count_param = &func.sig.params[2];
        assert_eq!(count_param.name, "count");
        if let HirType::Name { path } = &count_param.ty {
            assert_eq!(path, &vec!["Optional<Int>".to_string()]);
        }
    }

    #[test]
    fn test_constructor_with_provider_init_lowered() {
        // Test that a constructor initializing a known provider is lowered to constructor function.
        let source = r#"
@provider
class State {
    count: number;
}

export default class Counter {
    constructor() {
        const state: State = { count: 0 };
    }

    increment(): void {
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "counter.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        // Find the module
        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        assert_eq!(module.name, "Counter");

        // Should have constructor and increment functions
        assert_eq!(module.funcs.len(), 2, "should have 2 functions");

        let constructor = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "constructor")
            .expect("should have constructor function");

        // Constructor should return void
        if let Some(HirType::Name { path }) = &constructor.sig.ret {
            assert_eq!(path, &vec!["void".to_string()]);
        } else {
            panic!("constructor should have void return type");
        }

        // Constructor should have a body
        assert!(constructor.body.is_some(), "constructor should have a body");
    }

    #[test]
    fn test_constructor_without_provider_init_ignored() {
        // Test that a constructor without provider initialization is ignored.
        let source = r#"
export default class Greeter {
    constructor() {
        console.log("Hello");
    }

    greet(): string {
        return "Hello";
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "greeter.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        // Find the module
        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        // Should only have the greet function (constructor is ignored)
        assert_eq!(
            module.funcs.len(),
            1,
            "should have only 1 function (constructor ignored)"
        );
        assert_eq!(module.funcs[0].sig.name, "greet");
    }

    #[test]
    fn test_constructor_with_non_provider_type_ignored() {
        // Test that constructor with non-provider type initialization is ignored.
        let source = r#"
@data
class User {
    name: string;
}

export default class Service {
    constructor() {
        const user: User = { name: "test" };
    }

    process(): void {
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "service.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        // Find the module
        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        // User is @data not @provider, so constructor should be ignored
        assert_eq!(
            module.funcs.len(),
            1,
            "should have only 1 function (constructor ignored)"
        );
        assert_eq!(module.funcs[0].sig.name, "process");
    }

    #[test]
    fn test_provider_context_tracking() {
        // Test that ProviderContext correctly tracks provider names.
        let mut ctx = ProviderContext::new();
        ctx.add_provider("State".to_string());
        ctx.add_provider("AppState".to_string());

        assert!(ctx.is_provider("State"));
        assert!(ctx.is_provider("AppState"));
        assert!(!ctx.is_provider("User")); // Not a provider
        assert!(!ctx.is_provider("NonExistent"));
    }

    #[test]
    fn test_constructor_params_lowered() {
        // Test that constructor with parameters has them lowered.
        let source = r#"
@provider
class Config {
    value: string;
}

export default class Configurable {
    constructor(name: string) {
        const config: Config = { value: name };
    }

    getValue(): string {
        return "";
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "config.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        let constructor = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "constructor")
            .expect("should have constructor function");

        // Constructor should have 1 parameter
        assert_eq!(constructor.sig.params.len(), 1);
        assert_eq!(constructor.sig.params[0].name, "name");
        if let HirType::Name { path } = &constructor.sig.params[0].ty {
            assert_eq!(path, &vec!["String".to_string()]);
        }
    }

    #[test]
    fn test_provider_param_renamed_to_self() {
        // Test that first parameter of provider type is renamed to "self".
        // Pattern: `increment(state: State)` → `increment(State self)`
        let source = r#"
@provider
class State {
    count: number;
}

export default class Counter {
    constructor() {
        const state: State = { count: 0 };
    }

    increment(state: State): void {
    }

    decrement(state: State): void {
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "counter.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        // Check increment method
        let increment = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "increment")
            .expect("should have increment method");

        assert_eq!(increment.sig.params.len(), 1);
        assert_eq!(
            increment.sig.params[0].name, "self",
            "provider param should be renamed to 'self'"
        );
        if let HirType::Name { path } = &increment.sig.params[0].ty {
            assert_eq!(path, &vec!["State".to_string()]);
        }

        // Check decrement method
        let decrement = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "decrement")
            .expect("should have decrement method");

        assert_eq!(decrement.sig.params.len(), 1);
        assert_eq!(
            decrement.sig.params[0].name, "self",
            "provider param should be renamed to 'self'"
        );
    }

    #[test]
    fn test_provider_param_with_additional_params() {
        // Test that provider param is renamed but other params are preserved.
        // Pattern: `setUser(state: State, user: User)` → `setUser(State self, User user)`
        let source = r#"
@provider
class State {
    user: string;
}

export default class Controller {
    constructor() {
        const state: State = { user: "" };
    }

    setUser(state: State, user: string): void {
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "controller.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        let set_user = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "setUser")
            .expect("should have setUser method");

        // Should have 2 params: self (State) and user (String)
        assert_eq!(set_user.sig.params.len(), 2);

        // First param should be renamed to "self"
        assert_eq!(set_user.sig.params[0].name, "self");
        if let HirType::Name { path } = &set_user.sig.params[0].ty {
            assert_eq!(path, &vec!["State".to_string()]);
        }

        // Second param should keep its name
        assert_eq!(set_user.sig.params[1].name, "user");
        if let HirType::Name { path } = &set_user.sig.params[1].ty {
            assert_eq!(path, &vec!["String".to_string()]);
        }
    }

    #[test]
    fn test_non_provider_first_param_not_renamed() {
        // Test that non-provider first parameter is NOT renamed.
        let source = r#"
@data
class User {
    name: string;
}

export default class Service {
    process(user: User): void {
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "service.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        let process = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "process")
            .expect("should have process method");

        // User is @data not @provider, so param should keep its name
        assert_eq!(process.sig.params.len(), 1);
        assert_eq!(
            process.sig.params[0].name, "user",
            "non-provider param should keep its name"
        );
    }

    #[test]
    fn test_method_without_params_unaffected() {
        // Test that methods without parameters are unaffected.
        let source = r#"
@provider
class State {
    count: number;
}

export default class Counter {
    constructor() {
        const state: State = { count: 0 };
    }

    reset(): void {
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "counter.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        let reset = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "reset")
            .expect("should have reset method");

        // No params
        assert_eq!(reset.sig.params.len(), 0);
    }

    // ============================================
    // Phase 4: Expression & Statement Mapping Tests
    // ============================================

    #[test]
    fn test_null_literal_to_optional_none() {
        // Test that `null` is mapped to `Optional.none()`
        let source = r#"
export function getUser(): string {
    const x = null;
    return "";
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        let func = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "getUser")
            .expect("should have getUser function");

        let body = func.body.as_ref().expect("should have body");
        let first_stmt = body.stmts.first().expect("should have statements");

        // Check that the init expression is a Call to Optional.none
        if let HirStmt::VarDecl {
            init: Some(expr), ..
        } = first_stmt
            && let HirExpr::Call { callee, args, .. } = expr
            && let HirExpr::Member { object, member, .. } = callee.as_ref()
            && let HirExpr::Ident { name, .. } = object.as_ref()
        {
            assert_eq!(name, "Optional");
            assert_eq!(member, "none");
            assert!(args.is_empty());
            return; // Test passed
        }
        panic!("null should be lowered to Optional.none() call");
    }

    #[test]
    fn test_console_log_to_log_info() {
        // Test that `console.log(x)` is mapped to `Log.info(x)`
        let source = r#"
export function logMessage(msg: string): void {
    console.log(msg);
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        let func = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "logMessage")
            .expect("should have logMessage function");

        let body = func.body.as_ref().expect("should have body");
        let first_stmt = body.stmts.first().expect("should have statements");

        // Check that the expression is a Call to Log.info
        if let HirStmt::Expr { expr, .. } = first_stmt
            && let HirExpr::Call { callee, args, .. } = expr
            && let HirExpr::Member { object, member, .. } = callee.as_ref()
            && let HirExpr::Ident { name, .. } = object.as_ref()
        {
            assert_eq!(name, "Log");
            assert_eq!(member, "info");
            assert_eq!(args.len(), 1);
            return; // Test passed
        }
        panic!("console.log should be lowered to Log.info call");
    }

    #[test]
    fn test_console_methods_mapping() {
        // Test various console methods are mapped correctly
        let source = r#"
export function testLogs(): void {
    console.info("info");
    console.warn("warning");
    console.error("error");
    console.debug("debug");
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        let func = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "testLogs")
            .expect("should have testLogs function");

        let body = func.body.as_ref().expect("should have body");

        // Check all 4 log calls
        let expected_methods = ["info", "warn", "error", "debug"];
        for (i, expected_method) in expected_methods.iter().enumerate() {
            let stmt = &body.stmts[i];
            if let HirStmt::Expr { expr, .. } = stmt
                && let HirExpr::Call { callee, .. } = expr
                && let HirExpr::Member { object, member, .. } = callee.as_ref()
                && let HirExpr::Ident { name, .. } = object.as_ref()
            {
                assert_eq!(name, "Log");
                assert_eq!(member, *expected_method);
                continue;
            }
            panic!(
                "console.{} should be lowered to Log.{}",
                expected_method, expected_method
            );
        }
    }

    #[test]
    fn test_template_literal_simple() {
        // Test that template literals are converted to string concatenation
        let source = r#"
export function greet(name: string): string {
    return `Hello ${name}!`;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        let func = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "greet")
            .expect("should have greet function");

        let body = func.body.as_ref().expect("should have body");
        let first_stmt = body.stmts.first().expect("should have statements");

        // Should be Return { expr: Binary { left: Binary { "Hello ", name }, "+", "!" } }
        if let HirStmt::Return {
            expr: Some(expr), ..
        } = first_stmt
            && let HirExpr::Binary { op, .. } = expr
        {
            assert!(matches!(op, arth::compiler::hir::HirBinOp::Add));
            return; // Test passed - template literal was converted to concatenation
        }
        panic!("template literal should be lowered to string concatenation");
    }

    #[test]
    fn test_template_literal_empty() {
        // Test that empty template literal returns empty string
        let source = r#"
export function empty(): string {
    return ``;
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        let func = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "empty")
            .expect("should have empty function");

        let body = func.body.as_ref().expect("should have body");
        let first_stmt = body.stmts.first().expect("should have statements");

        // Should be Return { expr: Str { value: "" } }
        if let HirStmt::Return {
            expr: Some(HirExpr::Str { value, .. }),
            ..
        } = first_stmt
        {
            assert_eq!(value, "");
            return; // Test passed
        }
        panic!("empty template literal should be lowered to empty string");
    }

    #[test]
    fn test_date_now_mapping() {
        // Test that `Date.now()` is mapped to `DateTime.nowMillis()`
        let source = r#"
export function getTime(): number {
    return Date.now();
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        let func = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "getTime")
            .expect("should have getTime function");

        let body = func.body.as_ref().expect("should have body");
        let first_stmt = body.stmts.first().expect("should have statements");

        if let HirStmt::Return {
            expr: Some(expr), ..
        } = first_stmt
            && let HirExpr::Call { callee, .. } = expr
            && let HirExpr::Member { object, member, .. } = callee.as_ref()
            && let HirExpr::Ident { name, .. } = object.as_ref()
        {
            assert_eq!(name, "DateTime");
            assert_eq!(member, "nowMillis");
            return; // Test passed
        }
        panic!("Date.now() should be lowered to DateTime.nowMillis()");
    }

    #[test]
    fn test_json_methods_mapping() {
        // Test that JSON.stringify/parse are mapped correctly
        let source = r#"
export function encodeData(data: string): string {
    return JSON.stringify(data);
}

export function decodeData(str: string): string {
    return JSON.parse(str);
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        // Check JSON.stringify → Json.stringify
        let encode_func = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "encodeData")
            .expect("should have encodeData function");
        let body = encode_func.body.as_ref().expect("should have body");
        if let HirStmt::Return {
            expr: Some(HirExpr::Call { callee, .. }),
            ..
        } = body.stmts.first().unwrap()
            && let HirExpr::Member { object, member, .. } = callee.as_ref()
            && let HirExpr::Ident { name, .. } = object.as_ref()
        {
            assert_eq!(name, "Json");
            assert_eq!(member, "stringify");
        }

        // Check JSON.parse → Json.parse
        let decode_func = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "decodeData")
            .expect("should have decodeData function");
        let body = decode_func.body.as_ref().expect("should have body");
        if let HirStmt::Return {
            expr: Some(HirExpr::Call { callee, .. }),
            ..
        } = body.stmts.first().unwrap()
            && let HirExpr::Member { object, member, .. } = callee.as_ref()
            && let HirExpr::Ident { name, .. } = object.as_ref()
        {
            assert_eq!(name, "Json");
            assert_eq!(member, "parse");
        }
    }

    #[test]
    fn test_math_methods_pass_through() {
        // Test that Math.* methods pass through unchanged
        let source = r#"
export function mathOps(): number {
    return Math.abs(-5);
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let module = hir
            .decls
            .iter()
            .find_map(|d| {
                if let HirDecl::Module(m) = d {
                    Some(m)
                } else {
                    None
                }
            })
            .expect("should have a module");

        let func = module
            .funcs
            .iter()
            .find(|f| f.sig.name == "mathOps")
            .expect("should have mathOps function");

        let body = func.body.as_ref().expect("should have body");
        if let HirStmt::Return {
            expr: Some(HirExpr::Call { callee, .. }),
            ..
        } = body.stmts.first().unwrap()
            && let HirExpr::Member { object, member, .. } = callee.as_ref()
            && let HirExpr::Ident { name, .. } = object.as_ref()
        {
            assert_eq!(name, "Math");
            assert_eq!(member, "abs");
            return; // Test passed
        }
        panic!("Math.abs should pass through as Math.abs");
    }
}
