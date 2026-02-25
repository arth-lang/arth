//! Attribute validation pass for the type checker.
//!
//! This module validates attributes on declarations:
//! - Checks that attributes are valid for their targets
//! - Validates attribute arguments
//! - Collects test/bench functions
//! - Tracks deprecated declarations for usage warnings
//! - Collects must_use information for return value checking

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::compiler::ast::{self as AS, Decl};
use crate::compiler::attrs::{
    AttrTarget, BuiltinAttr, DeprecatedInfo, InlineHint, parse_deprecated_args,
    parse_derive_targets, parse_inline_args, parse_must_use_reason, validate_attr,
};
use crate::compiler::diagnostics::{Diagnostic, Reporter};
use crate::compiler::source::SourceFile;

/// Collected information about test and benchmark functions.
#[derive(Clone, Debug, Default)]
pub struct TestCollection {
    /// Test functions: (package, module, function_name, span)
    pub tests: Vec<TestEntry>,
    /// Benchmark functions: (package, module, function_name, span)
    pub benches: Vec<TestEntry>,
}

/// Entry for a test or benchmark function.
#[derive(Clone, Debug)]
pub struct TestEntry {
    pub package: String,
    pub module: Option<String>,
    pub function: String,
    pub file: PathBuf,
}

/// Information about deprecated declarations.
#[derive(Clone, Debug, Default)]
pub struct DeprecationIndex {
    /// Deprecated structs: (package, name) -> deprecation info
    pub structs: HashMap<(String, String), DeprecatedInfo>,
    /// Deprecated modules: (package, name) -> deprecation info
    pub modules: HashMap<(String, String), DeprecatedInfo>,
    /// Deprecated functions: (package, module_opt, name) -> deprecation info
    pub functions: HashMap<(String, Option<String>, String), DeprecatedInfo>,
    /// Deprecated fields: (package, struct_name, field_name) -> deprecation info
    pub fields: HashMap<(String, String, String), DeprecatedInfo>,
    /// Deprecated enums: (package, name) -> deprecation info
    pub enums: HashMap<(String, String), DeprecatedInfo>,
}

/// Information about @must_use declarations.
#[derive(Clone, Debug, Default)]
pub struct MustUseIndex {
    /// Functions with @must_use: (package, module_opt, name) -> optional reason
    pub functions: HashMap<(String, Option<String>, String), Option<String>>,
    /// Structs with @must_use: (package, name) -> optional reason
    pub structs: HashMap<(String, String), Option<String>>,
    /// Enums with @must_use: (package, name) -> optional reason
    pub enums: HashMap<(String, String), Option<String>>,
}

/// Information about @inline hints.
#[derive(Clone, Debug, Default)]
pub struct InlineIndex {
    /// Functions with @inline: (package, module_opt, name) -> inline hint
    pub functions: HashMap<(String, Option<String>, String), InlineHint>,
}

/// Information about @allow suppressions.
#[derive(Clone, Debug, Default)]
pub struct AllowIndex {
    /// Suppressed lints on functions: (package, module_opt, name) -> set of lint IDs
    pub functions: HashMap<(String, Option<String>, String), HashSet<String>>,
    /// Suppressed lints on structs: (package, name) -> set of lint IDs
    pub structs: HashMap<(String, String), HashSet<String>>,
    /// Suppressed lints on modules: (package, name) -> set of lint IDs
    pub modules: HashMap<(String, String), HashSet<String>>,
    /// Suppressed lints on fields: (package, struct_name, field_name) -> set of lint IDs
    pub fields: HashMap<(String, String, String), HashSet<String>>,
}

impl AllowIndex {
    /// Check if a lint is suppressed for a function.
    pub fn is_suppressed_for_function(
        &self,
        pkg: &str,
        module: Option<&str>,
        name: &str,
        lint_id: &str,
    ) -> bool {
        let key = (
            pkg.to_string(),
            module.map(|s| s.to_string()),
            name.to_string(),
        );
        self.functions
            .get(&key)
            .map(|lints| lints.contains(lint_id))
            .unwrap_or(false)
    }

