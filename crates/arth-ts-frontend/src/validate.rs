//! TS subset validator.
//!
//! This module walks the SWC TS AST and enforces the restricted
//! subset described in `docs/ts-subset.md`. It is intentionally
//! conservative: only constructs that clearly fit the guest subset
//! (control flow, arrays/objects, simple unions/generics, and
//! sandbox restrictions) are accepted. Everything else is rejected
//! with `TsLoweringError::Unsupported`.
//!
//! ## Phase 8: Validation & Error Reporting
//!
//! This module also implements anti-pattern detection:
//! - `this.state` usage (should use provider parameters instead)
//! - Class-level field declarations without `@data` or `@provider` decorators
//! - Bare `null`/`undefined` types (should use `Optional<T>`)
//!
//! These checks provide helpful error messages with suggestions for
//! correct patterns.

use std::collections::HashSet;
use std::path::PathBuf;

use swc_ecma_ast as ast;

use crate::diagnostics::{Diagnostic, DiagnosticBag, DiagnosticCategory, SourceSpan};
use crate::lower::TsLoweringError;

/// Validation result containing errors and warnings.
#[derive(Debug, Default)]
pub struct ValidationResult {
    /// Collected diagnostics (errors and warnings).
    pub diagnostics: DiagnosticBag,
    /// Whether validation passed (no errors, warnings are ok).
    pub success: bool,
}

impl ValidationResult {
    /// Create a successful result with no diagnostics.
    pub fn success() -> Self {
        Self {
            diagnostics: DiagnosticBag::new(),
            success: true,
        }
    }

    /// Create a failed result from a diagnostics bag.
    pub fn failed(diagnostics: DiagnosticBag) -> Self {
        Self {
            diagnostics,
            success: false,
        }
    }
}

/// Validate that the given parsed TS module conforms to the Arth TS subset.
pub fn validate_ts_subset(module: &ast::Module) -> Result<(), TsLoweringError> {
    let bound_names = collect_module_value_bindings(module);
    let validator = Validator::new(bound_names, None);
    validator.validate_module(module)
}

/// Validate with detailed diagnostics including warnings.
///
/// This function performs the same validation as `validate_ts_subset` but also
/// collects warnings for anti-patterns and provides detailed diagnostics with
/// source locations and helpful suggestions.
pub fn validate_ts_subset_with_diagnostics(
    module: &ast::Module,
    file_path: Option<PathBuf>,
) -> ValidationResult {
    let bound_names = collect_module_value_bindings(module);
    let mut validator = Validator::new(bound_names, file_path);

    match validator.validate_module(module) {
        Ok(()) => {
            // Even on success, we may have warnings
            ValidationResult {
                diagnostics: validator.take_diagnostics(),
                success: true,
            }
        }
        Err(e) => {
            // Add the error to diagnostics and return failure
            validator.diagnostics.add(Diagnostic::error(
                DiagnosticCategory::Validation,
                e.to_string(),
            ));
            ValidationResult::failed(validator.take_diagnostics())
        }
    }
}

/// Collect value-level bindings (imports, vars, functions, classes) so that
/// we can distinguish ambient globals from local names.
fn collect_module_value_bindings(module: &ast::Module) -> HashSet<String> {
    let mut names = HashSet::new();

    for item in &module.body {
        match item {
            ast::ModuleItem::ModuleDecl(decl) => match decl {
                ast::ModuleDecl::Import(import) => {
                    for spec in &import.specifiers {
                        match spec {
                            ast::ImportSpecifier::Named(n) => {
                                names.insert(n.local.sym.to_string());
                            }
                            ast::ImportSpecifier::Default(d) => {
                                names.insert(d.local.sym.to_string());
                            }
                            ast::ImportSpecifier::Namespace(ns) => {
                                names.insert(ns.local.sym.to_string());
                            }
                        }
                    }
                }
                ast::ModuleDecl::ExportDecl(ed) => match &ed.decl {
                    ast::Decl::Fn(f) => {
                        names.insert(f.ident.sym.to_string());
                    }
                    ast::Decl::Class(c) => {
                        names.insert(c.ident.sym.to_string());
                    }
                    ast::Decl::Var(var) => collect_var_decl_bindings(var, &mut names),
                    _ => {}
                },
                ast::ModuleDecl::ExportDefaultDecl(ed) => match &ed.decl {
                    ast::DefaultDecl::Fn(f) => {
                        if let Some(id) = &f.ident {
                            names.insert(id.sym.to_string());
                        }
                    }
                    ast::DefaultDecl::Class(c) => {
                        if let Some(id) = &c.ident {
                            names.insert(id.sym.to_string());
                        }
                    }
                    ast::DefaultDecl::TsInterfaceDecl(_) => {}
                },
                _ => {}
            },
            ast::ModuleItem::Stmt(stmt) => collect_stmt_bindings(stmt, &mut names),
        }
    }

    names
}

fn collect_stmt_bindings(stmt: &ast::Stmt, names: &mut HashSet<String>) {
    if let ast::Stmt::Decl(decl) = stmt {
        match decl {
            ast::Decl::Fn(f) => {
                names.insert(f.ident.sym.to_string());
            }
            ast::Decl::Class(c) => {
                names.insert(c.ident.sym.to_string());
            }
            ast::Decl::Var(var) => collect_var_decl_bindings(var, names),
            _ => {}
        }
    }
}

fn collect_var_decl_bindings(var: &ast::VarDecl, names: &mut HashSet<String>) {
    for d in &var.decls {
        collect_pat_bindings(&d.name, names);
    }
}

fn collect_pat_bindings(pat: &ast::Pat, names: &mut HashSet<String>) {
    use ast::Pat::*;

    match pat {
        Ident(id) => {
            names.insert(id.id.sym.to_string());
        }
        Array(arr) => {
            for elem in arr.elems.iter().flatten() {
                collect_pat_bindings(elem, names);
            }
        }
        Object(obj) => {
            for prop in &obj.props {
                match prop {
                    ast::ObjectPatProp::KeyValue(kv) => {
                        collect_pat_bindings(&kv.value, names);
                    }
                    ast::ObjectPatProp::Assign(a) => {
                        names.insert(a.key.sym.to_string());
                    }
                    ast::ObjectPatProp::Rest(r) => {
                        collect_pat_bindings(&r.arg, names);
                    }
                }
            }
        }
        Assign(a) => collect_pat_bindings(&a.left, names),
        Rest(r) => collect_pat_bindings(&r.arg, names),
        Expr(_) => {}
        Invalid(_) => {}
    }
}

struct Validator {
    bound_value_names: HashSet<String>,
    /// Optional file path for generating source spans.
    #[allow(dead_code)] // Used by make_span for diagnostic location tracking
    file_path: Option<PathBuf>,
    /// Collected diagnostics (errors and warnings).
    diagnostics: DiagnosticBag,
}

impl Validator {
    fn new(bound_value_names: HashSet<String>, file_path: Option<PathBuf>) -> Self {
        Self {
            bound_value_names,
            file_path,
            diagnostics: DiagnosticBag::new(),
        }
    }

    /// Take the collected diagnostics, leaving an empty bag.
    fn take_diagnostics(&mut self) -> DiagnosticBag {
        std::mem::take(&mut self.diagnostics)
    }

    /// Create a source span from an SWC span.
    #[allow(dead_code)] // Infrastructure for future warning location tracking
    fn make_span(&self, span: swc_common::Span) -> Option<SourceSpan> {
        self.file_path
            .as_ref()
            .map(|path| SourceSpan::from_swc(span, path.clone()))
    }

    /// Add a warning diagnostic.
    #[allow(dead_code)] // Infrastructure for future warning support
    fn warn(&mut self, span: swc_common::Span, message: impl Into<String>) {
        let mut diag = Diagnostic::warning(DiagnosticCategory::Validation, message);
        if let Some(source_span) = self.make_span(span) {
            diag = diag.with_span(source_span);
        }
        self.diagnostics.add(diag);
    }

    /// Add a warning with help text.
    #[allow(dead_code)] // Infrastructure for future warning support
    fn warn_with_help(
        &mut self,
        span: swc_common::Span,
        message: impl Into<String>,
        help: impl Into<String>,
    ) {
        let mut diag = Diagnostic::warning(DiagnosticCategory::Validation, message).with_help(help);
        if let Some(source_span) = self.make_span(span) {
            diag = diag.with_span(source_span);
        }
        self.diagnostics.add(diag);
    }

    fn validate_param_type_ann(
        &self,
        param_name: &str,
        type_ann: &Option<Box<ast::TsTypeAnn>>,
    ) -> Result<(), TsLoweringError> {
        // WAID controllers commonly use `_ctx: any` for an unused framework context.
        // Allow `any`/`unknown` only for parameters that are explicitly "unused"
        // (convention: leading underscore), so that value-level `any` doesn't
        // silently leak into state fields / return types.
        if param_name.starts_with('_')
            && let Some(ann) = type_ann.as_ref()
            && let ast::TsType::TsKeywordType(kw) = &*ann.type_ann
        {
            use ast::TsKeywordTypeKind as K;
            if matches!(kw.kind, K::TsAnyKeyword | K::TsUnknownKeyword) {
                return Ok(());
            }
        }

        self.validate_ts_type_ann(type_ann)
    }

    fn validate_module(&self, module: &ast::Module) -> Result<(), TsLoweringError> {
        for item in &module.body {
            self.validate_module_item(item)?;
        }
        Ok(())
    }

