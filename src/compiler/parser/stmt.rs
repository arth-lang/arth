use crate::compiler::ast::{Block, Expr, Ident, NamePath, Pattern, Stmt};
use crate::compiler::lexer::{Token, TokenKind};
use crate::compiler::source::Span;

use super::expr;
use super::util::join_span;

/// Parse a pattern for enum pattern matching.
/// Patterns supported:
/// - `_` : wildcard
/// - `x` : variable binding (starts with lowercase)
/// - `123` or `"str"` : literal (falls back to expression)
/// - `EnumType.Variant` : unit variant pattern
/// - `EnumType.Variant(p1, p2)` : tuple variant pattern with sub-patterns
fn parse_pattern(tokens: &[Token], i: &mut usize) -> Option<Pattern> {
    let start = *i;

    // Check for wildcard
    if let Some(tok) = tokens.get(*i)
        && let TokenKind::Ident(name) = &tok.kind
        && name == "_"
    {
        *i += 1;
        return Some(Pattern::Wildcard);
    }

    // Try to parse enum variant pattern: EnumName.VariantName or EnumName.VariantName(patterns...)
    // First identifier
    let first_ident = match tokens.get(*i).map(|t| &t.kind) {
        Some(TokenKind::Ident(name)) => {
            *i += 1;
            name.clone()
        }
        _ => {
            // Not an identifier, try parsing as literal expression
            *i = start;
            return parse_literal_pattern(tokens, i);
        }
    };

    // Check for dot (enum variant pattern)
    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Dot)) {
        *i += 1; // consume '.'

        // Expect variant name
        let variant_name = match tokens.get(*i).map(|t| &t.kind) {
            Some(TokenKind::Ident(name)) => {
                let span_start = tokens.get(*i).map(|t| t.span.start).unwrap_or(0);
                *i += 1;
                (name.clone(), span_start)
            }
            _ => {
                // Malformed, fall back to expression
                *i = start;
                return parse_literal_pattern(tokens, i);
            }
        };

        // Parse optional payload patterns: (p1, p2, ...)
        let mut payloads = Vec::new();
        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::LParen)) {
            *i += 1; // consume '('

            // Parse payload patterns
            while !matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RParen)) {
                if *i >= tokens.len() {
                    break;
                }

                // Skip commas
                if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Comma)) {
                    *i += 1;
                    continue;
                }

                // Parse sub-pattern
                if let Some(sub) = parse_pattern(tokens, i) {
                    payloads.push(sub);
                } else {
                    break;
                }
            }

            // Consume ')'
            if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RParen)) {
                *i += 1;
            }
        }

        let span_end = tokens.get(*i).map(|t| t.span.end).unwrap_or(variant_name.1);
        return Some(Pattern::Variant {
            enum_ty: NamePath::new(vec![Ident(first_ident)]),
            variant: Ident(variant_name.0),
            payloads,
            span: Span::new(start, span_end),
        });
    }

    // No dot - check if this is a binding pattern (lowercase identifier)
    // or if it should be treated as an expression
    let first_char = first_ident.chars().next().unwrap_or('A');
    if first_char.is_lowercase() && first_ident != "true" && first_ident != "false" {
        // Binding pattern
        return Some(Pattern::Binding(Ident(first_ident)));
    }

    // Fall back to expression parsing
    *i = start;
    parse_literal_pattern(tokens, i)
}

/// Parse a literal pattern (int, string, bool) by parsing as expression
fn parse_literal_pattern(tokens: &[Token], i: &mut usize) -> Option<Pattern> {
    expr::parse_expr(tokens, i).map(|expr| Pattern::Literal(Box::new(expr)))
}

// Parse an 'if' statement starting at `i` (which must point to TokenKind::If).
// Supports chained `else if` by nesting the next `if` into a synthetic else-Block.
pub(super) fn parse_if_stmt(tokens: &[Token], i: &mut usize) -> Option<Stmt> {
    if !matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::If)) {
        return None;
    }
    // consume 'if'
    *i += 1;
    // optional '('
    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::LParen)) {
        *i += 1;
    }
    let cond = expr::parse_expr(tokens, i).unwrap_or(Expr::Bool(true));
    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RParen)) {
        *i += 1;
    }
    let then_blk = parse_block(tokens, i).unwrap_or(Block {
        stmts: vec![],
        span: Span::new(0, 0),
    });

    // else ... or else if ...
    let else_blk = if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Else)) {
        *i += 1;
        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::If)) {
            // Parse nested `if` and wrap it in a synthetic Block
            let mut j = *i;
            if let Some(nested) = parse_if_stmt(tokens, &mut j) {
                *i = j;
                Some(Block {
                    stmts: vec![nested],
                    span: Span::new(0, 0),
                })
            } else {
                None
            }
        } else {
            parse_block(tokens, i)
        }
    } else {
        None
    };

    Some(Stmt::If {
        cond,
        then_blk,
        else_blk,
    })
}

