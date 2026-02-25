use crate::compiler::ast::{
    AstId, Attr, Block, FuncDecl, FuncSig, GenericParam, InterfaceMethod, NamePath, Param,
    Visibility,
};
use crate::compiler::diagnostics::{Diagnostic, Reporter};
use crate::compiler::lexer::{Token, TokenKind};
use crate::compiler::source::Span;

use super::stmt::parse_block;
use super::util::join_span;

// Parse generic parameters: <T, U extends Bound>
pub(super) fn parse_generic_params(tokens: &[Token], i: &mut usize) -> Vec<GenericParam> {
    let mut params = Vec::new();
    if !matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Lt)) {
        return params;
    }
    *i += 1;
    while let Some(name) = super::types::parse_ident(tokens, i) {
        let mut bound = None;
        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Extends)) {
            *i += 1;
            bound = super::types::parse_name_path(tokens, i);
        }
        params.push(GenericParam { name, bound });
        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Comma)) {
            *i += 1;
            continue;
        }
        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Gt)) {
            *i += 1;
            break;
        }
    }
    params
}

pub(super) fn parse_function_sig(
    tokens: &[Token],
    j: usize,
    reporter: &mut Reporter,
    path: &std::path::Path,
    leading_doc: Option<String>,
    leading_attrs: Vec<Attr>,
) -> Option<(FuncSig, usize)> {
    let mut i = j;
    let sig_start = tokens
        .get(i)
        .map(|t| t.span)
        .unwrap_or_else(|| Span::new(0, 0));
    // qualifiers
    let mut vis = Visibility::Default;
    let mut is_static = false;
    let mut is_final = false;
    let mut is_async = false;
    let mut is_unsafe = false;
    loop {
        match tokens.get(i).map(|t| &t.kind) {
            Some(TokenKind::Public) => {
                vis = Visibility::Public;
                i += 1;
            }
            Some(TokenKind::Internal) => {
                vis = Visibility::Internal;
                i += 1;
            }
            Some(TokenKind::Private) => {
                vis = Visibility::Private;
                i += 1;
            }
            Some(TokenKind::Static) => {
                is_static = true;
                i += 1;
            }
            Some(TokenKind::Final) => {
                is_final = true;
                i += 1;
            }
            Some(TokenKind::Async) => {
                is_async = true;
                i += 1;
            }
            Some(TokenKind::Unsafe) => {
                is_unsafe = true;
                i += 1;
            }
            _ => break,
        }
    }
    // return type or void
    let mut consumed_void = false;
    let ret = if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::Void)) {
        consumed_void = true;
        None
    } else {
        super::types::parse_type_name(tokens, &mut i)
    };
    if consumed_void {
        i += 1;
    }
    // name
    let name = match super::types::parse_ident(tokens, &mut i) {
        Some(n) => n,
        None => {
            if let Some(t) = tokens.get(i) {
                reporter.emit(
                    Diagnostic::error("expected function name")
                        .with_file(path.to_path_buf())
                        .with_span(t.span),
                );
            }
            return None;
        }
    };
    // generics
    let generics = parse_generic_params(tokens, &mut i);
    // params
    if !matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::LParen)) {
        if let Some(t) = tokens.get(i) {
            reporter.emit(
                Diagnostic::error("expected '(' after function name")
                    .with_file(path.to_path_buf())
                    .with_span(t.span),
            );
        }
        return None;
    }
    i += 1;
    let mut params: Vec<Param> = Vec::new();
    loop {
        if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::RParen)) {
            i += 1;
            break;
        }
        if let Some(pty) = super::types::parse_type_name(tokens, &mut i) {
            if let Some(pn) = super::types::parse_ident(tokens, &mut i) {
                params.push(Param { name: pn, ty: pty });
            } else if let Some(t) = tokens.get(i) {
                reporter.emit(
                    Diagnostic::error("expected parameter name")
                        .with_file(path.to_path_buf())
                        .with_span(t.span),
                );
            }
            if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::Comma)) {
                i += 1;
                continue;
            }
            if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::RParen)) {
                i += 1;
                break;
            }
        } else {
            if let Some(t) = tokens.get(i) {
                reporter.emit(
                    Diagnostic::error("expected parameter type")
                        .with_file(path.to_path_buf())
                        .with_span(t.span),
                );
            }
            break;
        }
    }
    // throws
    let mut throws: Vec<NamePath> = Vec::new();
    if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::Throws)) {
        i += 1;
        if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::LParen)) {
            i += 1;
            loop {
                if let Some(ty) = super::types::parse_name_path(tokens, &mut i) {
                    throws.push(ty);
                    if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::Comma)) {
                        i += 1;
                        continue;
                    }
                }
                if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::RParen)) {
                    i += 1;
                    break;
                } else {
                    if let Some(t) = tokens.get(i) {
                        reporter.emit(
                            Diagnostic::error("expected ')' to close throws list")
                                .with_file(path.to_path_buf())
                                .with_span(t.span),
                        );
                    }
                    break;
                }
            }
        }
    }

    let end_span = tokens
        .get(i.saturating_sub(1))
        .map(|t| t.span)
        .unwrap_or(sig_start);
    Some((
        FuncSig {
            vis,
            is_static,
            is_final,
            is_async,
            is_unsafe,
            name,
            ret,
            params,
            generics,
            throws,
            doc: leading_doc,
            attrs: leading_attrs,
            span: join_span(sig_start, end_span),
        },
        i,
    ))
}

