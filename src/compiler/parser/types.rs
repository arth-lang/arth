use crate::compiler::ast::{Ident, NamePath};
use crate::compiler::lexer::{Token, TokenKind};

/// Parse a type-like name (dotted identifiers) with optional generic type arguments.
/// Examples: `int`, `String`, `List<int>`, `Map<String, int>`, `pkg.Container<T>`
pub(super) fn parse_type_name(tokens: &[Token], i: &mut usize) -> Option<NamePath> {
    let path = parse_name_path_parts(tokens, i)?;

    // Check for generic type arguments <...>
    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Lt)) {
        let type_args = parse_type_args(tokens, i)?;
        Some(NamePath::with_type_args(path, type_args))
    } else {
        Some(NamePath::new(path))
    }
}

/// Parse generic type arguments: `<T1, T2, ...>`
/// Returns Vec of type arguments, or None on parse error
fn parse_type_args(tokens: &[Token], i: &mut usize) -> Option<Vec<NamePath>> {
    // Expect '<'
    if !matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Lt)) {
        return None;
    }
    *i += 1;

    let mut args = Vec::new();

    // Parse first type argument
    if let Some(arg) = parse_type_name(tokens, i) {
        args.push(arg);
    }

    // Parse remaining type arguments separated by commas
    while matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Comma)) {
        *i += 1;
        if let Some(arg) = parse_type_name(tokens, i) {
            args.push(arg);
        } else {
            break;
        }
    }

    // Expect '>' or '>>' (for nested generics like Map<String, List<int>>)
    match tokens.get(*i).map(|t| &t.kind) {
        Some(TokenKind::Gt) => {
            *i += 1;
        }
        Some(TokenKind::Shr) => {
            // '>>' - consume only one '>' and leave the other for the outer generic
            // This is a hack: we pretend we consumed '>' but really we consumed '>>'
            // The outer parser will handle this case
            *i += 1;
        }
        _ => {
            // Allow missing '>' for better error recovery
        }
    }

    Some(args)
}

pub(super) fn parse_ident(tokens: &[Token], i: &mut usize) -> Option<Ident> {
    match tokens.get(*i) {
        Some(t) => match &t.kind {
            TokenKind::Ident(s) => {
                *i += 1;
                Some(Ident(s.clone()))
            }
            _ => None,
        },
        None => None,
    }
}

/// Parse just the path part of a name (dotted identifiers), without generic args.
/// Returns the path components as Vec<Ident>.
fn parse_name_path_parts(tokens: &[Token], i: &mut usize) -> Option<Vec<Ident>> {
    let mut parts: Vec<Ident> = Vec::new();
    match tokens.get(*i) {
        Some(t) => match &t.kind {
            TokenKind::Ident(s) => {
                *i += 1;
                parts.push(Ident(s.clone()));
            }
            _ => return None,
        },
        None => return None,
    }
    loop {
        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Dot)) {
            *i += 1;
        } else {
            break;
        }
        match tokens.get(*i) {
            Some(t) => match &t.kind {
                TokenKind::Ident(s) => {
                    *i += 1;
                    parts.push(Ident(s.clone()));
                }
                _ => break,
            },
            None => break,
        }
    }
    Some(parts)
}

/// Parse a name path without generic arguments (backward compatibility).
/// For new code, prefer parse_type_name which captures generics.
pub(super) fn parse_name_path(tokens: &[Token], i: &mut usize) -> Option<NamePath> {
    let parts = parse_name_path_parts(tokens, i)?;
    Some(NamePath::new(parts))
}