// Parse a block delimited by '{' '}' and produce a Block AST.
pub(super) fn parse_block(tokens: &[Token], i: &mut usize) -> Option<Block> {
    if !super::inc_depth() {
        return None; // Nesting depth exceeded
    }
    if !matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::LBrace)) {
        super::dec_depth();
        return None;
    }
    let lbrace_span = tokens
        .get(*i)
        .map(|t| t.span)
        .unwrap_or_else(|| Span::new(0, 0));
    *i += 1;
    let start = *i;
    let mut depth = 1usize;
    while let Some(t) = tokens.get(*i) {
        match &t.kind {
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => {
                depth -= 1;
                *i += 1;
                if depth == 0 {
                    break;
                } else {
                    continue;
                }
            }
            _ => {}
        }
        *i += 1;
    }
    let end = *i - 1; // position of matching '}' just processed
    let rbrace_span = tokens.get(end).map(|t| t.span).unwrap_or(lbrace_span);
    let inner = &tokens[start..end];
    // Build statements from inner tokens
    let mut k = 0usize;
    let mut stmts: Vec<Stmt> = Vec::new();
    while k < inner.len() {
        // Recognize a leading label: Ident ':' followed by a loop/switch
        if let (Some(TokenKind::Ident(lbl)), Some(TokenKind::Colon)) = (
            inner.get(k).map(|t| &t.kind),
            inner.get(k + 1).map(|t| &t.kind),
        ) {
            let mut p = k + 2;
            let parsed_stmt = match inner.get(p).map(|t| &t.kind) {
                Some(TokenKind::While) => {
                    // Parse while like below
                    p += 1;
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::LParen)) {
                        p += 1;
                    }
                    let cond = expr::parse_expr(inner, &mut p).unwrap_or(Expr::Bool(false));
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::RParen)) {
                        p += 1;
                    }
                    let body = parse_block(inner, &mut p).unwrap_or(Block {
                        stmts: vec![],
                        span: Span::new(0, 0),
                    });
                    Some(Stmt::Labeled {
                        label: Ident(lbl.clone()),
                        stmt: Box::new(Stmt::While { cond, body }),
                    })
                }
                Some(TokenKind::For) => {
                    // Reuse normal 'for' parsing including for-each sugar
                    let mut q = p; // p currently at 'for'
                    if let Some(fs) = parse_for_stmt(inner, &mut q) {
                        p = q;
                        Some(Stmt::Labeled {
                            label: Ident(lbl.clone()),
                            stmt: Box::new(fs),
                        })
                    } else {
                        None
                    }
                }
                Some(TokenKind::Switch) => {
                    // Parse switch like below, then wrap
                    p += 1;
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::LParen)) {
                        p += 1;
                    }
                    let scrut = expr::parse_expr(inner, &mut p).unwrap_or(Expr::Int(0));
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::RParen)) {
                        p += 1;
                    }
                    if !matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::LBrace)) {
                        // Not a proper switch; fall through to default processing
                        None
                    } else {
                        p += 1;
                        let mut cases: Vec<(Expr, Block)> = Vec::new();
                        let mut pattern_cases_vec: Vec<(Pattern, Block)> = Vec::new();
                        let mut default_blk: Option<Block> = None;
                        while p < inner.len() {
                            if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::RBrace)) {
                                p += 1;
                                break;
                            }
                            match inner.get(p).map(|t| &t.kind) {
                                Some(TokenKind::Case) => {
                                    p += 1;
                                    // Try parsing as pattern first
                                    let pattern_start = p;
                                    if let Some(pat) = parse_pattern(inner, &mut p) {
                                        let is_real_pattern = matches!(
                                            pat,
                                            Pattern::Variant { .. }
                                                | Pattern::Binding(_)
                                                | Pattern::Wildcard
                                        );
                                        if let Some(t) = inner.get(p) {
                                            match t.kind {
                                                TokenKind::Colon | TokenKind::Unknown(':') => {
                                                    p += 1
                                                }
                                                _ => {}
                                            }
                                        }
                                        let blk = parse_case_body(inner, &mut p);
                                        if is_real_pattern {
                                            pattern_cases_vec.push((pat, blk));
                                        } else if let Pattern::Literal(expr) = pat {
                                            cases.push((*expr, blk));
                                        } else {
                                            p = pattern_start;
                                            let ce = expr::parse_expr(inner, &mut p)
                                                .unwrap_or(Expr::Int(0));
                                            if let Some(t) = inner.get(p) {
                                                match t.kind {
                                                    TokenKind::Colon | TokenKind::Unknown(':') => {
                                                        p += 1
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            let blk2 = parse_case_body(inner, &mut p);
                                            cases.push((ce, blk2));
                                        }
                                    } else {
                                        let ce =
                                            expr::parse_expr(inner, &mut p).unwrap_or(Expr::Int(0));
                                        if let Some(t) = inner.get(p) {
                                            match t.kind {
                                                TokenKind::Colon | TokenKind::Unknown(':') => {
                                                    p += 1
                                                }
                                                _ => {}
                                            }
                                        }
                                        let blk = parse_case_body(inner, &mut p);
                                        cases.push((ce, blk));
                                    }
                                }
                                Some(TokenKind::Default) => {
                                    p += 1;
                                    if let Some(t) = inner.get(p) {
                                        match t.kind {
                                            TokenKind::Colon | TokenKind::Unknown(':') => p += 1,
                                            _ => {}
                                        }
                                    }
                                    default_blk = Some(parse_case_body(inner, &mut p));
                                }
                                _ => p += 1,
                            }
                        }
                        Some(Stmt::Labeled {
                            label: Ident(lbl.clone()),
                            stmt: Box::new(Stmt::Switch {
                                expr: scrut,
                                cases,
                                pattern_cases: pattern_cases_vec,
                                default: default_blk,
                            }),
                        })
                    }
                }
                _ => None,
            };
            if let Some(ls) = parsed_stmt {
                stmts.push(ls);
                k = p;
                continue;
            }
            // If label not followed by supported stmt, fall through to normal handling
        }

        match inner.get(k).map(|t| &t.kind) {
            Some(TokenKind::If) => {
                let mut p = k;
                if let Some(if_stmt) = parse_if_stmt(inner, &mut p) {
                    stmts.push(if_stmt);
                    k = p;
                } else {
                    k += 1;
                }
            }
            Some(TokenKind::While) => {
                let mut p = k + 1;
                if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::LParen)) {
                    p += 1;
                }
                let cond = expr::parse_expr(inner, &mut p).unwrap_or(Expr::Bool(false));
                if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::RParen)) {
                    p += 1;
                }
                let body = parse_block(inner, &mut p).unwrap_or(Block {
                    stmts: vec![],
                    span: Span::new(0, 0),
                });
                stmts.push(Stmt::While { cond, body });
                k = p;
            }
            Some(TokenKind::For) => {
                let mut p = k;
                if let Some(fstmt) = parse_for_stmt(inner, &mut p) {
                    stmts.push(fstmt);
                    k = p;
                } else {
                    k += 1;
                }
            }
            Some(TokenKind::Switch) => {
                let mut p = k + 1;
                if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::LParen)) {
                    p += 1;
                }
                let scrut = expr::parse_expr(inner, &mut p).unwrap_or(Expr::Int(0));
                if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::RParen)) {
                    p += 1;
                }
                // Parse switch body: '{' case/default blocks '}'
                if !matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::LBrace)) {
                    k += 1;
                    continue;
                }
                p += 1; // into brace
                let mut cases: Vec<(Expr, Block)> = Vec::new();
                let mut pattern_cases: Vec<(Pattern, Block)> = Vec::new();
                let mut default_blk: Option<Block> = None;
                while p < inner.len() {
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::RBrace)) {
                        p += 1;
                        break;
                    }
                    match inner.get(p).map(|t| &t.kind) {
                        Some(TokenKind::Case) => {
                            p += 1;
                            // Try parsing as pattern first
                            let pattern_start = p;
                            if let Some(pat) = parse_pattern(inner, &mut p) {
                                // Check if this is a "real" pattern (Variant, Binding, Wildcard)
                                // vs a Literal that should go in the legacy cases
                                let is_real_pattern = matches!(
                                    pat,
                                    Pattern::Variant { .. }
                                        | Pattern::Binding(_)
                                        | Pattern::Wildcard
                                );

                                // expect ':' and consume it
                                if let Some(t) = inner.get(p) {
                                    match t.kind {
                                        TokenKind::Colon | TokenKind::Unknown(':') => p += 1,
                                        _ => {}
                                    }
                                }
                                let blk = parse_case_body(inner, &mut p);

                                if is_real_pattern {
                                    pattern_cases.push((pat, blk));
                                } else if let Pattern::Literal(expr) = pat {
                                    cases.push((*expr, blk));
                                } else {
                                    // Fallback: treat as expression case
                                    p = pattern_start;
                                    let ce =
                                        expr::parse_expr(inner, &mut p).unwrap_or(Expr::Int(0));
                                    if let Some(t) = inner.get(p) {
                                        match t.kind {
                                            TokenKind::Colon | TokenKind::Unknown(':') => p += 1,
                                            _ => {}
                                        }
                                    }
                                    let blk2 = parse_case_body(inner, &mut p);
                                    cases.push((ce, blk2));
                                }
                            } else {
                                // Fallback to expression parsing
                                let ce = expr::parse_expr(inner, &mut p).unwrap_or(Expr::Int(0));
                                if let Some(t) = inner.get(p) {
                                    match t.kind {
                                        TokenKind::Colon | TokenKind::Unknown(':') => p += 1,
                                        _ => {}
                                    }
                                }
                                let blk = parse_case_body(inner, &mut p);
                                cases.push((ce, blk));
                            }
                        }
                        Some(TokenKind::Default) => {
                            p += 1;
                            if let Some(t) = inner.get(p) {
                                match t.kind {
                                    TokenKind::Colon | TokenKind::Unknown(':') => p += 1,
                                    _ => {}
                                }
                            }
                            default_blk = Some(parse_case_body(inner, &mut p));
                        }
                        _ => {
                            p += 1;
                        }
                    }
                }
                stmts.push(Stmt::Switch {
                    expr: scrut,
                    cases,
                    pattern_cases,
                    default: default_blk,
                });
                k = p;
            }
            Some(TokenKind::Try) => {
                let mut p = k + 1;
                let try_blk = parse_block(inner, &mut p).unwrap_or(Block {
                    stmts: vec![],
                    span: Span::new(0, 0),
                });
                let mut catches: Vec<crate::compiler::ast::CatchClause> = Vec::new();
                let mut finally_blk: Option<Block> = None;
                loop {
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Catch)) {
                        p += 1;
                        let mut cty: Option<crate::compiler::ast::NamePath> = None;
                        let mut cname: Option<crate::compiler::ast::Ident> = None;
                        if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::LParen)) {
                            p += 1;
                            cty = super::types::parse_name_path(inner, &mut p);
                            // Optional variable name
                            if let Some(id) = super::types::parse_ident(inner, &mut p) {
                                cname = Some(id);
                            }
                            if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::RParen)) {
                                p += 1;
                            }
                        }
                        let blk = parse_block(inner, &mut p).unwrap_or(Block {
                            stmts: vec![],
                            span: Span::new(0, 0),
                        });
                        catches.push(crate::compiler::ast::CatchClause {
                            ty: cty,
                            var: cname,
                            blk,
                        });
                        continue;
                    }
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Finally)) {
                        p += 1;
                        finally_blk = parse_block(inner, &mut p);
                    }
                    break;
                }
                stmts.push(Stmt::Try {
                    try_blk,
                    catches,
                    finally_blk,
                });
                k = p;
            }
            Some(TokenKind::Ident(name)) if name == "println" => {
                let mut p = k + 1;
                if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::LParen)) {
                    p += 1;
                }
                // Always parse a full expression to support concatenation chains
                let e = expr::parse_expr(inner, &mut p).unwrap_or(Expr::Int(0));
                if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::RParen)) {
                    p += 1;
                }
                if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                    p += 1;
                }
                // If the expression is a single string literal, use PrintStr; else PrintExpr
                let stmt = match e {
                    Expr::Str(ref s) => Stmt::PrintStr(s.clone()),
                    _ => Stmt::PrintExpr(e),
                };
                stmts.push(stmt);
                k = p;
            }
            Some(TokenKind::Ident(name)) if name == "print" => {
                let mut p = k + 1;
                if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::LParen)) {
                    p += 1;
                }
                // Always parse a full expression to support concatenation chains
                let e = expr::parse_expr(inner, &mut p).unwrap_or(Expr::Int(0));
                if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::RParen)) {
                    p += 1;
                }
                if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                    p += 1;
                }
                // If the expression is a single string literal, use PrintRawStr; else PrintRawExpr
                let stmt = match e {
                    Expr::Str(ref s) => Stmt::PrintRawStr(s.clone()),
                    _ => Stmt::PrintRawExpr(e),
                };
                stmts.push(stmt);
                k = p;
            }
            Some(TokenKind::Break) => {
                let mut p = k + 1;
                // Optional label identifier before ';'
                let label = if let Some(TokenKind::Ident(name)) = inner.get(p).map(|t| &t.kind) {
                    p += 1;
                    Some(Ident(name.clone()))
                } else {
                    None
                };
                if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                    p += 1;
                }
                stmts.push(Stmt::Break(label));
                k = p;
            }
            Some(TokenKind::Continue) => {
                let mut p = k + 1;
                let label = if let Some(TokenKind::Ident(name)) = inner.get(p).map(|t| &t.kind) {
                    p += 1;
                    Some(Ident(name.clone()))
                } else {
                    None
                };
                if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                    p += 1;
                }
                stmts.push(Stmt::Continue(label));
                k = p;
            }
            Some(TokenKind::Return) => {
                let mut p = k + 1;
                // Parse optional expression until ';'
                let ret_expr =
                    if !matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                        expr::parse_expr(inner, &mut p)
                    } else {
                        None
                    };
                if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                    p += 1;
                }
                stmts.push(Stmt::Return(ret_expr));
                k = p;
            }
            // throw expr; - throws an exception value
            Some(TokenKind::Throw) => {
                let mut p = k + 1;
                // Parse required expression (throw must have an exception value)
                let throw_expr = expr::parse_expr(inner, &mut p);
                if let Some(ex) = throw_expr {
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                        p += 1;
                    }
                    stmts.push(Stmt::Throw(ex));
                } else {
                    // Error: throw requires an expression
                    // For now, skip and move forward
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                        p += 1;
                    }
                }
                k = p;
            }
            // panic(msg); - unrecoverable error that unwinds within task boundary
            Some(TokenKind::Panic) => {
                let mut p = k + 1;
                // Optional parentheses around the message
                let has_paren = matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::LParen));
                if has_paren {
                    p += 1;
                }
                // Parse required message expression
                let panic_msg = expr::parse_expr(inner, &mut p);
                if has_paren {
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::RParen)) {
                        p += 1;
                    }
                }
                if let Some(msg) = panic_msg {
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                        p += 1;
                    }
                    stmts.push(Stmt::Panic(msg));
                } else {
                    // Error: panic requires a message
                    // For now, use a default empty message
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                        p += 1;
                    }
                    stmts.push(Stmt::Panic(Expr::Str("panic".to_string())));
                }
                k = p;
            }
            // unsafe { ... } block
            Some(TokenKind::Unsafe) => {
                let mut p = k + 1;
                let blk = parse_block(inner, &mut p).unwrap_or(Block {
                    stmts: vec![],
                    span: Span::new(0, 0),
                });
                stmts.push(Stmt::Unsafe(blk));
                k = p;
            }
            // var keyword for type-inferred variable declarations
            Some(TokenKind::Var) => {
                let mut p = k + 1;
                if let Some(name) = super::types::parse_ident(inner, &mut p) {
                    // Require initializer for var declarations (needed for type inference)
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Eq)) {
                        p += 1;
                        let init = expr::parse_expr(inner, &mut p);
                        if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                            p += 1;
                        }
                        // Use "var" as a special type name that signals type inference
                        stmts.push(Stmt::VarDecl {
                            is_final: false,
                            is_shared: false,
                            ty: NamePath::new(vec![Ident("var".to_string())]),
                            generics: vec![],
                            fn_params: vec![],
                            name,
                            init,
                        });
                        k = p;
                        continue;
                    }
                }
                k += 1; // skip on error
            }
            _ => {
                let mut p = k;
                let st = parse_simple_stmt(inner, &mut p);
                if let Some(node) = st {
                    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                        p += 1;
                    }
                    stmts.push(node);
                    k = p;
                } else {
                    k += 1;
                }
            }
        }
    }
    super::dec_depth();
    Some(Block {
        stmts,
        span: join_span(lbrace_span, rbrace_span),
    })
}