    fn validate_module_item(&self, item: &ast::ModuleItem) -> Result<(), TsLoweringError> {
        match item {
            ast::ModuleItem::ModuleDecl(decl) => match decl {
                ast::ModuleDecl::Import(import) => self.validate_import_decl(import),
                ast::ModuleDecl::ExportDecl(ed) => self.validate_export_decl(ed),
                ast::ModuleDecl::ExportDefaultDecl(ed) => self.validate_export_default_decl(ed),
                // Other export forms (named/default re‑exports) can be added
                // later once the module surface is nailed down.
                _ => Err(TsLoweringError::Unsupported(
                    "only ES `import` and declaration-style `export` forms are supported at the top level in the Arth TS guest subset; wrap executable code inside exported functions such as `export function main() { ... }` (see docs/ts-subset.md §3.1).",
                )),
            },
            ast::ModuleItem::Stmt(stmt) => self.validate_top_level_stmt(stmt),
        }
    }

    fn validate_import_decl(&self, import: &ast::ImportDecl) -> Result<(), TsLoweringError> {
        if import.src.value.is_empty() {
            return Err(TsLoweringError::Unsupported(
                "import source must be a non-empty string literal",
            ));
        }
        let Some(src) = import.src.value.as_str() else {
            return Err(TsLoweringError::Unsupported(
                "import source must be valid UTF-8",
            ));
        };

        // Relative imports (`./`, `../`) are treated as local module wiring and
        // are allowed; they do not widen the host surface.
        if src.starts_with("./") || src.starts_with("../") {
            return Ok(());
        }

        // All non-relative imports are treated as host capabilities. For TS guest
        // modules we restrict these to a small `arth:` namespace so that the host
        // import surface stays explicit and auditable (see docs/ts-subset.md §6.1, §6.6).
        if let Some(rest) = src.strip_prefix("arth:") {
            if !is_allowed_arth_host_module(rest) {
                return Err(TsLoweringError::Unsupported(
                    "only selected `arth:` capability modules are allowed as host imports in the Arth TS guest subset; use `arth:log`, `arth:time`, `arth:math`, `arth:rand`, `arth:array`, `arth:map`, or `arth:option`, or switch to a relative import (`./`, `../`) for local code (see docs/ts-subset.md §6.6).",
                ));
            }
            return Ok(());
        }

        Err(TsLoweringError::Unsupported(
            "only relative imports (`./` or `../`) and `arth:*` capability modules are allowed as imports in the Arth TS guest subset; avoid Node/browser built-in modules and ambient host packages (see docs/ts-subset.md §6.6).",
        ))
    }

    fn validate_export_decl(&self, ed: &ast::ExportDecl) -> Result<(), TsLoweringError> {
        match &ed.decl {
            ast::Decl::Fn(f) => self.validate_fn_decl(f),
            ast::Decl::Class(c) => self.validate_class_decl(c),
            ast::Decl::Var(v) => self.validate_var_decl(v, true),
            ast::Decl::TsInterface(i) => self.validate_interface_decl(i),
            ast::Decl::TsTypeAlias(a) => self.validate_type_alias_decl(a),
            _ => Err(TsLoweringError::Unsupported(
                "only exported functions, classes, const/let, interfaces, and type aliases are supported",
            )),
        }
    }

    fn validate_export_default_decl(
        &self,
        ed: &ast::ExportDefaultDecl,
    ) -> Result<(), TsLoweringError> {
        match &ed.decl {
            // WAID-style controllers: `export default class Controller { ... }`
            ast::DefaultDecl::Class(c) => self.validate_class_expr(c),
            // Allow default-exported functions, but require an identifier so that
            // the lowering pipeline can name the emitted HIR function.
            ast::DefaultDecl::Fn(f) => {
                if f.ident.is_none() {
                    return Err(TsLoweringError::Unsupported(
                        "default-exported functions must be named in the Arth TS guest subset (e.g. `export default function main() { ... }`); anonymous default exports are not lowered",
                    ));
                }
                self.validate_function_expr(f)
            }
            ast::DefaultDecl::TsInterfaceDecl(i) => self.validate_interface_decl(i),
        }
    }

    fn validate_top_level_stmt(&self, stmt: &ast::Stmt) -> Result<(), TsLoweringError> {
        match stmt {
            ast::Stmt::Decl(decl) => self.validate_decl(decl, true),
            _ => Err(TsLoweringError::Unsupported(
                "top-level statements must be declarations in the Arth TS guest subset; move executable code into exported functions (for example `export function main() { ... }`) and call it from the host (see docs/ts-subset.md §3.1).",
            )),
        }
    }

    fn validate_decl(&self, decl: &ast::Decl, is_top_level: bool) -> Result<(), TsLoweringError> {
        match decl {
            ast::Decl::Fn(f) => self.validate_fn_decl(f),
            ast::Decl::Class(c) => self.validate_class_decl(c),
            ast::Decl::Var(v) => self.validate_var_decl(v, is_top_level),
            ast::Decl::TsInterface(i) => self.validate_interface_decl(i),
            ast::Decl::TsTypeAlias(a) => self.validate_type_alias_decl(a),
            _ => Err(TsLoweringError::Unsupported(
                "declaration kind is not supported in TS guest subset",
            )),
        }
    }

    fn validate_fn_decl(&self, f: &ast::FnDecl) -> Result<(), TsLoweringError> {
        if f.function.is_generator {
            return Err(TsLoweringError::Unsupported(
                "generators (`function*` / `yield`) are not part of the Arth TS guest subset; rewrite to use plain async/await or explicit state machines instead (see docs/ts-subset.md §3.4).",
            ));
        }

        if let Some(type_params) = &f.function.type_params {
            self.validate_type_params(type_params)?;
        }

        for param in &f.function.params {
            match &param.pat {
                ast::Pat::Ident(id) => {
                    self.validate_param_type_ann(id.id.sym.as_ref(), &id.type_ann)?;
                }
                ast::Pat::Assign(assign) => {
                    // Default parameter: param: Type = value
                    if let ast::Pat::Ident(id) = assign.left.as_ref() {
                        self.validate_param_type_ann(id.id.sym.as_ref(), &id.type_ann)?;
                    } else {
                        return Err(TsLoweringError::Unsupported(
                            "destructured default parameters are not supported; use simple identifier with default value instead.",
                        ));
                    }
                }
                ast::Pat::Rest(rest) => {
                    // Rest parameter: ...args: Type[]
                    if let ast::Pat::Ident(id) = rest.arg.as_ref() {
                        self.validate_param_type_ann(id.id.sym.as_ref(), &id.type_ann)?;
                    } else {
                        return Err(TsLoweringError::Unsupported(
                            "destructured rest parameters are not supported; use simple identifier for rest parameter.",
                        ));
                    }
                }
                _ => {
                    return Err(TsLoweringError::Unsupported(
                        "only simple identifier, default, and rest parameters are allowed in the Arth TS guest subset; destructure inside the function body instead of in the parameter list (see docs/ts-subset.md §3.3).",
                    ));
                }
            }
        }

        self.validate_ts_type_ann(&f.function.return_type)?;

        if let Some(body) = &f.function.body {
            self.validate_block_stmt(body)?;
        }

        Ok(())
    }

