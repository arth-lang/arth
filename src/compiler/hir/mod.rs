#![allow(dead_code)]
#![allow(clippy::match_single_binding)]
#![allow(clippy::only_used_in_recursion)]
use std::path::PathBuf;
use std::sync::Arc;

pub mod core;

use crate::compiler::ast::{
    Decl, EnumDecl, EnumVariant, FuncDecl, FuncSig, Ident, InterfaceDecl, InterfaceMethod,
    ModuleDecl, NamePath, Param, StructDecl, StructField,
};
use crate::compiler::hir::core::{HirId, Span as HirSpan};
use crate::compiler::source;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirPackage(pub Vec<String>);

impl std::fmt::Display for HirPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.join("."))
    }
}

impl From<String> for HirPackage {
    fn from(s: String) -> Self {
        let parts = s
            .split('.')
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string())
            .collect();
        HirPackage(parts)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirSourceLanguage {
    Arth,
    Ts,
}

#[derive(Clone, Debug)]
pub struct HirFile {
    pub path: PathBuf,
    pub package: Option<HirPackage>,
    pub decls: Vec<HirDecl>,
    pub notes: Vec<LoweringNote>,
    pub source_language: Option<HirSourceLanguage>,
    pub is_guest: bool,
}

/// External function declaration for FFI
#[derive(Clone, Debug)]
pub struct HirExternFunc {
    pub name: String,
    /// ABI specification (e.g., "C")
    pub abi: String,
    pub params: Vec<HirParam>,
    pub ret: Option<HirType>,
    pub doc: Option<String>,
    pub attrs: Vec<HirAttr>,
    pub id: HirId,
    pub span: HirSpan,
}

#[derive(Clone, Debug)]
pub enum HirDecl {
    Module(HirModule),
    Struct(HirStruct),
    Interface(HirInterface),
    Enum(HirEnum),
    Provider(HirProvider),
    Function(HirFunc),
    /// External function declaration (FFI)
    ExternFunc(HirExternFunc),
}

#[derive(Clone, Debug)]
pub struct LoweringNote {
    pub span: Option<HirSpan>,
    pub message: String,
}

// --- Types (shapes only) ---

/// Generic type parameter declaration (e.g., `T` or `T extends Bound`)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirGenericParam {
    pub name: String,
    /// Optional bound constraint (e.g., `Sendable`, `Comparable<T>`)
    pub bound: Option<HirType>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirType {
    /// Simple named type like `Int`, `String`, or qualified `pkg.Name`
    Name { path: Vec<String> },
    /// Generic type with type arguments like `List<Int>`, `Map<String, Int>`
    Generic {
        path: Vec<String>,
        args: Vec<HirType>,
    },
    /// Type parameter reference (e.g., `T` in a generic context)
    TypeParam { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirParam {
    pub name: String,
    pub ty: HirType,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirField {
    pub name: String,
    pub ty: HirType,
    pub doc: Option<String>,
    pub attrs: Vec<HirAttr>,
    pub span: Option<HirSpan>,
    // Whether this field is declared `shared` in the source.
    pub is_shared: bool,
    // Whether this field is declared `final` (immutable after initialization).
    pub is_final: bool,
    // Visibility modifier for this field.
    pub vis: crate::compiler::ast::Visibility,
}

#[derive(Clone, Debug)]
pub struct HirModule {
    pub name: String,
    /// Whether this module is exported (accessible from other packages)
    pub is_exported: bool,
    pub funcs: Vec<HirFunc>,
    /// List of interface names this module implements
    /// Target type is inferred from method signatures
    pub implements: Vec<Vec<String>>, // each entry is a qualified path
    pub doc: Option<String>,
    pub attrs: Vec<HirAttr>,
    pub id: HirId,
    pub span: HirSpan,
}

#[derive(Clone, Debug)]
pub struct HirStruct {
    pub name: String,
    pub generics: Vec<HirGenericParam>,
    pub fields: Vec<HirField>,
    pub doc: Option<String>,
    pub attrs: Vec<HirAttr>,
    pub id: HirId,
    pub span: HirSpan,
}

/// A method declaration within an interface.
/// Can be either an abstract method (no body) or a default method (with body).
#[derive(Clone, Debug)]
pub struct HirInterfaceMethod {
    pub sig: HirFuncSig,
    /// If Some, this is a default method with an implementation.
    /// If None, this is an abstract method that must be implemented by conforming modules.
    pub default_body: Option<HirBlock>,
}

#[derive(Clone, Debug)]
pub struct HirInterface {
    pub name: String,
    pub generics: Vec<HirGenericParam>,
    pub methods: Vec<HirInterfaceMethod>,
    /// List of interface names this interface extends
    pub extends: Vec<Vec<String>>, // each entry is a qualified path
    pub doc: Option<String>,
    pub attrs: Vec<HirAttr>,
    pub id: HirId,
    pub span: HirSpan,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HirEnumVariant {
    Unit {
        name: String,
        discriminant: Option<Box<HirExpr>>,
    },
    Tuple {
        name: String,
        types: Vec<HirType>,
        discriminant: Option<Box<HirExpr>>,
    },
}

impl HirEnumVariant {
    /// Get the variant name
    pub fn name(&self) -> &str {
        match self {
            HirEnumVariant::Unit { name, .. } => name,
            HirEnumVariant::Tuple { name, .. } => name,
        }
    }

    /// Get the optional discriminant expression
    pub fn discriminant(&self) -> Option<&HirExpr> {
        match self {
            HirEnumVariant::Unit { discriminant, .. } => discriminant.as_deref(),
            HirEnumVariant::Tuple { discriminant, .. } => discriminant.as_deref(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HirEnum {
    pub name: String,
    pub generics: Vec<HirGenericParam>,
    pub variants: Vec<HirEnumVariant>,
    pub doc: Option<String>,
    pub attrs: Vec<HirAttr>,
    pub id: HirId,
    pub span: HirSpan,
}

#[derive(Clone, Debug)]
pub struct HirProvider {
    pub name: String,
    pub fields: Vec<HirField>,
    pub doc: Option<String>,
    pub attrs: Vec<HirAttr>,
    pub id: HirId,
    pub span: HirSpan,
}

#[derive(Clone, Debug)]
pub struct HirFunc {
    pub sig: HirFuncSig,
    pub id: HirId,
    pub span: HirSpan,
    pub body: Option<HirBlock>,
}

#[derive(Clone, Debug)]
pub struct HirFuncSig {
    pub name: String,
    pub generics: Vec<HirGenericParam>,
    pub params: Vec<HirParam>,
    pub ret: Option<HirType>,
    pub is_async: bool,
    /// Whether this function is marked as unsafe
    pub is_unsafe: bool,
    pub doc: Option<String>,
    pub attrs: Vec<HirAttr>,
    pub span: Option<HirSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirAttr {
    pub name: String,
    pub args: Option<String>,
}

// --- Bodies: blocks, statements, expressions ---

#[derive(Clone, Debug, PartialEq)]
pub struct HirBlock {
    pub id: HirId,
    pub span: HirSpan,
    pub stmts: Vec<HirStmt>,
}

/// Catch clause in a try statement, preserving exception type and variable binding
#[derive(Clone, Debug, PartialEq)]
pub struct HirCatchClause {
    pub id: HirId,
    pub span: HirSpan,
    /// Exception type to catch (e.g., `DivisionError`)
    pub ty: Option<HirType>,
    /// Variable name to bind the caught exception
    pub var: Option<String>,
    /// Catch block body
    pub block: HirBlock,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirAssignOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Shl,
    Shr,
    And,
    Or,
    Xor,
}

/// HIR pattern for enum pattern matching in switch statements
#[derive(Clone, Debug, PartialEq)]
pub enum HirPattern {
    /// Wildcard pattern: `_`
    Wildcard { id: HirId, span: HirSpan },
    /// Variable binding: `x`
    Binding {
        id: HirId,
        span: HirSpan,
        name: String,
    },
    /// Literal pattern (int, string, bool)
    Literal {
        id: HirId,
        span: HirSpan,
        expr: HirExpr,
    },
    /// Enum variant pattern: `EnumType.Variant(p1, p2, ...)`
    Variant {
        id: HirId,
        span: HirSpan,
        enum_name: String,
        variant_name: String,
        payloads: Vec<HirPattern>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum HirStmt {
    PrintStr {
        id: HirId,
        span: HirSpan,
        text: String,
    },
    PrintExpr {
        id: HirId,
        span: HirSpan,
        expr: HirExpr,
    },
    /// Print without newline (raw print)
    PrintRawStr {
        id: HirId,
        span: HirSpan,
        text: String,
    },
    PrintRawExpr {
        id: HirId,
        span: HirSpan,
        expr: HirExpr,
    },
    If {
        id: HirId,
        span: HirSpan,
        cond: HirExpr,
        then_blk: HirBlock,
        else_blk: Option<HirBlock>,
    },
    While {
        id: HirId,
        span: HirSpan,
        cond: HirExpr,
        body: HirBlock,
    },
    Labeled {
        id: HirId,
        span: HirSpan,
        label: String,
        stmt: Box<HirStmt>,
    },
    For {
        id: HirId,
        span: HirSpan,
        init: Option<Box<HirStmt>>,
        cond: Option<HirExpr>,
        step: Option<Box<HirStmt>>,
        body: HirBlock,
    },
    Switch {
        id: HirId,
        span: HirSpan,
        expr: HirExpr,
        /// Legacy expression-based cases
        cases: Vec<(HirExpr, HirBlock)>,
        /// Pattern-based cases for enum matching
        pattern_cases: Vec<(HirPattern, HirBlock)>,
        default: Option<HirBlock>,
    },
    Try {
        id: HirId,
        span: HirSpan,
        try_blk: HirBlock,
        catches: Vec<HirCatchClause>,
        finally_blk: Option<HirBlock>,
    },
    Assign {
        id: HirId,
        span: HirSpan,
        name: String,
        expr: HirExpr,
    },
    AssignOp {
        id: HirId,
        span: HirSpan,
        name: String,
        op: HirAssignOp,
        expr: HirExpr,
    },
    // Minimal field assignment for demo: `obj.field = expr;`
    FieldAssign {
        id: HirId,
        span: HirSpan,
        object: HirExpr,
        field: String,
        expr: HirExpr,
    },
    VarDecl {
        id: HirId,
        span: HirSpan,
        ty: HirType,
        name: String,
        init: Option<HirExpr>,
        // Whether the variable is declared `shared` (cross-task shareable handle)
        is_shared: bool,
    },
    Break {
        id: HirId,
        span: HirSpan,
        label: Option<String>,
    },
    Continue {
        id: HirId,
        span: HirSpan,
        label: Option<String>,
    },
    Return {
        id: HirId,
        span: HirSpan,
        expr: Option<HirExpr>,
    },
    /// Throw statement: `throw expr;` throws an exception value
    Throw {
        id: HirId,
        span: HirSpan,
        expr: HirExpr,
    },
    /// Panic statement: `panic(msg);` - unrecoverable error that unwinds within task boundary.
    /// Panics execute drops along the unwind path and propagate to join() as a failure.
    /// Unlike throw, panics cannot be caught and are reserved for invariant violations.
    Panic {
        id: HirId,
        span: HirSpan,
        msg: HirExpr,
    },
    Block(HirBlock),
    // Generic expression statement (e.g., function call for side effects)
    Expr {
        id: HirId,
        span: HirSpan,
        expr: HirExpr,
    },
    /// Unsafe block: code inside can perform unsafe operations
    Unsafe {
        id: HirId,
        span: HirSpan,
        block: HirBlock,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum HirExpr {
    Int {
        id: HirId,
        span: HirSpan,
        value: i64,
    },
    Float {
        id: HirId,
        span: HirSpan,
        value: f64,
    },
    Str {
        id: HirId,
        span: HirSpan,
        value: String,
    },
    Char {
        id: HirId,
        span: HirSpan,
        value: char,
    },
    Bool {
        id: HirId,
        span: HirSpan,
        value: bool,
    },
    Await {
        id: HirId,
        span: HirSpan,
        expr: Box<HirExpr>,
    },
    Cast {
        id: HirId,
        span: HirSpan,
        to: HirType,
        expr: Box<HirExpr>,
    },
    Ident {
        id: HirId,
        span: HirSpan,
        name: String,
    },
    Binary {
        id: HirId,
        span: HirSpan,
        left: Box<HirExpr>,
        op: HirBinOp,
        right: Box<HirExpr>,
    },
    Unary {
        id: HirId,
        span: HirSpan,
        op: HirUnOp,
        expr: Box<HirExpr>,
    },
    Call {
        id: HirId,
        span: HirSpan,
        callee: Box<HirExpr>,
        args: Vec<HirExpr>,
        // Optional borrow annotation for validation passes (exclusive/provider/release).
        borrow: Option<HirBorrowKind>,
    },
    Member {
        id: HirId,
        span: HirSpan,
        object: Box<HirExpr>,
        member: String,
    },
    /// Optional chaining: expr?.field - returns None if expr is None
    OptionalMember {
        id: HirId,
        span: HirSpan,
        object: Box<HirExpr>,
        member: String,
    },
    Index {
        id: HirId,
        span: HirSpan,
        object: Box<HirExpr>,
        index: Box<HirExpr>,
    },
    // Collection literals
    ListLit {
        id: HirId,
        span: HirSpan,
        elements: Vec<HirExpr>,
    },
    MapLit {
        id: HirId,
        span: HirSpan,
        pairs: Vec<(HirExpr, HirExpr)>,
        spread: Option<Box<HirExpr>>, // struct update syntax: { ..existing, field: value }
    },
    /// Struct literal: TypeName { field: value, ... }
    /// Preserves type information for proper codegen
    StructLit {
        id: HirId,
        span: HirSpan,
        /// The struct type name as a type path
        type_name: HirType,
        /// Field-value pairs: (field_name, value_expr)
        fields: Vec<(String, HirExpr)>,
        /// Optional spread expression for struct update syntax
        spread: Option<Box<HirExpr>>,
    },
    /// Enum variant constructor: EnumType.Variant or EnumType.Variant(args...)
    /// Used for constructing enum values
    EnumVariant {
        id: HirId,
        span: HirSpan,
        /// The enum type name
        enum_name: String,
        /// The variant name
        variant_name: String,
        /// Payload arguments for tuple variants (empty for unit variants)
        args: Vec<HirExpr>,
    },
    // Ternary conditional expression
    Conditional {
        id: HirId,
        span: HirSpan,
        cond: Box<HirExpr>,
        then_expr: Box<HirExpr>,
        else_expr: Box<HirExpr>,
    },
    // Lambda expression (first-class function with potential captures)
    Lambda {
        id: HirId,
        span: HirSpan,
        params: Vec<HirParam>,
        body: HirBlock,
        ret: Option<HirType>,
        // Captured variables from enclosing scope: (name, type)
        captures: Vec<(String, HirType)>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Shl,
    Shr,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And, // logical AND (current VM treats as bitwise on 0/1)
    Or,  // logical OR (current VM treats as bitwise on 0/1)
    BitAnd,
    BitOr,
    Xor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirUnOp {
    Neg,
    Not,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirBorrowKind {
    ExclusiveLocal,
    Provider,
    Release,
}

fn name_path_to_string(np: &NamePath) -> String {
    np.path
        .iter()
        .map(|Ident(s)| s.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn name_path_to_vec(np: &NamePath) -> Vec<String> {
    np.path.iter().map(|Ident(s)| s.clone()).collect()
}

fn name_path_to_type(np: &NamePath) -> HirType {
    HirType::Name {
        path: np.path.iter().map(|Ident(s)| s.clone()).collect(),
    }
}

/// Convert an AST expression (identifier or member chain) to a HirType
/// Used for struct literal type names like `Point { ... }` or `pkg.User { ... }`
fn expr_to_hir_type(expr: &crate::compiler::ast::Expr) -> HirType {
    use crate::compiler::ast::Expr as AE;
    match expr {
        AE::Ident(Ident(name)) => HirType::Name {
            path: vec![name.clone()],
        },
        AE::Member(obj, Ident(member)) => {
            // Collect the member chain into a path
            let mut path = expr_to_path(obj);
            path.push(member.clone());
            HirType::Name { path }
        }
        _ => {
            // Fallback for unexpected expression types
            HirType::Name {
                path: vec!["<unknown>".to_string()],
            }
        }
    }
}

/// Helper to extract path segments from member chain expressions
fn expr_to_path(expr: &crate::compiler::ast::Expr) -> Vec<String> {
    use crate::compiler::ast::Expr as AE;
    match expr {
        AE::Ident(Ident(name)) => vec![name.clone()],
        AE::Member(obj, Ident(member)) => {
            let mut path = expr_to_path(obj);
            path.push(member.clone());
            path
        }
        _ => vec![],
    }
}

/// Convert AST GenericParam to HirGenericParam
fn generic_param_to_hir(gp: &crate::compiler::ast::GenericParam) -> HirGenericParam {
    HirGenericParam {
        name: gp.name.0.clone(),
        bound: gp.bound.as_ref().map(name_path_to_type),
    }
}

/// Convert a slice of AST generic params to HIR
fn generics_to_hir(generics: &[crate::compiler::ast::GenericParam]) -> Vec<HirGenericParam> {
    generics.iter().map(generic_param_to_hir).collect()
}

/// Convert a type, recognizing type parameters from the given generic context
fn name_path_to_type_with_generics(np: &NamePath, generic_names: &[String]) -> HirType {
    // Single-segment names might be type parameters
    if np.path.len() == 1 {
        let name = &np.path[0].0;
        if generic_names.contains(name) {
            return HirType::TypeParam { name: name.clone() };
        }
    }
    HirType::Name {
        path: np.path.iter().map(|Ident(s)| s.clone()).collect(),
    }
}

fn param_to_hir(p: &Param) -> HirParam {
    HirParam {
        name: p.name.0.clone(),
        ty: name_path_to_type(&p.ty),
    }
}

/// Convert param with awareness of generic type parameters
fn param_to_hir_with_generics(p: &Param, generic_names: &[String]) -> HirParam {
    HirParam {
        name: p.name.0.clone(),
        ty: name_path_to_type_with_generics(&p.ty, generic_names),
    }
}

fn func_sig_to_hir(sig: &FuncSig, file: &Arc<PathBuf>) -> HirFuncSig {
    // Collect generic parameter names
    let generic_names: Vec<String> = sig.generics.iter().map(|g| g.name.0.clone()).collect();

    HirFuncSig {
        name: sig.name.0.clone(),
        generics: generics_to_hir(&sig.generics),
        params: sig
            .params
            .iter()
            .map(|p| param_to_hir_with_generics(p, &generic_names))
            .collect(),
        ret: sig
            .ret
            .as_ref()
            .map(|np| name_path_to_type_with_generics(np, &generic_names)),
        is_async: sig.is_async,
        is_unsafe: sig.is_unsafe,
        doc: sig.doc.clone(),
        attrs: sig
            .attrs
            .iter()
            .map(|a| HirAttr {
                name: name_path_to_string(&a.name),
                args: a.args.clone(),
            })
            .collect(),
        span: Some(HirSpan {
            file: file.clone(),
            start: sig.span.start as u32,
            end: sig.span.end as u32,
        }),
    }
}

pub fn make_hir_file(path: PathBuf, package: Option<String>, ast_decls: &[Decl]) -> HirFile {
    let file_arc = Arc::new(path.clone());
    let mut next_id: u32 = 1;
    let mut fresh = || {
        let id = next_id;
        next_id += 1;
        HirId(id)
    };

    fn to_hir_span(file: &Arc<PathBuf>, sp: &source::Span) -> HirSpan {
        HirSpan {
            file: file.clone(),
            start: sp.start as u32,
            end: sp.end as u32,
        }
    }

    // Collect all free variable references in an expression
    fn collect_free_vars(
        e: &crate::compiler::ast::Expr,
        free: &mut std::collections::HashSet<String>,
    ) {
        use crate::compiler::ast::Expr as AE;
        match e {
            AE::Ident(Ident(name)) => {
                free.insert(name.clone());
            }
            AE::Binary(l, _, r) => {
                collect_free_vars(l, free);
                collect_free_vars(r, free);
            }
            AE::Unary(_, ex) => collect_free_vars(ex, free),
            AE::Call(callee, args) => {
                collect_free_vars(callee, free);
                for a in args {
                    collect_free_vars(a, free);
                }
            }
            AE::Member(obj, _) => collect_free_vars(obj, free),
            AE::Index(obj, idx) => {
                collect_free_vars(obj, free);
                collect_free_vars(idx, free);
            }
            AE::ListLit(elems) => {
                for elem in elems {
                    collect_free_vars(elem, free);
                }
            }
            AE::MapLit { pairs, spread } => {
                if let Some(spread_expr) = spread {
                    collect_free_vars(spread_expr, free);
                }
                for (k, v) in pairs {
                    collect_free_vars(k, free);
                    collect_free_vars(v, free);
                }
            }
            AE::Ternary(c, t, f) => {
                collect_free_vars(c, free);
                collect_free_vars(t, free);
                collect_free_vars(f, free);
            }
            AE::Cast(_, ex) => collect_free_vars(ex, free),
            AE::Await(ex) => collect_free_vars(ex, free),
            AE::FnLiteral(_, body) => collect_free_vars_block(body, free),
            _ => {}
        }
    }

    // Collect free variables in a block, accounting for local declarations
    fn collect_free_vars_block(
        blk: &crate::compiler::ast::Block,
        free: &mut std::collections::HashSet<String>,
    ) {
        let mut locals = std::collections::HashSet::new();
        for stmt in &blk.stmts {
            collect_free_vars_stmt(stmt, free, &mut locals);
        }
        // Remove locally-declared variables from free set
        for local in &locals {
            free.remove(local);
        }
    }

    fn collect_free_vars_pattern(
        p: &crate::compiler::ast::Pattern,
        free: &mut std::collections::HashSet<String>,
        locals: &mut std::collections::HashSet<String>,
    ) {
        use crate::compiler::ast::Pattern as AP;
        match p {
            AP::Wildcard => {}
            AP::Binding(name) => {
                // Bindings introduce new locals in the case block scope
                locals.insert(name.0.clone());
            }
            AP::Literal(expr) => {
                collect_free_vars(expr, free);
            }
            AP::Variant { payloads, .. } => {
                for sub in payloads {
                    collect_free_vars_pattern(sub, free, locals);
                }
            }
        }
    }

    fn collect_free_vars_stmt(
        s: &crate::compiler::ast::Stmt,
        free: &mut std::collections::HashSet<String>,
        locals: &mut std::collections::HashSet<String>,
    ) {
        use crate::compiler::ast::Stmt as AS;
        match s {
            AS::VarDecl { name, init, .. } => {
                if let Some(e) = init {
                    collect_free_vars(e, free);
                }
                locals.insert(name.0.clone());
            }
            AS::Assign { expr, .. } | AS::AssignOp { expr, .. } => collect_free_vars(expr, free),
            AS::FieldAssign { object, expr, .. } => {
                collect_free_vars(object, free);
                collect_free_vars(expr, free);
            }
            AS::PrintExpr(e) | AS::PrintRawExpr(e) | AS::Expr(e) => collect_free_vars(e, free),
            AS::If {
                cond,
                then_blk,
                else_blk,
            } => {
                collect_free_vars(cond, free);
                collect_free_vars_block(then_blk, free);
                if let Some(eb) = else_blk {
                    collect_free_vars_block(eb, free);
                }
            }
            AS::While { cond, body } => {
                collect_free_vars(cond, free);
                collect_free_vars_block(body, free);
            }
            AS::For {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(i) = init {
                    collect_free_vars_stmt(i, free, locals);
                }
                if let Some(c) = cond {
                    collect_free_vars(c, free);
                }
                if let Some(st) = step {
                    collect_free_vars_stmt(st, free, locals);
                }
                collect_free_vars_block(body, free);
            }
            AS::Switch {
                expr,
                cases,
                pattern_cases,
                default,
            } => {
                collect_free_vars(expr, free);
                for (case_expr, case_blk) in cases {
                    collect_free_vars(case_expr, free);
                    collect_free_vars_block(case_blk, free);
                }
                // Also collect from pattern cases (bindings introduce new locals)
                for (pat, blk) in pattern_cases {
                    collect_free_vars_pattern(pat, free, locals);
                    collect_free_vars_block(blk, free);
                }
                if let Some(def) = default {
                    collect_free_vars_block(def, free);
                }
            }
            AS::Return(eopt) => {
                if let Some(e) = eopt {
                    collect_free_vars(e, free);
                }
            }
            AS::Throw(e) => {
                collect_free_vars(e, free);
            }
            AS::Panic(e) => {
                collect_free_vars(e, free);
            }
            AS::Try {
                try_blk,
                catches,
                finally_blk,
            } => {
                collect_free_vars_block(try_blk, free);
                for c in catches {
                    collect_free_vars_block(&c.blk, free);
                }
                if let Some(fb) = finally_blk {
                    collect_free_vars_block(fb, free);
                }
            }
            AS::Block(b) => collect_free_vars_block(b, free),
            AS::Unsafe(b) => collect_free_vars_block(b, free),
            AS::Labeled { stmt, .. } => collect_free_vars_stmt(stmt, free, locals),
            _ => {}
        }
    }

    fn detect_borrow_kind(callee: &crate::compiler::ast::Expr) -> Option<HirBorrowKind> {
        use crate::compiler::ast::Expr as AE;
        use crate::compiler::ast::Ident as AIdent;
        match callee {
            AE::Ident(AIdent(name)) if name == "borrowMut" => Some(HirBorrowKind::ExclusiveLocal),
            AE::Ident(AIdent(name)) if name == "borrowFromProvider" => {
                Some(HirBorrowKind::Provider)
            }
            AE::Ident(AIdent(name)) if name == "release" => Some(HirBorrowKind::Release),
            _ => None,
        }
    }

    fn lower_pattern(
        p: &crate::compiler::ast::Pattern,
        file: &Arc<PathBuf>,
        fresh: &mut dyn FnMut() -> HirId,
        span_ctx: &HirSpan,
    ) -> HirPattern {
        use crate::compiler::ast::Pattern as AP;
        match p {
            AP::Wildcard => HirPattern::Wildcard {
                id: fresh(),
                span: span_ctx.clone(),
            },
            AP::Binding(name) => HirPattern::Binding {
                id: fresh(),
                span: span_ctx.clone(),
                name: name.0.clone(),
            },
            AP::Literal(expr) => HirPattern::Literal {
                id: fresh(),
                span: span_ctx.clone(),
                expr: lower_expr(expr, file, fresh, span_ctx),
            },
            AP::Variant {
                enum_ty,
                variant,
                payloads,
                span,
            } => {
                let pat_span = HirSpan {
                    file: file.clone(),
                    start: span.start as u32,
                    end: span.end as u32,
                };
                HirPattern::Variant {
                    id: fresh(),
                    span: pat_span.clone(),
                    enum_name: enum_ty
                        .path
                        .iter()
                        .map(|i| i.0.clone())
                        .collect::<Vec<_>>()
                        .join("."),
                    variant_name: variant.0.clone(),
                    payloads: payloads
                        .iter()
                        .map(|sub| lower_pattern(sub, file, fresh, &pat_span))
                        .collect(),
                }
            }
        }
    }

    fn lower_expr(
        e: &crate::compiler::ast::Expr,
        file: &Arc<PathBuf>,
        fresh: &mut dyn FnMut() -> HirId,
        span_ctx: &HirSpan,
    ) -> HirExpr {
        use crate::compiler::ast::Expr as AE;
        match e {
            AE::FnLiteral(params, body) => {
                // Perform capture analysis
                let mut free_vars = std::collections::HashSet::new();
                collect_free_vars_block(body, &mut free_vars);

                // Remove lambda parameters from free variables
                for p in params {
                    free_vars.remove(&p.name.0);
                }

                // Convert captures to (name, type) tuples
                let captures: Vec<(String, HirType)> = free_vars
                    .into_iter()
                    .map(|name| {
                        (
                            name,
                            HirType::Name {
                                path: vec!["Unknown".to_string()],
                            },
                        )
                    })
                    .collect();

                HirExpr::Lambda {
                    id: fresh(),
                    span: span_ctx.clone(),
                    params: params.iter().map(param_to_hir).collect(),
                    body: lower_block(body, file, fresh),
                    ret: None, // Return type inference happens in type checker
                    captures,
                }
            }
            AE::Int(v) => HirExpr::Int {
                id: fresh(),
                span: span_ctx.clone(),
                value: *v,
            },
            AE::Float(v) => HirExpr::Float {
                id: fresh(),
                span: span_ctx.clone(),
                value: *v,
            },
            AE::Str(s) => HirExpr::Str {
                id: fresh(),
                span: span_ctx.clone(),
                value: s.clone(),
            },
            AE::Char(c) => HirExpr::Char {
                id: fresh(),
                span: span_ctx.clone(),
                value: *c,
            },
            AE::Bool(b) => HirExpr::Bool {
                id: fresh(),
                span: span_ctx.clone(),
                value: *b,
            },
            AE::Await(inner) => HirExpr::Await {
                id: fresh(),
                span: span_ctx.clone(),
                expr: Box::new(lower_expr(inner, file, fresh, span_ctx)),
            },
            AE::Cast(tnp, inner) => HirExpr::Cast {
                id: fresh(),
                span: span_ctx.clone(),
                to: name_path_to_type(tnp),
                expr: Box::new(lower_expr(inner, file, fresh, span_ctx)),
            },
            AE::Ident(Ident(n)) => HirExpr::Ident {
                id: fresh(),
                span: span_ctx.clone(),
                name: n.clone(),
            },
            AE::Binary(l, op, r) => HirExpr::Binary {
                id: fresh(),
                span: span_ctx.clone(),
                left: Box::new(lower_expr(l, file, fresh, span_ctx)),
                op: match op {
                    crate::compiler::ast::BinOp::Add => HirBinOp::Add,
                    crate::compiler::ast::BinOp::Sub => HirBinOp::Sub,
                    crate::compiler::ast::BinOp::Mul => HirBinOp::Mul,
                    crate::compiler::ast::BinOp::Div => HirBinOp::Div,
                    crate::compiler::ast::BinOp::Mod => HirBinOp::Mod,
                    crate::compiler::ast::BinOp::Shl => HirBinOp::Shl,
                    crate::compiler::ast::BinOp::Shr => HirBinOp::Shr,
                    crate::compiler::ast::BinOp::Lt => HirBinOp::Lt,
                    crate::compiler::ast::BinOp::Le => HirBinOp::Le,
                    crate::compiler::ast::BinOp::Gt => HirBinOp::Gt,
                    crate::compiler::ast::BinOp::Ge => HirBinOp::Ge,
                    crate::compiler::ast::BinOp::Eq => HirBinOp::Eq,
                    crate::compiler::ast::BinOp::Ne => HirBinOp::Ne,
                    crate::compiler::ast::BinOp::And => HirBinOp::And,
                    crate::compiler::ast::BinOp::Or => HirBinOp::Or,
                    crate::compiler::ast::BinOp::BitAnd => HirBinOp::BitAnd,
                    crate::compiler::ast::BinOp::BitOr => HirBinOp::BitOr,
                    crate::compiler::ast::BinOp::BitXor => HirBinOp::Xor,
                },
                right: Box::new(lower_expr(r, file, fresh, span_ctx)),
            },
            AE::Unary(op, ex) => HirExpr::Unary {
                id: fresh(),
                span: span_ctx.clone(),
                op: match op {
                    crate::compiler::ast::UnOp::Neg => HirUnOp::Neg,
                    crate::compiler::ast::UnOp::Not => HirUnOp::Not,
                },
                expr: Box::new(lower_expr(ex, file, fresh, span_ctx)),
            },
            AE::Call(callee, args) => {
                let borrow = detect_borrow_kind(callee);
                HirExpr::Call {
                    id: fresh(),
                    span: span_ctx.clone(),
                    callee: Box::new(lower_expr(callee, file, fresh, span_ctx)),
                    args: args
                        .iter()
                        .map(|a| lower_expr(a, file, fresh, span_ctx))
                        .collect(),
                    borrow,
                }
            }
            AE::Member(obj, Ident(m)) => HirExpr::Member {
                id: fresh(),
                span: span_ctx.clone(),
                object: Box::new(lower_expr(obj, file, fresh, span_ctx)),
                member: m.clone(),
            },
            AE::OptionalMember(obj, Ident(m)) => HirExpr::OptionalMember {
                id: fresh(),
                span: span_ctx.clone(),
                object: Box::new(lower_expr(obj, file, fresh, span_ctx)),
                member: m.clone(),
            },
            AE::Index(obj, idx) => HirExpr::Index {
                id: fresh(),
                span: span_ctx.clone(),
                object: Box::new(lower_expr(obj, file, fresh, span_ctx)),
                index: Box::new(lower_expr(idx, file, fresh, span_ctx)),
            },
            AE::ListLit(elems) => HirExpr::ListLit {
                id: fresh(),
                span: span_ctx.clone(),
                elements: elems
                    .iter()
                    .map(|e| lower_expr(e, file, fresh, span_ctx))
                    .collect(),
            },
            AE::MapLit { pairs, spread } => HirExpr::MapLit {
                id: fresh(),
                span: span_ctx.clone(),
                pairs: pairs
                    .iter()
                    .map(|(k, v)| {
                        (
                            lower_expr(k, file, fresh, span_ctx),
                            lower_expr(v, file, fresh, span_ctx),
                        )
                    })
                    .collect(),
                spread: spread
                    .as_ref()
                    .map(|s| Box::new(lower_expr(s, file, fresh, span_ctx))),
            },
            AE::StructLit {
                type_name,
                fields,
                spread,
            } => {
                // Convert the type name expression to a HirType
                let ty = expr_to_hir_type(type_name);
                HirExpr::StructLit {
                    id: fresh(),
                    span: span_ctx.clone(),
                    type_name: ty,
                    fields: fields
                        .iter()
                        .map(|(Ident(name), val)| {
                            (name.clone(), lower_expr(val, file, fresh, span_ctx))
                        })
                        .collect(),
                    spread: spread
                        .as_ref()
                        .map(|s| Box::new(lower_expr(s, file, fresh, span_ctx))),
                }
            }
            AE::Ternary(c, t, f) => HirExpr::Conditional {
                id: fresh(),
                span: span_ctx.clone(),
                cond: Box::new(lower_expr(c, file, fresh, span_ctx)),
                then_expr: Box::new(lower_expr(t, file, fresh, span_ctx)),
                else_expr: Box::new(lower_expr(f, file, fresh, span_ctx)),
            },
        }
    }

    fn enum_variants_to_hir(
        e: &EnumDecl,
        file: &Arc<PathBuf>,
        fresh: &mut dyn FnMut() -> HirId,
    ) -> Vec<HirEnumVariant> {
        let default_span = HirSpan {
            file: file.clone(),
            start: e.span.start as u32,
            end: e.span.end as u32,
        };

        e.variants
            .iter()
            .map(|v| match v {
                EnumVariant::Unit { name, discriminant } => HirEnumVariant::Unit {
                    name: name.0.clone(),
                    discriminant: discriminant
                        .as_ref()
                        .map(|d| Box::new(lower_expr(d, file, fresh, &default_span))),
                },
                EnumVariant::Tuple {
                    name,
                    types,
                    discriminant,
                } => HirEnumVariant::Tuple {
                    name: name.0.clone(),
                    types: types.iter().map(name_path_to_type).collect(),
                    discriminant: discriminant
                        .as_ref()
                        .map(|d| Box::new(lower_expr(d, file, fresh, &default_span))),
                },
            })
            .collect()
    }

    fn lower_block(
        blk: &crate::compiler::ast::Block,
        file: &Arc<PathBuf>,
        fresh: &mut dyn FnMut() -> HirId,
    ) -> HirBlock {
        use crate::compiler::ast::Stmt as AS;
        let span = to_hir_span(file, &blk.span);
        let mut out: Vec<HirStmt> = Vec::new();
        for s in &blk.stmts {
            let id = fresh();
            let stmt = match s {
                AS::PrintStr(t) => HirStmt::PrintStr {
                    id,
                    span: span.clone(),
                    text: t.clone(),
                },
                AS::PrintExpr(e) => HirStmt::PrintExpr {
                    id,
                    span: span.clone(),
                    expr: lower_expr(e, file, fresh, &span),
                },
                AS::PrintRawStr(t) => HirStmt::PrintRawStr {
                    id,
                    span: span.clone(),
                    text: t.clone(),
                },
                AS::PrintRawExpr(e) => HirStmt::PrintRawExpr {
                    id,
                    span: span.clone(),
                    expr: lower_expr(e, file, fresh, &span),
                },
                AS::If {
                    cond,
                    then_blk,
                    else_blk,
                } => HirStmt::If {
                    id,
                    span: span.clone(),
                    cond: lower_expr(cond, file, fresh, &span),
                    then_blk: lower_block(then_blk, file, fresh),
                    else_blk: else_blk.as_ref().map(|b| lower_block(b, file, fresh)),
                },
                AS::While { cond, body } => HirStmt::While {
                    id,
                    span: span.clone(),
                    cond: lower_expr(cond, file, fresh, &span),
                    body: lower_block(body, file, fresh),
                },
                AS::For {
                    init,
                    cond,
                    step,
                    body,
                } => {
                    // Desugar: for (init; cond; step) body => { init?; while (cond? true) { body; step?; } }
                    let mut stmts: Vec<HirStmt> = Vec::new();
                    if let Some(st) = init.as_ref() {
                        let init_stmt = match &**st {
                            t => match t {
                                AS::PrintStr(t) => HirStmt::PrintStr {
                                    id: fresh(),
                                    span: span.clone(),
                                    text: t.clone(),
                                },
                                AS::PrintExpr(e) => HirStmt::PrintExpr {
                                    id: fresh(),
                                    span: span.clone(),
                                    expr: lower_expr(e, file, fresh, &span),
                                },
                                AS::PrintRawStr(t) => HirStmt::PrintRawStr {
                                    id: fresh(),
                                    span: span.clone(),
                                    text: t.clone(),
                                },
                                AS::PrintRawExpr(e) => HirStmt::PrintRawExpr {
                                    id: fresh(),
                                    span: span.clone(),
                                    expr: lower_expr(e, file, fresh, &span),
                                },
                                AS::If {
                                    cond,
                                    then_blk,
                                    else_blk,
                                } => HirStmt::If {
                                    id: fresh(),
                                    span: span.clone(),
                                    cond: lower_expr(cond, file, fresh, &span),
                                    then_blk: lower_block(then_blk, file, fresh),
                                    else_blk: else_blk
                                        .as_ref()
                                        .map(|b| lower_block(b, file, fresh)),
                                },
                                AS::While { cond, body } => HirStmt::While {
                                    id: fresh(),
                                    span: span.clone(),
                                    cond: lower_expr(cond, file, fresh, &span),
                                    body: lower_block(body, file, fresh),
                                },
                                AS::For { body: b, .. } => {
                                    HirStmt::Block(lower_block(b, file, fresh))
                                }
                                AS::Switch {
                                    expr,
                                    cases,
                                    pattern_cases,
                                    default,
                                } => HirStmt::Switch {
                                    id: fresh(),
                                    span: span.clone(),
                                    expr: lower_expr(expr, file, fresh, &span),
                                    cases: cases
                                        .iter()
                                        .map(|(e, b)| {
                                            (
                                                lower_expr(e, file, fresh, &span),
                                                lower_block(b, file, fresh),
                                            )
                                        })
                                        .collect(),
                                    pattern_cases: pattern_cases
                                        .iter()
                                        .map(|(p, b)| {
                                            (
                                                lower_pattern(p, file, fresh, &span),
                                                lower_block(b, file, fresh),
                                            )
                                        })
                                        .collect(),
                                    default: default.as_ref().map(|b| lower_block(b, file, fresh)),
                                },
                                AS::Try {
                                    try_blk,
                                    catches,
                                    finally_blk,
                                } => HirStmt::Try {
                                    id: fresh(),
                                    span: span.clone(),
                                    try_blk: lower_block(try_blk, file, fresh),
                                    catches: catches
                                        .iter()
                                        .map(|c| {
                                            let catch_span = to_hir_span(file, &c.blk.span);
                                            HirCatchClause {
                                                id: fresh(),
                                                span: catch_span,
                                                ty: c.ty.as_ref().map(name_path_to_type),
                                                var: c.var.as_ref().map(|Ident(s)| s.clone()),
                                                block: lower_block(&c.blk, file, fresh),
                                            }
                                        })
                                        .collect(),
                                    finally_blk: finally_blk
                                        .as_ref()
                                        .map(|b| lower_block(b, file, fresh)),
                                },
                                AS::Assign { name, expr } => HirStmt::Assign {
                                    id: fresh(),
                                    span: span.clone(),
                                    name: name.0.clone(),
                                    expr: lower_expr(expr, file, fresh, &span),
                                },
                                AS::FieldAssign {
                                    object,
                                    field,
                                    expr,
                                } => HirStmt::FieldAssign {
                                    id: fresh(),
                                    span: span.clone(),
                                    object: lower_expr(object, file, fresh, &span),
                                    field: field.0.clone(),
                                    expr: lower_expr(expr, file, fresh, &span),
                                },
                                AS::AssignOp { name, op, expr } => HirStmt::AssignOp {
                                    id: fresh(),
                                    span: span.clone(),
                                    name: name.0.clone(),
                                    op: match op {
                                        crate::compiler::ast::AssignOp::Add => HirAssignOp::Add,
                                        crate::compiler::ast::AssignOp::Sub => HirAssignOp::Sub,
                                        crate::compiler::ast::AssignOp::Mul => HirAssignOp::Mul,
                                        crate::compiler::ast::AssignOp::Div => HirAssignOp::Div,
                                        crate::compiler::ast::AssignOp::Mod => HirAssignOp::Mod,
                                        crate::compiler::ast::AssignOp::Shl => HirAssignOp::Shl,
                                        crate::compiler::ast::AssignOp::Shr => HirAssignOp::Shr,
                                        crate::compiler::ast::AssignOp::And => HirAssignOp::And,
                                        crate::compiler::ast::AssignOp::Or => HirAssignOp::Or,
                                        crate::compiler::ast::AssignOp::Xor => HirAssignOp::Xor,
                                    },
                                    expr: lower_expr(expr, file, fresh, &span),
                                },
                                AS::VarDecl {
                                    ty,
                                    name,
                                    init,
                                    is_shared,
                                    ..
                                } => HirStmt::VarDecl {
                                    id: fresh(),
                                    span: span.clone(),
                                    ty: name_path_to_type(ty),
                                    name: name.0.clone(),
                                    init: init.as_ref().map(|e| lower_expr(e, file, fresh, &span)),
                                    is_shared: *is_shared,
                                },
                                AS::Break(lbl) => HirStmt::Break {
                                    id: fresh(),
                                    span: span.clone(),
                                    label: lbl.as_ref().map(|i| i.0.clone()),
                                },
                                AS::Continue(lbl) => HirStmt::Continue {
                                    id: fresh(),
                                    span: span.clone(),
                                    label: lbl.as_ref().map(|i| i.0.clone()),
                                },
                                AS::Return(eopt) => HirStmt::Return {
                                    id: fresh(),
                                    span: span.clone(),
                                    expr: eopt.as_ref().map(|e| lower_expr(e, file, fresh, &span)),
                                },
                                AS::Throw(e) => HirStmt::Throw {
                                    id: fresh(),
                                    span: span.clone(),
                                    expr: lower_expr(e, file, fresh, &span),
                                },
                                AS::Panic(e) => HirStmt::Panic {
                                    id: fresh(),
                                    span: span.clone(),
                                    msg: lower_expr(e, file, fresh, &span),
                                },
                                AS::Block(b) => HirStmt::Block(lower_block(b, file, fresh)),
                                AS::Unsafe(b) => HirStmt::Unsafe {
                                    id: fresh(),
                                    span: span.clone(),
                                    block: lower_block(b, file, fresh),
                                },
                                AS::Expr(e) => HirStmt::Expr {
                                    id: fresh(),
                                    span: span.clone(),
                                    expr: lower_expr(e, file, fresh, &span),
                                },
                                AS::Labeled { label, stmt } => {
                                    let inner = match &**stmt {
                                        t => match t {
                                            AS::PrintStr(t) => HirStmt::PrintStr {
                                                id: fresh(),
                                                span: span.clone(),
                                                text: t.clone(),
                                            },
                                            AS::PrintExpr(e) => HirStmt::PrintExpr {
                                                id: fresh(),
                                                span: span.clone(),
                                                expr: lower_expr(e, file, fresh, &span),
                                            },
                                            AS::PrintRawStr(t) => HirStmt::PrintRawStr {
                                                id: fresh(),
                                                span: span.clone(),
                                                text: t.clone(),
                                            },
                                            AS::PrintRawExpr(e) => HirStmt::PrintRawExpr {
                                                id: fresh(),
                                                span: span.clone(),
                                                expr: lower_expr(e, file, fresh, &span),
                                            },
                                            AS::If {
                                                cond,
                                                then_blk,
                                                else_blk,
                                            } => HirStmt::If {
                                                id: fresh(),
                                                span: span.clone(),
                                                cond: lower_expr(cond, file, fresh, &span),
                                                then_blk: lower_block(then_blk, file, fresh),
                                                else_blk: else_blk
                                                    .as_ref()
                                                    .map(|b| lower_block(b, file, fresh)),
                                            },
                                            AS::While { cond, body } => HirStmt::While {
                                                id: fresh(),
                                                span: span.clone(),
                                                cond: lower_expr(cond, file, fresh, &span),
                                                body: lower_block(body, file, fresh),
                                            },
                                            AS::For { body: b, .. } => {
                                                HirStmt::Block(lower_block(b, file, fresh))
                                            }
                                            AS::Switch {
                                                expr,
                                                cases,
                                                pattern_cases,
                                                default,
                                            } => HirStmt::Switch {
                                                id: fresh(),
                                                span: span.clone(),
                                                expr: lower_expr(expr, file, fresh, &span),
                                                cases: cases
                                                    .iter()
                                                    .map(|(e, b)| {
                                                        (
                                                            lower_expr(e, file, fresh, &span),
                                                            lower_block(b, file, fresh),
                                                        )
                                                    })
                                                    .collect(),
                                                pattern_cases: pattern_cases
                                                    .iter()
                                                    .map(|(p, b)| {
                                                        (
                                                            lower_pattern(p, file, fresh, &span),
                                                            lower_block(b, file, fresh),
                                                        )
                                                    })
                                                    .collect(),
                                                default: default
                                                    .as_ref()
                                                    .map(|b| lower_block(b, file, fresh)),
                                            },
                                            AS::Try {
                                                try_blk,
                                                catches,
                                                finally_blk,
                                            } => HirStmt::Try {
                                                id: fresh(),
                                                span: span.clone(),
                                                try_blk: lower_block(try_blk, file, fresh),
                                                catches: catches
                                                    .iter()
                                                    .map(|c| {
                                                        let catch_span =
                                                            to_hir_span(file, &c.blk.span);
                                                        HirCatchClause {
                                                            id: fresh(),
                                                            span: catch_span,
                                                            ty: c
                                                                .ty
                                                                .as_ref()
                                                                .map(name_path_to_type),
                                                            var: c
                                                                .var
                                                                .as_ref()
                                                                .map(|Ident(s)| s.clone()),
                                                            block: lower_block(&c.blk, file, fresh),
                                                        }
                                                    })
                                                    .collect(),
                                                finally_blk: finally_blk
                                                    .as_ref()
                                                    .map(|b| lower_block(b, file, fresh)),
                                            },
                                            AS::Assign { name, expr } => HirStmt::Assign {
                                                id: fresh(),
                                                span: span.clone(),
                                                name: name.0.clone(),
                                                expr: lower_expr(expr, file, fresh, &span),
                                            },
                                            AS::FieldAssign {
                                                object,
                                                field,
                                                expr,
                                            } => HirStmt::FieldAssign {
                                                id: fresh(),
                                                span: span.clone(),
                                                object: lower_expr(object, file, fresh, &span),
                                                field: field.0.clone(),
                                                expr: lower_expr(expr, file, fresh, &span),
                                            },
                                            AS::AssignOp { name, op, expr } => HirStmt::AssignOp {
                                                id: fresh(),
                                                span: span.clone(),
                                                name: name.0.clone(),
                                                op: match op {
                                                    crate::compiler::ast::AssignOp::Add => {
                                                        HirAssignOp::Add
                                                    }
                                                    crate::compiler::ast::AssignOp::Sub => {
                                                        HirAssignOp::Sub
                                                    }
                                                    crate::compiler::ast::AssignOp::Mul => {
                                                        HirAssignOp::Mul
                                                    }
                                                    crate::compiler::ast::AssignOp::Div => {
                                                        HirAssignOp::Div
                                                    }
                                                    crate::compiler::ast::AssignOp::Mod => {
                                                        HirAssignOp::Mod
                                                    }
                                                    crate::compiler::ast::AssignOp::Shl => {
                                                        HirAssignOp::Shl
                                                    }
                                                    crate::compiler::ast::AssignOp::Shr => {
                                                        HirAssignOp::Shr
                                                    }
                                                    crate::compiler::ast::AssignOp::And => {
                                                        HirAssignOp::And
                                                    }
                                                    crate::compiler::ast::AssignOp::Or => {
                                                        HirAssignOp::Or
                                                    }
                                                    crate::compiler::ast::AssignOp::Xor => {
                                                        HirAssignOp::Xor
                                                    }
                                                },
                                                expr: lower_expr(expr, file, fresh, &span),
                                            },
                                            AS::VarDecl {
                                                ty,
                                                name,
                                                init,
                                                is_shared,
                                                ..
                                            } => HirStmt::VarDecl {
                                                id: fresh(),
                                                span: span.clone(),
                                                ty: name_path_to_type(ty),
                                                name: name.0.clone(),
                                                init: init
                                                    .as_ref()
                                                    .map(|e| lower_expr(e, file, fresh, &span)),
                                                is_shared: *is_shared,
                                            },
                                            AS::Break(lbl) => HirStmt::Break {
                                                id: fresh(),
                                                span: span.clone(),
                                                label: lbl.as_ref().map(|i| i.0.clone()),
                                            },
                                            AS::Continue(lbl) => HirStmt::Continue {
                                                id: fresh(),
                                                span: span.clone(),
                                                label: lbl.as_ref().map(|i| i.0.clone()),
                                            },
                                            AS::Return(eopt) => HirStmt::Return {
                                                id: fresh(),
                                                span: span.clone(),
                                                expr: eopt
                                                    .as_ref()
                                                    .map(|e| lower_expr(e, file, fresh, &span)),
                                            },
                                            AS::Throw(e) => HirStmt::Throw {
                                                id: fresh(),
                                                span: span.clone(),
                                                expr: lower_expr(e, file, fresh, &span),
                                            },
                                            AS::Panic(e) => HirStmt::Panic {
                                                id: fresh(),
                                                span: span.clone(),
                                                msg: lower_expr(e, file, fresh, &span),
                                            },
                                            AS::Block(b) => {
                                                HirStmt::Block(lower_block(b, file, fresh))
                                            }
                                            AS::Unsafe(b) => HirStmt::Unsafe {
                                                id: fresh(),
                                                span: span.clone(),
                                                block: lower_block(b, file, fresh),
                                            },
                                            AS::Expr(e) => HirStmt::Expr {
                                                id: fresh(),
                                                span: span.clone(),
                                                expr: lower_expr(e, file, fresh, &span),
                                            },
                                            AS::Labeled { .. } => HirStmt::Block(HirBlock {
                                                id: fresh(),
                                                span: span.clone(),
                                                stmts: vec![],
                                            }),
                                        },
                                    };
                                    HirStmt::Labeled {
                                        id: fresh(),
                                        span: span.clone(),
                                        label: label.0.clone(),
                                        stmt: Box::new(inner),
                                    }
                                }
                            },
                        };
                        stmts.push(init_stmt);
                    }
                    let cond_hir = cond
                        .as_ref()
                        .map(|e| lower_expr(e, file, fresh, &span))
                        .unwrap_or_else(|| HirExpr::Bool {
                            id: fresh(),
                            span: span.clone(),
                            value: true,
                        });
                    let mut body_hir = lower_block(body, file, fresh);
                    if let Some(st) = step.as_ref() {
                        let step_stmt = match &**st {
                            t => match t {
                                AS::PrintStr(t) => HirStmt::PrintStr {
                                    id: fresh(),
                                    span: span.clone(),
                                    text: t.clone(),
                                },
                                AS::PrintExpr(e) => HirStmt::PrintExpr {
                                    id: fresh(),
                                    span: span.clone(),
                                    expr: lower_expr(e, file, fresh, &span),
                                },
                                AS::PrintRawStr(t) => HirStmt::PrintRawStr {
                                    id: fresh(),
                                    span: span.clone(),
                                    text: t.clone(),
                                },
                                AS::PrintRawExpr(e) => HirStmt::PrintRawExpr {
                                    id: fresh(),
                                    span: span.clone(),
                                    expr: lower_expr(e, file, fresh, &span),
                                },
                                AS::If {
                                    cond,
                                    then_blk,
                                    else_blk,
                                } => HirStmt::If {
                                    id: fresh(),
                                    span: span.clone(),
                                    cond: lower_expr(cond, file, fresh, &span),
                                    then_blk: lower_block(then_blk, file, fresh),
                                    else_blk: else_blk
                                        .as_ref()
                                        .map(|b| lower_block(b, file, fresh)),
                                },
                                AS::While { cond, body } => HirStmt::While {
                                    id: fresh(),
                                    span: span.clone(),
                                    cond: lower_expr(cond, file, fresh, &span),
                                    body: lower_block(body, file, fresh),
                                },
                                AS::For { body: b, .. } => {
                                    HirStmt::Block(lower_block(b, file, fresh))
                                }
                                AS::Switch {
                                    expr,
                                    cases,
                                    pattern_cases,
                                    default,
                                } => HirStmt::Switch {
                                    id: fresh(),
                                    span: span.clone(),
                                    expr: lower_expr(expr, file, fresh, &span),
                                    cases: cases
                                        .iter()
                                        .map(|(e, b)| {
                                            (
                                                lower_expr(e, file, fresh, &span),
                                                lower_block(b, file, fresh),
                                            )
                                        })
                                        .collect(),
                                    pattern_cases: pattern_cases
                                        .iter()
                                        .map(|(p, b)| {
                                            (
                                                lower_pattern(p, file, fresh, &span),
                                                lower_block(b, file, fresh),
                                            )
                                        })
                                        .collect(),
                                    default: default.as_ref().map(|b| lower_block(b, file, fresh)),
                                },
                                AS::Try {
                                    try_blk,
                                    catches,
                                    finally_blk,
                                } => HirStmt::Try {
                                    id: fresh(),
                                    span: span.clone(),
                                    try_blk: lower_block(try_blk, file, fresh),
                                    catches: catches
                                        .iter()
                                        .map(|c| {
                                            let catch_span = to_hir_span(file, &c.blk.span);
                                            HirCatchClause {
                                                id: fresh(),
                                                span: catch_span,
                                                ty: c.ty.as_ref().map(name_path_to_type),
                                                var: c.var.as_ref().map(|Ident(s)| s.clone()),
                                                block: lower_block(&c.blk, file, fresh),
                                            }
                                        })
                                        .collect(),
                                    finally_blk: finally_blk
                                        .as_ref()
                                        .map(|b| lower_block(b, file, fresh)),
                                },
                                AS::Assign { name, expr } => HirStmt::Assign {
                                    id: fresh(),
                                    span: span.clone(),
                                    name: name.0.clone(),
                                    expr: lower_expr(expr, file, fresh, &span),
                                },
                                AS::FieldAssign {
                                    object,
                                    field,
                                    expr,
                                } => HirStmt::FieldAssign {
                                    id: fresh(),
                                    span: span.clone(),
                                    object: lower_expr(object, file, fresh, &span),
                                    field: field.0.clone(),
                                    expr: lower_expr(expr, file, fresh, &span),
                                },
                                AS::AssignOp { name, op, expr } => HirStmt::AssignOp {
                                    id: fresh(),
                                    span: span.clone(),
                                    name: name.0.clone(),
                                    op: match op {
                                        crate::compiler::ast::AssignOp::Add => HirAssignOp::Add,
                                        crate::compiler::ast::AssignOp::Sub => HirAssignOp::Sub,
                                        crate::compiler::ast::AssignOp::Mul => HirAssignOp::Mul,
                                        crate::compiler::ast::AssignOp::Div => HirAssignOp::Div,
                                        crate::compiler::ast::AssignOp::Mod => HirAssignOp::Mod,
                                        crate::compiler::ast::AssignOp::Shl => HirAssignOp::Shl,
                                        crate::compiler::ast::AssignOp::Shr => HirAssignOp::Shr,
                                        crate::compiler::ast::AssignOp::And => HirAssignOp::And,
                                        crate::compiler::ast::AssignOp::Or => HirAssignOp::Or,
                                        crate::compiler::ast::AssignOp::Xor => HirAssignOp::Xor,
                                    },
                                    expr: lower_expr(expr, file, fresh, &span),
                                },
                                AS::VarDecl {
                                    ty,
                                    name,
                                    init,
                                    is_shared,
                                    ..
                                } => HirStmt::VarDecl {
                                    id: fresh(),
                                    span: span.clone(),
                                    ty: name_path_to_type(ty),
                                    name: name.0.clone(),
                                    init: init.as_ref().map(|e| lower_expr(e, file, fresh, &span)),
                                    is_shared: *is_shared,
                                },
                                AS::Break(lbl) => HirStmt::Break {
                                    id: fresh(),
                                    span: span.clone(),
                                    label: lbl.as_ref().map(|i| i.0.clone()),
                                },
                                AS::Continue(lbl) => HirStmt::Continue {
                                    id: fresh(),
                                    span: span.clone(),
                                    label: lbl.as_ref().map(|i| i.0.clone()),
                                },
                                AS::Return(eopt) => HirStmt::Return {
                                    id: fresh(),
                                    span: span.clone(),
                                    expr: eopt.as_ref().map(|e| lower_expr(e, file, fresh, &span)),
                                },
                                AS::Throw(e) => HirStmt::Throw {
                                    id: fresh(),
                                    span: span.clone(),
                                    expr: lower_expr(e, file, fresh, &span),
                                },
                                AS::Panic(e) => HirStmt::Panic {
                                    id: fresh(),
                                    span: span.clone(),
                                    msg: lower_expr(e, file, fresh, &span),
                                },
                                AS::Block(b) => HirStmt::Block(lower_block(b, file, fresh)),
                                AS::Unsafe(b) => HirStmt::Unsafe {
                                    id: fresh(),
                                    span: span.clone(),
                                    block: lower_block(b, file, fresh),
                                },
                                AS::Expr(e) => HirStmt::Expr {
                                    id: fresh(),
                                    span: span.clone(),
                                    expr: lower_expr(e, file, fresh, &span),
                                },
                                AS::Labeled { label, stmt } => {
                                    // Lower inner stmt to HIR and wrap in a label node
                                    let inner = match &**stmt {
                                        t => match t {
                                            AS::PrintStr(t) => HirStmt::PrintStr {
                                                id: fresh(),
                                                span: span.clone(),
                                                text: t.clone(),
                                            },
                                            AS::PrintExpr(e) => HirStmt::PrintExpr {
                                                id: fresh(),
                                                span: span.clone(),
                                                expr: lower_expr(e, file, fresh, &span),
                                            },
                                            AS::PrintRawStr(t) => HirStmt::PrintRawStr {
                                                id: fresh(),
                                                span: span.clone(),
                                                text: t.clone(),
                                            },
                                            AS::PrintRawExpr(e) => HirStmt::PrintRawExpr {
                                                id: fresh(),
                                                span: span.clone(),
                                                expr: lower_expr(e, file, fresh, &span),
                                            },
                                            AS::If {
                                                cond,
                                                then_blk,
                                                else_blk,
                                            } => HirStmt::If {
                                                id: fresh(),
                                                span: span.clone(),
                                                cond: lower_expr(cond, file, fresh, &span),
                                                then_blk: lower_block(then_blk, file, fresh),
                                                else_blk: else_blk
                                                    .as_ref()
                                                    .map(|b| lower_block(b, file, fresh)),
                                            },
                                            AS::While { cond, body } => HirStmt::While {
                                                id: fresh(),
                                                span: span.clone(),
                                                cond: lower_expr(cond, file, fresh, &span),
                                                body: lower_block(body, file, fresh),
                                            },
                                            AS::For { .. } => {
                                                HirStmt::Block(lower_block(body, file, fresh))
                                            }
                                            AS::Switch {
                                                expr,
                                                cases,
                                                pattern_cases,
                                                default,
                                            } => HirStmt::Switch {
                                                id: fresh(),
                                                span: span.clone(),
                                                expr: lower_expr(expr, file, fresh, &span),
                                                cases: cases
                                                    .iter()
                                                    .map(|(e, b)| {
                                                        (
                                                            lower_expr(e, file, fresh, &span),
                                                            lower_block(b, file, fresh),
                                                        )
                                                    })
                                                    .collect(),
                                                pattern_cases: pattern_cases
                                                    .iter()
                                                    .map(|(p, b)| {
                                                        (
                                                            lower_pattern(p, file, fresh, &span),
                                                            lower_block(b, file, fresh),
                                                        )
                                                    })
                                                    .collect(),
                                                default: default
                                                    .as_ref()
                                                    .map(|b| lower_block(b, file, fresh)),
                                            },
                                            AS::Try {
                                                try_blk,
                                                catches,
                                                finally_blk,
                                            } => HirStmt::Try {
                                                id: fresh(),
                                                span: span.clone(),
                                                try_blk: lower_block(try_blk, file, fresh),
                                                catches: catches
                                                    .iter()
                                                    .map(|c| {
                                                        let catch_span =
                                                            to_hir_span(file, &c.blk.span);
                                                        HirCatchClause {
                                                            id: fresh(),
                                                            span: catch_span,
                                                            ty: c
                                                                .ty
                                                                .as_ref()
                                                                .map(name_path_to_type),
                                                            var: c
                                                                .var
                                                                .as_ref()
                                                                .map(|Ident(s)| s.clone()),
                                                            block: lower_block(&c.blk, file, fresh),
                                                        }
                                                    })
                                                    .collect(),
                                                finally_blk: finally_blk
                                                    .as_ref()
                                                    .map(|b| lower_block(b, file, fresh)),
                                            },
                                            AS::Assign { name, expr } => HirStmt::Assign {
                                                id: fresh(),
                                                span: span.clone(),
                                                name: name.0.clone(),
                                                expr: lower_expr(expr, file, fresh, &span),
                                            },
                                            AS::FieldAssign {
                                                object,
                                                field,
                                                expr,
                                            } => HirStmt::FieldAssign {
                                                id: fresh(),
                                                span: span.clone(),
                                                object: lower_expr(object, file, fresh, &span),
                                                field: field.0.clone(),
                                                expr: lower_expr(expr, file, fresh, &span),
                                            },
                                            AS::AssignOp { name, op, expr } => HirStmt::AssignOp {
                                                id: fresh(),
                                                span: span.clone(),
                                                name: name.0.clone(),
                                                op: match op {
                                                    crate::compiler::ast::AssignOp::Add => {
                                                        HirAssignOp::Add
                                                    }
                                                    crate::compiler::ast::AssignOp::Sub => {
                                                        HirAssignOp::Sub
                                                    }
                                                    crate::compiler::ast::AssignOp::Mul => {
                                                        HirAssignOp::Mul
                                                    }
                                                    crate::compiler::ast::AssignOp::Div => {
                                                        HirAssignOp::Div
                                                    }
                                                    crate::compiler::ast::AssignOp::Mod => {
                                                        HirAssignOp::Mod
                                                    }
                                                    crate::compiler::ast::AssignOp::Shl => {
                                                        HirAssignOp::Shl
                                                    }
                                                    crate::compiler::ast::AssignOp::Shr => {
                                                        HirAssignOp::Shr
                                                    }
                                                    crate::compiler::ast::AssignOp::And => {
                                                        HirAssignOp::And
                                                    }
                                                    crate::compiler::ast::AssignOp::Or => {
                                                        HirAssignOp::Or
                                                    }
                                                    crate::compiler::ast::AssignOp::Xor => {
                                                        HirAssignOp::Xor
                                                    }
                                                },
                                                expr: lower_expr(expr, file, fresh, &span),
                                            },
                                            AS::VarDecl {
                                                ty,
                                                name,
                                                init,
                                                is_shared,
                                                ..
                                            } => HirStmt::VarDecl {
                                                id: fresh(),
                                                span: span.clone(),
                                                ty: name_path_to_type(ty),
                                                name: name.0.clone(),
                                                init: init
                                                    .as_ref()
                                                    .map(|e| lower_expr(e, file, fresh, &span)),
                                                is_shared: *is_shared,
                                            },
                                            AS::Break(lbl) => HirStmt::Break {
                                                id: fresh(),
                                                span: span.clone(),
                                                label: lbl.as_ref().map(|i| i.0.clone()),
                                            },
                                            AS::Continue(lbl) => HirStmt::Continue {
                                                id: fresh(),
                                                span: span.clone(),
                                                label: lbl.as_ref().map(|i| i.0.clone()),
                                            },
                                            AS::Return(eopt) => HirStmt::Return {
                                                id: fresh(),
                                                span: span.clone(),
                                                expr: eopt
                                                    .as_ref()
                                                    .map(|e| lower_expr(e, file, fresh, &span)),
                                            },
                                            AS::Throw(e) => HirStmt::Throw {
                                                id: fresh(),
                                                span: span.clone(),
                                                expr: lower_expr(e, file, fresh, &span),
                                            },
                                            AS::Panic(e) => HirStmt::Panic {
                                                id: fresh(),
                                                span: span.clone(),
                                                msg: lower_expr(e, file, fresh, &span),
                                            },
                                            AS::Block(b) => {
                                                HirStmt::Block(lower_block(b, file, fresh))
                                            }
                                            AS::Unsafe(b) => HirStmt::Unsafe {
                                                id: fresh(),
                                                span: span.clone(),
                                                block: lower_block(b, file, fresh),
                                            },
                                            AS::Expr(e) => HirStmt::Expr {
                                                id: fresh(),
                                                span: span.clone(),
                                                expr: lower_expr(e, file, fresh, &span),
                                            },
                                            AS::Labeled { .. } => {
                                                HirStmt::Block(lower_block(body, file, fresh))
                                            }
                                        },
                                    };
                                    HirStmt::Labeled {
                                        id: fresh(),
                                        span: span.clone(),
                                        label: label.0.clone(),
                                        stmt: Box::new(inner),
                                    }
                                }
                            },
                        };
                        body_hir.stmts.push(step_stmt);
                    }
                    let while_stmt = HirStmt::While {
                        id: fresh(),
                        span: span.clone(),
                        cond: cond_hir,
                        body: body_hir,
                    };
                    stmts.push(while_stmt);
                    HirStmt::Block(HirBlock {
                        id: fresh(),
                        span: span.clone(),
                        stmts,
                    })
                }
                AS::Switch {
                    expr,
                    cases,
                    pattern_cases,
                    default,
                } => HirStmt::Switch {
                    id,
                    span: span.clone(),
                    expr: lower_expr(expr, file, fresh, &span),
                    cases: cases
                        .iter()
                        .map(|(e, b)| {
                            (
                                lower_expr(e, file, fresh, &span),
                                lower_block(b, file, fresh),
                            )
                        })
                        .collect(),
                    pattern_cases: pattern_cases
                        .iter()
                        .map(|(p, b)| {
                            (
                                lower_pattern(p, file, fresh, &span),
                                lower_block(b, file, fresh),
                            )
                        })
                        .collect(),
                    default: default.as_ref().map(|b| lower_block(b, file, fresh)),
                },
                AS::Try {
                    try_blk,
                    catches,
                    finally_blk,
                } => HirStmt::Try {
                    id,
                    span: span.clone(),
                    try_blk: lower_block(try_blk, file, fresh),
                    catches: catches
                        .iter()
                        .map(|c| {
                            let catch_span = to_hir_span(file, &c.blk.span);
                            HirCatchClause {
                                id: fresh(),
                                span: catch_span,
                                ty: c.ty.as_ref().map(name_path_to_type),
                                var: c.var.as_ref().map(|Ident(s)| s.clone()),
                                block: lower_block(&c.blk, file, fresh),
                            }
                        })
                        .collect(),
                    finally_blk: finally_blk.as_ref().map(|b| lower_block(b, file, fresh)),
                },
                AS::Assign { name, expr } => HirStmt::Assign {
                    id,
                    span: span.clone(),
                    name: name.0.clone(),
                    expr: lower_expr(expr, file, fresh, &span),
                },
                AS::FieldAssign {
                    object,
                    field,
                    expr,
                } => HirStmt::FieldAssign {
                    id,
                    span: span.clone(),
                    object: lower_expr(object, file, fresh, &span),
                    field: field.0.clone(),
                    expr: lower_expr(expr, file, fresh, &span),
                },
                AS::AssignOp { name, op, expr } => HirStmt::AssignOp {
                    id,
                    span: span.clone(),
                    name: name.0.clone(),
                    op: match op {
                        crate::compiler::ast::AssignOp::Add => HirAssignOp::Add,
                        crate::compiler::ast::AssignOp::Sub => HirAssignOp::Sub,
                        crate::compiler::ast::AssignOp::Mul => HirAssignOp::Mul,
                        crate::compiler::ast::AssignOp::Div => HirAssignOp::Div,
                        crate::compiler::ast::AssignOp::Mod => HirAssignOp::Mod,
                        crate::compiler::ast::AssignOp::Shl => HirAssignOp::Shl,
                        crate::compiler::ast::AssignOp::Shr => HirAssignOp::Shr,
                        crate::compiler::ast::AssignOp::And => HirAssignOp::And,
                        crate::compiler::ast::AssignOp::Or => HirAssignOp::Or,
                        crate::compiler::ast::AssignOp::Xor => HirAssignOp::Xor,
                    },
                    expr: lower_expr(expr, file, fresh, &span),
                },
                AS::VarDecl {
                    ty,
                    name,
                    init,
                    is_shared,
                    ..
                } => HirStmt::VarDecl {
                    id,
                    span: span.clone(),
                    ty: name_path_to_type(ty),
                    name: name.0.clone(),
                    init: init.as_ref().map(|e| lower_expr(e, file, fresh, &span)),
                    is_shared: *is_shared,
                },
                AS::Break(lbl) => HirStmt::Break {
                    id,
                    span: span.clone(),
                    label: lbl.as_ref().map(|i| i.0.clone()),
                },
                AS::Continue(lbl) => HirStmt::Continue {
                    id,
                    span: span.clone(),
                    label: lbl.as_ref().map(|i| i.0.clone()),
                },
                AS::Labeled { label, stmt } => {
                    // Lower inner stmt similarly and wrap as labeled
                    let inner = match &**stmt {
                        t => match t {
                            AS::PrintStr(t) => HirStmt::PrintStr {
                                id: fresh(),
                                span: span.clone(),
                                text: t.clone(),
                            },
                            AS::PrintExpr(e) => HirStmt::PrintExpr {
                                id: fresh(),
                                span: span.clone(),
                                expr: lower_expr(e, file, fresh, &span),
                            },
                            AS::PrintRawStr(t) => HirStmt::PrintRawStr {
                                id: fresh(),
                                span: span.clone(),
                                text: t.clone(),
                            },
                            AS::PrintRawExpr(e) => HirStmt::PrintRawExpr {
                                id: fresh(),
                                span: span.clone(),
                                expr: lower_expr(e, file, fresh, &span),
                            },
                            AS::If {
                                cond,
                                then_blk,
                                else_blk,
                            } => HirStmt::If {
                                id: fresh(),
                                span: span.clone(),
                                cond: lower_expr(cond, file, fresh, &span),
                                then_blk: lower_block(then_blk, file, fresh),
                                else_blk: else_blk.as_ref().map(|b| lower_block(b, file, fresh)),
                            },
                            AS::While { cond, body } => HirStmt::While {
                                id: fresh(),
                                span: span.clone(),
                                cond: lower_expr(cond, file, fresh, &span),
                                body: lower_block(body, file, fresh),
                            },
                            AS::For { body: b, .. } => HirStmt::Block(lower_block(b, file, fresh)),
                            AS::Switch {
                                expr,
                                cases,
                                pattern_cases,
                                default,
                            } => HirStmt::Switch {
                                id: fresh(),
                                span: span.clone(),
                                expr: lower_expr(expr, file, fresh, &span),
                                cases: cases
                                    .iter()
                                    .map(|(e, b)| {
                                        (
                                            lower_expr(e, file, fresh, &span),
                                            lower_block(b, file, fresh),
                                        )
                                    })
                                    .collect(),
                                pattern_cases: pattern_cases
                                    .iter()
                                    .map(|(p, b)| {
                                        (
                                            lower_pattern(p, file, fresh, &span),
                                            lower_block(b, file, fresh),
                                        )
                                    })
                                    .collect(),
                                default: default.as_ref().map(|b| lower_block(b, file, fresh)),
                            },
                            AS::Try {
                                try_blk,
                                catches,
                                finally_blk,
                            } => HirStmt::Try {
                                id: fresh(),
                                span: span.clone(),
                                try_blk: lower_block(try_blk, file, fresh),
                                catches: catches
                                    .iter()
                                    .map(|c| {
                                        let catch_span = to_hir_span(file, &c.blk.span);
                                        HirCatchClause {
                                            id: fresh(),
                                            span: catch_span,
                                            ty: c.ty.as_ref().map(name_path_to_type),
                                            var: c.var.as_ref().map(|Ident(s)| s.clone()),
                                            block: lower_block(&c.blk, file, fresh),
                                        }
                                    })
                                    .collect(),
                                finally_blk: finally_blk
                                    .as_ref()
                                    .map(|b| lower_block(b, file, fresh)),
                            },
                            AS::Assign { name, expr } => HirStmt::Assign {
                                id: fresh(),
                                span: span.clone(),
                                name: name.0.clone(),
                                expr: lower_expr(expr, file, fresh, &span),
                            },
                            AS::FieldAssign {
                                object,
                                field,
                                expr,
                            } => HirStmt::FieldAssign {
                                id: fresh(),
                                span: span.clone(),
                                object: lower_expr(object, file, fresh, &span),
                                field: field.0.clone(),
                                expr: lower_expr(expr, file, fresh, &span),
                            },
                            AS::AssignOp { name, op, expr } => HirStmt::AssignOp {
                                id: fresh(),
                                span: span.clone(),
                                name: name.0.clone(),
                                op: match op {
                                    crate::compiler::ast::AssignOp::Add => HirAssignOp::Add,
                                    crate::compiler::ast::AssignOp::Sub => HirAssignOp::Sub,
                                    crate::compiler::ast::AssignOp::Mul => HirAssignOp::Mul,
                                    crate::compiler::ast::AssignOp::Div => HirAssignOp::Div,
                                    crate::compiler::ast::AssignOp::Mod => HirAssignOp::Mod,
                                    crate::compiler::ast::AssignOp::Shl => HirAssignOp::Shl,
                                    crate::compiler::ast::AssignOp::Shr => HirAssignOp::Shr,
                                    crate::compiler::ast::AssignOp::And => HirAssignOp::And,
                                    crate::compiler::ast::AssignOp::Or => HirAssignOp::Or,
                                    crate::compiler::ast::AssignOp::Xor => HirAssignOp::Xor,
                                },
                                expr: lower_expr(expr, file, fresh, &span),
                            },
                            AS::VarDecl {
                                ty,
                                name,
                                init,
                                is_shared,
                                ..
                            } => HirStmt::VarDecl {
                                id: fresh(),
                                span: span.clone(),
                                ty: name_path_to_type(ty),
                                name: name.0.clone(),
                                init: init.as_ref().map(|e| lower_expr(e, file, fresh, &span)),
                                is_shared: *is_shared,
                            },
                            AS::Break(lbl) => HirStmt::Break {
                                id: fresh(),
                                span: span.clone(),
                                label: lbl.as_ref().map(|i| i.0.clone()),
                            },
                            AS::Continue(lbl) => HirStmt::Continue {
                                id: fresh(),
                                span: span.clone(),
                                label: lbl.as_ref().map(|i| i.0.clone()),
                            },
                            AS::Return(eopt) => HirStmt::Return {
                                id: fresh(),
                                span: span.clone(),
                                expr: eopt.as_ref().map(|e| lower_expr(e, file, fresh, &span)),
                            },
                            AS::Throw(e) => HirStmt::Throw {
                                id: fresh(),
                                span: span.clone(),
                                expr: lower_expr(e, file, fresh, &span),
                            },
                            AS::Panic(e) => HirStmt::Panic {
                                id: fresh(),
                                span: span.clone(),
                                msg: lower_expr(e, file, fresh, &span),
                            },
                            AS::Block(b) => HirStmt::Block(lower_block(b, file, fresh)),
                            AS::Unsafe(b) => HirStmt::Unsafe {
                                id: fresh(),
                                span: span.clone(),
                                block: lower_block(b, file, fresh),
                            },
                            AS::Expr(e) => HirStmt::Expr {
                                id: fresh(),
                                span: span.clone(),
                                expr: lower_expr(e, file, fresh, &span),
                            },
                            AS::Labeled { .. } => HirStmt::Block(HirBlock {
                                id: fresh(),
                                span: span.clone(),
                                stmts: vec![],
                            }),
                        },
                    };
                    HirStmt::Labeled {
                        id,
                        span: span.clone(),
                        label: label.0.clone(),
                        stmt: Box::new(inner),
                    }
                }
                AS::Return(eopt) => HirStmt::Return {
                    id,
                    span: span.clone(),
                    expr: eopt.as_ref().map(|e| lower_expr(e, file, fresh, &span)),
                },
                AS::Throw(e) => HirStmt::Throw {
                    id,
                    span: span.clone(),
                    expr: lower_expr(e, file, fresh, &span),
                },
                AS::Panic(e) => HirStmt::Panic {
                    id,
                    span: span.clone(),
                    msg: lower_expr(e, file, fresh, &span),
                },
                AS::Block(b) => HirStmt::Block(lower_block(b, file, fresh)),
                AS::Unsafe(b) => HirStmt::Unsafe {
                    id,
                    span: span.clone(),
                    block: lower_block(b, file, fresh),
                },
                AS::Expr(e) => HirStmt::Expr {
                    id,
                    span: span.clone(),
                    expr: lower_expr(e, file, fresh, &span),
                },
            };
            out.push(stmt);
        }
        HirBlock {
            id: fresh(),
            span,
            stmts: out,
        }
    }

    let mut decls: Vec<HirDecl> = Vec::new();
    for d in ast_decls {
        match d {
            Decl::Module(ModuleDecl {
                name,
                is_exported,
                items,
                implements,
                doc,
                attrs,
                span,
                ..
            }) => {
                let funcs = items
                    .iter()
                    .map(
                        |FuncDecl {
                             sig, body, span, ..
                         }| HirFunc {
                            sig: func_sig_to_hir(sig, &file_arc),
                            id: fresh(),
                            span: to_hir_span(&file_arc, span),
                            body: body.as_ref().map(|b| lower_block(b, &file_arc, &mut fresh)),
                        },
                    )
                    .collect();
                decls.push(HirDecl::Module(HirModule {
                    name: name.0.clone(),
                    is_exported: *is_exported,
                    funcs,
                    implements: implements.iter().map(name_path_to_vec).collect(),
                    doc: doc.clone(),
                    attrs: attrs
                        .iter()
                        .map(|a| HirAttr {
                            name: name_path_to_string(&a.name),
                            args: a.args.clone(),
                        })
                        .collect(),
                    id: fresh(),
                    span: to_hir_span(&file_arc, span),
                }));
            }
            Decl::TypeAlias(_a) => {
                // Type aliases do not produce HIR items in the MVP.
            }
            Decl::Struct(StructDecl {
                name,
                generics,
                fields,
                doc,
                attrs,
                span,
                ..
            }) => {
                // Collect generic parameter names for type resolution
                let generic_names: Vec<String> =
                    generics.iter().map(|g| g.name.0.clone()).collect();

                let fields: Vec<HirField> = fields
                    .iter()
                    .map(
                        |StructField {
                             name,
                             ty,
                             doc,
                             attrs,
                             span,
                             is_shared,
                             is_final,
                             vis,
                         }| HirField {
                            name: name.0.clone(),
                            ty: name_path_to_type_with_generics(ty, &generic_names),
                            doc: doc.clone(),
                            attrs: attrs
                                .iter()
                                .map(|a| HirAttr {
                                    name: name_path_to_string(&a.name),
                                    args: a.args.clone(),
                                })
                                .collect(),
                            span: Some(to_hir_span(&file_arc, span)),
                            is_shared: *is_shared,
                            is_final: *is_final,
                            vis: vis.clone(),
                        },
                    )
                    .collect();
                decls.push(HirDecl::Struct(HirStruct {
                    name: name.0.clone(),
                    generics: generics_to_hir(generics),
                    fields,
                    doc: doc.clone(),
                    attrs: attrs
                        .iter()
                        .map(|a| HirAttr {
                            name: name_path_to_string(&a.name),
                            args: a.args.clone(),
                        })
                        .collect(),
                    id: fresh(),
                    span: to_hir_span(&file_arc, span),
                }));
            }
            Decl::Interface(InterfaceDecl {
                name,
                generics,
                methods,
                extends,
                doc,
                attrs,
                span,
                ..
            }) => {
                let hir_methods: Vec<HirInterfaceMethod> = methods
                    .iter()
                    .map(|m| HirInterfaceMethod {
                        sig: func_sig_to_hir(&m.sig, &file_arc),
                        default_body: m
                            .default_body
                            .as_ref()
                            .map(|blk| lower_block(blk, &file_arc, &mut fresh)),
                    })
                    .collect();
                decls.push(HirDecl::Interface(HirInterface {
                    name: name.0.clone(),
                    generics: generics_to_hir(generics),
                    methods: hir_methods,
                    extends: extends.iter().map(name_path_to_vec).collect(),
                    doc: doc.clone(),
                    attrs: attrs
                        .iter()
                        .map(|a| HirAttr {
                            name: name_path_to_string(&a.name),
                            args: a.args.clone(),
                        })
                        .collect(),
                    id: fresh(),
                    span: to_hir_span(&file_arc, span),
                }));
            }
            Decl::Enum(e) => {
                decls.push(HirDecl::Enum(HirEnum {
                    name: e.name.0.clone(),
                    generics: generics_to_hir(&e.generics),
                    variants: enum_variants_to_hir(e, &file_arc, &mut fresh),
                    doc: e.doc.clone(),
                    attrs: e
                        .attrs
                        .iter()
                        .map(|a| HirAttr {
                            name: name_path_to_string(&a.name),
                            args: a.args.clone(),
                        })
                        .collect(),
                    id: fresh(),
                    span: to_hir_span(&file_arc, &e.span),
                }));
            }
            Decl::Provider(p) => {
                let fields: Vec<HirField> = p
                    .fields
                    .iter()
                    .map(
                        |StructField {
                             name,
                             ty,
                             doc,
                             attrs,
                             span,
                             is_shared,
                             is_final,
                             vis,
                         }| HirField {
                            name: name.0.clone(),
                            ty: name_path_to_type(ty),
                            doc: doc.clone(),
                            attrs: attrs
                                .iter()
                                .map(|a| HirAttr {
                                    name: name_path_to_string(&a.name),
                                    args: a.args.clone(),
                                })
                                .collect(),
                            span: Some(to_hir_span(&file_arc, span)),
                            is_shared: *is_shared,
                            is_final: *is_final,
                            vis: vis.clone(),
                        },
                    )
                    .collect();
                decls.push(HirDecl::Provider(HirProvider {
                    name: p.name.0.clone(),
                    fields,
                    doc: p.doc.clone(),
                    attrs: p
                        .attrs
                        .iter()
                        .map(|a| HirAttr {
                            name: name_path_to_string(&a.name),
                            args: a.args.clone(),
                        })
                        .collect(),
                    id: fresh(),
                    span: to_hir_span(&file_arc, &p.span),
                }));
            }
            Decl::Function(FuncDecl {
                sig, body, span, ..
            }) => {
                decls.push(HirDecl::Function(HirFunc {
                    sig: func_sig_to_hir(sig, &file_arc),
                    id: fresh(),
                    span: to_hir_span(&file_arc, span),
                    body: body.as_ref().map(|b| lower_block(b, &file_arc, &mut fresh)),
                }));
            }
            Decl::ExternFunc(crate::compiler::ast::ExternFuncDecl {
                abi,
                name,
                ret,
                params,
                doc,
                attrs,
                span,
                ..
            }) => {
                decls.push(HirDecl::ExternFunc(HirExternFunc {
                    name: name.0.clone(),
                    abi: abi.clone(),
                    params: params.iter().map(param_to_hir).collect(),
                    ret: ret.as_ref().map(name_path_to_type),
                    doc: doc.clone(),
                    attrs: attrs
                        .iter()
                        .map(|a| HirAttr {
                            name: name_path_to_string(&a.name),
                            args: a.args.clone(),
                        })
                        .collect(),
                    id: fresh(),
                    span: to_hir_span(&file_arc, span),
                }));
            }
        }
    }

    // =========================================================================
    // Post-process: Synthesize default methods into implementing modules
    // =========================================================================
    //
    // For each module that implements an interface with default methods,
    // add the default method implementations to the module if not already present.
    {
        use std::collections::HashMap;

        // Step 1: Build index of interfaces -> default methods
        let mut interface_defaults: HashMap<String, Vec<HirInterfaceMethod>> = HashMap::new();
        for d in &decls {
            if let HirDecl::Interface(ifc) = d {
                let defaults: Vec<HirInterfaceMethod> = ifc
                    .methods
                    .iter()
                    .filter(|m| m.default_body.is_some())
                    .cloned()
                    .collect();
                if !defaults.is_empty() {
                    interface_defaults.insert(ifc.name.clone(), defaults);
                }
            }
        }

        // Step 2: For each module, add missing default methods
        for d in decls.iter_mut() {
            if let HirDecl::Module(m) = d {
                // Collect existing function names in this module
                let existing_names: std::collections::HashSet<String> =
                    m.funcs.iter().map(|f| f.sig.name.clone()).collect();

                // Check each interface this module implements
                for ifc_path in &m.implements {
                    // Get interface name (last segment of path)
                    let ifc_name = ifc_path.last().cloned().unwrap_or_default();

                    // Get default methods for this interface
                    if let Some(defaults) = interface_defaults.get(&ifc_name) {
                        for dm in defaults {
                            // Only add if module doesn't already have this function
                            if !existing_names.contains(&dm.sig.name) {
                                // Clone and add the default method as a module function
                                let default_func = HirFunc {
                                    sig: dm.sig.clone(),
                                    id: fresh(),
                                    span: dm.sig.span.clone().unwrap_or_else(|| HirSpan {
                                        file: file_arc.clone(),
                                        start: 0,
                                        end: 0,
                                    }),
                                    body: dm.default_body.clone(),
                                };
                                m.funcs.push(default_func);
                            }
                        }
                    }
                }
            }
        }
    }

    // Collect non-fatal lowering notes by scanning AST blocks for recoveries
    let mut notes: Vec<LoweringNote> = Vec::new();
    fn collect_notes_in_block(
        blk: &crate::compiler::ast::Block,
        file: &Arc<PathBuf>,
        notes: &mut Vec<LoweringNote>,
    ) {
        if blk.span.start == 0 && blk.span.end == 0 {
            notes.push(LoweringNote {
                span: Some(HirSpan {
                    file: file.clone(),
                    start: 0,
                    end: 0,
                }),
                message: "placeholder block inserted due to parse recovery".to_string(),
            });
        }
        use crate::compiler::ast::Stmt as AS;
        for s in &blk.stmts {
            match s {
                AS::If {
                    then_blk, else_blk, ..
                } => {
                    collect_notes_in_block(then_blk, file, notes);
                    if let Some(b) = else_blk {
                        collect_notes_in_block(b, file, notes);
                    }
                }
                AS::While { body, .. } => collect_notes_in_block(body, file, notes),
                AS::For { cond, body, .. } => {
                    if cond.is_none() {
                        notes.push(LoweringNote {
                            span: Some(HirSpan {
                                file: file.clone(),
                                start: body.span.start as u32,
                                end: body.span.end as u32,
                            }),
                            message: "missing for-loop condition; assumed 'true' during lowering"
                                .to_string(),
                        });
                    }
                    collect_notes_in_block(body, file, notes);
                }
                AS::Switch { cases, default, .. } => {
                    for (_, b) in cases {
                        collect_notes_in_block(b, file, notes);
                    }
                    if let Some(db) = default {
                        collect_notes_in_block(db, file, notes);
                    }
                }
                AS::Try {
                    try_blk,
                    catches,
                    finally_blk,
                } => {
                    collect_notes_in_block(try_blk, file, notes);
                    for c in catches {
                        collect_notes_in_block(&c.blk, file, notes);
                    }
                    if let Some(fb) = finally_blk {
                        collect_notes_in_block(fb, file, notes);
                    }
                }
                AS::Block(b) => collect_notes_in_block(b, file, notes),
                AS::Unsafe(b) => collect_notes_in_block(b, file, notes),
                _ => {}
            }
        }
    }
    for d in ast_decls {
        match d {
            Decl::Module(m) => {
                for f in &m.items {
                    if let Some(b) = &f.body {
                        collect_notes_in_block(b, &file_arc, &mut notes);
                    }
                }
            }
            Decl::Function(f) => {
                if let Some(b) = &f.body {
                    collect_notes_in_block(b, &file_arc, &mut notes);
                }
            }
            _ => {}
        }
    }

    HirFile {
        path,
        package: package.map(Into::into),
        decls,
        notes,
        source_language: Some(HirSourceLanguage::Arth),
        is_guest: false,
    }
}

pub fn dump_hir(h: &HirFile) -> String {
    let mut out = String::new();
    out.push_str(&format!("hir-file: {}\n", h.path.display()));
    out.push_str(&format!(
        "package: {}\n",
        h.package
            .as_ref()
            .map(|p| p.to_string())
            .unwrap_or_else(|| "<none>".to_string())
    ));
    let lang = match &h.source_language {
        Some(HirSourceLanguage::Arth) => "arth",
        Some(HirSourceLanguage::Ts) => "ts",
        None => "<unknown>",
    };
    out.push_str(&format!("source_language: {}\n", lang));
    out.push_str(&format!(
        "guest: {}\n",
        if h.is_guest { "yes" } else { "no" }
    ));
    out.push_str("decls:\n");
    for d in &h.decls {
        match d {
            HirDecl::Module(m) => {
                out.push_str(&format!("- module {}\n", m.name));
                for f in &m.funcs {
                    out.push_str(&format!("  - func {}(", f.sig.name));
                    let names = f
                        .sig
                        .params
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>();
                    out.push_str(&names.join(", "));
                    out.push(')');
                    if let Some(_ret) = &f.sig.ret {
                        // Intentionally omit type text in dump for now
                    }
                    out.push('\n');
                }
            }
            HirDecl::Struct(s) => {
                out.push_str(&format!("- struct {}\n", s.name));
            }
            HirDecl::Interface(i) => {
                out.push_str(&format!("- interface {}\n", i.name));
                for m in &i.methods {
                    let is_default = m.default_body.is_some();
                    let prefix = if is_default { "default " } else { "" };
                    out.push_str(&format!("  - {}sig {}(", prefix, m.sig.name));
                    let names = m
                        .sig
                        .params
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>();
                    out.push_str(&names.join(", "));
                    out.push(')');
                    if let Some(_ret) = &m.sig.ret {
                        // Intentionally omit type text in dump for now
                    }
                    out.push('\n');
                }
            }
            HirDecl::Enum(e) => {
                out.push_str(&format!("- enum {}\n", e.name));
                for v in &e.variants {
                    match v {
                        HirEnumVariant::Unit { name, .. } => {
                            out.push_str(&format!("  - {}\n", name))
                        }
                        HirEnumVariant::Tuple { name, .. } => {
                            out.push_str(&format!("  - {}(..)\n", name))
                        }
                    }
                }
            }
            HirDecl::Provider(p) => {
                out.push_str(&format!("- provider {}\n", p.name));
            }
            HirDecl::Function(f) => {
                out.push_str(&format!("- func {}(", f.sig.name));
                let names = f
                    .sig
                    .params
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>();
                out.push_str(&names.join(", "));
                out.push(')');
                if let Some(_ret) = &f.sig.ret {
                    // Intentionally omit type text in dump for now
                }
                out.push('\n');
            }
            HirDecl::ExternFunc(ef) => {
                out.push_str(&format!("- extern \"{}\" fn {}(", ef.abi, ef.name));
                let names = ef
                    .params
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>();
                out.push_str(&names.join(", "));
                out.push(')');
                out.push('\n');
            }
        }
    }
    out
}

/// Debug printer for HIR files.
/// Wrapper kept to match roadmap naming (`hir::dump(&HirFile)`).
pub fn dump(h: &HirFile) -> String {
    dump_hir(h)
}

#[cfg(test)]
mod tests;