/// Parse a switch case body. Supports both braced `{ ... }` and brace-less
/// statement sequences. Brace-less cases collect statements until hitting
/// `case`, `default`, or `}` (end of switch body).
fn parse_case_body(tokens: &[Token], i: &mut usize) -> Block {
    // If there's a brace, use standard block parsing
    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::LBrace)) {
        return parse_block(tokens, i).unwrap_or(Block {
            stmts: vec![],
            span: Span::new(0, 0),
        });
    }

    // Brace-less case: collect statements until we hit case, default, or }
    let start_span = tokens
        .get(*i)
        .map(|t| t.span)
        .unwrap_or_else(|| Span::new(0, 0));
    let mut stmts: Vec<Stmt> = Vec::new();

    while *i < tokens.len() {
        // Stop at case body terminators
        match tokens.get(*i).map(|t| &t.kind) {
            Some(TokenKind::Case) | Some(TokenKind::Default) | Some(TokenKind::RBrace) | None => {
                break;
            }
            _ => {}
        }

        // Parse a single statement based on current token
        match tokens.get(*i).map(|t| &t.kind) {
            Some(TokenKind::Ident(name)) if name == "println" => {
                let mut p = *i + 1;
                if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::LParen)) {
                    p += 1;
                }
                let e = expr::parse_expr(tokens, &mut p).unwrap_or(Expr::Int(0));
                if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::RParen)) {
                    p += 1;
                }
                if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                    p += 1;
                }
                let stmt = match e {
                    Expr::Str(ref s) => Stmt::PrintStr(s.clone()),
                    _ => Stmt::PrintExpr(e),
                };
                stmts.push(stmt);
                *i = p;
            }
            Some(TokenKind::Ident(name)) if name == "print" => {
                let mut p = *i + 1;
                if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::LParen)) {
                    p += 1;
                }
                let e = expr::parse_expr(tokens, &mut p).unwrap_or(Expr::Int(0));
                if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::RParen)) {
                    p += 1;
                }
                if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                    p += 1;
                }
                let stmt = match e {
                    Expr::Str(ref s) => Stmt::PrintRawStr(s.clone()),
                    _ => Stmt::PrintRawExpr(e),
                };
                stmts.push(stmt);
                *i = p;
            }
            Some(TokenKind::Break) => {
                let mut p = *i + 1;
                let label = if let Some(TokenKind::Ident(name)) = tokens.get(p).map(|t| &t.kind) {
                    p += 1;
                    Some(Ident(name.clone()))
                } else {
                    None
                };
                if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                    p += 1;
                }
                stmts.push(Stmt::Break(label));
                *i = p;
            }
            Some(TokenKind::Continue) => {
                let mut p = *i + 1;
                let label = if let Some(TokenKind::Ident(name)) = tokens.get(p).map(|t| &t.kind) {
                    p += 1;
                    Some(Ident(name.clone()))
                } else {
                    None
                };
                if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                    p += 1;
                }
                stmts.push(Stmt::Continue(label));
                *i = p;
            }
            Some(TokenKind::Return) => {
                let mut p = *i + 1;
                let ret_expr =
                    if !matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                        expr::parse_expr(tokens, &mut p)
                    } else {
                        None
                    };
                if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                    p += 1;
                }
                stmts.push(Stmt::Return(ret_expr));
                *i = p;
            }
            Some(TokenKind::If) => {
                if let Some(stmt) = parse_if_stmt(tokens, i) {
                    stmts.push(stmt);
                } else {
                    *i += 1; // skip on error
                }
            }
            Some(TokenKind::While) => {
                let mut p = *i + 1;
                if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::LParen)) {
                    p += 1;
                }
                let cond = expr::parse_expr(tokens, &mut p).unwrap_or(Expr::Bool(false));
                if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::RParen)) {
                    p += 1;
                }
                let body = parse_block(tokens, &mut p).unwrap_or(Block {
                    stmts: vec![],
                    span: Span::new(0, 0),
                });
                stmts.push(Stmt::While { cond, body });
                *i = p;
            }
            Some(TokenKind::Var) => {
                // Type-inferred variable declaration: var name = expr;
                let mut p = *i + 1;
                if let Some(name) = super::types::parse_ident(tokens, &mut p) {
                    // Require initializer for var declarations (needed for type inference)
                    if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Eq)) {
                        p += 1;
                        let init = expr::parse_expr(tokens, &mut p);
                        if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                            p += 1;
                        }
                        // Use "var" as a special type name that signals type inference
                        stmts.push(Stmt::VarDecl {
                            is_final: false,
                            is_shared: false,
                            ty: NamePath::new(vec![Ident("var".to_string())]),
                            generics: vec![],
                            fn_params: vec![],
                            name,
                            init,
                        });
                        *i = p;
                        continue;
                    }
                }
                *i += 1; // skip on error
            }
            Some(TokenKind::LBrace) => {
                // Nested block
                if let Some(blk) = parse_block(tokens, i) {
                    stmts.push(Stmt::Block(blk));
                } else {
                    *i += 1;
                }
            }
            Some(TokenKind::Ident(_)) => {
                // Try variable declaration first: Type Name = expr;
                if let Some((vardecl, new_pos)) = try_parse_vardecl(tokens, *i) {
                    stmts.push(vardecl);
                    *i = new_pos;
                    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                        *i += 1;
                    }
                    continue;
                }
                // Then try assignment or other simple statements
                let mut p = *i;
                if let Some(stmt) = parse_simple_stmt(tokens, &mut p) {
                    if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                        p += 1;
                    }
                    stmts.push(stmt);
                    *i = p;
                } else {
                    *i += 1; // skip unknown token
                }
            }
            _ => {
                // Skip unknown tokens
                *i += 1;
            }
        }
    }

    let end_span = tokens
        .get(i.saturating_sub(1))
        .map(|t| t.span)
        .unwrap_or(start_span);

    Block {
        stmts,
        span: join_span(start_span, end_span),
    }
}

