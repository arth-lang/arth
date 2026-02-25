use crate::compiler::source::Span;

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    // Declarations/keywords we already used
    Package,
    Module,
    Public,
    Import,
    Internal,
    Private,
    Export,
    Struct,
    Interface,
    Enum,
    Sealed,
    Provider,
    Shared,
    // Type alias declarations
    Type,
    Extends,
    Static,
    Final,
    Void,
    Async,
    Await,
    Throws,
    // Type inference keyword
    Var,
    // First-class function literal keyword
    Fn,
    // Unsafe code and FFI
    Unsafe,
    Extern,
    Implements,
    // Import aliasing keyword
    As,
    True,
    False,
    // Control statements
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
    Throw,
    Panic,
    // General
    Ident(String),
    Dot,
    DotDot, // '..' for struct spread syntax
    Colon,
    Question,
    QuestionDot, // '?.' for optional chaining
    Semicolon,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Lt,
    Gt,
    Le,
    Ge,
    Shl,
    Shr,
    Arrow,
    // Bitwise single-char operators
    Ampersand, // '&'
    Pipe,      // '|'
    Caret,     // '^'
    // Assignment compounds
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    ShlEq,
    ShrEq,
    AndEq, // '&='
    OrEq,  // '|='
    XorEq, // '^='
    Eq,
    EqEq,
    AndAnd,
    OrOr,
    Bang,
    BangEq,
    Number(i64),
    Float(f64),
    CharLit(char),
    StringLit(String),
    DocLine(String),
    DocBlock(String),
    At, // '@' for attributes
    Eof,
    Unknown(char),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
