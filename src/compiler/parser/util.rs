use crate::compiler::lexer::{Token, TokenKind};
use crate::compiler::source::Span;

pub(super) fn skip_to_sync(tokens: &[Token], mut i: usize) -> usize {
    while i < tokens.len() {
        match tokens.get(i).map(|t| &t.kind) {
            Some(TokenKind::Semicolon) | Some(TokenKind::RBrace) => {
                i += 1;
                break;
            }
            _ => i += 1,
        }
    }
    i
}

#[allow(dead_code)]
pub(super) fn skip_balanced(start_idx: usize, l: TokenKind, r: TokenKind, toks: &[Token]) -> usize {
    let mut depth = 0usize;
    let mut k = start_idx;
    while let Some(t) = toks.get(k) {
        if t.kind == l {
            depth += 1;
        } else if t.kind == r {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return k + 1;
            }
        }
        k += 1;
    }
    k
}

pub(super) fn join_span(a: Span, b: Span) -> Span {
    Span {
        start: a.start,
        end: b.end,
        start_line: a.start_line,
        start_col: a.start_col,
        end_line: b.end_line,
        end_col: b.end_col,
    }
}