// Parse a 'for' statement at inner[k] (k points to 'for').
// Supports both C-style for and foreach: for(T name : expr) { ... }
fn parse_for_stmt(inner: &[Token], k: &mut usize) -> Option<Stmt> {
    if !matches!(inner.get(*k).map(|t| &t.kind), Some(TokenKind::For)) {
        return None;
    }
    let mut p = *k + 1;
    if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::LParen)) {
        p += 1;
    }
    // Lookahead: try to parse Type Ident ':' Expr ')' ...
    let foreach = {
        let type_start = p;
        let mut tp = p;
        if let Some(var_ty) = super::types::parse_type_name(inner, &mut tp) {
            if let Some(var_ident) = super::types::parse_ident(inner, &mut tp) {
                if matches!(inner.get(tp).map(|t| &t.kind), Some(TokenKind::Colon)) {
                    // Extract generic args of the variable type from [type_start, tp)
                    let mut generics: Vec<crate::compiler::ast::NamePath> = Vec::new();
                    let mut gscan = type_start;
                    while gscan < tp {
                        if matches!(inner.get(gscan).map(|t| &t.kind), Some(TokenKind::Lt)) {
                            let mut g = gscan + 1;
                            loop {
                                if g >= tp {
                                    break;
                                }
                                if let Some(arg) = super::types::parse_type_name(inner, &mut g) {
                                    generics.push(arg);
                                }
                                match inner.get(g).map(|t| &t.kind) {
                                    Some(TokenKind::Comma) => {
                                        g += 1;
                                        continue;
                                    }
                                    Some(TokenKind::Gt) | Some(TokenKind::Shr) => {
                                        break;
                                    }
                                    _ => break,
                                }
                            }
                            break;
                        }
                        gscan += 1;
                    }
                    // consume ':'
                    tp += 1;
                    // Parse iterable expr
                    let iter_expr = expr::parse_expr(inner, &mut tp).unwrap_or(Expr::Int(0));
                    // consume optional ')'
                    if matches!(inner.get(tp).map(|t| &t.kind), Some(TokenKind::RParen)) {
                        tp += 1;
                    }
                    // Parse body block
                    let body = parse_block(inner, &mut tp).unwrap_or(Block {
                        stmts: vec![],
                        span: Span::new(0, 0),
                    });

                    // Desugar foreach into index-based for over List: for (int __i=0; __i < List.len(iter); __i+=1) { T name = iter.get(__i); body }
                    let idx_name = Ident("__i".to_string());
                    let init = Stmt::VarDecl {
                        is_final: false,
                        is_shared: false,
                        ty: crate::compiler::ast::NamePath::new(vec![Ident("int".to_string())]),
                        generics: vec![],
                        fn_params: vec![],
                        name: idx_name.clone(),
                        init: Some(Expr::Int(0)),
                    };
                    // cond: __i < List.len(iter_expr)
                    let list_ident = Expr::Ident(Ident("List".to_string()));
                    let len_member = Expr::Member(Box::new(list_ident), Ident("len".to_string()));
                    let len_call = Expr::Call(Box::new(len_member), vec![iter_expr.clone()]);
                    let cond = Expr::Binary(
                        Box::new(Expr::Ident(idx_name.clone())),
                        crate::compiler::ast::BinOp::Lt,
                        Box::new(len_call),
                    );
                    // step: __i += 1
                    let step = Stmt::AssignOp {
                        name: idx_name.clone(),
                        op: crate::compiler::ast::AssignOp::Add,
                        expr: Expr::Int(1),
                    };
                    // head of body: T name = iter_expr.get(__i);
                    let get_member =
                        Expr::Member(Box::new(iter_expr.clone()), Ident("get".to_string()));
                    let get_call =
                        Expr::Call(Box::new(get_member), vec![Expr::Ident(idx_name.clone())]);
                    let head_decl = Stmt::VarDecl {
                        is_final: false,
                        is_shared: false,
                        ty: var_ty,
                        generics,
                        fn_params: vec![],
                        name: var_ident,
                        init: Some(get_call),
                    };
                    let mut body_stmts = Vec::with_capacity(1 + body.stmts.len());
                    body_stmts.push(head_decl);
                    body_stmts.extend(body.stmts.into_iter());
                    let new_body = Block {
                        stmts: body_stmts,
                        span: body.span,
                    };
                    *k = tp;
                    Some(Stmt::For {
                        init: Some(Box::new(init)),
                        cond: Some(cond),
                        step: Some(Box::new(step)),
                        body: new_body,
                    })
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    };

    if let Some(stmt) = foreach {
        return Some(stmt);
    }

    // Fallback: regular C-style for
    // p currently points after '(' (or first token following 'for')
    // init
    let init = if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
        p += 1;
        None
    } else {
        // Try assignment or vardecl; do not consume trailing ';' here
        let start_p = p;
        let init_stmt = parse_simple_stmt(inner, &mut p);
        if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
            p += 1;
            init_stmt.map(Box::new)
        } else {
            p = start_p;
            if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                p += 1;
                None
            } else {
                None
            }
        }
    };
    // cond
    let cond = if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
        p += 1;
        None
    } else {
        let e = expr::parse_expr(inner, &mut p);
        if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
            p += 1;
        }
        e
    };
    // step
    let step = if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::RParen)) {
        p += 1;
        None
    } else {
        let s = parse_simple_stmt(inner, &mut p).map(Box::new);
        if matches!(inner.get(p).map(|t| &t.kind), Some(TokenKind::RParen)) {
            p += 1;
        }
        s
    };
    let body = parse_block(inner, &mut p).unwrap_or(Block {
        stmts: vec![],
        span: Span::new(0, 0),
    });
    *k = p;
    Some(Stmt::For {
        init,
        cond,
        step,
        body,
    })
}