    fn validate_class_decl(&self, c: &ast::ClassDecl) -> Result<(), TsLoweringError> {
        let class = &c.class;

        if class.super_class.is_some() {
            return Err(TsLoweringError::Unsupported(
                "class inheritance (`extends`) is outside the current Arth TS guest subset; flatten hierarchies into standalone classes or use composition instead (see docs/ts-subset.md §3.4–§3.5).",
            ));
        }

        // `implements` clauses are allowed - regular classes can implement interfaces
        // and the implements list is preserved in the HirModule.

        // Allow only @provider and @data decorators on classes.
        // @provider → HirProvider (provider declaration)
        // @data → HirStruct (struct declaration)
        for dec in &class.decorators {
            if !is_allowed_class_decorator(&dec.expr) {
                return Err(TsLoweringError::Unsupported(
                    "only @provider and @data class decorators are supported in the Arth TS guest subset; other decorators are not allowed (see docs/ts-subset.md §3.2).",
                ));
            }
        }

        // Phase 8: Check if class has fields but no @data or @provider decorator.
        // Regular classes map to modules (behavior only), which cannot have instance fields.
        let is_data_or_provider = has_data_or_provider_decorator(class);
        if !is_data_or_provider && has_instance_fields(class) {
            return Err(TsLoweringError::Unsupported(
                "class-level fields are not allowed in controller classes in the Arth TS guest subset; classes without @data or @provider decorators map to Arth modules, which have no instance state. Use one of:\n\n  1. `@data class` for data-only structs:\n     @data\n     class User { name: string; email: string; }\n\n  2. `@provider class` for shared state:\n     @provider\n     class State { count: number; }\n\n  3. Provider parameters for controller state:\n     @provider\n     interface State { count: number; }\n     export class Controller {\n       increment(state: State) { state.count++; }\n     }\n\n(see docs/arth-ts-controller-spec.md §Anti-Patterns)",
            ));
        }

        if let Some(type_params) = &class.type_params {
            self.validate_type_params(type_params)?;
        }

        let mut seen_ctor = false;

        for member in &class.body {
            match member {
                ast::ClassMember::ClassProp(prop) => {
                    if prop.is_static {
                        return Err(TsLoweringError::Unsupported(
                            "static class fields are not yet supported in the Arth TS guest subset; encode static data in a separate module or helper function instead (see docs/ts-subset.md §3.5).",
                        ));
                    }
                    match &prop.key {
                        ast::PropName::Ident(id) => {
                            if is_forbidden_prototype_property(id.sym.as_ref()) {
                                return Err(TsLoweringError::Unsupported(
                                    "class fields named `prototype` / `__proto__` are forbidden in the Arth TS guest subset to prevent prototype manipulation; choose a different field name (see docs/ts-subset.md §4).",
                                ));
                            }
                        }
                        ast::PropName::Str(s) => {
                            if s.value == "prototype" || s.value == "__proto__" {
                                return Err(TsLoweringError::Unsupported(
                                    "class fields named `prototype` / `__proto__` are forbidden in the Arth TS guest subset to prevent prototype manipulation; choose a different field name (see docs/ts-subset.md §4).",
                                ));
                            }
                        }
                        _ => {
                            return Err(TsLoweringError::Unsupported(
                                "only identifier and string-named class fields are supported in the Arth TS guest subset; computed or symbol keys are not allowed (see docs/ts-subset.md §3.5).",
                            ));
                        }
                    }
                    self.validate_ts_type_ann(&prop.type_ann)?;
                }
                ast::ClassMember::Constructor(cons) => {
                    use swc_ecma_ast::ParamOrTsParamProp;

                    if seen_ctor {
                        return Err(TsLoweringError::Unsupported(
                            "multiple constructors are not supported",
                        ));
                    }
                    seen_ctor = true;

                    for p in &cons.params {
                        match p {
                            ParamOrTsParamProp::Param(param) => {
                                if let ast::Pat::Ident(id) = &param.pat {
                                    self.validate_param_type_ann(id.id.sym.as_ref(), &id.type_ann)?;
                                } else {
                                    return Err(TsLoweringError::Unsupported(
                                        "constructor parameters must be simple identifiers in the Arth TS guest subset; destructure inside the body instead of in the parameter list (see docs/ts-subset.md §3.3–§3.4).",
                                    ));
                                }
                            }
                            ParamOrTsParamProp::TsParamProp(_) => {
                                return Err(TsLoweringError::Unsupported(
                                    "constructor parameter properties (e.g. `constructor(public x: T)`) are not yet supported in the Arth TS guest subset; declare fields explicitly on the class instead (see docs/ts-subset.md §3.4).",
                                ));
                            }
                        }
                    }

                    if let Some(body) = &cons.body {
                        self.validate_block_stmt(body)?;
                    }
                }
                ast::ClassMember::Method(method) => {
                    use swc_ecma_ast::MethodKind;

                    if method.kind != MethodKind::Method {
                        return Err(TsLoweringError::Unsupported(
                            "getters/setters are not yet supported in the Arth TS guest subset; use explicit methods (e.g. `getFoo()` / `setFoo(...)`) instead (see docs/ts-subset.md §3.5).",
                        ));
                    }
                    if method.is_abstract {
                        return Err(TsLoweringError::Unsupported(
                            "abstract methods are not yet supported in the Arth TS guest subset; model behavior via interfaces and concrete implementations instead (see docs/ts-subset.md §3.2–§3.4).",
                        ));
                    }

                    if let Some(type_params) = &method.function.type_params {
                        self.validate_type_params(type_params)?;
                    }

                    for p in &method.function.params {
                        match &p.pat {
                            ast::Pat::Ident(id) => {
                                self.validate_param_type_ann(id.id.sym.as_ref(), &id.type_ann)?;
                            }
                            ast::Pat::Assign(assign) => {
                                // Default parameter: param: Type = value
                                if let ast::Pat::Ident(id) = assign.left.as_ref() {
                                    self.validate_param_type_ann(id.id.sym.as_ref(), &id.type_ann)?;
                                } else {
                                    return Err(TsLoweringError::Unsupported(
                                        "destructured default parameters are not supported in method signatures; use simple identifier with default value instead.",
                                    ));
                                }
                            }
                            ast::Pat::Rest(rest) => {
                                // Rest parameter: ...args: Type[]
                                if let ast::Pat::Ident(id) = rest.arg.as_ref() {
                                    self.validate_param_type_ann(id.id.sym.as_ref(), &id.type_ann)?;
                                } else {
                                    return Err(TsLoweringError::Unsupported(
                                        "destructured rest parameters are not supported; use simple identifier for rest parameter.",
                                    ));
                                }
                            }
                            _ => {
                                return Err(TsLoweringError::Unsupported(
                                    "method parameters must be simple identifiers, default values, or rest parameters in the Arth TS guest subset; destructure inside the method body instead (see docs/ts-subset.md §3.3).",
                                ));
                            }
                        }
                    }

                    self.validate_ts_type_ann(&method.function.return_type)?;

                    if let Some(body) = &method.function.body {
                        self.validate_block_stmt(body)?;
                    }
                }
                _ => {
                    return Err(TsLoweringError::Unsupported(
                        "this class member form is not part of the Arth TS guest subset; restrict classes to fields, a single constructor, and simple methods (see docs/ts-subset.md §3.4–§3.5).",
                    ));
                }
            }
        }

        Ok(())
    }

    fn validate_interface_decl(&self, iface: &ast::TsInterfaceDecl) -> Result<(), TsLoweringError> {
        if let Some(type_params) = &iface.type_params {
            self.validate_type_params(type_params)?;
        }

        for ext in &iface.extends {
            if let Some(type_args) = &ext.type_args {
                for t in &type_args.params {
                    self.validate_ts_type(t)?;
                }
            }
        }

        for m in &iface.body.body {
            match m {
                ast::TsTypeElement::TsPropertySignature(prop) => {
                    match &*prop.key {
                        ast::Expr::Ident(id) => {
                            if is_forbidden_prototype_property(id.sym.as_ref()) {
                                return Err(TsLoweringError::Unsupported(
                                    "interface fields named `prototype` / `__proto__` are forbidden in the Arth TS guest subset to prevent prototype manipulation; choose a different field name (see docs/ts-subset.md §4).",
                                ));
                            }
                        }
                        ast::Expr::Lit(ast::Lit::Str(s)) => {
                            if s.value == "prototype" || s.value == "__proto__" {
                                return Err(TsLoweringError::Unsupported(
                                    "interface fields named `prototype` / `__proto__` are forbidden in the Arth TS guest subset to prevent prototype manipulation; choose a different field name (see docs/ts-subset.md §4).",
                                ));
                            }
                        }
                        _ => {
                            return Err(TsLoweringError::Unsupported(
                                "interface field keys must be identifiers or string literals in the Arth TS guest subset; index signatures and computed keys are not yet supported (see docs/ts-subset.md §3.2).",
                            ));
                        }
                    }

                    if let Some(type_ann) = &prop.type_ann {
                        self.validate_ts_type(&type_ann.type_ann)?;
                    }
                }
                ast::TsTypeElement::TsMethodSignature(method) => {
                    // Method signatures are allowed in behavior contract interfaces.
                    // Validate method name is an identifier
                    match method.key.as_ref() {
                        ast::Expr::Ident(_) => {}
                        _ => {
                            return Err(TsLoweringError::Unsupported(
                                "interface method keys must be identifiers in the Arth TS guest subset",
                            ));
                        }
                    }

                    // Validate parameter types
                    for param in &method.params {
                        if let ast::TsFnParam::Ident(id) = param
                            && let Some(type_ann) = &id.type_ann
                        {
                            self.validate_ts_type(&type_ann.type_ann)?;
                        }
                    }

                    // Validate return type
                    if let Some(type_ann) = &method.type_ann {
                        self.validate_ts_type(&type_ann.type_ann)?;
                    }
                }
                _ => {
                    return Err(TsLoweringError::Unsupported(
                        "only property and method signatures are allowed in interfaces in the Arth TS guest subset; index signatures and call signatures are not yet supported (see docs/ts-subset.md §3.2).",
                    ));
                }
            }
        }

        Ok(())
    }

    fn validate_type_alias_decl(
        &self,
        alias: &ast::TsTypeAliasDecl,
    ) -> Result<(), TsLoweringError> {
        if let Some(type_params) = &alias.type_params {
            self.validate_type_params(type_params)?;
        }
        self.validate_ts_type(&alias.type_ann)
    }

    fn validate_type_params(&self, params: &ast::TsTypeParamDecl) -> Result<(), TsLoweringError> {
        for p in &params.params {
            if let Some(constraint) = &p.constraint {
                self.validate_ts_type(constraint)?;
            }
            if let Some(default) = &p.default {
                self.validate_ts_type(default)?;
            }
        }
        Ok(())
    }

    fn validate_block_stmt(&self, block: &ast::BlockStmt) -> Result<(), TsLoweringError> {
        for stmt in &block.stmts {
            self.validate_stmt(stmt)?;
        }
        Ok(())
    }

