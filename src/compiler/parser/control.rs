#![allow(dead_code)]
use crate::compiler::ast::ControlKind;
use crate::compiler::lexer::{Token, TokenKind};

// Try to parse a control statement starting at index i. Returns the next index
// (consumed) and pushes ControlKind if recognized. If not recognized, returns i.
pub fn try_parse_control(tokens: &[Token], i: usize, out: &mut Vec<ControlKind>) -> usize {
    if i >= tokens.len() {
        return i;
    }
    match tokens[i].kind {
        TokenKind::If => {
            out.push(ControlKind::If);
            skip_paren_and_block(tokens, i + 1)
        }
        TokenKind::Else => {
            out.push(ControlKind::Else);
            // else if (...) ... or else { ... }
            let k = i + 1;
            if matches!(tokens.get(k).map(|t| &t.kind), Some(TokenKind::If)) {
                return try_parse_control(tokens, k, out);
            }
            maybe_skip_block(tokens, k)
        }
        TokenKind::While => {
            out.push(ControlKind::While);
            skip_paren_and_block(tokens, i + 1)
        }
        TokenKind::For => {
            out.push(ControlKind::For);
            skip_paren_and_block(tokens, i + 1)
        }
        TokenKind::Switch => {
            out.push(ControlKind::Switch);
            skip_paren_and_block(tokens, i + 1)
        }
        TokenKind::Try => {
            out.push(ControlKind::Try);
            let mut k = maybe_skip_block(tokens, i + 1);
            // zero or more catch blocks, optional finally
            loop {
                if matches!(tokens.get(k).map(|t| &t.kind), Some(TokenKind::Catch)) {
                    out.push(ControlKind::Catch);
                    k += 1;
                    if matches!(tokens.get(k).map(|t| &t.kind), Some(TokenKind::LParen)) {
                        k = skip_balanced(tokens, k, TokenKind::LParen, TokenKind::RParen);
                    }
                    k = maybe_skip_block(tokens, k);
                    continue;
                }
                if matches!(tokens.get(k).map(|t| &t.kind), Some(TokenKind::Finally)) {
                    out.push(ControlKind::Finally);
                    k = maybe_skip_block(tokens, k + 1);
                }
                break;
            }
            k
        }
        TokenKind::Break => {
            out.push(ControlKind::Break);
            skip_until_semicolon(tokens, i + 1)
        }
        TokenKind::Continue => {
            out.push(ControlKind::Continue);
            skip_until_semicolon(tokens, i + 1)
        }
        TokenKind::Return => {
            out.push(ControlKind::Return);
            skip_until_semicolon(tokens, i + 1)
        }
        TokenKind::Throw => {
            out.push(ControlKind::Throw);
            skip_until_semicolon(tokens, i + 1)
        }
        TokenKind::Case => {
            out.push(ControlKind::Case);
            skip_until_colon(tokens, i + 1)
        }
        TokenKind::Default => {
            out.push(ControlKind::Default);
            skip_until_colon(tokens, i + 1)
        }
        _ => i,
    }
}

fn skip_until_semicolon(tokens: &[Token], mut k: usize) -> usize {
    while let Some(t) = tokens.get(k) {
        k += 1;
        if matches!(t.kind, TokenKind::Semicolon) {
            break;
        }
    }
    k
}

fn skip_until_colon(tokens: &[Token], mut k: usize) -> usize {
    while let Some(t) = tokens.get(k) {
        k += 1;
        if matches!(t.kind, TokenKind::Colon) {
            break;
        }
    }
    k
}

fn maybe_skip_block(tokens: &[Token], k: usize) -> usize {
    if matches!(tokens.get(k).map(|t| &t.kind), Some(TokenKind::LBrace)) {
        skip_balanced(tokens, k, TokenKind::LBrace, TokenKind::RBrace)
    } else {
        k
    }
}

fn skip_paren_and_block(tokens: &[Token], mut k: usize) -> usize {
    if matches!(tokens.get(k).map(|t| &t.kind), Some(TokenKind::LParen)) {
        k = skip_balanced(tokens, k, TokenKind::LParen, TokenKind::RParen);
    }
    maybe_skip_block(tokens, k)
}

fn skip_balanced(tokens: &[Token], mut k: usize, l: TokenKind, r: TokenKind) -> usize {
    let mut depth = 0usize;
    while let Some(t) = tokens.get(k) {
        if t.kind == l {
            depth += 1;
        }
        if t.kind == r {
            depth -= 1;
            if depth == 0 {
                return k + 1;
            }
        }
        k += 1;
    }
    k
}