// Parse a simple statement like assignment without consuming trailing ';'
pub(super) fn parse_simple_stmt(tokens: &[Token], i: &mut usize) -> Option<Stmt> {
    // Try variable declaration first
    if let Some((vd, p)) = try_parse_vardecl(tokens, *i) {
        // Don't consume semicolon - caller handles it
        *i = p;
        return Some(vd);
    }
    // Field assignment: expr '.' Ident '=' expr (handles nested access like a.b.c = value)
    // First try to parse a member expression and check if followed by '=' and RHS
    {
        let start = *i;
        let mut p = *i;
        // Parse a primary/postfix expression that might end with member access
        if let Some(mut lhs) = expr::parse_primary(tokens, &mut p) {
            // Keep parsing member accesses
            while let (
                Some(Token {
                    kind: TokenKind::Dot,
                    ..
                }),
                Some(Token {
                    kind: TokenKind::Ident(member),
                    ..
                }),
            ) = (tokens.get(p), tokens.get(p + 1))
            {
                p += 2;
                lhs = Expr::Member(Box::new(lhs), Ident(member.clone()));
            }

            // Now check if we have '=' followed by RHS
            if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Eq)) {
                // Make sure LHS is a member expression
                if let Expr::Member(obj, field) = lhs {
                    p += 1;
                    if let Some(rhs) = expr::parse_expr(tokens, &mut p) {
                        *i = p;
                        return Some(Stmt::FieldAssign {
                            object: *obj,
                            field,
                            expr: rhs,
                        });
                    }
                }
            }
        }
        // Restore position if we didn't succeed
        *i = start;
    }
    // Then assignment
    match tokens.get(*i).map(|t| &t.kind) {
        Some(TokenKind::Ident(name)) => {
            let mut p = *i + 1;
            // Support post-increment/decrement: name '++' or name '--'
            if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Plus))
                && matches!(tokens.get(p + 1).map(|t| &t.kind), Some(TokenKind::Plus))
            {
                p += 2;
                *i = p;
                return Some(Stmt::AssignOp {
                    name: Ident(name.clone()),
                    op: crate::compiler::ast::AssignOp::Add,
                    expr: Expr::Int(1),
                });
            }
            if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Minus))
                && matches!(tokens.get(p + 1).map(|t| &t.kind), Some(TokenKind::Minus))
            {
                p += 2;
                *i = p;
                return Some(Stmt::AssignOp {
                    name: Ident(name.clone()),
                    op: crate::compiler::ast::AssignOp::Sub,
                    expr: Expr::Int(1),
                });
            }
            if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Eq)) {
                p += 1;
                let e = expr::parse_expr(tokens, &mut p)?;
                *i = p;
                return Some(Stmt::Assign {
                    name: Ident(name.clone()),
                    expr: e,
                });
            } else if matches!(
                tokens.get(p).map(|t| &t.kind),
                Some(
                    TokenKind::PlusEq
                        | TokenKind::MinusEq
                        | TokenKind::StarEq
                        | TokenKind::SlashEq
                        | TokenKind::PercentEq
                        | TokenKind::ShlEq
                        | TokenKind::ShrEq
                        | TokenKind::AndEq
                        | TokenKind::OrEq
                        | TokenKind::XorEq
                )
            ) {
                let op = match tokens.get(p).map(|t| &t.kind) {
                    Some(TokenKind::PlusEq) => crate::compiler::ast::AssignOp::Add,
                    Some(TokenKind::MinusEq) => crate::compiler::ast::AssignOp::Sub,
                    Some(TokenKind::StarEq) => crate::compiler::ast::AssignOp::Mul,
                    Some(TokenKind::SlashEq) => crate::compiler::ast::AssignOp::Div,
                    Some(TokenKind::PercentEq) => crate::compiler::ast::AssignOp::Mod,
                    Some(TokenKind::ShlEq) => crate::compiler::ast::AssignOp::Shl,
                    Some(TokenKind::ShrEq) => crate::compiler::ast::AssignOp::Shr,
                    Some(TokenKind::AndEq) => crate::compiler::ast::AssignOp::And,
                    Some(TokenKind::OrEq) => crate::compiler::ast::AssignOp::Or,
                    Some(TokenKind::XorEq) => crate::compiler::ast::AssignOp::Xor,
                    _ => return None, // Should be unreachable given outer check
                };
                p += 1;
                let e = expr::parse_expr(tokens, &mut p)?;
                *i = p;
                return Some(Stmt::AssignOp {
                    name: Ident(name.clone()),
                    op,
                    expr: e,
                });
            }
            // Not an assignment: treat as a general expression statement
            let mut q = *i;
            if let Some(e) = expr::parse_expr(tokens, &mut q) {
                *i = q;
                return Some(Stmt::Expr(e));
            }
            None
        }
        _ => {
            // Fallback: parse a general expression statement
            let mut p = *i;
            if let Some(e) = expr::parse_expr(tokens, &mut p) {
                *i = p;
                return Some(Stmt::Expr(e));
            }
            None
        }
    }
}

