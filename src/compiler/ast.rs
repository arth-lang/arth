use crate::compiler::source::Span;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageName(pub Vec<Ident>);

impl std::fmt::Display for PackageName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self
            .0
            .iter()
            .map(|i| i.0.as_str())
            .collect::<Vec<_>>()
            .join(".");
        f.write_str(&s)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileAst {
    pub package: Option<PackageName>,
    // Imports in this file
    pub imports: Vec<ImportSpec>,
    // Top-level declarations (module/struct/interface/enum/provider/impl/functions)
    pub decls: Vec<Decl>,
}

// --- Attributes and docs ---

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attr {
    pub name: NamePath,
    // Raw argument substring inside parentheses, if present (unparsed)
    pub args: Option<String>,
}

// Stable per-file AST identifiers for cross-references during lowering/resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AstId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Visibility {
    Default,
    Public,
    Internal,
    Private,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportSpec {
    pub path: Vec<Ident>,
    pub star: bool,
    /// Optional alias for the imported item: `import foo.Bar as MyBar;`
    pub alias: Option<Ident>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamePath {
    pub path: Vec<Ident>,
    /// Generic type arguments, e.g., `Container<int>` -> type_args = [NamePath for int]
    pub type_args: Vec<NamePath>,
}

impl NamePath {
    /// Create a NamePath without generic type arguments
    pub fn new(path: Vec<Ident>) -> Self {
        NamePath {
            path,
            type_args: Vec::new(),
        }
    }

    /// Create a NamePath with generic type arguments
    pub fn with_type_args(path: Vec<Ident>, type_args: Vec<NamePath>) -> Self {
        NamePath { path, type_args }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenericParam {
    pub name: Ident,
    pub bound: Option<NamePath>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: Ident,
    pub ty: NamePath,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncSig {
    pub vis: Visibility,
    pub is_static: bool,
    pub is_final: bool,
    pub is_async: bool,
    /// Whether this function is marked as unsafe (can perform unsafe operations)
    pub is_unsafe: bool,
    pub name: Ident,
    pub ret: Option<NamePath>, // None => void
    pub params: Vec<Param>,
    pub generics: Vec<GenericParam>,
    pub throws: Vec<NamePath>,
    pub doc: Option<String>,
    pub attrs: Vec<Attr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FuncDecl {
    pub sig: FuncSig,
    pub body: Option<Block>,
    pub span: Span,
    pub id: AstId,
    pub body_id: Option<AstId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleDecl {
    pub name: Ident,
    // Whether this module is exported (accessible from other packages)
    pub is_exported: bool,
    // Optional list of interfaces this module implements
    // Target type is inferred from method signatures
    pub implements: Vec<NamePath>,
    pub items: Vec<FuncDecl>,
    pub doc: Option<String>,
    pub attrs: Vec<Attr>,
    pub span: Span,
    pub id: AstId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructField {
    pub vis: Visibility,
    pub is_final: bool,
    pub is_shared: bool,
    pub name: Ident,
    pub ty: NamePath,
    pub doc: Option<String>,
    pub attrs: Vec<Attr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructDecl {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub fields: Vec<StructField>,
    pub doc: Option<String>,
    pub attrs: Vec<Attr>,
    pub span: Span,
    pub id: AstId,
}

/// A method declaration within an interface.
/// Can be either an abstract method (no body) or a default method (with body).
#[derive(Clone, Debug, PartialEq)]
pub struct InterfaceMethod {
    pub sig: FuncSig,
    /// If Some, this is a default method with an implementation.
    /// If None, this is an abstract method that must be implemented by conforming modules.
    pub default_body: Option<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InterfaceDecl {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub extends: Vec<NamePath>,
    pub methods: Vec<InterfaceMethod>,
    pub doc: Option<String>,
    pub attrs: Vec<Attr>,
    pub span: Span,
    pub id: AstId,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EnumVariant {
    Unit {
        name: Ident,
        discriminant: Option<Box<Expr>>,
    },
    Tuple {
        name: Ident,
        types: Vec<NamePath>,
        discriminant: Option<Box<Expr>>,
    },
}

impl EnumVariant {
    /// Get the variant name
    pub fn name(&self) -> &Ident {
        match self {
            EnumVariant::Unit { name, .. } => name,
            EnumVariant::Tuple { name, .. } => name,
        }
    }

    /// Get the optional discriminant expression
    pub fn discriminant(&self) -> Option<&Expr> {
        match self {
            EnumVariant::Unit { discriminant, .. } => discriminant.as_deref(),
            EnumVariant::Tuple { discriminant, .. } => discriminant.as_deref(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumDecl {
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub variants: Vec<EnumVariant>,
    pub is_sealed: bool,
    pub doc: Option<String>,
    pub attrs: Vec<Attr>,
    pub span: Span,
    pub id: AstId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderDecl {
    pub name: Ident,
    // Provider fields with modifiers; `shared` allowed here
    pub fields: Vec<StructField>,
    pub doc: Option<String>,
    pub attrs: Vec<Attr>,
    pub span: Span,
    pub id: AstId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeAliasDecl {
    pub name: Ident,
    pub aliased: NamePath,
    pub doc: Option<String>,
    pub attrs: Vec<Attr>,
    pub span: Span,
    pub id: AstId,
}

/// External function declaration for FFI
/// Syntax: `extern "C" fn name(params) -> ret;`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternFuncDecl {
    pub vis: Visibility,
    /// ABI specification (e.g., "C")
    pub abi: String,
    pub name: Ident,
    pub ret: Option<NamePath>,
    pub params: Vec<Param>,
    pub doc: Option<String>,
    pub attrs: Vec<Attr>,
    pub span: Span,
    pub id: AstId,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decl {
    Module(ModuleDecl),
    Struct(StructDecl),
    Interface(InterfaceDecl),
    Enum(EnumDecl),
    Provider(ProviderDecl),
    Function(FuncDecl),
    TypeAlias(TypeAliasDecl),
    /// External function declaration (FFI)
    ExternFunc(ExternFuncDecl),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlKind {
    If,
    Else,
    While,
    For,
    Switch,
    Case,
    Default,
    Try,
    Catch,
    Finally,
    Break,
    Continue,
    Return,
    #[allow(dead_code)]
    Throw,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Char(char),
    Bool(bool),
    Ident(Ident),
    // Suspension point: await an awaitable expression (Task<T>, Receiver<T>, etc.)
    Await(Box<Expr>),
    // Explicit cast: (Type)expr
    Cast(NamePath, Box<Expr>),
    // Function literal: fn(Type name, ...) { ... }
    // Return type is implicit (contextual); body is a block.
    FnLiteral(Vec<Param>, Block),
    // Ternary conditional expression: cond ? then_expr : else_expr
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Binary(Box<Expr>, BinOp, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Member(Box<Expr>, Ident),
    /// Optional chaining: expr?.field - returns None if expr is None
    OptionalMember(Box<Expr>, Ident),
    Index(Box<Expr>, Box<Expr>),
    // Collection literals
    ListLit(Vec<Expr>),
    MapLit {
        pairs: Vec<(Expr, Expr)>,
        spread: Option<Box<Expr>>, // struct update syntax: { ..existing, field: value }
    },
    /// Struct literal: TypeName { field: value, ... } or TypeName { ..spread, field: value }
    /// Preserves the type name for proper type checking and codegen
    StructLit {
        /// The struct type name (e.g., "Point", "pkg.User")
        type_name: Box<Expr>,
        /// Field-value pairs where the key is always an Ident
        fields: Vec<(Ident, Expr)>,
        /// Optional spread expression for struct update syntax
        spread: Option<Box<Expr>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    // Shifts
    Shl,
    Shr,
    // Comparisons
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    // Logical operators (short-circuit semantics in future)
    And,
    Or,
    // Bitwise operators
    BitAnd,
    BitOr,
    BitXor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatchClause {
    pub ty: Option<NamePath>,
    pub var: Option<Ident>,
    pub blk: Block,
}

/// Pattern for matching values in switch expressions (enum pattern matching)
#[derive(Clone, Debug, PartialEq)]
pub enum Pattern {
    /// Wildcard pattern: `_` - matches anything, binds nothing
    Wildcard,
    /// Variable binding: `x` - matches anything, binds to variable
    Binding(Ident),
    /// Literal pattern: matches exact value (int, string, bool)
    Literal(Box<Expr>),
    /// Enum variant pattern: `EnumType.Variant(p1, p2, ...)`
    Variant {
        enum_ty: NamePath,
        variant: Ident,
        payloads: Vec<Pattern>,
        span: Span,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    PrintStr(String),
    PrintExpr(Expr),
    /// Print without newline (raw print)
    PrintRawStr(String),
    PrintRawExpr(Expr),
    If {
        cond: Expr,
        then_blk: Block,
        else_blk: Option<Block>,
    },
    While {
        cond: Expr,
        body: Block,
    },
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        step: Option<Box<Stmt>>,
        body: Block,
    },
    // Labeled statement (used for labeled loops/switch)
    Labeled {
        label: Ident,
        stmt: Box<Stmt>,
    },
    Switch {
        expr: Expr,
        /// Legacy expression-based cases (for int/string switches)
        cases: Vec<(Expr, Block)>,
        /// Pattern-based cases (for enum pattern matching)
        pattern_cases: Vec<(Pattern, Block)>,
        default: Option<Block>,
    },
    Try {
        try_blk: Block,
        catches: Vec<CatchClause>,
        finally_blk: Option<Block>,
    },
    Assign {
        name: Ident,
        expr: Expr,
    },
    // Assignment to a field on a simple object expression, e.g., `p.x = 1;`
    // This is a minimal node to enable struct field writes in demos.
    FieldAssign {
        object: Expr,
        field: Ident,
        expr: Expr,
    },
    AssignOp {
        name: Ident,
        op: AssignOp,
        expr: Expr,
    },
    VarDecl {
        is_final: bool,
        is_shared: bool,
        ty: NamePath,
        // Optional generic type arguments on the declared type (e.g., List<String>, Map<Int,Int>)
        generics: Vec<NamePath>,
        // For function types: parameter types if declared as Fn<Ret>(ParamTypes...) Name;
        fn_params: Vec<NamePath>,
        name: Ident,
        init: Option<Expr>,
    },
    // Optional target label for break/continue
    Break(Option<Ident>),
    Continue(Option<Ident>),
    // return; or return expr;
    Return(Option<Expr>),
    // throw expr; - throws an exception value
    Throw(Expr),
    // panic(msg); - unrecoverable error that unwinds within task boundary
    // Panics run drops and propagate to join() as a failure
    Panic(Expr),
    #[allow(dead_code)]
    Block(Block),
    // Generic expression statement (e.g., function call for side effects)
    Expr(Expr),
    // Unsafe block: `unsafe { ... }`
    // Code inside unsafe blocks can perform unsafe operations (raw pointers, FFI calls)
    Unsafe(Block),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignOp {
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
