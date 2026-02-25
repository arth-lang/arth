use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::compiler::ast::{Decl, FileAst, FuncDecl, ModuleDecl, NamePath, Visibility};
use crate::compiler::diagnostics::{Diagnostic, Reporter};
use crate::compiler::source::SourceFile;
use crate::compiler::stdlib::{StdlibIndex, StdlibSymbolKind};

mod registry;
pub use registry::{PackageInfo, PackageRegistry};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SymbolKind {
    Module,
    Struct,
    Interface,
    Enum,
    Provider,
    Function,
    TypeAlias,
}

#[derive(Clone, Debug)]
struct SymbolEntry {
    kind: SymbolKind,
    vis: Visibility,
    file: PathBuf,
}

#[derive(Default)]
pub(crate) struct PackageSymbols {
    symbols: HashMap<String, SymbolEntry>,
}

/// External package symbol information for resolver registration
pub struct ExternalPackageSymbol {
    /// Package name (e.g., "demo")
    pub package: String,
    /// Symbol name (e.g., "Math")
    pub name: String,
    /// Whether this is a module or function
    pub is_module: bool,
}

/// Register external package symbols in the packages map
pub(crate) fn register_external_symbols(
    packages: &mut HashMap<String, PackageSymbols>,
    symbols: &[ExternalPackageSymbol],
) {
    for sym in symbols {
        let pkg_syms = packages.entry(sym.package.clone()).or_default();
        let kind = if sym.is_module {
            SymbolKind::Module
        } else {
            SymbolKind::Function
        };
        pkg_syms.symbols.insert(
            sym.name.clone(),
            SymbolEntry {
                kind,
                vis: Visibility::Public, // External symbols are always public
                file: PathBuf::from("<external>"),
            },
        );
    }
}

fn pkg_string(ast: &FileAst) -> Option<String> {
    ast.package.as_ref().map(|p| {
        p.0.iter()
            .map(|id| id.0.as_str())
            .collect::<Vec<_>>()
            .join(".")
    })
}

// Note: parent_dir_segments and check_package_dir_mapping functions
// were moved to registry.rs where they are now implemented in
// validate_package_dir_mapping and extract_dir_segments.

fn record_top_level_symbols(
    packages: &mut HashMap<String, PackageSymbols>,
    sf: &SourceFile,
    ast: &FileAst,
    reporter: &mut Reporter,
) {
    let pkg = match pkg_string(ast) {
        Some(p) => p,
        None => {
            reporter.emit(
                Diagnostic::error("file is missing a package declaration")
                    .with_file(sf.path.clone()),
            );
            return;
        }
    };
    let p = packages.entry(pkg.clone()).or_default();
    for d in &ast.decls {
        match d {
            Decl::Module(m) => {
                let name = m.name.0.clone();
                if let Some(prev) = p.symbols.get(&name) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "duplicate symbol '{}' in package '{}' (module)",
                            name, pkg
                        ))
                        .with_file(sf.path.clone()),
                    );
                    reporter.emit(
                        Diagnostic::error(format!(
                            "note: previously declared here as {:?}",
                            prev.kind
                        ))
                        .with_file(prev.file.clone()),
                    );
                } else {
                    // Use 'export' flag to determine visibility:
                    // export module -> Public (visible from other packages)
                    // module -> Internal (visible only within same top-level package)
                    let vis = if m.is_exported {
                        Visibility::Public
                    } else {
                        Visibility::Internal
                    };
                    p.symbols.insert(
                        name,
                        SymbolEntry {
                            kind: SymbolKind::Module,
                            vis,
                            file: sf.path.clone(),
                        },
                    );
                }
            }
            Decl::Struct(s) => {
                let name = s.name.0.clone();
                if let Some(prev) = p.symbols.get(&name) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "duplicate symbol '{}' in package '{}' (struct)",
                            name, pkg
                        ))
                        .with_file(sf.path.clone()),
                    );
                    reporter.emit(
                        Diagnostic::error("previous declaration").with_file(prev.file.clone()),
                    );
                } else {
                    // No top-level visibility on structs in AST; default to public for now.
                    p.symbols.insert(
                        name,
                        SymbolEntry {
                            kind: SymbolKind::Struct,
                            vis: Visibility::Public,
                            file: sf.path.clone(),
                        },
                    );
                }
            }
            Decl::Interface(i) => {
                let name = i.name.0.clone();
                if let Some(prev) = p.symbols.get(&name) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "duplicate symbol '{}' in package '{}' (interface)",
                            name, pkg
                        ))
                        .with_file(sf.path.clone()),
                    );
                    reporter.emit(
                        Diagnostic::error("previous declaration").with_file(prev.file.clone()),
                    );
                } else {
                    p.symbols.insert(
                        name,
                        SymbolEntry {
                            kind: SymbolKind::Interface,
                            vis: Visibility::Public,
                            file: sf.path.clone(),
                        },
                    );
                }
            }
            Decl::Enum(e) => {
                let name = e.name.0.clone();
                if let Some(prev) = p.symbols.get(&name) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "duplicate symbol '{}' in package '{}' (enum)",
                            name, pkg
                        ))
                        .with_file(sf.path.clone()),
                    );
                    reporter.emit(
                        Diagnostic::error("previous declaration").with_file(prev.file.clone()),
                    );
                } else {
                    p.symbols.insert(
                        name,
                        SymbolEntry {
                            kind: SymbolKind::Enum,
                            vis: Visibility::Public,
                            file: sf.path.clone(),
                        },
                    );
                }
            }
            Decl::TypeAlias(a) => {
                let name = a.name.0.clone();
                if let Some(prev) = p.symbols.get(&name) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "duplicate symbol '{}' in package '{}' (type alias)",
                            name, pkg
                        ))
                        .with_file(sf.path.clone()),
                    );
                    reporter.emit(
                        Diagnostic::error("previous declaration").with_file(prev.file.clone()),
                    );
                } else {
                    p.symbols.insert(
                        name,
                        SymbolEntry {
                            kind: SymbolKind::TypeAlias,
                            vis: Visibility::Public,
                            file: sf.path.clone(),
                        },
                    );
                }
            }
            Decl::Provider(pr) => {
                let name = pr.name.0.clone();
                if let Some(prev) = p.symbols.get(&name) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "duplicate symbol '{}' in package '{}' (provider)",
                            name, pkg
                        ))
                        .with_file(sf.path.clone()),
                    );
                    reporter.emit(
                        Diagnostic::error("previous declaration").with_file(prev.file.clone()),
                    );
                } else {
                    p.symbols.insert(
                        name,
                        SymbolEntry {
                            kind: SymbolKind::Provider,
                            vis: Visibility::Public,
                            file: sf.path.clone(),
                        },
                    );
                }
            }
            Decl::Function(f) => {
                // Reject top-level free functions
                reporter.emit(
                    Diagnostic::error(
                        "top-level functions are not allowed; functions must be declared inside modules"
                    )
                    .with_file(sf.path.clone())
                    .with_span(f.span),
                );
            }
            Decl::ExternFunc(ef) => {
                // Add extern function to package symbols
                let name = ef.name.0.clone();
                if let Some(prev) = p.symbols.get(&name) {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "duplicate symbol '{}' in package '{}' (extern function)",
                            name, pkg
                        ))
                        .with_file(sf.path.clone()),
                    );
                    reporter.emit(
                        Diagnostic::error(format!(
                            "note: previously declared here as {:?}",
                            prev.kind
                        ))
                        .with_file(prev.file.clone()),
                    );
                } else {
                    p.symbols.insert(
                        name,
                        SymbolEntry {
                            kind: SymbolKind::Module, // Treat as callable entity
                            vis: match ef.vis {
                                crate::compiler::ast::Visibility::Public => Visibility::Public,
                                crate::compiler::ast::Visibility::Internal => Visibility::Internal,
                                crate::compiler::ast::Visibility::Private => Visibility::Private,
                                crate::compiler::ast::Visibility::Default => Visibility::Default,
                            },
                            file: sf.path.clone(),
                        },
                    );
                }
            }
        }
    }
}

