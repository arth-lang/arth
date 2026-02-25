use crate::compiler::source::Span;

pub mod control;

mod tokens;
pub use tokens::{Token, TokenKind};

mod core;
pub use core::{Lexer, lex_all};

mod with_reporter;
pub use with_reporter::lex_all_with_reporter;

#[cfg(test)]
mod tests;