    /// Check if a lint is suppressed for a struct.
    pub fn is_suppressed_for_struct(&self, pkg: &str, name: &str, lint_id: &str) -> bool {
        let key = (pkg.to_string(), name.to_string());
        self.structs
            .get(&key)
            .map(|lints| lints.contains(lint_id))
            .unwrap_or(false)
    }
}

/// Complete attribute analysis results.
#[derive(Clone, Debug, Default)]
pub struct AttributeAnalysis {
    pub tests: TestCollection,
    pub deprecations: DeprecationIndex,
    pub must_use: MustUseIndex,
    pub inlines: InlineIndex,
    pub allows: AllowIndex,
    /// Unknown attributes encountered (for warnings)
    pub unknown_attrs: Vec<(String, PathBuf)>,
}

/// Get the package string from an AST file.
fn pkg_string(ast: &AS::FileAst) -> Option<String> {
    ast.package.as_ref().map(|p| {
        p.0.iter()
            .map(|i| i.0.as_str())
            .collect::<Vec<_>>()
            .join(".")
    })
}

/// Convert AST attribute name to string.
fn attr_name_string(attr: &AS::Attr) -> String {
    attr.name
        .path
        .iter()
        .map(|i| i.0.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

/// Validate attributes on a single declaration and collect information.
fn validate_decl_attrs(
    attrs: &[AS::Attr],
    target: AttrTarget,
    pkg: &str,
    decl_name: &str,
    module_name: Option<&str>,
    sf: &SourceFile,
    reporter: &mut Reporter,
    analysis: &mut AttributeAnalysis,
) {
    let mut seen_builtins: HashSet<BuiltinAttr> = HashSet::new();

    for attr in attrs {
        let name = attr_name_string(attr);

        // Validate the attribute
        match validate_attr(&name, attr.args.as_deref(), target) {
            Ok(validated) => {
                // Check for duplicates
                if let Some(builtin) = validated.builtin {
                    if !builtin.allow_multiple() && seen_builtins.contains(&builtin) {
                        reporter.emit(
                            Diagnostic::warning(format!("duplicate attribute @{}", name))
                                .with_file(sf.path.clone()),
                        );
                    }
                    seen_builtins.insert(builtin);

                    // Process specific builtins
                    match builtin {
                        BuiltinAttr::Test => {
                            if target == AttrTarget::Function {
                                analysis.tests.tests.push(TestEntry {
                                    package: pkg.to_string(),
                                    module: module_name.map(|s| s.to_string()),
                                    function: decl_name.to_string(),
                                    file: sf.path.clone(),
                                });
                            }
                        }
                        BuiltinAttr::Bench => {
                            if target == AttrTarget::Function {
                                analysis.tests.benches.push(TestEntry {
                                    package: pkg.to_string(),
                                    module: module_name.map(|s| s.to_string()),
                                    function: decl_name.to_string(),
                                    file: sf.path.clone(),
                                });
                            }
                        }
                        BuiltinAttr::Deprecated => {
                            let info = parse_deprecated_args(&validated.args);
                            match target {
                                AttrTarget::Struct => {
                                    analysis
                                        .deprecations
                                        .structs
                                        .insert((pkg.to_string(), decl_name.to_string()), info);
                                }
                                AttrTarget::Module => {
                                    analysis
                                        .deprecations
                                        .modules
                                        .insert((pkg.to_string(), decl_name.to_string()), info);
                                }
                                AttrTarget::Function | AttrTarget::ExternFunc => {
                                    analysis.deprecations.functions.insert(
                                        (
                                            pkg.to_string(),
                                            module_name.map(|s| s.to_string()),
                                            decl_name.to_string(),
                                        ),
                                        info,
                                    );
                                }
                                AttrTarget::Enum => {
                                    analysis
                                        .deprecations
                                        .enums
                                        .insert((pkg.to_string(), decl_name.to_string()), info);
                                }
                                AttrTarget::Field => {
                                    // Field deprecation needs struct context - handled separately
                                }
                                _ => {}
                            }
                        }
                        BuiltinAttr::MustUse => {
                            let reason = parse_must_use_reason(&validated.args);
                            match target {
                                AttrTarget::Function | AttrTarget::ExternFunc => {
                                    analysis.must_use.functions.insert(
                                        (
                                            pkg.to_string(),
                                            module_name.map(|s| s.to_string()),
                                            decl_name.to_string(),
                                        ),
                                        reason,
                                    );
                                }
                                AttrTarget::Struct => {
                                    analysis
                                        .must_use
                                        .structs
                                        .insert((pkg.to_string(), decl_name.to_string()), reason);
                                }
                                AttrTarget::Enum => {
                                    analysis
                                        .must_use
                                        .enums
                                        .insert((pkg.to_string(), decl_name.to_string()), reason);
                                }
                                _ => {}
                            }
                        }
                        BuiltinAttr::Inline => {
                            if matches!(target, AttrTarget::Function | AttrTarget::ExternFunc) {
                                let hint = parse_inline_args(&validated.args);
                                analysis.inlines.functions.insert(
                                    (
                                        pkg.to_string(),
                                        module_name.map(|s| s.to_string()),
                                        decl_name.to_string(),
                                    ),
                                    hint,
                                );
                            }
                        }
                        BuiltinAttr::Derive => {
                            // Validate derive targets
                            match parse_derive_targets(&validated.args) {
                                Ok(_targets) => {
                                    // Targets validated successfully
                                }
                                Err(e) => {
                                    reporter.emit(
                                        Diagnostic::error(e.message).with_file(sf.path.clone()),
                                    );
                                }
                            }
                        }
                        BuiltinAttr::Intrinsic => {
                            // Intrinsic validation is handled elsewhere
                        }
                        BuiltinAttr::Allow => {
                            // Collect lint suppressions
                            let lint_id = match &validated.args {
                                crate::compiler::attrs::AttrArg::Ident(id) => id.clone(),
                                crate::compiler::attrs::AttrArg::IdentList(ids)
                                    if !ids.is_empty() =>
                                {
                                    ids[0].clone()
                                }
                                crate::compiler::attrs::AttrArg::String(s) => s.clone(),
                                _ => continue,
                            };
                            match target {
                                AttrTarget::Function | AttrTarget::ExternFunc => {
                                    let key = (
                                        pkg.to_string(),
                                        module_name.map(|s| s.to_string()),
                                        decl_name.to_string(),
                                    );
                                    analysis
                                        .allows
                                        .functions
                                        .entry(key)
                                        .or_default()
                                        .insert(lint_id);
                                }
                                AttrTarget::Struct => {
                                    let key = (pkg.to_string(), decl_name.to_string());
                                    analysis
                                        .allows
                                        .structs
                                        .entry(key)
                                        .or_default()
                                        .insert(lint_id);
                                }
                                AttrTarget::Module => {
                                    let key = (pkg.to_string(), decl_name.to_string());
                                    analysis
                                        .allows
                                        .modules
                                        .entry(key)
                                        .or_default()
                                        .insert(lint_id);
                                }
                                _ => {}
                            }
                        }
                        BuiltinAttr::Rename
                        | BuiltinAttr::Default
                        | BuiltinAttr::JsonIgnore
                        | BuiltinAttr::FfiOwned
                        | BuiltinAttr::FfiBorrowed
                        | BuiltinAttr::FfiTransfers => {
                            // Field/FFI attributes handled in their specific contexts
                        }
                        BuiltinAttr::Cfg => {
                            // Conditional compilation handled in a separate pass
                        }
                    }
                } else {
                    // Unknown attribute - emit warning
                    analysis.unknown_attrs.push((name.clone(), sf.path.clone()));
                    reporter.emit(
                        Diagnostic::warning(format!("unknown attribute @{}", name))
                            .with_file(sf.path.clone()),
                    );
                }
            }
            Err(e) => {
                reporter.emit(Diagnostic::error(e.message).with_file(sf.path.clone()));
            }
        }
    }
}

/// Validate field attributes within a struct or provider.
fn validate_field_attrs(
    field: &AS::StructField,
    pkg: &str,
    parent_name: &str,
    sf: &SourceFile,
    reporter: &mut Reporter,
    analysis: &mut AttributeAnalysis,
) {
    for attr in &field.attrs {
        let name = attr_name_string(attr);

        match validate_attr(&name, attr.args.as_deref(), AttrTarget::Field) {
            Ok(validated) => {
                if let Some(builtin) = validated.builtin {
                    match builtin {
                        BuiltinAttr::Deprecated => {
                            let info = parse_deprecated_args(&validated.args);
                            analysis.deprecations.fields.insert(
                                (
                                    pkg.to_string(),
                                    parent_name.to_string(),
                                    field.name.0.clone(),
                                ),
                                info,
                            );
                        }
                        BuiltinAttr::Rename | BuiltinAttr::Default | BuiltinAttr::JsonIgnore => {
                            // Valid field attributes, handled during codegen
                        }
                        _ => {
                            // Other builtins not valid on fields - already caught by validate_attr
                        }
                    }
                } else if validated.builtin.is_none() {
                    analysis.unknown_attrs.push((name.clone(), sf.path.clone()));
                    reporter.emit(
                        Diagnostic::warning(format!("unknown attribute @{}", name))
                            .with_file(sf.path.clone()),
                    );
                }
            }
            Err(e) => {
                reporter.emit(Diagnostic::error(e.message).with_file(sf.path.clone()));
            }
        }
    }
}

/// Validate all attributes in a file and collect analysis information.
pub fn validate_file_attrs(
    sf: &SourceFile,
    ast: &AS::FileAst,
    reporter: &mut Reporter,
    analysis: &mut AttributeAnalysis,
) {
    let pkg = match pkg_string(ast) {
        Some(p) => p,
        None => return,
    };

    for decl in &ast.decls {
        match decl {
            Decl::Struct(s) => {
                validate_decl_attrs(
                    &s.attrs,
                    AttrTarget::Struct,
                    &pkg,
                    &s.name.0,
                    None,
                    sf,
                    reporter,
                    analysis,
                );
                // Validate field attributes
                for field in &s.fields {
                    validate_field_attrs(field, &pkg, &s.name.0, sf, reporter, analysis);
                }
            }
            Decl::Module(m) => {
                validate_decl_attrs(
                    &m.attrs,
                    AttrTarget::Module,
                    &pkg,
                    &m.name.0,
                    None,
                    sf,
                    reporter,
                    analysis,
                );
                // Validate function attributes within module
                for func in &m.items {
                    validate_decl_attrs(
                        &func.sig.attrs,
                        AttrTarget::Function,
                        &pkg,
                        &func.sig.name.0,
                        Some(&m.name.0),
                        sf,
                        reporter,
                        analysis,
                    );
                }
            }
            Decl::Interface(i) => {
                validate_decl_attrs(
                    &i.attrs,
                    AttrTarget::Interface,
                    &pkg,
                    &i.name.0,
                    None,
                    sf,
                    reporter,
                    analysis,
                );
                // Interface method signatures may have attributes
                for method in &i.methods {
                    validate_decl_attrs(
                        &method.sig.attrs,
                        AttrTarget::Function,
                        &pkg,
                        &method.sig.name.0,
                        None,
                        sf,
                        reporter,
                        analysis,
                    );
                }
            }
            Decl::Enum(e) => {
                validate_decl_attrs(
                    &e.attrs,
                    AttrTarget::Enum,
                    &pkg,
                    &e.name.0,
                    None,
                    sf,
                    reporter,
                    analysis,
                );
            }
            Decl::Provider(p) => {
                validate_decl_attrs(
                    &p.attrs,
                    AttrTarget::Provider,
                    &pkg,
                    &p.name.0,
                    None,
                    sf,
                    reporter,
                    analysis,
                );
                // Validate field attributes
                for field in &p.fields {
                    validate_field_attrs(field, &pkg, &p.name.0, sf, reporter, analysis);
                }
            }
            Decl::TypeAlias(ta) => {
                validate_decl_attrs(
                    &ta.attrs,
                    AttrTarget::TypeAlias,
                    &pkg,
                    &ta.name.0,
                    None,
                    sf,
                    reporter,
                    analysis,
                );
            }
            Decl::ExternFunc(ef) => {
                validate_decl_attrs(
                    &ef.attrs,
                    AttrTarget::ExternFunc,
                    &pkg,
                    &ef.name.0,
                    None,
                    sf,
                    reporter,
                    analysis,
                );
            }
            Decl::Function(f) => {
                // Free functions (will be rejected elsewhere, but still validate attrs)
                validate_decl_attrs(
                    &f.sig.attrs,
                    AttrTarget::Function,
                    &pkg,
                    &f.sig.name.0,
                    None,
                    sf,
                    reporter,
                    analysis,
                );
            }
        }
    }
}

/// Main entry point: validate attributes across all files.
pub fn validate_attributes(
    files: &[(SourceFile, AS::FileAst)],
    reporter: &mut Reporter,
) -> AttributeAnalysis {
    let mut analysis = AttributeAnalysis::default();

    // First pass: collect all attribute info
    for (sf, ast) in files {
        validate_file_attrs(sf, ast, reporter, &mut analysis);
    }

    // Second pass: check for deprecated usage and must_use violations in function bodies
    for (sf, ast) in files {
        let pkg = match pkg_string(ast) {
            Some(p) => p,
            None => continue,
        };
        check_usages_in_file(sf, ast, &pkg, &analysis, reporter);
    }

    // Summary: emit info if tests/benches found
    let test_count = analysis.tests.tests.len();
    let bench_count = analysis.tests.benches.len();
    if test_count > 0 || bench_count > 0 {
        // This could be logged at a debug level
        // println!("Collected {} tests, {} benchmarks", test_count, bench_count);
    }

    analysis
}

/// Check for deprecated usages and must_use violations in all function bodies in a file.
fn check_usages_in_file(
    sf: &SourceFile,
    ast: &AS::FileAst,
    pkg: &str,
    analysis: &AttributeAnalysis,
    reporter: &mut Reporter,
) {
    for decl in &ast.decls {
        match decl {
            Decl::Module(m) => {
                for func in &m.items {
                    if let Some(body) = &func.body {
                        check_usages_in_block(body, sf, pkg, analysis, reporter);
                    }
                }
            }
            Decl::Function(f) => {
                if let Some(body) = &f.body {
                    check_usages_in_block(body, sf, pkg, analysis, reporter);
                }
            }
            _ => {}
        }
    }
}

/// Check for deprecation and must_use in a block.
fn check_usages_in_block(
    block: &AS::Block,
    sf: &SourceFile,
    pkg: &str,
    analysis: &AttributeAnalysis,
    reporter: &mut Reporter,
) {
    for stmt in &block.stmts {
        check_usages_in_stmt(stmt, sf, pkg, analysis, reporter);
    }
}

/// Check for deprecation and must_use in a statement.
fn check_usages_in_stmt(
    stmt: &AS::Stmt,
    sf: &SourceFile,
    pkg: &str,
    analysis: &AttributeAnalysis,
    reporter: &mut Reporter,
) {
    match stmt {
        AS::Stmt::Expr(e) => {
            // This is an expression statement - result is discarded
            // Check for @must_use violation
            check_must_use_violation(e, sf, pkg, &analysis.must_use, reporter);
            check_usages_in_expr(e, sf, pkg, analysis, reporter);
        }
        AS::Stmt::PrintExpr(e)
        | AS::Stmt::Return(Some(e))
        | AS::Stmt::Throw(e)
        | AS::Stmt::Panic(e) => {
            check_usages_in_expr(e, sf, pkg, analysis, reporter);
        }
        AS::Stmt::VarDecl { init: Some(e), .. } | AS::Stmt::Assign { expr: e, .. } => {
            check_usages_in_expr(e, sf, pkg, analysis, reporter);
        }
        AS::Stmt::AssignOp { expr: e, .. } => {
            check_usages_in_expr(e, sf, pkg, analysis, reporter);
        }
        AS::Stmt::FieldAssign { object, expr, .. } => {
            check_usages_in_expr(object, sf, pkg, analysis, reporter);
            check_usages_in_expr(expr, sf, pkg, analysis, reporter);
        }
        AS::Stmt::If {
            cond,
            then_blk,
            else_blk,
        } => {
            check_usages_in_expr(cond, sf, pkg, analysis, reporter);
            check_usages_in_block(then_blk, sf, pkg, analysis, reporter);
            if let Some(eb) = else_blk {
                check_usages_in_block(eb, sf, pkg, analysis, reporter);
            }
        }
        AS::Stmt::While { cond, body } => {
            check_usages_in_expr(cond, sf, pkg, analysis, reporter);
            check_usages_in_block(body, sf, pkg, analysis, reporter);
        }
        AS::Stmt::For {
            init,
            cond,
            step,
            body,
        } => {
            if let Some(i) = init {
                check_usages_in_stmt(i, sf, pkg, analysis, reporter);
            }
            if let Some(c) = cond {
                check_usages_in_expr(c, sf, pkg, analysis, reporter);
            }
            if let Some(s) = step {
                check_usages_in_stmt(s, sf, pkg, analysis, reporter);
            }
            check_usages_in_block(body, sf, pkg, analysis, reporter);
        }
        AS::Stmt::Switch {
            expr,
            cases,
            default,
            ..
        } => {
            check_usages_in_expr(expr, sf, pkg, analysis, reporter);
            for (e, b) in cases {
                check_usages_in_expr(e, sf, pkg, analysis, reporter);
                check_usages_in_block(b, sf, pkg, analysis, reporter);
            }
            if let Some(db) = default {
                check_usages_in_block(db, sf, pkg, analysis, reporter);
            }
        }
        AS::Stmt::Try {
            try_blk,
            catches,
            finally_blk,
        } => {
            check_usages_in_block(try_blk, sf, pkg, analysis, reporter);
            for c in catches {
                check_usages_in_block(&c.blk, sf, pkg, analysis, reporter);
            }
            if let Some(fb) = finally_blk {
                check_usages_in_block(fb, sf, pkg, analysis, reporter);
            }
        }
        AS::Stmt::Block(b) | AS::Stmt::Unsafe(b) => {
            check_usages_in_block(b, sf, pkg, analysis, reporter);
        }
        AS::Stmt::Labeled { stmt, .. } => {
            check_usages_in_stmt(stmt, sf, pkg, analysis, reporter);
        }
        _ => {}
    }
}

/// Check if a discarded expression violates @must_use.
fn check_must_use_violation(
    expr: &AS::Expr,
    sf: &SourceFile,
    pkg: &str,
    must_use: &MustUseIndex,
    reporter: &mut Reporter,
) {
    // Get the function being called (if it's a call expression)
    if let AS::Expr::Call(callee, _) = expr
        && let Some((name, reason)) = get_must_use_function(callee, pkg, must_use)
    {
        let msg = match reason {
            Some(r) => format!(
                "unused result of call to '{}' which must be used: {}",
                name, r
            ),
            None => format!(
                "unused result of call to '{}' which is marked as #[must_use]",
                name
            ),
        };
        reporter.emit(Diagnostic::warning(msg).with_file(sf.path.clone()));
    }
}

/// Get the name and must_use reason if the callee is a @must_use function.
fn get_must_use_function(
    callee: &AS::Expr,
    pkg: &str,
    must_use: &MustUseIndex,
) -> Option<(String, Option<String>)> {
    match callee {
        AS::Expr::Member(base, func_name) => {
            // Module.function pattern
            if let AS::Expr::Ident(module_name) = &**base {
                let key = (
                    pkg.to_string(),
                    Some(module_name.0.clone()),
                    func_name.0.clone(),
                );
                if let Some(reason) = must_use.functions.get(&key) {
                    return Some((format!("{}.{}", module_name.0, func_name.0), reason.clone()));
                }
            }
        }
        AS::Expr::Ident(func_name) => {
            // Free function pattern (rare but possible)
            let key = (pkg.to_string(), None, func_name.0.clone());
            if let Some(reason) = must_use.functions.get(&key) {
                return Some((func_name.0.clone(), reason.clone()));
            }
        }
        _ => {}
    }
    None
}

/// Check for deprecation in an expression.
fn check_usages_in_expr(
    expr: &AS::Expr,
    sf: &SourceFile,
    pkg: &str,
    analysis: &AttributeAnalysis,
    reporter: &mut Reporter,
) {
    // Check this expression for deprecated usage
    if let Some((name, info)) = check_deprecated_usage(expr, pkg, &analysis.deprecations) {
        reporter.emit(
            Diagnostic::warning(format_deprecation_warning(&name, &info))
                .with_file(sf.path.clone()),
        );
    }

    // Recursively check subexpressions
    match expr {
        AS::Expr::Binary(l, _, r) => {
            check_usages_in_expr(l, sf, pkg, analysis, reporter);
            check_usages_in_expr(r, sf, pkg, analysis, reporter);
        }
        AS::Expr::Unary(_, e) | AS::Expr::Await(e) | AS::Expr::Cast(_, e) => {
            check_usages_in_expr(e, sf, pkg, analysis, reporter);
        }
        AS::Expr::Ternary(c, t, f) => {
            check_usages_in_expr(c, sf, pkg, analysis, reporter);
            check_usages_in_expr(t, sf, pkg, analysis, reporter);
            check_usages_in_expr(f, sf, pkg, analysis, reporter);
        }
        AS::Expr::Call(callee, args) => {
            check_usages_in_expr(callee, sf, pkg, analysis, reporter);
            for a in args {
                check_usages_in_expr(a, sf, pkg, analysis, reporter);
            }
        }
        AS::Expr::Member(base, _) => {
            check_usages_in_expr(base, sf, pkg, analysis, reporter);
        }
        AS::Expr::Index(base, idx) => {
            check_usages_in_expr(base, sf, pkg, analysis, reporter);
            check_usages_in_expr(idx, sf, pkg, analysis, reporter);
        }
        AS::Expr::ListLit(items) => {
            for item in items {
                check_usages_in_expr(item, sf, pkg, analysis, reporter);
            }
        }
        AS::Expr::MapLit { pairs, spread } => {
            if let Some(spread_expr) = spread {
                check_usages_in_expr(spread_expr, sf, pkg, analysis, reporter);
            }
            for (k, v) in pairs {
                check_usages_in_expr(k, sf, pkg, analysis, reporter);
                check_usages_in_expr(v, sf, pkg, analysis, reporter);
            }
        }
        AS::Expr::FnLiteral(_, body) => {
            check_usages_in_block(body, sf, pkg, analysis, reporter);
        }
        _ => {}
    }
}

/// Check if an expression is using a deprecated item.
/// Returns deprecation info if the expression references a deprecated item.
pub fn check_deprecated_usage(
    expr: &AS::Expr,
    current_pkg: &str,
    deprecations: &DeprecationIndex,
) -> Option<(String, DeprecatedInfo)> {
    match expr {
        AS::Expr::Ident(id) => {
            // Could be a struct constructor or local variable
            // Check structs in current package
            if let Some(info) = deprecations
                .structs
                .get(&(current_pkg.to_string(), id.0.clone()))
            {
                return Some((id.0.clone(), info.clone()));
            }
        }
        AS::Expr::Member(base, member) => {
            // Could be Module.function or field access
            if let AS::Expr::Ident(module_name) = &**base {
                // Check if it's a deprecated module function
                if let Some(info) = deprecations.functions.get(&(
                    current_pkg.to_string(),
                    Some(module_name.0.clone()),
                    member.0.clone(),
                )) {
                    return Some((format!("{}.{}", module_name.0, member.0), info.clone()));
                }
                // Check if module itself is deprecated
                if let Some(info) = deprecations
                    .modules
                    .get(&(current_pkg.to_string(), module_name.0.clone()))
                {
                    return Some((module_name.0.clone(), info.clone()));
                }
            }
        }
        AS::Expr::Call(callee, _) => {
            // Recurse into callee
            return check_deprecated_usage(callee, current_pkg, deprecations);
        }
        _ => {}
    }
    None
}

/// Format a deprecation warning message.
pub fn format_deprecation_warning(name: &str, info: &DeprecatedInfo) -> String {
    let mut msg = format!("use of deprecated item '{}'", name);
    if let Some(since) = &info.since {
        msg.push_str(&format!(" (deprecated since {})", since));
    }
    if let Some(note) = &info.note {
        msg.push_str(&format!(": {}", note));
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::ast::{Attr, Ident, NamePath};

    fn make_attr(name: &str, args: Option<&str>) -> AS::Attr {
        Attr {
            name: NamePath {
                path: vec![Ident(name.to_string())],
                type_args: vec![],
            },
            args: args.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_validate_test_on_function() {
        // @test is valid on functions
        let result = validate_attr("test", None, AttrTarget::Function);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_test_on_struct_fails() {
        // @test is not valid on structs
        let result = validate_attr("test", None, AttrTarget::Struct);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_derive_on_struct() {
        // @derive(Eq, Hash) is valid on structs
        let result = validate_attr("derive", Some("Eq, Hash"), AttrTarget::Struct);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_deprecated_with_args() {
        // @deprecated(since="1.0", note="Use newFoo") is valid
        let result = validate_attr(
            "deprecated",
            Some("since=\"1.0\", note=\"Use newFoo\""),
            AttrTarget::Function,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_attr_name_string() {
        let attr = make_attr("test", None);
        assert_eq!(attr_name_string(&attr), "test");
    }

    #[test]
    fn test_format_deprecation_warning() {
        let info = DeprecatedInfo {
            since: Some("2025.1".to_string()),
            note: Some("Use newFoo instead".to_string()),
        };
        let msg = format_deprecation_warning("oldFoo", &info);
        assert!(msg.contains("oldFoo"));
        assert!(msg.contains("2025.1"));
        assert!(msg.contains("Use newFoo instead"));
    }

    #[test]
    fn test_check_deprecated_usage_struct() {
        // Test that check_deprecated_usage detects deprecated struct usage
        let mut deprecations = DeprecationIndex::default();
        deprecations.structs.insert(
            ("mypackage".to_string(), "OldStruct".to_string()),
            DeprecatedInfo {
                since: Some("1.0".to_string()),
                note: Some("Use NewStruct".to_string()),
            },
        );

        // Create an expression that references OldStruct
        let expr = AS::Expr::Ident(Ident("OldStruct".to_string()));
        let result = check_deprecated_usage(&expr, "mypackage", &deprecations);
        assert!(result.is_some());
        let (name, info) = result.unwrap();
        assert_eq!(name, "OldStruct");
        assert_eq!(info.since, Some("1.0".to_string()));
    }

    #[test]
    fn test_check_deprecated_usage_module_function() {
        // Test that check_deprecated_usage detects deprecated module function usage
        let mut deprecations = DeprecationIndex::default();
        deprecations.functions.insert(
            (
                "mypackage".to_string(),
                Some("OldModule".to_string()),
                "oldFunc".to_string(),
            ),
            DeprecatedInfo {
                since: None,
                note: Some("Use newFunc".to_string()),
            },
        );

        // Create an expression OldModule.oldFunc
        let expr = AS::Expr::Member(
            Box::new(AS::Expr::Ident(Ident("OldModule".to_string()))),
            Ident("oldFunc".to_string()),
        );
        let result = check_deprecated_usage(&expr, "mypackage", &deprecations);
        assert!(result.is_some());
        let (name, _info) = result.unwrap();
        assert_eq!(name, "OldModule.oldFunc");
    }

    #[test]
    fn test_check_deprecated_usage_no_match() {
        // Test that non-deprecated items don't trigger
        let deprecations = DeprecationIndex::default();
        let expr = AS::Expr::Ident(Ident("NewStruct".to_string()));
        let result = check_deprecated_usage(&expr, "mypackage", &deprecations);
        assert!(result.is_none());
    }

    #[test]
    fn test_get_must_use_function() {
        // Test that get_must_use_function detects must_use functions
        let mut must_use = MustUseIndex::default();
        must_use.functions.insert(
            (
                "mypackage".to_string(),
                Some("Result".to_string()),
                "unwrap".to_string(),
            ),
            Some("consider handling the error case".to_string()),
        );

        // Create an expression Result.unwrap
        let callee = AS::Expr::Member(
            Box::new(AS::Expr::Ident(Ident("Result".to_string()))),
            Ident("unwrap".to_string()),
        );
        let result = get_must_use_function(&callee, "mypackage", &must_use);
        assert!(result.is_some());
        let (name, reason) = result.unwrap();
        assert_eq!(name, "Result.unwrap");
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("error case"));
    }

    #[test]
    fn test_get_must_use_function_no_match() {
        // Test that non-must_use functions don't trigger
        let must_use = MustUseIndex::default();
        let callee = AS::Expr::Member(
            Box::new(AS::Expr::Ident(Ident("Console".to_string()))),
            Ident("println".to_string()),
        );
        let result = get_must_use_function(&callee, "mypackage", &must_use);
        assert!(result.is_none());
    }
}