fn top_package_segment(pkg: &str) -> &str {
    pkg.split('.').next().unwrap_or("")
}

fn can_import(vis: &Visibility, from_pkg: &str, into_pkg: &str) -> bool {
    match vis {
        Visibility::Public => true,
        Visibility::Internal => top_package_segment(from_pkg) == top_package_segment(into_pkg),
        Visibility::Private | Visibility::Default => from_pkg == into_pkg,
    }
}

fn resolve_imports_for_file(
    pkg_syms: &HashMap<String, PackageSymbols>,
    file_pkg: &str,
    imports: &[(Vec<String>, bool, Option<String>)],
    sf: &SourceFile,
    reporter: &mut Reporter,
) -> ImportedSymbols {
    // import_name -> (package, original_name, kind)
    let mut brought: HashMap<String, (String, String, SymbolKind)> = HashMap::new();
    for (path, star, alias) in imports {
        if *star {
            // Star imports don't support aliasing (alias is ignored)
            let target_pkg = path.join(".");
            if let Some(ps) = pkg_syms.get(&target_pkg) {
                for (name, sym) in &ps.symbols {
                    if !can_import(&sym.vis, &target_pkg, file_pkg) {
                        // Skip non-visible symbols on star import.
                        continue;
                    }
                    if let Some((other_pkg, _, _)) = brought.get(name) {
                        if other_pkg != &target_pkg {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "import conflict: '{}' from '{}' also brought from '{}'",
                                    name, target_pkg, other_pkg
                                ))
                                .with_file(sf.path.clone()),
                            );
                        }
                    } else {
                        // For star imports, import_name = original_name
                        brought.insert(name.clone(), (target_pkg.clone(), name.clone(), sym.kind));
                    }
                }
            } else {
                reporter.emit(
                    Diagnostic::error(format!("unknown package '{}' in import", target_pkg))
                        .with_file(sf.path.clone()),
                );
            }
        } else {
            if path.is_empty() {
                continue;
            }
            let original_name = path.last().unwrap().clone();
            // Use alias if provided, otherwise use the original name
            let import_name = alias.clone().unwrap_or_else(|| original_name.clone());
            let target_pkg = if path.len() > 1 {
                path[..path.len() - 1].join(".")
            } else {
                String::new()
            };
            if target_pkg.is_empty() {
                reporter.emit(
                    Diagnostic::error(format!(
                        "invalid import '{}': missing package",
                        original_name
                    ))
                    .with_file(sf.path.clone()),
                );
                continue;
            }
            if let Some(ps) = pkg_syms.get(&target_pkg) {
                if let Some(sym) = ps.symbols.get(&original_name) {
                    if !can_import(&sym.vis, &target_pkg, file_pkg) {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "'{}' is not visible for import from '{}'",
                                original_name, target_pkg
                            ))
                            .with_file(sf.path.clone()),
                        );
                        continue;
                    }
                    if let Some((other_pkg, _, _)) = brought.get(&import_name) {
                        if other_pkg != &target_pkg {
                            reporter.emit(
                                Diagnostic::error(format!(
                                    "import conflict: '{}' from '{}' also brought from '{}'",
                                    import_name, target_pkg, other_pkg
                                ))
                                .with_file(sf.path.clone()),
                            );
                        }
                    } else {
                        // Store import_name -> (pkg, original_name, kind)
                        brought.insert(import_name, (target_pkg, original_name, sym.kind));
                    }
                } else {
                    reporter.emit(
                        Diagnostic::error(format!(
                            "'{}' not found in package '{}'",
                            original_name, target_pkg
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
            } else {
                reporter.emit(
                    Diagnostic::error(format!("unknown package '{}' in import", target_pkg))
                        .with_file(sf.path.clone()),
                );
            }
        }
    }
    ImportedSymbols { symbols: brought }
}