    fn validate_stmt(&self, stmt: &ast::Stmt) -> Result<(), TsLoweringError> {
        use ast::Stmt::*;

        match stmt {
            Return(ret) => {
                if let Some(arg) = &ret.arg {
                    self.validate_expr(arg)?;
                }
                Ok(())
            }
            Expr(expr_stmt) => self.validate_expr(&expr_stmt.expr),
            Decl(decl) => self.validate_decl(decl, false),
            If(if_stmt) => {
                self.validate_expr(&if_stmt.test)?;
                self.validate_stmt(&if_stmt.cons)?;
                if let Some(alt) = &if_stmt.alt {
                    self.validate_stmt(alt)?;
                }
                Ok(())
            }
            While(while_stmt) => {
                self.validate_expr(&while_stmt.test)?;
                self.validate_stmt(&while_stmt.body)
            }
            DoWhile(do_while) => {
                self.validate_stmt(&do_while.body)?;
                self.validate_expr(&do_while.test)?;
                Ok(())
            }
            For(for_stmt) => self.validate_for_stmt(for_stmt),
            ForIn(_) => Err(TsLoweringError::Unsupported(
                "`for..in` over objects is not allowed in the Arth TS guest subset; use `for (x of arr)` over arrays or an indexed `for (let i = 0; i < arr.length; i++)` loop instead (see docs/ts-subset.md §3.3).",
            )),
            ForOf(for_of) => self.validate_for_of_stmt(for_of),
            Switch(sw) => self.validate_switch_stmt(sw),
            Throw(_) | Try(_) => Err(TsLoweringError::Unsupported(
                "exceptions (`throw` / `try` / `catch` / `finally`) are not allowed in the Arth TS guest subset; model failure with `Result`/`Optional`-style return values instead (see docs/ts-subset.md §3.7).",
            )),
            Break(b) => {
                if b.label.is_some() {
                    return Err(TsLoweringError::Unsupported(
                        "labeled `break` is not allowed",
                    ));
                }
                Ok(())
            }
            Continue(c) => {
                if c.label.is_some() {
                    return Err(TsLoweringError::Unsupported(
                        "labeled `continue` is not allowed",
                    ));
                }
                Ok(())
            }
            Labeled(_) => Err(TsLoweringError::Unsupported(
                "labeled statements are not allowed in the Arth TS guest subset; rely on structured loops and early `break`/`return` instead (see docs/ts-subset.md §3.3).",
            )),
            Block(block) => self.validate_block_stmt(block),
            Empty(_) => Ok(()),
            Debugger(_) => Err(TsLoweringError::Unsupported(
                "`debugger` statements are not allowed in the Arth TS guest subset; use host-side debugging tools or explicit logging instead.",
            )),
            With(_) => Err(TsLoweringError::Unsupported(
                "`with` blocks are not allowed in the Arth TS guest subset; access fields explicitly via `obj.field` (see docs/ts-subset.md §3.3).",
            )),
        }
    }

    fn validate_for_stmt(&self, for_stmt: &ast::ForStmt) -> Result<(), TsLoweringError> {
        if let Some(init) = &for_stmt.init {
            match init {
                ast::VarDeclOrExpr::VarDecl(var) => self.validate_var_decl(var, false)?,
                ast::VarDeclOrExpr::Expr(expr) => self.validate_expr(expr)?,
            }
        }

        if let Some(test) = &for_stmt.test {
            self.validate_expr(test)?;
        }

        if let Some(update) = &for_stmt.update {
            self.validate_expr(update)?;
        }

        self.validate_stmt(&for_stmt.body)
    }

    fn validate_for_of_stmt(&self, for_of: &ast::ForOfStmt) -> Result<(), TsLoweringError> {
        if for_of.is_await {
            return Err(TsLoweringError::Unsupported(
                "`for await..of` is not supported",
            ));
        }

        match &for_of.left {
            ast::ForHead::VarDecl(var) => self.validate_var_decl(var, false)?,
            ast::ForHead::Pat(pat) => {
                if matches!(pat.as_ref(), ast::Pat::Invalid(_)) {
                    return Err(TsLoweringError::Unsupported(
                        "invalid pattern in `for..of` is not allowed",
                    ));
                }
            }
            ast::ForHead::UsingDecl(_) => {
                return Err(TsLoweringError::Unsupported(
                    "`using` declarations are not supported in the Arth TS guest subset.",
                ));
            }
        }

        self.validate_expr(&for_of.right)?;
        self.validate_stmt(&for_of.body)
    }

    fn validate_switch_stmt(&self, sw: &ast::SwitchStmt) -> Result<(), TsLoweringError> {
        self.validate_expr(&sw.discriminant)?;

        for case in &sw.cases {
            if let Some(test) = &case.test {
                match &**test {
                    ast::Expr::Lit(ast::Lit::Str(_)) | ast::Expr::Lit(ast::Lit::Num(_)) => {}
                    _ => {
                        return Err(TsLoweringError::Unsupported(
                            "switch cases must match on string or numeric literals",
                        ));
                    }
                }
            }

            for stmt in &case.cons {
                self.validate_stmt(stmt)?;
            }
        }

        Ok(())
    }

    fn validate_var_decl(
        &self,
        var_decl: &ast::VarDecl,
        is_top_level: bool,
    ) -> Result<(), TsLoweringError> {
        use swc_ecma_ast::VarDeclKind;

        if matches!(var_decl.kind, VarDeclKind::Var) {
            return Err(TsLoweringError::Unsupported(
                "`var` declarations are not allowed in the Arth TS guest subset; use block-scoped `let` or `const` instead (see docs/ts-subset.md §3.3).",
            ));
        }

        for decl in &var_decl.decls {
            if let Some(init) = &decl.init {
                self.validate_expr(init)?;
            } else if is_top_level {
                return Err(TsLoweringError::Unsupported(
                    "top-level `const`/`let` declarations must have an initializer in the Arth TS guest subset; give every exported or module-scope binding an explicit value (see docs/ts-subset.md §3.1).",
                ));
            }
        }

        Ok(())
    }

    fn validate_expr(&self, expr: &ast::Expr) -> Result<(), TsLoweringError> {
        use ast::Expr::*;

        match expr {
            Lit(l) => self.validate_lit(l),
            Ident(id) => self.validate_ident_expr(id),
            Bin(b) => self.validate_binary_expr(b),
            Call(c) => self.validate_call_expr(c),
            New(n) => self.validate_new_expr(n),
            Array(arr) => self.validate_array_lit(arr),
            Object(obj) => self.validate_object_lit(obj),
            Member(m) => self.validate_member_expr(m),
            Assign(a) => self.validate_assign_expr(a),
            Unary(u) => self.validate_unary_expr(u),
            Update(u) => {
                self.validate_expr(&u.arg)?;
                Ok(())
            }
            Cond(c) => {
                self.validate_expr(&c.test)?;
                self.validate_expr(&c.cons)?;
                self.validate_expr(&c.alt)?;
                Ok(())
            }
            Seq(seq) => {
                for e in &seq.exprs {
                    self.validate_expr(e)?;
                }
                Ok(())
            }
            Paren(p) => self.validate_expr(&p.expr),
            Arrow(a) => self.validate_arrow_expr(a),
            Fn(f) => self.validate_function_expr(f),
            Class(c) => self.validate_class_expr(c),
            This(_) => Ok(()),
            Await(a) => self.validate_expr(&a.arg),
            TsAs(a) => {
                self.validate_expr(&a.expr)?;
                self.validate_ts_type(&a.type_ann)?;
                Ok(())
            }
            TsConstAssertion(a) => {
                self.validate_expr(&a.expr)?;
                Ok(())
            }
            TsNonNull(nn) => self.validate_expr(&nn.expr),
            TsTypeAssertion(a) => {
                self.validate_expr(&a.expr)?;
                self.validate_ts_type(&a.type_ann)?;
                Ok(())
            }
            TsInstantiation(inst) => {
                self.validate_expr(&inst.expr)?;
                for t in &inst.type_args.params {
                    self.validate_ts_type(t)?;
                }
                Ok(())
            }
            // Template literals are allowed (desugared to string concatenation)
            Tpl(tpl) => {
                for expr in &tpl.exprs {
                    self.validate_expr(expr)?;
                }
                Ok(())
            }
            OptChain(_) | TaggedTpl(_) | Yield(_) | MetaProp(_) | SuperProp(_) => Err(
                TsLoweringError::Unsupported("expression kind is outside TS guest subset"),
            ),
            _ => Err(TsLoweringError::Unsupported(
                "expression kind is outside TS guest subset",
            )),
        }
    }

    fn validate_lit(&self, lit: &ast::Lit) -> Result<(), TsLoweringError> {
        match lit {
            // null is allowed (desugared to Optional.none())
            ast::Lit::Num(_) | ast::Lit::Bool(_) | ast::Lit::Str(_) | ast::Lit::Null(_) => Ok(()),
            ast::Lit::BigInt(_) => Err(TsLoweringError::Unsupported(
                "`bigint` literals are not yet supported in the Arth TS guest subset; use `number` and explicit range checks instead (see docs/ts-subset.md §3.2).",
            )),
            ast::Lit::Regex(_) => Err(TsLoweringError::Unsupported(
                "regular expression literals are not yet supported in the Arth TS guest subset; implement parsing with plain string/array APIs instead.",
            )),
            ast::Lit::JSXText(_) => Err(TsLoweringError::Unsupported(
                "JSX literals are not supported in the Arth TS guest subset; keep guest code JSX-free and render via host or higher-level APIs instead.",
            )),
        }
    }

    fn validate_ident_expr(&self, id: &ast::Ident) -> Result<(), TsLoweringError> {
        let name = id.sym.as_ref();

        if self.is_ambient_global(name) {
            return Err(TsLoweringError::Unsupported(
                "direct use of ambient host globals (`window`, `document`, `process`, `global`, `globalThis`) is not allowed; pass capabilities via imports instead",
            ));
        }

        Ok(())
    }