pub(super) fn parse_function_decl(
    tokens: &[Token],
    j: usize,
    reporter: &mut Reporter,
    path: &std::path::Path,
    leading_doc: Option<String>,
    leading_attrs: Vec<Attr>,
    next_id: &mut u32,
) -> Option<(FuncDecl, usize)> {
    let (sig, mut i) = parse_function_sig(tokens, j, reporter, path, leading_doc, leading_attrs)?;
    let (body, span) = if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
        let semi_span = tokens.get(i).map(|t| t.span).unwrap_or(sig.span);
        i += 1;
        (None, join_span(sig.span, semi_span))
    } else if let Some(b) = parse_block(tokens, &mut i) {
        let sp = join_span(sig.span, b.span);
        (Some(b), sp)
    } else {
        (None, sig.span)
    };
    let id = {
        let idv = *next_id;
        *next_id += 1;
        AstId(idv)
    };
    let body_id = if body.is_some() {
        let idv = *next_id;
        *next_id += 1;
        Some(AstId(idv))
    } else {
        None
    };
    Some((
        FuncDecl {
            sig,
            body,
            span,
            id,
            body_id,
        },
        i,
    ))
}

pub(super) fn parse_field_mods(tokens: &[Token], mut i: usize) -> (Visibility, bool, bool, usize) {
    let mut vis = Visibility::Default;
    let mut is_final = false;
    let mut is_shared = false;
    loop {
        match tokens.get(i).map(|t| &t.kind) {
            Some(TokenKind::Public) => {
                vis = Visibility::Public;
                i += 1;
            }
            Some(TokenKind::Internal) => {
                vis = Visibility::Internal;
                i += 1;
            }
            Some(TokenKind::Private) => {
                vis = Visibility::Private;
                i += 1;
            }
            Some(TokenKind::Final) => {
                is_final = true;
                i += 1;
            }
            Some(TokenKind::Shared) => {
                is_shared = true;
                i += 1;
            }
            _ => break,
        }
    }
    (vis, is_final, is_shared, i)
}

/// Parse an interface method declaration.
/// Interface methods can be:
/// - Abstract: `ReturnType methodName(params);` - no body, must be implemented
/// - Default: `default ReturnType methodName(params) { body }` - has a default implementation
pub(super) fn parse_interface_method(
    tokens: &[Token],
    j: usize,
    reporter: &mut Reporter,
    path: &std::path::Path,
    leading_doc: Option<String>,
    leading_attrs: Vec<Attr>,
) -> Option<(InterfaceMethod, usize)> {
    let mut i = j;

    // Check for 'default' keyword
    let is_default = matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::Default));
    if is_default {
        i += 1;
    }

    // Parse the function signature
    let (sig, new_i) = parse_function_sig(tokens, i, reporter, path, leading_doc, leading_attrs)?;
    i = new_i;

    // Parse body if default method, otherwise expect semicolon
    let default_body = if is_default {
        // Default methods must have a body
        if let Some(body) = parse_block(tokens, &mut i) {
            Some(body)
        } else {
            reporter.emit(
                Diagnostic::error("default interface method must have a body")
                    .with_file(path.to_path_buf())
                    .with_span(sig.span),
            );
            // Return a dummy empty body to allow recovery
            Some(Block {
                stmts: Vec::new(),
                span: sig.span,
            })
        }
    } else {
        // Abstract methods expect semicolon
        if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
            i += 1;
        }
        None
    };

    Some((InterfaceMethod { sig, default_body }, i))
}