fn check_local_scopes_in_func(func: &FuncDecl, sf: &SourceFile, reporter: &mut Reporter) {
    use crate::compiler::ast::{Block, Expr, Stmt};
    let mut scopes: Vec<HashSet<String>> = vec![HashSet::new()];
    // Seed params into the function scope
    for p in &func.sig.params {
        scopes[0].insert(p.name.0.clone());
    }
    fn check_assign_target(name: &str, scopes: &[HashSet<String>]) -> bool {
        for s in scopes.iter().rev() {
            if s.contains(name) {
                return true;
            }
        }
        false
    }
    fn is_in_scope(name: &str, scopes: &[HashSet<String>]) -> bool {
        for s in scopes.iter().rev() {
            if s.contains(name) {
                return true;
            }
        }
        false
    }
    fn declare_local(
        scopes: &mut [HashSet<String>],
        name: &str,
        sf: &SourceFile,
        reporter: &mut Reporter,
    ) {
        let Some(cur) = scopes.last_mut() else {
            // Scope stack underflow — should never happen in well-formed code
            return;
        };
        if cur.contains(name) {
            reporter.emit(
                Diagnostic::error(format!("redefinition of local '{}' in same scope", name))
                    .with_file(sf.path.clone()),
            );
        } else {
            cur.insert(name.to_string());
        }
    }

    // Collect free variables (captures) in a lambda body
    fn collect_lambda_captures(
        body: &Block,
        params: &[crate::compiler::ast::Param],
    ) -> HashSet<String> {
        let mut used: HashSet<String> = HashSet::new();
        let mut lambda_locals: HashSet<String> = HashSet::new();

        // Add lambda params to locals
        for p in params {
            lambda_locals.insert(p.name.0.clone());
        }

        fn visit_expr(e: &Expr, locals: &HashSet<String>, used: &mut HashSet<String>) {
            match e {
                Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Char(_) | Expr::Bool(_) => {}
                Expr::Ident(id) => {
                    if !locals.contains(&id.0) {
                        used.insert(id.0.clone());
                    }
                }
                Expr::Binary(l, _, r) => {
                    visit_expr(l, locals, used);
                    visit_expr(r, locals, used);
                }
                Expr::Unary(_, inner) => visit_expr(inner, locals, used),
                Expr::Call(callee, args) => {
                    visit_expr(callee, locals, used);
                    for a in args {
                        visit_expr(a, locals, used);
                    }
                }
                Expr::Member(obj, _) => visit_expr(obj, locals, used),
                Expr::OptionalMember(obj, _) => visit_expr(obj, locals, used),
                Expr::Index(obj, idx) => {
                    visit_expr(obj, locals, used);
                    visit_expr(idx, locals, used);
                }
                Expr::ListLit(elems) => {
                    for elem in elems {
                        visit_expr(elem, locals, used);
                    }
                }
                Expr::MapLit { pairs, spread } => {
                    if let Some(sp) = spread {
                        visit_expr(sp, locals, used);
                    }
                    for (k, v) in pairs {
                        visit_expr(k, locals, used);
                        visit_expr(v, locals, used);
                    }
                }
                Expr::Ternary(c, t, f) => {
                    visit_expr(c, locals, used);
                    visit_expr(t, locals, used);
                    visit_expr(f, locals, used);
                }
                Expr::Cast(_, inner) => visit_expr(inner, locals, used),
                Expr::Await(inner) => visit_expr(inner, locals, used),
                Expr::FnLiteral(_, _) => {
                    // Nested lambdas have independent captures; don't traverse
                }
                Expr::StructLit {
                    type_name,
                    fields,
                    spread,
                } => {
                    visit_expr(type_name, locals, used);
                    if let Some(sp) = spread {
                        visit_expr(sp, locals, used);
                    }
                    for (_, v) in fields {
                        visit_expr(v, locals, used);
                    }
                }
            }
        }

        fn visit_block_for_captures(
            blk: &Block,
            locals: &mut HashSet<String>,
            used: &mut HashSet<String>,
        ) {
            for st in &blk.stmts {
                match st {
                    Stmt::VarDecl { name, init, .. } => {
                        if let Some(e) = init {
                            visit_expr(e, locals, used);
                        }
                        locals.insert(name.0.clone());
                    }
                    Stmt::Assign { expr, .. } | Stmt::AssignOp { expr, .. } => {
                        visit_expr(expr, locals, used);
                    }
                    Stmt::FieldAssign { object, expr, .. } => {
                        visit_expr(object, locals, used);
                        visit_expr(expr, locals, used);
                    }
                    Stmt::PrintExpr(e) | Stmt::PrintRawExpr(e) | Stmt::Expr(e) => {
                        visit_expr(e, locals, used);
                    }
                    Stmt::If {
                        cond,
                        then_blk,
                        else_blk,
                    } => {
                        visit_expr(cond, locals, used);
                        visit_block_for_captures(then_blk, locals, used);
                        if let Some(eb) = else_blk {
                            visit_block_for_captures(eb, locals, used);
                        }
                    }
                    Stmt::While { cond, body } => {
                        visit_expr(cond, locals, used);
                        visit_block_for_captures(body, locals, used);
                    }
                    Stmt::For {
                        init,
                        cond,
                        step,
                        body,
                    } => {
                        if let Some(i) = init
                            && let Stmt::VarDecl { name, init, .. } = i.as_ref()
                        {
                            if let Some(e) = init {
                                visit_expr(e, locals, used);
                            }
                            locals.insert(name.0.clone());
                        }
                        if let Some(c) = cond {
                            visit_expr(c, locals, used);
                        }
                        if let Some(s) = step
                            && let Stmt::AssignOp { expr, .. } = s.as_ref()
                        {
                            visit_expr(expr, locals, used);
                        }
                        visit_block_for_captures(body, locals, used);
                    }
                    Stmt::Switch {
                        expr,
                        cases,
                        pattern_cases,
                        default,
                    } => {
                        visit_expr(expr, locals, used);
                        for (case_e, blk) in cases {
                            visit_expr(case_e, locals, used);
                            visit_block_for_captures(blk, locals, used);
                        }
                        for (_pat, blk) in pattern_cases {
                            visit_block_for_captures(blk, locals, used);
                        }
                        if let Some(db) = default {
                            visit_block_for_captures(db, locals, used);
                        }
                    }
                    Stmt::Return(Some(x)) => visit_expr(x, locals, used),
                    Stmt::Return(None) => {}
                    Stmt::Throw(e) => visit_expr(e, locals, used),
                    Stmt::Panic(e) => visit_expr(e, locals, used),
                    Stmt::Try {
                        try_blk,
                        catches,
                        finally_blk,
                    } => {
                        visit_block_for_captures(try_blk, locals, used);
                        for c in catches {
                            visit_block_for_captures(&c.blk, locals, used);
                        }
                        if let Some(fb) = finally_blk {
                            visit_block_for_captures(fb, locals, used);
                        }
                    }
                    Stmt::Block(b) => visit_block_for_captures(b, locals, used),
                    Stmt::Unsafe(b) => visit_block_for_captures(b, locals, used),
                    Stmt::Labeled { stmt, .. } => {
                        if let Stmt::VarDecl { name, init, .. } = stmt.as_ref() {
                            if let Some(e) = init {
                                visit_expr(e, locals, used);
                            }
                            locals.insert(name.0.clone());
                        }
                    }
                    _ => {}
                }
            }
        }

        visit_block_for_captures(body, &mut lambda_locals, &mut used);
        used
    }

    // Walk an expression to find lambda literals and validate their captures
    fn walk_expr(
        e: &Expr,
        scopes: &mut Vec<HashSet<String>>,
        sf: &SourceFile,
        reporter: &mut Reporter,
    ) {
        match e {
            Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Char(_) | Expr::Bool(_) => {}
            Expr::Ident(_) => {}
            Expr::Binary(l, _, r) => {
                walk_expr(l, scopes, sf, reporter);
                walk_expr(r, scopes, sf, reporter);
            }
            Expr::Unary(_, inner) => walk_expr(inner, scopes, sf, reporter),
            Expr::Call(callee, args) => {
                walk_expr(callee, scopes, sf, reporter);
                for a in args {
                    walk_expr(a, scopes, sf, reporter);
                }
            }
            Expr::Member(obj, _) => walk_expr(obj, scopes, sf, reporter),
            Expr::OptionalMember(obj, _) => walk_expr(obj, scopes, sf, reporter),
            Expr::Index(obj, idx) => {
                walk_expr(obj, scopes, sf, reporter);
                walk_expr(idx, scopes, sf, reporter);
            }
            Expr::ListLit(elems) => {
                for elem in elems {
                    walk_expr(elem, scopes, sf, reporter);
                }
            }
            Expr::MapLit { pairs, spread } => {
                if let Some(sp) = spread {
                    walk_expr(sp, scopes, sf, reporter);
                }
                for (k, v) in pairs {
                    walk_expr(k, scopes, sf, reporter);
                    walk_expr(v, scopes, sf, reporter);
                }
            }
            Expr::Ternary(c, t, f) => {
                walk_expr(c, scopes, sf, reporter);
                walk_expr(t, scopes, sf, reporter);
                walk_expr(f, scopes, sf, reporter);
            }
            Expr::Cast(_, inner) => walk_expr(inner, scopes, sf, reporter),
            Expr::Await(inner) => walk_expr(inner, scopes, sf, reporter),
            Expr::FnLiteral(params, body) => {
                // Validate that all captured variables exist in enclosing scopes
                let captures = collect_lambda_captures(body, params);
                for cap_name in &captures {
                    if !is_in_scope(cap_name, scopes.as_slice()) {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "lambda captures undefined variable '{}'",
                                cap_name
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                }
                // Recursively walk the lambda body for nested lambdas
                scopes.push(HashSet::new());
                // Add lambda params to scope
                if let Some(scope) = scopes.last_mut() {
                    for p in params {
                        scope.insert(p.name.0.clone());
                    }
                }
                walk_block(body, sf, scopes, reporter);
                let _ = scopes.pop();
            }
            Expr::StructLit {
                type_name,
                fields,
                spread,
            } => {
                walk_expr(type_name, scopes, sf, reporter);
                if let Some(sp) = spread {
                    walk_expr(sp, scopes, sf, reporter);
                }
                for (_, v) in fields {
                    walk_expr(v, scopes, sf, reporter);
                }
            }
        }
    }

    fn walk_block(
        blk: &Block,
        sf: &SourceFile,
        scopes: &mut Vec<HashSet<String>>,
        reporter: &mut Reporter,
    ) {
        for st in &blk.stmts {
            match st {
                Stmt::VarDecl { name, init, .. } => {
                    // Walk initializer expression first (before declaring the variable)
                    if let Some(e) = init {
                        walk_expr(e, scopes, sf, reporter);
                    }
                    declare_local(scopes.as_mut_slice(), &name.0, sf, reporter);
                }
                Stmt::Assign { name, expr } => {
                    if !check_assign_target(&name.0, scopes.as_slice()) {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "assignment to undeclared local '{}'",
                                name.0
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                    walk_expr(expr, scopes, sf, reporter);
                }
                Stmt::AssignOp { name, expr, .. } => {
                    if !check_assign_target(&name.0, scopes.as_slice()) {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "compound assignment to undeclared local '{}'",
                                name.0
                            ))
                            .with_file(sf.path.clone()),
                        );
                    }
                    walk_expr(expr, scopes, sf, reporter);
                }
                Stmt::FieldAssign { object, expr, .. } => {
                    walk_expr(object, scopes, sf, reporter);
                    walk_expr(expr, scopes, sf, reporter);
                }
                Stmt::PrintExpr(e) | Stmt::PrintRawExpr(e) | Stmt::Expr(e) => {
                    walk_expr(e, scopes, sf, reporter);
                }
                Stmt::If {
                    cond,
                    then_blk,
                    else_blk,
                } => {
                    walk_expr(cond, scopes, sf, reporter);
                    scopes.push(HashSet::new());
                    walk_block(then_blk, sf, scopes, reporter);
                    let _ = scopes.pop();
                    if let Some(eb) = else_blk {
                        scopes.push(HashSet::new());
                        walk_block(eb, sf, scopes, reporter);
                        let _ = scopes.pop();
                    }
                }
                Stmt::While { cond, body } => {
                    walk_expr(cond, scopes, sf, reporter);
                    scopes.push(HashSet::new());
                    walk_block(body, sf, scopes, reporter);
                    let _ = scopes.pop();
                }
                Stmt::For {
                    init,
                    cond,
                    step,
                    body,
                } => {
                    // For-loop scoping: init, cond, step, and body share the same scope
                    scopes.push(HashSet::new());
                    // Process init (may declare loop variable)
                    if let Some(init_stmt) = init {
                        match init_stmt.as_ref() {
                            Stmt::VarDecl { name, init, .. } => {
                                if let Some(e) = init {
                                    walk_expr(e, scopes, sf, reporter);
                                }
                                declare_local(scopes.as_mut_slice(), &name.0, sf, reporter);
                            }
                            Stmt::Expr(e) => walk_expr(e, scopes, sf, reporter),
                            _ => {}
                        }
                    }
                    // Walk condition expression
                    if let Some(c) = cond {
                        walk_expr(c, scopes, sf, reporter);
                    }
                    // Walk step expression
                    if let Some(s) = step {
                        match s.as_ref() {
                            Stmt::AssignOp { expr, .. } => walk_expr(expr, scopes, sf, reporter),
                            Stmt::Expr(e) => walk_expr(e, scopes, sf, reporter),
                            _ => {}
                        }
                    }
                    // Walk body
                    walk_block(body, sf, scopes, reporter);
                    let _ = scopes.pop();
                }
                Stmt::Switch {
                    expr,
                    cases,
                    pattern_cases,
                    default,
                } => {
                    walk_expr(expr, scopes, sf, reporter);
                    for (case_e, blk) in cases {
                        walk_expr(case_e, scopes, sf, reporter);
                        scopes.push(HashSet::new());
                        walk_block(blk, sf, scopes, reporter);
                        let _ = scopes.pop();
                    }
                    for (_pat, blk) in pattern_cases {
                        scopes.push(HashSet::new());
                        walk_block(blk, sf, scopes, reporter);
                        let _ = scopes.pop();
                    }
                    if let Some(db) = default {
                        scopes.push(HashSet::new());
                        walk_block(db, sf, scopes, reporter);
                        let _ = scopes.pop();
                    }
                }
                Stmt::Try {
                    try_blk,
                    catches,
                    finally_blk,
                } => {
                    scopes.push(HashSet::new());
                    walk_block(try_blk, sf, scopes, reporter);
                    let _ = scopes.pop();
                    for c in catches {
                        scopes.push(HashSet::new());
                        // Add the catch variable to scope if present
                        if let Some(var) = &c.var {
                            if let Some(scope) = scopes.last_mut() {
                                scope.insert(var.0.clone());
                            }
                        }
                        walk_block(&c.blk, sf, scopes, reporter);
                        let _ = scopes.pop();
                    }
                    if let Some(fb) = finally_blk {
                        scopes.push(HashSet::new());
                        walk_block(fb, sf, scopes, reporter);
                        let _ = scopes.pop();
                    }
                }
                Stmt::Return(Some(e)) => {
                    walk_expr(e, scopes, sf, reporter);
                }
                Stmt::Return(None) => {}
                Stmt::Throw(e) => walk_expr(e, scopes, sf, reporter),
                Stmt::Panic(e) => walk_expr(e, scopes, sf, reporter),
                Stmt::Block(b) => {
                    scopes.push(HashSet::new());
                    walk_block(b, sf, scopes, reporter);
                    let _ = scopes.pop();
                }
                Stmt::Unsafe(b) => {
                    scopes.push(HashSet::new());
                    walk_block(b, sf, scopes, reporter);
                    let _ = scopes.pop();
                }
                Stmt::Labeled { stmt, .. } => {
                    // Handle labeled statements
                    match stmt.as_ref() {
                        Stmt::VarDecl { name, init, .. } => {
                            if let Some(e) = init {
                                walk_expr(e, scopes, sf, reporter);
                            }
                            declare_local(scopes.as_mut_slice(), &name.0, sf, reporter);
                        }
                        Stmt::While { cond, body } => {
                            walk_expr(cond, scopes, sf, reporter);
                            scopes.push(HashSet::new());
                            walk_block(body, sf, scopes, reporter);
                            let _ = scopes.pop();
                        }
                        Stmt::For {
                            init,
                            cond,
                            step,
                            body,
                        } => {
                            scopes.push(HashSet::new());
                            if let Some(init_stmt) = init
                                && let Stmt::VarDecl { name, init, .. } = init_stmt.as_ref()
                            {
                                if let Some(e) = init {
                                    walk_expr(e, scopes, sf, reporter);
                                }
                                declare_local(scopes.as_mut_slice(), &name.0, sf, reporter);
                            }
                            if let Some(c) = cond {
                                walk_expr(c, scopes, sf, reporter);
                            }
                            if let Some(s) = step
                                && let Stmt::AssignOp { expr, .. } = s.as_ref()
                            {
                                walk_expr(expr, scopes, sf, reporter);
                            }
                            walk_block(body, sf, scopes, reporter);
                            let _ = scopes.pop();
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(body) = &func.body {
        walk_block(body, sf, &mut scopes, reporter);
    }
}

fn check_locals(files: &[(SourceFile, FileAst)], reporter: &mut Reporter) {
    for (sf, ast) in files {
        for d in &ast.decls {
            if let Decl::Function(f) = d {
                check_local_scopes_in_func(f, sf, reporter);
            }
            if let Decl::Module(m) = d {
                for f in &m.items {
                    check_local_scopes_in_func(f, sf, reporter);
                }
            }
        }
    }
}

fn detect_import_cycles(
    edges: &HashMap<String, HashSet<String>>,
    reporter: &mut Reporter,
    files: &[(SourceFile, FileAst)],
) {
    // Map packages to one file path for diagnostics
    let mut pkg_to_file: HashMap<String, PathBuf> = HashMap::new();
    for (sf, ast) in files {
        if let Some(p) = pkg_string(ast) {
            pkg_to_file.entry(p).or_insert(sf.path.clone());
        }
    }

    fn dfs<'a>(
        u: &'a str,
        edges: &'a HashMap<String, HashSet<String>>,
        seen: &mut HashSet<&'a str>,
        stack: &mut Vec<&'a str>,
        onstack: &mut HashSet<&'a str>,
        cycles: &mut Vec<Vec<String>>,
    ) {
        seen.insert(u);
        onstack.insert(u);
        stack.push(u);
        if let Some(nei) = edges.get(u) {
            for v in nei {
                let vref: &str = v.as_str();
                if !seen.contains(&vref) {
                    dfs(vref, edges, seen, stack, onstack, cycles);
                } else if onstack.contains(&vref) {
                    // Found cycle; collect from vref to end
                    if let Some(pos) = stack.iter().position(|&k| k == vref) {
                        let mut cyc: Vec<String> =
                            stack[pos..].iter().map(|s| (*s).to_string()).collect();
                        cyc.push(vref.to_string());
                        cycles.push(cyc);
                    }
                }
            }
        }
        let _ = stack.pop();
        onstack.remove(u);
    }

    let mut seen: HashSet<&str> = HashSet::new();
    let mut onstack: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = Vec::new();
    let mut cycles: Vec<Vec<String>> = Vec::new();
    for u in edges.keys() {
        let ur: &str = u.as_str();
        if !seen.contains(&ur) {
            dfs(ur, edges, &mut seen, &mut stack, &mut onstack, &mut cycles);
        }
    }
    if let Some(cyc) = cycles.into_iter().next() {
        let msg = format!("package import cycle detected: {}", cyc.join(" -> "));
        // Diagnose on the first package's file
        if let Some(pf) = pkg_to_file.get(&cyc[0]) {
            reporter.emit(Diagnostic::error(msg).with_file(pf.clone()));
        } else {
            reporter.emit(Diagnostic::error(msg));
        }
    }
}

/// Compute module initialization order from package dependency edges.
///
/// Uses Kahn's algorithm for topological sorting. Packages with no dependencies
/// are initialized first, followed by packages that depend only on already-initialized
/// packages.
///
/// Prerequisites: `detect_import_cycles` must have been called first to ensure
/// there are no cycles (otherwise this function may produce incomplete results).
fn compute_initialization_order(
    edges: &HashMap<String, HashSet<String>>,
    all_packages: &HashSet<String>,
) -> ModuleInitOrder {
    // Build in-degree map (how many packages does each package depend on?)
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut reverse_edges: HashMap<String, HashSet<String>> = HashMap::new();

    // Initialize all packages with in-degree 0
    for pkg in all_packages {
        in_degree.insert(pkg.clone(), 0);
    }

    // Count dependencies and build reverse edges
    for (pkg, deps) in edges {
        for dep in deps {
            // Only count dependencies that are in our package set
            if all_packages.contains(dep) {
                *in_degree.entry(pkg.clone()).or_insert(0) += 1;
                reverse_edges
                    .entry(dep.clone())
                    .or_default()
                    .insert(pkg.clone());
            }
        }
    }

    // Kahn's algorithm: start with packages that have no dependencies
    let mut queue: Vec<String> = in_degree
        .iter()
        .filter(|&(_, deg)| *deg == 0)
        .map(|(pkg, _)| pkg.clone())
        .collect();
    queue.sort(); // Deterministic ordering for reproducibility

    let mut init_order: Vec<String> = Vec::new();

    while let Some(pkg) = queue.pop() {
        init_order.push(pkg.clone());

        // Decrement in-degree for all packages that depend on this one
        if let Some(dependents) = reverse_edges.get(&pkg) {
            for dependent in dependents {
                if let Some(deg) = in_degree.get_mut(dependent) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(dependent.clone());
                        queue.sort(); // Keep sorted for determinism
                    }
                }
            }
        }
    }

    // Reverse init order gives us packages that must be initialized last (dependencies first)
    // Wait, we want dependencies first. Let's trace through:
    // If A imports B, edges has A -> B (A depends on B)
    // B has in-degree 0 (nothing depends on it being imported)
    // A has in-degree 1 (depends on B)
    // So B is processed first, then A
    // That means init_order = [B, A], which is correct (B initializes before A)

    let deinit_order: Vec<String> = init_order.iter().rev().cloned().collect();

    ModuleInitOrder {
        init_order,
        deinit_order,
        dependencies: edges.clone(),
    }
}

/// Symbols visible via imports in a file
#[derive(Clone, Debug, Default)]
pub(crate) struct ImportedSymbols {
    /// import name (or alias) -> (source_package, original_name, symbol_kind)
    pub symbols: HashMap<String, (String, String, SymbolKind)>,
}

/// Module initialization order computed from package dependencies.
///
/// Packages are ordered topologically so that a package is initialized
/// only after all packages it imports have been initialized.
#[derive(Clone, Debug, Default)]
pub struct ModuleInitOrder {
    /// Package names in initialization order (dependencies first).
    pub init_order: Vec<String>,
    /// Package names in deinitialization order (dependents first, i.e., reverse of init).
    pub deinit_order: Vec<String>,
    /// The dependency edges: package -> set of packages it depends on.
    pub dependencies: HashMap<String, HashSet<String>>,
}

pub struct ResolvedProgram {
    pub(crate) packages: HashMap<String, PackageSymbols>,
    /// Authoritative package registry
    pub registry: PackageRegistry,
    /// Import-visible symbols per file (keyed by file path)
    pub(crate) file_imports: HashMap<PathBuf, ImportedSymbols>,
    /// Module initialization order based on import dependencies
    pub module_init_order: ModuleInitOrder,
}

impl ResolvedProgram {
    /// Create an empty ResolvedProgram (mainly for tests)
    pub fn empty() -> Self {
        ResolvedProgram {
            packages: HashMap::new(),
            registry: PackageRegistry::default(),
            file_imports: HashMap::new(),
            module_init_order: ModuleInitOrder::default(),
        }
    }

    /// Create a ResolvedProgram with the given packages (for tests)
    #[cfg(test)]
    pub(crate) fn with_packages(packages: HashMap<String, PackageSymbols>) -> Self {
        ResolvedProgram {
            packages,
            registry: PackageRegistry::default(),
            file_imports: HashMap::new(),
            module_init_order: ModuleInitOrder::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedKind {
    Module,
    Struct,
    Interface,
    Enum,
    Provider,
    Function,
}

impl From<SymbolKind> for ResolvedKind {
    fn from(k: SymbolKind) -> Self {
        match k {
            SymbolKind::Module => ResolvedKind::Module,
            SymbolKind::Struct => ResolvedKind::Struct,
            SymbolKind::Interface => ResolvedKind::Interface,
            SymbolKind::Enum => ResolvedKind::Enum,
            SymbolKind::Provider => ResolvedKind::Provider,
            SymbolKind::Function => ResolvedKind::Function,
            SymbolKind::TypeAlias => ResolvedKind::Struct, // treat as nominal type for lookup purposes
        }
    }
}

pub fn lookup_symbol_kind(
    rp: &ResolvedProgram,
    current_pkg: &str,
    name_path: &[String],
) -> Option<ResolvedKind> {
    if name_path.is_empty() {
        return None;
    }
    if name_path.len() == 1 {
        let name = &name_path[0];
        let p = rp.packages.get(current_pkg)?;
        let se = p.symbols.get(name)?;
        return Some(se.kind.into());
    }
    let sym = name_path.last().unwrap().clone();
    let pkg = name_path[..name_path.len() - 1].join(".");
    let p = rp.packages.get(&pkg)?;
    let se = p.symbols.get(&sym)?;
    Some(se.kind.into())
}

/// Result of resolving a qualified name
#[derive(Clone, Debug)]
pub struct ResolvedSymbol {
    /// Package where the symbol is defined
    pub package: String,
    /// Symbol name
    pub name: String,
    /// Symbol kind
    pub kind: ResolvedKind,
}

/// Resolve a qualified name like "pkg.sub.Symbol" from a given context.
///
/// This function tries multiple resolution strategies:
/// 1. If single-segment, look up in current package first
/// 2. If single-segment and not found, check imports
/// 3. If multi-segment, treat all but last as package path
///
/// Returns the resolved symbol info including source package.
pub fn resolve_qualified_name(
    rp: &ResolvedProgram,
    from_package: &str,
    from_file: Option<&Path>,
    qualified_name: &[String],
) -> Option<ResolvedSymbol> {
    if qualified_name.is_empty() {
        return None;
    }

    // Single-segment name: check current package first, then imports
    if qualified_name.len() == 1 {
        let name = &qualified_name[0];

        // First check current package
        if let Some(pkg_syms) = rp.packages.get(from_package) {
            if let Some(se) = pkg_syms.symbols.get(name) {
                return Some(ResolvedSymbol {
                    package: from_package.to_string(),
                    name: name.clone(),
                    kind: se.kind.into(),
                });
            }
        }

        // Check imports for this file
        if let Some(file_path) = from_file {
            if let Some(imported) = rp.file_imports.get(file_path) {
                if let Some((src_pkg, original_name, kind)) = imported.symbols.get(name) {
                    return Some(ResolvedSymbol {
                        package: src_pkg.clone(),
                        name: original_name.clone(),
                        kind: (*kind).into(),
                    });
                }
            }
        }

        return None;
    }

    // Multi-segment name: treat as package.symbol
    let sym_name = qualified_name.last().unwrap().clone();
    let pkg_name = qualified_name[..qualified_name.len() - 1].join(".");

    // Check if it's a valid package
    if let Some(pkg_syms) = rp.packages.get(&pkg_name) {
        if let Some(se) = pkg_syms.symbols.get(&sym_name) {
            // Check visibility
            if can_import(&se.vis, &pkg_name, from_package) {
                return Some(ResolvedSymbol {
                    package: pkg_name,
                    name: sym_name,
                    kind: se.kind.into(),
                });
            }
        }
    }

    None
}

/// Look up a symbol by name in the given context, checking current package and imports.
///
/// This is a convenience function that wraps resolve_qualified_name for simple lookups.
pub fn lookup_symbol(
    rp: &ResolvedProgram,
    current_pkg: &str,
    current_file: Option<&Path>,
    name: &str,
) -> Option<ResolvedSymbol> {
    resolve_qualified_name(rp, current_pkg, current_file, &[name.to_string()])
}

/// Check if a package exists in the registry
pub fn has_package(rp: &ResolvedProgram, pkg_name: &str) -> bool {
    rp.registry.has_package(pkg_name) || rp.packages.contains_key(pkg_name)
}

/// Get information about a package
pub fn get_package_info<'a>(rp: &'a ResolvedProgram, pkg_name: &str) -> Option<&'a PackageInfo> {
    rp.registry.get_package(pkg_name)
}

/// Add stdlib symbols from the StdlibIndex to the packages map.
/// This replaces the manual add_stdlib_stubs() with data from parsed .arth files.
fn add_stdlib_from_index(packages: &mut HashMap<String, PackageSymbols>, stdlib: &StdlibIndex) {
    let std_file = PathBuf::from("<stdlib>");

    for pkg_name in stdlib.package_names() {
        let pkg = packages
            .entry(pkg_name.to_string())
            .or_insert_with(PackageSymbols::default);

        for (sym_name, kind, vis) in stdlib.get_package_symbols(pkg_name) {
            let symbol_kind = match kind {
                StdlibSymbolKind::Module => SymbolKind::Module,
                StdlibSymbolKind::Struct => SymbolKind::Struct,
                StdlibSymbolKind::Enum => SymbolKind::Enum,
                StdlibSymbolKind::Interface => SymbolKind::Interface,
            };

            pkg.symbols.entry(sym_name).or_insert(SymbolEntry {
                kind: symbol_kind,
                vis,
                file: std_file.clone(),
            });
        }
    }
}

/// Validate that all `implements` clauses in modules reference valid, visible interfaces.
fn validate_implements_clauses(
    files: &[(SourceFile, FileAst)],
    packages: &HashMap<String, PackageSymbols>,
    reporter: &mut Reporter,
) {
    for (sf, ast) in files {
        let current_pkg = match pkg_string(ast) {
            Some(p) => p,
            None => continue,
        };

        for decl in &ast.decls {
            if let Decl::Module(module) = decl {
                for imp in &module.implements {
                    validate_interface_reference(
                        &current_pkg,
                        imp,
                        &module.name.0,
                        packages,
                        sf,
                        reporter,
                    );
                }
            }
        }
    }
}

/// Validate that an interface reference in an `implements` clause is valid.
fn validate_interface_reference(
    current_pkg: &str,
    iface: &NamePath,
    module_name: &str,
    packages: &HashMap<String, PackageSymbols>,
    sf: &SourceFile,
    reporter: &mut Reporter,
) {
    let path: Vec<&str> = iface.path.iter().map(|id| id.0.as_str()).collect();
    if path.is_empty() {
        return;
    }

    // Determine the package and symbol name
    let (iface_pkg, iface_name) = if path.len() == 1 {
        // Unqualified name - look in current package
        (current_pkg.to_string(), path[0].to_string())
    } else {
        // Qualified name - package is all but last, name is last
        (
            path[..path.len() - 1].join("."),
            path.last().unwrap().to_string(),
        )
    };

    // Look up the symbol in the package
    match packages.get(&iface_pkg) {
        Some(pkg_syms) => {
            match pkg_syms.symbols.get(&iface_name) {
                Some(entry) => {
                    // Check it's an interface
                    if entry.kind != SymbolKind::Interface {
                        reporter.emit(
                            Diagnostic::error(format!(
                                "module '{}' implements '{}' which is not an interface (found {:?})",
                                module_name,
                                path.join("."),
                                entry.kind
                            ))
                            .with_file(sf.path.clone()),
                        );
                        return;
                    }
                    // Check visibility if from different package
                    if iface_pkg != current_pkg {
                        match entry.vis {
                            Visibility::Private | Visibility::Internal => {
                                reporter.emit(
                                    Diagnostic::error(format!(
                                        "module '{}' cannot implement interface '{}': interface is not exported from package '{}'",
                                        module_name,
                                        path.join("."),
                                        iface_pkg
                                    ))
                                    .with_file(sf.path.clone()),
                                );
                            }
                            Visibility::Public | Visibility::Default => {
                                // OK - visible from other packages
                            }
                        }
                    }
                }
                None => {
                    // Check if it's a stdlib interface that might not be in packages yet
                    // This is a soft warning since stdlib interfaces may be loaded later
                    reporter.emit(
                        Diagnostic::error(format!(
                            "module '{}' implements unknown interface '{}' in package '{}'",
                            module_name, iface_name, iface_pkg
                        ))
                        .with_file(sf.path.clone()),
                    );
                }
            }
        }
        None => {
            reporter.emit(
                Diagnostic::error(format!(
                    "module '{}' implements interface '{}' from unknown package '{}'",
                    module_name,
                    path.join("."),
                    iface_pkg
                ))
                .with_file(sf.path.clone()),
            );
        }
    }
}

/// Check implementation coherence: ensure no duplicate implementations of the same
/// interface for the same type across the project.
///
/// For example, if two modules both implement `Comparable<Point>`, this is a coherence
/// violation because there would be ambiguity about which implementation to use.
fn check_impl_coherence(files: &[(SourceFile, FileAst)], reporter: &mut Reporter) {
    // Track implementations: (interface_key, type_key) -> (module_name, package, file_path)
    // interface_key includes type arguments, e.g., "Comparable<Point>" or just "Display"
    let mut implementations: HashMap<(String, String), (String, String, PathBuf)> = HashMap::new();

    for (sf, ast) in files {
        let current_pkg = match pkg_string(ast) {
            Some(p) => p,
            None => continue,
        };

        for decl in &ast.decls {
            if let Decl::Module(module) = decl {
                // Infer the target type for this module
                let target_type = infer_module_target_type(module);

                for iface in &module.implements {
                    // Build a key that includes the interface name and type arguments
                    let iface_key = format_interface_key(iface);

                    // The coherence key is (interface_with_args, target_type)
                    // e.g., ("Comparable<int>", "Point") or ("Display", "User")
                    let key = (
                        iface_key.clone(),
                        target_type
                            .clone()
                            .unwrap_or_else(|| "<unknown>".to_string()),
                    );

                    if let Some((existing_module, existing_pkg, existing_file)) =
                        implementations.get(&key)
                    {
                        // Duplicate implementation found
                        reporter.emit(
                            Diagnostic::error(format!(
                                "coherence violation: module '{}' in package '{}' implements '{}' for type '{}', \
                                 but module '{}' in package '{}' already implements it (in {})",
                                module.name.0,
                                current_pkg,
                                iface_key,
                                key.1,
                                existing_module,
                                existing_pkg,
                                existing_file.display()
                            ))
                            .with_file(sf.path.clone()),
                        );
                    } else {
                        implementations.insert(
                            key,
                            (module.name.0.clone(), current_pkg.clone(), sf.path.clone()),
                        );
                    }
                }
            }
        }
    }
}

/// Infer the target type that a module implements interfaces for.
/// Uses multiple strategies: first parameter types, naming convention.
fn infer_module_target_type(module: &ModuleDecl) -> Option<String> {
    let is_primitive = |name: &str| {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "int"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "float"
                | "f32"
                | "f64"
                | "bool"
                | "char"
                | "string"
                | "void"
                | "bytes"
        )
    };

    // Strategy 1: Prefer non-primitive first parameters
    let non_primitive_target: Option<String> = module
        .items
        .iter()
        .filter_map(|f| f.sig.params.first())
        .find_map(|p| {
            let name = p.ty.path.last().map(|i| i.0.clone())?;
            if is_primitive(&name) {
                None
            } else {
                Some(name)
            }
        });

    if non_primitive_target.is_some() {
        return non_primitive_target;
    }

    // Strategy 2: Fall back to any first parameter
    let any_target: Option<String> = module
        .items
        .iter()
        .find_map(|f| f.sig.params.first())
        .and_then(|p| p.ty.path.last().map(|i| i.0.clone()));

    if any_target.is_some() {
        return any_target;
    }

    // Strategy 3: Naming convention (e.g., "PointFns" -> "Point")
    for suffix in &["Fns", "Ops", "Impl", "Module"] {
        if module.name.0.ends_with(suffix) && module.name.0.len() > suffix.len() {
            return Some(module.name.0[..module.name.0.len() - suffix.len()].to_string());
        }
    }

    None
}

/// Format an interface reference as a string key including type arguments.
/// e.g., NamePath for "Comparable<int>" becomes "Comparable<int>"
fn format_interface_key(iface: &NamePath) -> String {
    let base_name = iface
        .path
        .iter()
        .map(|id| id.0.as_str())
        .collect::<Vec<_>>()
        .join(".");

    if iface.type_args.is_empty() {
        base_name
    } else {
        let args: Vec<String> = iface
            .type_args
            .iter()
            .map(|arg| {
                arg.path
                    .iter()
                    .map(|id| id.0.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
            })
            .collect();
        format!("{}<{}>", base_name, args.join(", "))
    }
}

pub fn resolve_project(
    root: &Path,
    files: &[(SourceFile, FileAst)],
    reporter: &mut Reporter,
) -> ResolvedProgram {
    // 1) Build the authoritative package registry
    // This handles root directory detection, package validation, and file mapping
    let mut registry = PackageRegistry::build(root, files, reporter);

    // For backwards compatibility, also compute root_dir for legacy checks
    let root_dir = registry.source_root().to_path_buf();

    // 2) Collect top-level symbols per package
    let mut packages: HashMap<String, PackageSymbols> = HashMap::new();
    for (sf, ast) in files {
        record_top_level_symbols(&mut packages, sf, ast, reporter);
    }

    // 2.5) Load stdlib from .arth files and seed symbols for resolution
    // Stdlib is loaded from stdlib/src/*.arth files which are the single source of truth.
    let stdlib_path = Path::new("stdlib/src");
    if stdlib_path.exists() {
        if let Ok(stdlib) = StdlibIndex::load(stdlib_path) {
            add_stdlib_from_index(&mut packages, &stdlib);
            // Register stdlib packages in the registry
            for pkg_name in stdlib.package_names() {
                if !registry.has_package(pkg_name) {
                    registry.register_stdlib_package(pkg_name);
                }
            }
        }
    }

    // 3) Resolve imports per file and collect package-to-package edges
    let mut edges: HashMap<String, HashSet<String>> = HashMap::new();
    let mut file_imports: HashMap<PathBuf, ImportedSymbols> = HashMap::new();
    for (sf, ast) in files {
        let file_pkg = match pkg_string(ast) {
            Some(p) => p,
            None => continue,
        };
        let mut imports: Vec<(Vec<String>, bool, Option<String>)> = Vec::new();
        for imp in &ast.imports {
            let p: Vec<String> = imp.path.iter().map(|id| id.0.clone()).collect();
            let alias = imp.alias.as_ref().map(|a| a.0.clone());
            imports.push((p.clone(), imp.star, alias));
            // Build package edges
            if imp.star {
                let target_pkg = p.join(".");
                edges
                    .entry(file_pkg.clone())
                    .or_default()
                    .insert(target_pkg);
            } else if p.len() > 1 {
                edges
                    .entry(file_pkg.clone())
                    .or_default()
                    .insert(p[..p.len() - 1].join("."));
            }
        }
        let imported = resolve_imports_for_file(&packages, &file_pkg, &imports, sf, reporter);
        file_imports.insert(sf.path.clone(), imported);
    }

    // 4) Local scopes & shadowing checks
    check_locals(files, reporter);

    // 5) Cycle detection
    detect_import_cycles(&edges, reporter, files);

    // 6) Interface conformance: validate implements clauses reference valid interfaces
    validate_implements_clauses(files, &packages, reporter);

    // 7) Coherence checking: ensure no duplicate implementations
    check_impl_coherence(files, reporter);

    // 8) Compute module initialization order from dependency graph
    let all_packages: HashSet<String> = packages.keys().cloned().collect();
    let module_init_order = compute_initialization_order(&edges, &all_packages);

    // Suppress unused variable warning for root_dir (used for backwards compatibility)
    let _ = root_dir;

    ResolvedProgram {
        packages,
        registry,
        file_imports,
        module_init_order,
    }
}

/// Resolve a project with external package symbols
///
/// Same as `resolve_project` but registers external package symbols before import resolution.
pub fn resolve_project_with_externals(
    root: &Path,
    files: &[(SourceFile, FileAst)],
    external_symbols: &[ExternalPackageSymbol],
    reporter: &mut Reporter,
) -> ResolvedProgram {
    // 1) Build the authoritative package registry
    let mut registry = PackageRegistry::build(root, files, reporter);
    let root_dir = registry.source_root().to_path_buf();

    // 2) Collect top-level symbols per package
    let mut packages: HashMap<String, PackageSymbols> = HashMap::new();
    for (sf, ast) in files {
        record_top_level_symbols(&mut packages, sf, ast, reporter);
    }

    // 2.1) Register external package symbols
    register_external_symbols(&mut packages, external_symbols);

    // Register external packages in the registry
    for sym in external_symbols {
        if !registry.has_package(&sym.package) {
            registry.register_external_package(&sym.package);
        }
    }

    // 2.5) Load stdlib from .arth files
    let stdlib_path = Path::new("stdlib/src");
    if stdlib_path.exists() {
        if let Ok(stdlib) = StdlibIndex::load(stdlib_path) {
            add_stdlib_from_index(&mut packages, &stdlib);
            for pkg_name in stdlib.package_names() {
                if !registry.has_package(pkg_name) {
                    registry.register_stdlib_package(pkg_name);
                }
            }
        }
    }

    // 3) Resolve imports per file and collect package-to-package edges
    let mut edges: HashMap<String, HashSet<String>> = HashMap::new();
    let mut file_imports: HashMap<PathBuf, ImportedSymbols> = HashMap::new();
    for (sf, ast) in files {
        let file_pkg = match pkg_string(ast) {
            Some(p) => p,
            None => continue,
        };
        let mut imports: Vec<(Vec<String>, bool, Option<String>)> = Vec::new();
        for imp in &ast.imports {
            let p: Vec<String> = imp.path.iter().map(|id| id.0.clone()).collect();
            let alias = imp.alias.as_ref().map(|a| a.0.clone());
            imports.push((p.clone(), imp.star, alias));
            if imp.star {
                let target_pkg = p.join(".");
                edges
                    .entry(file_pkg.clone())
                    .or_default()
                    .insert(target_pkg);
            } else if p.len() > 1 {
                edges
                    .entry(file_pkg.clone())
                    .or_default()
                    .insert(p[..p.len() - 1].join("."));
            }
        }
        let imported = resolve_imports_for_file(&packages, &file_pkg, &imports, sf, reporter);
        file_imports.insert(sf.path.clone(), imported);
    }

    // 4) Local scopes & shadowing checks
    check_locals(files, reporter);

    // 5) Cycle detection
    detect_import_cycles(&edges, reporter, files);

    // 6) Interface conformance
    validate_implements_clauses(files, &packages, reporter);

    // 7) Coherence checking
    check_impl_coherence(files, reporter);

    // 8) Compute module initialization order
    let all_packages: HashSet<String> = packages.keys().cloned().collect();
    let module_init_order = compute_initialization_order(&edges, &all_packages);

    let _ = root_dir;

    ResolvedProgram {
        packages,
        registry,
        file_imports,
        module_init_order,
    }
}

#[cfg(test)]
mod tests;