// Attempt to parse a variable declaration: Type Name (= expr)? at `k`.
pub(super) fn try_parse_vardecl(tokens: &[Token], k: usize) -> Option<(Stmt, usize)> {
    let mut i = k;
    // Optional 'final'/'shared' modifiers before type (in any order)
    let mut is_final = false;
    let mut is_shared = false;
    loop {
        match tokens.get(i).map(|t| &t.kind) {
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
    // Capture generics for the declared type by looking at the token window consumed by parse_type_name
    let type_start = i;
    let ty = super::types::parse_type_name(tokens, &mut i)?;
    let type_end = i;
    // Extract generic arguments between first '<' and the matching '>' within [type_start, type_end)
    let mut generics: Vec<crate::compiler::ast::NamePath> = Vec::new();
    let mut p = type_start;
    while p < type_end {
        if matches!(tokens.get(p).map(|t| &t.kind), Some(TokenKind::Lt)) {
            // parse arguments
            let mut g = p + 1;
            loop {
                if g >= type_end {
                    break;
                }
                if let Some(arg) = super::types::parse_type_name(tokens, &mut g) {
                    generics.push(arg);
                }
                match tokens.get(g).map(|t| &t.kind) {
                    Some(TokenKind::Comma) => {
                        g += 1;
                        continue;
                    }
                    Some(TokenKind::Gt) => {
                        break;
                    }
                    Some(TokenKind::Shr) => {
                        break;
                    }
                    _ => break,
                }
            }
            break;
        }
        p += 1;
    }
    // Optional function-type parameter list immediately after the type name: Fn<Ret>(T1, T2)
    let mut fn_params: Vec<crate::compiler::ast::NamePath> = Vec::new();
    if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::LParen)) {
        // Parse zero or more type names until ')'
        i += 1;
        loop {
            if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::RParen)) {
                i += 1;
                break;
            }
            if let Some(pt) = super::types::parse_type_name(tokens, &mut i) {
                fn_params.push(pt);
                if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::Comma)) {
                    i += 1;
                    continue;
                }
                if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::RParen)) {
                    i += 1;
                    break;
                }
            } else {
                break;
            }
        }
    }
    let name = super::types::parse_ident(tokens, &mut i)?;
    let init = if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::Eq)) {
        i += 1;
        expr::parse_expr(tokens, &mut i)
    } else {
        None
    };
    Some((
        Stmt::VarDecl {
            is_final,
            is_shared,
            ty,
            generics,
            fn_params,
            name,
            init,
        },
        i,
    ))
}