    fn validate_binary_expr(&self, b: &ast::BinExpr) -> Result<(), TsLoweringError> {
        use swc_ecma_ast::BinaryOp as B;

        self.validate_expr(&b.left)?;
        self.validate_expr(&b.right)?;

        match b.op {
            B::Add
            | B::Sub
            | B::Mul
            | B::Div
            | B::Mod
            | B::Lt
            | B::LtEq
            | B::Gt
            | B::GtEq
            | B::EqEq
            | B::EqEqEq
            | B::NotEq
            | B::NotEqEq
            | B::LogicalAnd
            | B::LogicalOr
            | B::BitAnd
            | B::BitOr
            | B::BitXor
            | B::LShift
            | B::RShift
            | B::ZeroFillRShift => Ok(()),
            B::In => Err(TsLoweringError::Unsupported(
                "the `in` operator on objects is not allowed in the Arth TS guest subset; prefer explicit property checks on fixed-shape records instead (see docs/ts-subset.md §3.5).",
            )),
            B::InstanceOf => Err(TsLoweringError::Unsupported(
                "`instanceof` against host classes is not allowed in the Arth TS guest subset; encode variants as tagged unions and use `switch`/field checks instead (see docs/ts-subset.md §3.2–§3.3).",
            )),
            B::NullishCoalescing => Err(TsLoweringError::Unsupported(
                "nullish coalescing (`??`) is not yet supported in the Arth TS guest subset; use explicit checks or conditional expressions instead (see docs/ts-subset.md §3.3).",
            )),
            B::Exp => Err(TsLoweringError::Unsupported(
                "exponentiation (`**`) is not yet supported in the Arth TS guest subset; use `arth:math.pow` or repeated multiplication instead (see docs/ts-subset.md §3.3).",
            )),
        }
    }

    fn validate_call_expr(&self, c: &ast::CallExpr) -> Result<(), TsLoweringError> {
        match &c.callee {
            ast::Callee::Expr(inner) => {
                self.check_forbidden_callee(inner)?;
                self.validate_expr(inner)?;
            }
            ast::Callee::Super(_) => {
                return Err(TsLoweringError::Unsupported(
                    "`super` calls are not supported",
                ));
            }
            ast::Callee::Import(_) => {
                return Err(TsLoweringError::Unsupported(
                    "dynamic `import(...)` is not allowed; use static imports",
                ));
            }
        }

        if let Some(type_args) = &c.type_args {
            for t in &type_args.params {
                self.validate_ts_type(t)?;
            }
        }

        for arg in &c.args {
            if arg.spread.is_some() {
                return Err(TsLoweringError::Unsupported(
                    "spread arguments (`...args`) are not yet supported",
                ));
            }
            self.validate_expr(&arg.expr)?;
        }

        Ok(())
    }

