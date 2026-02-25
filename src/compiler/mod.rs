pub mod ast;
pub mod attrs;
pub mod codegen;
pub mod diag;
pub mod diagnostics;
#[cfg(not(doctest))]
pub mod driver;
#[cfg(doctest)]
pub mod driver {}
pub mod fmt;
pub mod hir;
pub mod intrinsics;
pub mod ir;
pub mod lexer;
pub mod lint;
pub mod lower;
pub mod parser;
pub mod resolve;
pub mod source;
pub mod stdlib;
pub mod test_runner;
pub mod typeck;