    fn check_forbidden_callee(&self, expr: &ast::Expr) -> Result<(), TsLoweringError> {
        if let ast::Expr::Ident(id) = expr {
            let name = id.sym.as_ref();
            if name == "eval" && !self.bound_value_names.contains(name) {
                return Err(TsLoweringError::Unsupported(
                    "dynamic code execution via `eval` is forbidden in the Arth TS guest subset; move dynamic behavior into explicit data structures and deterministic interpreters instead (see docs/ts-subset.md §4).",
                ));
            }
        }

        if let ast::Expr::Member(m) = expr
            && let Some((obj, prop)) = member_static_names(m)
        {
            match (obj, prop) {
                ("Object", "defineProperty") => {
                    return Err(TsLoweringError::Unsupported(
                        "`Object.defineProperty` is forbidden in the Arth TS guest subset; construct plain objects with fixed fields instead (see docs/ts-subset.md §3.5, §4).",
                    ));
                }
                ("Object", "setPrototypeOf") => {
                    return Err(TsLoweringError::Unsupported(
                        "`Object.setPrototypeOf` is forbidden in the Arth TS guest subset; prototype mutation breaks sandboxing guarantees (see docs/ts-subset.md §4).",
                    ));
                }
                ("Object", "getPrototypeOf") => {
                    return Err(TsLoweringError::Unsupported(
                        "`Object.getPrototypeOf` is forbidden in the Arth TS guest subset; avoid runtime prototype inspection and rely on tagged unions/interfaces instead (see docs/ts-subset.md §4).",
                    ));
                }
                ("Reflect", _) => {
                    return Err(TsLoweringError::Unsupported(
                        "`Reflect` APIs are not allowed in the Arth TS guest subset; they imply a mutable runtime type system that breaks sandboxing guarantees (see docs/ts-subset.md §4).",
                    ));
                }
                (_, "call") | (_, "apply") | (_, "bind") => {
                    return Err(TsLoweringError::Unsupported(
                        "dynamic `this` patterns via `.call` / `.apply` / `.bind` are not allowed in the Arth TS guest subset; use plain functions or captured variables instead (see docs/ts-subset.md §3.4).",
                    ));
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn validate_new_expr(&self, n: &ast::NewExpr) -> Result<(), TsLoweringError> {
        if let ast::Expr::Ident(id) = &*n.callee {
            let name = id.sym.as_ref();
            if name == "Function" && !self.bound_value_names.contains(name) {
                return Err(TsLoweringError::Unsupported(
                    "dynamic code execution via `new Function(...)` is forbidden in the Arth TS guest subset; construct behavior from explicit data and pure functions instead (see docs/ts-subset.md §4).",
                ));
            }
        }

        if let Some(type_args) = &n.type_args {
            for t in &type_args.params {
                self.validate_ts_type(t)?;
            }
        }

        if let Some(args) = &n.args {
            for arg in args {
                if arg.spread.is_some() {
                    return Err(TsLoweringError::Unsupported(
                        "spread arguments are not yet supported in `new` expressions for the Arth TS guest subset; expand them explicitly before the `new` call.",
                    ));
                }
                self.validate_expr(&arg.expr)?;
            }
        }

        Ok(())
    }

    fn validate_array_lit(&self, arr: &ast::ArrayLit) -> Result<(), TsLoweringError> {
        for elem in &arr.elems {
            let Some(elem) = elem else {
                return Err(TsLoweringError::Unsupported(
                    "array holes (e.g. `[1,,2]`) are not allowed in the Arth TS guest subset; insert explicit placeholder values instead (see docs/ts-subset.md §3.5).",
                ));
            };

            if elem.spread.is_some() {
                return Err(TsLoweringError::Unsupported(
                    "array spread (`[...xs]`) is not yet supported in the Arth TS guest subset; build arrays via concatenation or explicit loops instead (see docs/ts-subset.md §3.5).",
                ));
            }

            self.validate_expr(&elem.expr)?;
        }

        Ok(())
    }

    fn validate_object_lit(&self, obj: &ast::ObjectLit) -> Result<(), TsLoweringError> {
        for prop in &obj.props {
            match prop {
                ast::PropOrSpread::Prop(p) => {
                    let p = &**p;
                    match p {
                        ast::Prop::KeyValue(kv) => {
                            match &kv.key {
                                ast::PropName::Ident(id) => {
                                    if is_forbidden_prototype_property(id.sym.as_ref()) {
                                        return Err(TsLoweringError::Unsupported(
                                            "object literal keys `prototype` / `__proto__` are forbidden in the Arth TS guest subset to prevent prototype manipulation; choose a different field name (see docs/ts-subset.md §4).",
                                        ));
                                    }
                                }
                                ast::PropName::Str(s) => {
                                    if s.value == "prototype" || s.value == "__proto__" {
                                        return Err(TsLoweringError::Unsupported(
                                            "object literal keys `prototype` / `__proto__` are forbidden in the Arth TS guest subset to prevent prototype manipulation; choose a different field name (see docs/ts-subset.md §4).",
                                        ));
                                    }
                                }
                                _ => {
                                    return Err(TsLoweringError::Unsupported(
                                        "object literal keys must be identifiers or string literals in the Arth TS guest subset; computed and symbol keys are not yet supported (see docs/ts-subset.md §3.5).",
                                    ));
                                }
                            }

                            self.validate_expr(&kv.value)?;
                        }
                        ast::Prop::Shorthand(id) => {
                            if is_forbidden_prototype_property(id.sym.as_ref()) {
                                return Err(TsLoweringError::Unsupported(
                                    "object literal keys `prototype` / `__proto__` are forbidden in the Arth TS guest subset to prevent prototype manipulation; choose a different field name (see docs/ts-subset.md §4).",
                                ));
                            }
                            self.validate_ident_expr(id)?;
                        }
                        ast::Prop::Method(_)
                        | ast::Prop::Getter(_)
                        | ast::Prop::Setter(_)
                        | ast::Prop::Assign(_) => {
                            return Err(TsLoweringError::Unsupported(
                                "object literal methods/getters/setters/assign props are not allowed in the Arth TS guest subset; use `key: () => { ... }` with plain functions instead (see docs/ts-subset.md §3.5).",
                            ));
                        }
                    }
                }
                ast::PropOrSpread::Spread(_) => {
                    return Err(TsLoweringError::Unsupported(
                        "object spread (`{ ...other }`) is not yet supported in the Arth TS guest subset; copy fields explicitly or via helper functions instead (see docs/ts-subset.md §3.5).",
                    ));
                }
            }
        }

        Ok(())
    }

    fn validate_member_expr(&self, m: &ast::MemberExpr) -> Result<(), TsLoweringError> {
        // Phase 8: Detect `this.state` anti-pattern
        // This is a common mistake where developers try to store state on `this`
        // instead of using provider parameters.
        if is_this_state_access(m) {
            return Err(TsLoweringError::Unsupported(
                "`this.state` is not allowed in the Arth TS guest subset; use a `@provider` decorated interface and pass state as a function parameter instead. Example:\n\n  @provider\n  interface State { count: number; }\n\n  export class Controller {\n    increment(state: State) {\n      state.count++;\n    }\n  }\n\n(see docs/arth-ts-controller-spec.md §Controller Pattern)",
            ));
        }

        // Phase 8: Detect `this.<field>` anti-pattern for class fields
        // Controllers should not store state on `this`
        if is_this_field_access(m) {
            return Err(TsLoweringError::Unsupported(
                "`this.<field>` access is not allowed in controller classes in the Arth TS guest subset; classes map to Arth modules which have no instance fields. Use a `@provider` interface for shared state, or pass data as function parameters. (see docs/arth-ts-controller-spec.md §Anti-Patterns)",
            ));
        }

        self.validate_expr(&m.obj)?;

        use swc_ecma_ast::MemberProp;

        match &m.prop {
            MemberProp::Ident(id) => {
                if is_forbidden_prototype_property(id.sym.as_ref()) {
                    return Err(TsLoweringError::Unsupported(
                        "prototype manipulation (`obj.prototype` / `obj.__proto__`) is forbidden in the Arth TS guest subset; encode behavior via plain objects and tagged unions instead (see docs/ts-subset.md §3.5, §4).",
                    ));
                }
            }
            MemberProp::Computed(c) => {
                if let ast::Expr::Lit(ast::Lit::Str(s)) = &*c.expr
                    && (s.value == "prototype" || s.value == "__proto__")
                {
                    return Err(TsLoweringError::Unsupported(
                        "prototype manipulation (`obj[\"prototype\"]` / `obj[\"__proto__\"]`) is forbidden in the Arth TS guest subset; encode behavior via plain objects and tagged unions instead (see docs/ts-subset.md §3.5, §4).",
                    ));
                }
                self.validate_expr(&c.expr)?;
            }
            MemberProp::PrivateName(_) => {
                return Err(TsLoweringError::Unsupported(
                    "private fields are not yet supported in the Arth TS guest subset; use public fields and naming conventions instead (see docs/ts-subset.md §3.5).",
                ));
            }
        }

        Ok(())
    }

    fn validate_assign_expr(&self, a: &ast::AssignExpr) -> Result<(), TsLoweringError> {
        use swc_ecma_ast::AssignOp as A;

        self.validate_assign_target(&a.left)?;
        self.validate_expr(&a.right)?;

        match a.op {
            A::Assign
            | A::AddAssign
            | A::SubAssign
            | A::MulAssign
            | A::DivAssign
            | A::ModAssign
            | A::BitOrAssign
            | A::BitAndAssign
            | A::BitXorAssign
            | A::LShiftAssign
            | A::RShiftAssign
            | A::ZeroFillRShiftAssign => Ok(()),
            _ => Err(TsLoweringError::Unsupported(
                "logical assignment operators (`&&=`, `||=`, `??=`) are not yet supported in the Arth TS guest subset; rewrite as explicit `if`/`?:` expressions (see docs/ts-subset.md §3.3).",
            )),
        }
    }

    fn validate_assign_target(&self, target: &ast::AssignTarget) -> Result<(), TsLoweringError> {
        use ast::AssignTarget::*;
        use ast::SimpleAssignTarget;

        match target {
            Simple(simple) => match simple {
                SimpleAssignTarget::Ident(bident) => {
                    // Treat as identifier assignment and reuse existing checks.
                    let expr = ast::Expr::Ident(bident.id.clone());
                    self.validate_assign_expr_target(&expr)
                }
                SimpleAssignTarget::Member(m) => {
                    let expr = ast::Expr::Member(m.clone());
                    self.validate_assign_expr_target(&expr)
                }
                _ => Err(TsLoweringError::Unsupported(
                    "assignment target kind is outside the Arth TS guest subset; assign only to identifiers or simple object fields (see docs/ts-subset.md §3.3).",
                )),
            },
            Pat(_) => Err(TsLoweringError::Unsupported(
                "destructuring assignments are not allowed in the Arth TS guest subset; assign to simple identifiers instead (see docs/ts-subset.md §3.3).",
            )),
        }
    }

    fn validate_assign_expr_target(&self, expr: &ast::Expr) -> Result<(), TsLoweringError> {
        match expr {
            ast::Expr::Ident(id) => {
                if self.is_ambient_global(id.sym.as_ref()) {
                    return Err(TsLoweringError::Unsupported(
                        "assignment to ambient host globals is forbidden in the Arth TS guest subset; pass capabilities in as function parameters or imports instead (see docs/ts-subset.md §3.6–§4).",
                    ));
                }
                Ok(())
            }
            ast::Expr::Member(m) => {
                // Disallow writes to ambient host namespaces like `globalThis.*`.
                if let ast::Expr::Ident(id) = &*m.obj
                    && self.is_ambient_global(id.sym.as_ref())
                {
                    return Err(TsLoweringError::Unsupported(
                        "mutation of ambient host namespaces (e.g. `globalThis.*`) is forbidden in the Arth TS guest subset; treat host capabilities as immutable imports instead (see docs/ts-subset.md §3.6–§4).",
                    ));
                }

                use swc_ecma_ast::MemberProp;

                match &m.prop {
                    MemberProp::Ident(id) => {
                        if is_forbidden_prototype_property(id.sym.as_ref()) {
                            return Err(TsLoweringError::Unsupported(
                                "mutation of `.prototype` / `.__proto__` is forbidden in the Arth TS guest subset; do not change object prototypes at runtime (see docs/ts-subset.md §4).",
                            ));
                        }
                    }
                    MemberProp::Computed(c) => {
                        if let ast::Expr::Lit(ast::Lit::Str(s)) = &*c.expr
                            && (s.value == "prototype" || s.value == "__proto__")
                        {
                            return Err(TsLoweringError::Unsupported(
                                "mutation of `prototype` / `__proto__` is forbidden in the Arth TS guest subset; do not change object prototypes at runtime (see docs/ts-subset.md §4).",
                            ));
                        }
                        self.validate_expr(&c.expr)?;
                    }
                    MemberProp::PrivateName(_) => {
                        return Err(TsLoweringError::Unsupported(
                            "assignment to private fields is not yet supported in the Arth TS guest subset; use public fields and naming conventions instead (see docs/ts-subset.md §3.5).",
                        ));
                    }
                }

                Ok(())
            }
            _ => {
                self.validate_expr(expr)?;
                Ok(())
            }
        }
    }

    fn validate_unary_expr(&self, u: &ast::UnaryExpr) -> Result<(), TsLoweringError> {
        use swc_ecma_ast::UnaryOp as U;

        self.validate_expr(&u.arg)?;

        match u.op {
            U::Minus | U::Bang => Ok(()),
            U::Plus | U::Tilde | U::TypeOf | U::Void | U::Delete => {
                Err(TsLoweringError::Unsupported(
                    "unary operators other than `-` and `!` are not yet supported in the Arth TS guest subset; avoid `+`, `~`, `typeof`, `void`, and `delete` (see docs/ts-subset.md §3.2–§3.3).",
                ))
            }
        }
    }

    fn validate_arrow_expr(&self, a: &ast::ArrowExpr) -> Result<(), TsLoweringError> {
        if let Some(type_params) = &a.type_params {
            self.validate_type_params(type_params)?;
        }

        for p in &a.params {
            if let ast::Pat::Ident(id) = p {
                self.validate_ts_type_ann(&id.type_ann)?;
            } else {
                return Err(TsLoweringError::Unsupported(
                    "arrow function parameters must be simple identifiers",
                ));
            }
        }

        if let Some(ret) = &a.return_type {
            self.validate_ts_type(&ret.type_ann)?;
        }

        match &*a.body {
            ast::BlockStmtOrExpr::BlockStmt(block) => self.validate_block_stmt(block),
            ast::BlockStmtOrExpr::Expr(expr) => self.validate_expr(expr),
        }
    }

    fn validate_function_expr(&self, f: &ast::FnExpr) -> Result<(), TsLoweringError> {
        if f.function.is_generator {
            return Err(TsLoweringError::Unsupported(
                "generators (`function*`) are not allowed",
            ));
        }

        if let Some(type_params) = &f.function.type_params {
            self.validate_type_params(type_params)?;
        }

        for p in &f.function.params {
            if let ast::Pat::Ident(id) = &p.pat {
                self.validate_ts_type_ann(&id.type_ann)?;
            } else {
                return Err(TsLoweringError::Unsupported(
                    "function expression parameters must be simple identifiers",
                ));
            }
        }

        self.validate_ts_type_ann(&f.function.return_type)?;

        if let Some(body) = &f.function.body {
            self.validate_block_stmt(body)?;
        }

        Ok(())
    }

    fn validate_class_expr(&self, c: &ast::ClassExpr) -> Result<(), TsLoweringError> {
        let dummy_ident = ast::Ident::new(
            "__anon_class".into(),
            c.class.span,
            swc_common::SyntaxContext::empty(),
        );
        let decl = ast::ClassDecl {
            ident: dummy_ident,
            declare: false,
            class: c.class.clone(),
        };
        self.validate_class_decl(&decl)
    }

    fn validate_ts_type_ann(
        &self,
        type_ann: &Option<Box<ast::TsTypeAnn>>,
    ) -> Result<(), TsLoweringError> {
        if let Some(ann) = type_ann {
            self.validate_ts_type(&ann.type_ann)?;
        }
        Ok(())
    }

    fn validate_ts_type(&self, ty: &ast::TsType) -> Result<(), TsLoweringError> {
        use ast::TsType::*;

        match ty {
            TsKeywordType(kw) => self.validate_ts_keyword_type(kw),
            TsTypeRef(tr) => self.validate_ts_type_ref(tr),
            TsArrayType(arr) => self.validate_ts_type(&arr.elem_type),
            TsTupleType(tuple) => {
                for elem in &tuple.elem_types {
                    self.validate_ts_type(&elem.ty)?;
                }
                Ok(())
            }
            TsLitType(lit) => self.validate_ts_lit_type(lit),
            TsTypeLit(tl) => self.validate_ts_type_lit(tl),
            TsUnionOrIntersectionType(ui) => match ui {
                ast::TsUnionOrIntersectionType::TsUnionType(u) => {
                    // Check if this is an optional pattern: T | null or T | undefined
                    // These are allowed and map to Optional<T> in Arth
                    if is_optional_union_type(u) {
                        // Validate only the non-null/undefined parts
                        for t in &u.types {
                            if !is_null_or_undefined_type(t) {
                                self.validate_ts_type(t)?;
                            }
                        }
                        Ok(())
                    } else {
                        // For other unions, validate all members
                        for t in &u.types {
                            self.validate_ts_type(t)?;
                        }
                        Ok(())
                    }
                }
                ast::TsUnionOrIntersectionType::TsIntersectionType(_) => {
                    Err(TsLoweringError::Unsupported(
                        "intersection types are not allowed in the Arth TS guest subset; keep value-level types to simple unions, arrays, objects, and interfaces instead (see docs/ts-subset.md §3.2).",
                    ))
                }
            },
            TsParenthesizedType(p) => self.validate_ts_type(&p.type_ann),
            TsOptionalType(_) => Err(TsLoweringError::Unsupported(
                "explicit optional types (`T?`) are not yet supported in the Arth TS guest subset; use `T | null` / `T | undefined` at the boundary and map to `Optional<T>` on the Arth side (see docs/ts-subset.md §3.2, §6.2).",
            )),
            TsRestType(_) => Err(TsLoweringError::Unsupported(
                "rest types are not yet supported in the Arth TS guest subset; keep tuple lengths fixed and pass arrays explicitly instead (see docs/ts-subset.md §3.5).",
            )),
            TsFnOrConstructorType(_) => Err(TsLoweringError::Unsupported(
                "function / constructor types are not yet supported in the Arth TS guest subset; keep runtime types to data only, and treat functions as values without rich type-level encodings.",
            )),
            TsTypeQuery(_) => Err(TsLoweringError::Unsupported(
                "`typeof` type queries are not yet supported in the Arth TS guest subset; refer to types by name instead of reflecting on values.",
            )),
            TsConditionalType(_) => Err(TsLoweringError::Unsupported(
                "conditional types are not allowed in the Arth TS guest subset; keep type-level logic simple and explicit instead of using advanced TS metaprogramming (see docs/ts-subset.md §3.2).",
            )),
            TsIndexedAccessType(_) => Err(TsLoweringError::Unsupported(
                "indexed access types are not allowed in the Arth TS guest subset; refer to concrete field and element types instead.",
            )),
            TsMappedType(_) => Err(TsLoweringError::Unsupported(
                "mapped types are not allowed in the Arth TS guest subset; define explicit object shapes instead of computed ones (see docs/ts-subset.md §3.2).",
            )),
            TsTypeOperator(_) => Err(TsLoweringError::Unsupported(
                "type operators (`keyof`, `readonly`, etc.) are not yet supported in the Arth TS guest subset; use concrete types instead of computed ones.",
            )),
            TsImportType(_) => Err(TsLoweringError::Unsupported(
                "import types are not yet supported in the Arth TS guest subset; refer to imported declarations directly instead.",
            )),
            TsThisType(_) | TsInferType(_) | TsTypePredicate(_) => {
                Err(TsLoweringError::Unsupported(
                    "advanced TS type features (e.g. `infer`, type predicates, `this` types) are not supported in the Arth TS guest subset; keep type-level reasoning simple and explicit (see docs/ts-subset.md §3.2).",
                ))
            }
        }
    }

    fn validate_ts_keyword_type(&self, kw: &ast::TsKeywordType) -> Result<(), TsLoweringError> {
        use ast::TsKeywordTypeKind as K;

        match kw.kind {
            K::TsNumberKeyword | K::TsStringKeyword | K::TsBooleanKeyword | K::TsVoidKeyword => {
                Ok(())
            }
            K::TsAnyKeyword | K::TsUnknownKeyword => Err(TsLoweringError::Unsupported(
                "`any` / `unknown` are not allowed in value positions in the Arth TS guest subset; use precise types (`number`, `string`, structs, or tagged unions) instead (see docs/ts-subset.md §3.2).",
            )),
            K::TsNullKeyword | K::TsUndefinedKeyword => Err(TsLoweringError::Unsupported(
                "`null` / `undefined` types are not yet supported in the Arth TS guest subset; represent absence via `Optional<T>`/union encoding at the boundary instead (see docs/ts-subset.md §3.2, §6.2).",
            )),
            K::TsNeverKeyword => Err(TsLoweringError::Unsupported(
                "`never` in value flow is not supported in the Arth TS guest subset; keep runtime types non-empty and model impossible states via explicit tagging.",
            )),
            _ => Err(TsLoweringError::Unsupported(
                "this keyword type is not supported in the Arth TS guest subset; restrict to `number`, `string`, `boolean`, and `void` (see docs/ts-subset.md §3.2).",
            )),
        }
    }

    fn validate_ts_type_ref(&self, tr: &ast::TsTypeRef) -> Result<(), TsLoweringError> {
        if let Some(type_params) = &tr.type_params {
            for t in &type_params.params {
                self.validate_ts_type(t)?;
            }
        }
        Ok(())
    }

    fn validate_ts_lit_type(&self, lit: &ast::TsLitType) -> Result<(), TsLoweringError> {
        match &lit.lit {
            ast::TsLit::Str(_) | ast::TsLit::Number(_) => Ok(()),
            _ => Err(TsLoweringError::Unsupported(
                "only string and numeric literal types are allowed",
            )),
        }
    }

    fn validate_ts_type_lit(&self, tl: &ast::TsTypeLit) -> Result<(), TsLoweringError> {
        for m in &tl.members {
            match m {
                ast::TsTypeElement::TsPropertySignature(prop) => {
                    if let Some(type_ann) = &prop.type_ann {
                        self.validate_ts_type(&type_ann.type_ann)?;
                    }
                }
                _ => {
                    return Err(TsLoweringError::Unsupported(
                        "only simple field members are allowed in object type literals",
                    ));
                }
            }
        }
        Ok(())
    }

    fn is_ambient_global(&self, name: &str) -> bool {
        matches!(
            name,
            "window" | "document" | "process" | "global" | "globalThis"
        ) && !self.bound_value_names.contains(name)
    }
}

fn is_forbidden_prototype_property(name: &str) -> bool {
    matches!(name, "prototype" | "__proto__")
}

/// Check if a union type is an optional pattern: T | null or T | undefined.
/// This allows expressing nullable types as Optional<T> in Arth.
fn is_optional_union_type(union: &ast::TsUnionType) -> bool {
    // An optional union has exactly 2 types, one of which is null or undefined
    if union.types.len() == 2 {
        union
            .types
            .iter()
            .any(|t| is_null_or_undefined_type(t.as_ref()))
    } else {
        false
    }
}

/// Check if a type is null or undefined keyword type.
fn is_null_or_undefined_type(ty: &ast::TsType) -> bool {
    if let ast::TsType::TsKeywordType(kw) = ty {
        matches!(
            kw.kind,
            ast::TsKeywordTypeKind::TsNullKeyword | ast::TsKeywordTypeKind::TsUndefinedKeyword
        )
    } else {
        false
    }
}

/// Check if a decorator expression is an allowed class decorator.
/// Currently allows: @provider, @data
fn is_allowed_class_decorator(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Ident(id) => {
            let name = id.sym.as_ref();
            matches!(name, "provider" | "data")
        }
        // Also allow @provider() and @data() call forms
        ast::Expr::Call(call) => {
            if let ast::Callee::Expr(callee) = &call.callee
                && let ast::Expr::Ident(id) = callee.as_ref()
            {
                let name = id.sym.as_ref();
                return matches!(name, "provider" | "data");
            }
            false
        }
        _ => false,
    }
}

fn is_allowed_arth_host_module(name: &str) -> bool {
    matches!(
        name,
        "log" | "time" | "math" | "rand" | "array" | "map" | "option"
    )
}

fn member_static_names(m: &ast::MemberExpr) -> Option<(&str, &str)> {
    let obj_ident = match &*m.obj {
        ast::Expr::Ident(id) => id.sym.as_ref(),
        _ => return None,
    };

    use swc_ecma_ast::MemberProp;

    match &m.prop {
        MemberProp::Ident(id) => Some((obj_ident, id.sym.as_ref())),
        MemberProp::Computed(c) => {
            if let ast::Expr::Lit(ast::Lit::Str(s)) = &*c.expr
                && (s.value == "prototype" || s.value == "__proto__")
            {
                return Some((obj_ident, "prototype"));
            }
            None
        }
        MemberProp::PrivateName(_) => None,
    }
}

// =============================================================================
// Phase 8: Anti-Pattern Detection Helpers
// =============================================================================

/// Check if a member expression is `this.state` access.
///
/// This is an anti-pattern in the Arth TS subset because:
/// - Classes map to modules, which have no instance fields
/// - State should be passed as provider parameters
///
/// Example anti-pattern:
/// ```typescript
/// export class Controller {
///   state = { count: 0 };  // ❌ No class fields
///   increment() {
///     this.state.count++;  // ❌ No this.state
///   }
/// }
/// ```
fn is_this_state_access(m: &ast::MemberExpr) -> bool {
    // Check if object is `this`
    if !matches!(&*m.obj, ast::Expr::This(_)) {
        return false;
    }

    // Check if property is `state`
    match &m.prop {
        swc_ecma_ast::MemberProp::Ident(id) => id.sym.as_ref() == "state",
        swc_ecma_ast::MemberProp::Computed(c) => {
            if let ast::Expr::Lit(ast::Lit::Str(s)) = &*c.expr {
                s.value == "state"
            } else {
                false
            }
        }
        swc_ecma_ast::MemberProp::PrivateName(_) => false,
    }
}

/// Check if a member expression is any `this.<field>` access.
///
/// This is an anti-pattern because controller classes map to Arth modules,
/// which have no instance fields. All state access should be via provider
/// parameters.
///
/// The check is intentionally broad: any access to `this.<something>` in a
/// controller method body indicates the developer is trying to use class
/// instance state, which won't work after lowering to Arth modules.
fn is_this_field_access(m: &ast::MemberExpr) -> bool {
    // Only flag `this.<ident>` where ident is not `state` (handled separately)
    if !matches!(&*m.obj, ast::Expr::This(_)) {
        return false;
    }

    // `this.state` is handled by is_this_state_access with a more specific message
    if is_this_state_access(m) {
        return false;
    }

    // Any other `this.<field>` access
    matches!(&m.prop, swc_ecma_ast::MemberProp::Ident(_))
}

/// Check if a class has the @data or @provider decorator.
fn has_data_or_provider_decorator(class: &ast::Class) -> bool {
    class
        .decorators
        .iter()
        .any(|d| is_allowed_class_decorator(&d.expr))
}

/// Check if a class has any instance fields (non-method members).
fn has_instance_fields(class: &ast::Class) -> bool {
    class
        .body
        .iter()
        .any(|member| matches!(member, ast::ClassMember::ClassProp(_)))
}

// =============================================================================
// Phase 8: Known Arth Type Validation
// =============================================================================

/// Known built-in Arth types that TS types can map to.
const KNOWN_ARTH_TYPES: &[&str] = &[
    // Primitives
    "String", "Int", "Float", "Bool", "Void", // Collections
    "List", "Map", "Set", "Array", // Optional/Result
    "Optional", "Result", // Async
    "Task", "Promise", // Common generics
    "Iterator", "Iterable",
];

/// Check if a type name is a known Arth type or a local type reference.
///
/// This doesn't validate that local type references exist (that's done by
/// the Arth type checker), but it does ensure the type follows expected
/// patterns.
#[allow(dead_code)] // Will be used in Phase 8 type validation
fn is_known_or_local_type(name: &str) -> bool {
    // Known built-in types
    if KNOWN_ARTH_TYPES.contains(&name) {
        return true;
    }

    // Local types should be PascalCase (start with uppercase)
    // This is a heuristic - actual validation happens in Arth type checking
    name.chars().next().is_some_and(|c| c.is_uppercase())
}

// =============================================================================
// Phase 8: Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use swc_common::{FileName, SourceMap, sync::Lrc};
    use swc_ecma_parser::{Parser, StringInput, Syntax, TsSyntax};

    fn parse_module(code: &str) -> ast::Module {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(FileName::Custom("test.ts".into()).into(), code.to_string());
        let mut parser = Parser::new(
            Syntax::Typescript(TsSyntax {
                decorators: true,
                tsx: false,
                ..Default::default()
            }),
            StringInput::from(&*fm),
            None,
        );
        parser.parse_module().expect("failed to parse TS module")
    }

    // -------------------------------------------------------------------------
    // Phase 8: this.state anti-pattern detection
    // -------------------------------------------------------------------------

    #[test]
    fn test_this_state_access_rejected() {
        let code = r#"
            export class Controller {
                increment() {
                    this.state.count++;
                }
            }
        "#;
        let module = parse_module(code);
        let result = validate_ts_subset(&module);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("this.state"));
        assert!(err.to_string().contains("@provider"));
    }

    #[test]
    fn test_this_field_access_rejected() {
        let code = r#"
            export class Controller {
                process() {
                    return this.cache.get("key");
                }
            }
        "#;
        let module = parse_module(code);
        let result = validate_ts_subset(&module);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("this.<field>"));
    }

    // -------------------------------------------------------------------------
    // Phase 8: Class fields without @data/@provider decorator
    // -------------------------------------------------------------------------

    #[test]
    fn test_class_fields_without_decorator_rejected() {
        let code = r#"
            export class Controller {
                count: number = 0;

                increment() {
                    this.count++;
                }
            }
        "#;
        let module = parse_module(code);
        let result = validate_ts_subset(&module);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("class-level fields"));
        assert!(err.to_string().contains("@data") || err.to_string().contains("@provider"));
    }

    #[test]
    fn test_data_class_with_fields_allowed() {
        let code = r#"
            @data
            class User {
                name: string;
                email: string;
            }
        "#;
        let module = parse_module(code);
        let result = validate_ts_subset(&module);
        assert!(
            result.is_ok(),
            "Expected @data class with fields to be valid"
        );
    }

    #[test]
    fn test_provider_class_with_fields_allowed() {
        let code = r#"
            @provider
            class State {
                count: number;
                user: User | null;
            }

            type User = { name: string; };
        "#;
        let module = parse_module(code);
        let result = validate_ts_subset(&module);
        assert!(
            result.is_ok(),
            "Expected @provider class with fields to be valid"
        );
    }

    #[test]
    fn test_class_without_fields_allowed() {
        let code = r#"
            export class Controller {
                increment(state: State) {
                    state.count++;
                }

                decrement(state: State) {
                    state.count--;
                }
            }
        "#;
        let module = parse_module(code);
        let result = validate_ts_subset(&module);
        assert!(result.is_ok(), "Expected class without fields to be valid");
    }

    // -------------------------------------------------------------------------
    // Phase 8: Validation with diagnostics
    // -------------------------------------------------------------------------

    #[test]
    fn test_validate_with_diagnostics_success() {
        let code = r#"
            export function main() {
                console.log("Hello");
            }
        "#;
        let module = parse_module(code);
        let result = validate_ts_subset_with_diagnostics(&module, None);
        assert!(result.success);
        assert!(!result.diagnostics.has_errors());
    }

    #[test]
    fn test_validate_with_diagnostics_error() {
        let code = r#"
            export class Controller {
                state = { count: 0 };
            }
        "#;
        let module = parse_module(code);
        let result = validate_ts_subset_with_diagnostics(&module, None);
        assert!(!result.success);
        assert!(result.diagnostics.has_errors());
    }

    // -------------------------------------------------------------------------
    // Phase 8: Helper function tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_is_known_or_local_type() {
        // Known Arth types
        assert!(is_known_or_local_type("String"));
        assert!(is_known_or_local_type("Int"));
        assert!(is_known_or_local_type("Bool"));
        assert!(is_known_or_local_type("List"));
        assert!(is_known_or_local_type("Optional"));

        // Local types (PascalCase)
        assert!(is_known_or_local_type("User"));
        assert!(is_known_or_local_type("MyController"));

        // Not valid types (lowercase)
        assert!(!is_known_or_local_type("user"));
        assert!(!is_known_or_local_type("myController"));
    }

    #[test]
    fn test_has_data_or_provider_decorator() {
        let code_data = r#"@data class User { name: string; }"#;
        let module_data = parse_module(code_data);
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Class(c))) = &module_data.body[0] {
            assert!(has_data_or_provider_decorator(&c.class));
        }

        let code_provider = r#"@provider class State { count: number; }"#;
        let module_provider = parse_module(code_provider);
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Class(c))) =
            &module_provider.body[0]
        {
            assert!(has_data_or_provider_decorator(&c.class));
        }

        let code_plain = r#"class Controller { process() {} }"#;
        let module_plain = parse_module(code_plain);
        if let ast::ModuleItem::Stmt(ast::Stmt::Decl(ast::Decl::Class(c))) = &module_plain.body[0] {
            assert!(!has_data_or_provider_decorator(&c.class));
        }
    }

    #[test]
    fn test_helpful_error_messages_contain_examples() {
        // Verify that error messages contain helpful examples
        let code = r#"
            export class Controller {
                state = { count: 0 };
            }
        "#;
        let module = parse_module(code);
        let result = validate_ts_subset(&module);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();

        // Should contain helpful suggestions
        assert!(
            msg.contains("@data class") || msg.contains("@provider"),
            "Error message should suggest @data or @provider"
        );
        assert!(
            msg.contains("docs/arth-ts-controller-spec.md"),
            "Error message should reference documentation"
        );
    }
}
