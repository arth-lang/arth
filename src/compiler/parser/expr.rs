use super::stmt::parse_block;
use super::types::parse_type_name;
use crate::compiler::ast::{BinOp, Expr, Ident, NamePath, UnOp};
use crate::compiler::lexer::{Token, TokenKind};

// Pratt parser with postfix (call, member, index), unary (!, -), and binary ops.

fn is_numeric_suffix_name(name: &str) -> bool {
    let base = name.to_ascii_lowercase();
    // Signed int aliases and explicit widths
    if base == "int" || base == "short" || base == "long" {
        return true;
    }
    if matches!(base.as_str(), "i8" | "i16" | "i32" | "i64" | "i128") {
        return true;
    }
    if base.starts_with("int") {
        if base.len() > 3 {
            if base[3..].parse::<u16>().is_ok() {
                return true;
            }
        }
    }
    // Unsigned int aliases and explicit widths
    if matches!(base.as_str(), "u8" | "u16" | "u32" | "u64" | "u128") {
        return true;
    }
    if base.starts_with("uint") {
        if base.len() > 4 {
            if base[4..].parse::<u16>().is_ok() {
                return true;
            }
        }
    }
    // Float aliases
    if matches!(
        base.as_str(),
        "float" | "double" | "f32" | "f64" | "float32" | "float64"
    ) {
        return true;
    }
    false
}

pub fn parse_expr(tokens: &[Token], i: &mut usize) -> Option<Expr> {
    if !super::inc_depth() {
        return None; // Nesting depth exceeded
    }
    let result = parse_precedence(tokens, i, 0);
    super::dec_depth();
    result
}

fn precedence(tok: &TokenKind) -> Option<u8> {
    match tok {
        // Lowest … highest (larger = tighter)
        // Ternary conditional has the lowest precedence
        TokenKind::Question => Some(0),
        // Logical OR/AND
        TokenKind::OrOr => Some(1),
        TokenKind::AndAnd => Some(2),
        // Bitwise OR/XOR/AND
        TokenKind::Pipe => Some(3),
        TokenKind::Caret => Some(4),
        TokenKind::Ampersand => Some(5),
        // Equality / Relational
        TokenKind::EqEq | TokenKind::BangEq => Some(6),
        TokenKind::Lt | TokenKind::Le | TokenKind::Gt | TokenKind::Ge => Some(7),
        // Shifts
        TokenKind::Shl | TokenKind::Shr => Some(8),
        // Additive / Multiplicative
        TokenKind::Plus | TokenKind::Minus => Some(9),
        TokenKind::Star | TokenKind::Slash | TokenKind::Percent => Some(10),
        _ => None,
    }
}

fn can_start_unary(tok: &TokenKind) -> bool {
    matches!(
        tok,
        TokenKind::Number(_)
            | TokenKind::Float(_)
            | TokenKind::StringLit(_)
            | TokenKind::CharLit(_)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Ident(_)
            | TokenKind::LParen
            | TokenKind::LBracket
            | TokenKind::LBrace
            | TokenKind::Fn
            | TokenKind::Await
            | TokenKind::Bang
            | TokenKind::Minus
    )
}

pub(super) fn parse_primary(tokens: &[Token], i: &mut usize) -> Option<Expr> {
    let t = tokens.get(*i)?;
    match &t.kind {
        TokenKind::Fn => {
            // Parse lambda: fn (Type name, ...)? { Block }
            *i += 1; // consume 'fn'
            if !matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::LParen)) {
                return None;
            }
            *i += 1; // consume '('
            let mut params: Vec<crate::compiler::ast::Param> = Vec::new();
            loop {
                if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RParen)) {
                    *i += 1; // ')'
                    break;
                }
                if let Some(pty) = parse_type_name(tokens, i) {
                    if let Some(TokenKind::Ident(name)) = tokens.get(*i).map(|t| &t.kind) {
                        let pn = Ident(name.clone());
                        *i += 1;
                        params.push(crate::compiler::ast::Param { name: pn, ty: pty });
                        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Comma)) {
                            *i += 1;
                            continue;
                        }
                        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RParen)) {
                            *i += 1;
                            break;
                        }
                    } else {
                        // expected param name
                        break;
                    }
                } else {
                    // empty or invalid param list
                    break;
                }
            }
            if let Some(body) = parse_block(tokens, i) {
                return Some(Expr::FnLiteral(params, body));
            }
            None
        }
        TokenKind::Number(n) => {
            *i += 1;
            let mut expr = Expr::Int(*n);
            if let Some(next) = tokens.get(*i) {
                if let TokenKind::Ident(ref name) = next.kind {
                    // Numeric literal type suffix: 1i32, 0xffu8, etc.
                    if next.span.start == t.span.end && is_numeric_suffix_name(name) {
                        *i += 1;
                        let np = NamePath::new(vec![Ident(name.clone())]);
                        expr = Expr::Cast(np, Box::new(expr));
                    }
                }
            }
            Some(expr)
        }
        TokenKind::Float(x) => {
            *i += 1;
            let mut expr = Expr::Float(*x);
            if let Some(next) = tokens.get(*i) {
                if let TokenKind::Ident(ref name) = next.kind {
                    // Float literal type suffix: 1.0f32, 2e10f64, etc.
                    if next.span.start == t.span.end && is_numeric_suffix_name(name) {
                        *i += 1;
                        let np = NamePath::new(vec![Ident(name.clone())]);
                        expr = Expr::Cast(np, Box::new(expr));
                    }
                }
            }
            Some(expr)
        }
        TokenKind::StringLit(s) => {
            *i += 1;
            Some(Expr::Str(s.clone()))
        }
        TokenKind::CharLit(c) => {
            *i += 1;
            Some(Expr::Char(*c))
        }
        TokenKind::True => {
            *i += 1;
            Some(Expr::Bool(true))
        }
        TokenKind::False => {
            *i += 1;
            Some(Expr::Bool(false))
        }
        TokenKind::Ident(name) => {
            *i += 1;
            Some(Expr::Ident(Ident(name.clone())))
        }
        TokenKind::LBracket => {
            // Parse list literal: [expr, expr, ...]
            *i += 1; // consume '['
            let mut elements: Vec<Expr> = Vec::new();
            loop {
                if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RBracket)) {
                    *i += 1; // consume ']'
                    break;
                }
                if let Some(elem) = parse_expr(tokens, i) {
                    elements.push(elem);
                    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Comma)) {
                        *i += 1; // consume ','
                        continue;
                    }
                    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RBracket)) {
                        *i += 1; // consume ']'
                        break;
                    }
                }
                break;
            }
            Some(Expr::ListLit(elements))
        }
        TokenKind::LBrace => {
            // Parse map literal: {k: v, k: v, ...} or { ..spread, k: v }
            // NOTE: This conflicts with block syntax, so we need to disambiguate.
            // We'll try to parse as map literal if we see:
            // - empty {}
            // - spread syntax: { ..expr
            // - key: value pattern
            let mut j = *i + 1;
            let is_map = if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::RBrace)) {
                // Empty {} - treat as empty map
                true
            } else if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::DotDot)) {
                // Spread syntax: { ..expr, ... }
                true
            } else if let Some(_) = parse_expr(tokens, &mut j) {
                // Check if there's a colon after the first expression
                matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Colon))
            } else {
                false
            };

            if is_map {
                *i += 1; // consume '{'
                let mut pairs: Vec<(Expr, Expr)> = Vec::new();
                let mut spread: Option<Box<Expr>> = None;

                loop {
                    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RBrace)) {
                        *i += 1; // consume '}'
                        break;
                    }

                    // Check for spread syntax: ..expr
                    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::DotDot)) {
                        *i += 1; // consume '..'
                        if let Some(spread_expr) = parse_expr(tokens, i) {
                            spread = Some(Box::new(spread_expr));
                            // Handle trailing comma after spread
                            if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Comma)) {
                                *i += 1; // consume ','
                                continue;
                            }
                            if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RBrace)) {
                                *i += 1; // consume '}'
                                break;
                            }
                        }
                        break;
                    }

                    // Parse key: value pair
                    if let Some(key) = parse_expr(tokens, i) {
                        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Colon)) {
                            *i += 1; // consume ':'
                            if let Some(value) = parse_expr(tokens, i) {
                                pairs.push((key, value));
                                if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Comma))
                                {
                                    *i += 1; // consume ','
                                    continue;
                                }
                                if matches!(
                                    tokens.get(*i).map(|t| &t.kind),
                                    Some(TokenKind::RBrace)
                                ) {
                                    *i += 1; // consume '}'
                                    break;
                                }
                            }
                        }
                    }
                    break;
                }
                Some(Expr::MapLit { pairs, spread })
            } else {
                // Not a map literal, return None to let statement parser handle it
                None
            }
        }
        TokenKind::LParen => {
            // Try to parse a cast of the form: (Type)expr
            let mut j = *i + 1;
            if let Some(np) = parse_type_name(tokens, &mut j) {
                if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::RParen)) {
                    let k = j + 1;
                    if let Some(next) = tokens.get(k).map(|t| &t.kind) {
                        if can_start_unary(next) {
                            // It's a cast
                            *i = k;
                            let operand = parse_unary(tokens, i)?;
                            return Some(Expr::Cast(np, Box::new(operand)));
                        }
                    }
                }
            }
            // Fallback: parenthesized expression
            *i += 1;
            let e = parse_expr(tokens, i)?;
            if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RParen)) {
                *i += 1;
            }
            Some(e)
        }
        _ => None,
    }
}
fn parse_unary(tokens: &[Token], i: &mut usize) -> Option<Expr> {
    match tokens.get(*i).map(|t| &t.kind) {
        Some(TokenKind::Await) => {
            // Parse 'await <expr>' as a dedicated AST node capturing the suspend point.
            *i += 1;
            let base = parse_unary(tokens, i)?;
            let e = parse_postfix(tokens, i, base);
            Some(Expr::Await(Box::new(e)))
        }
        Some(TokenKind::Bang) => {
            *i += 1;
            let e = parse_unary(tokens, i)?;
            Some(Expr::Unary(UnOp::Not, Box::new(e)))
        }
        Some(TokenKind::Minus) => {
            *i += 1;
            let e = parse_unary(tokens, i)?;
            Some(Expr::Unary(UnOp::Neg, Box::new(e)))
        }
        _ => parse_primary(tokens, i),
    }
}

fn parse_postfix(tokens: &[Token], i: &mut usize, mut lhs: Expr) -> Expr {
    loop {
        match tokens.get(*i).map(|t| &t.kind) {
            Some(TokenKind::LParen) => {
                *i += 1;
                let mut args: Vec<Expr> = Vec::new();
                loop {
                    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RParen)) {
                        *i += 1;
                        break;
                    }
                    if let Some(arg) = parse_expr(tokens, i) {
                        args.push(arg);
                    }
                    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Comma)) {
                        *i += 1;
                        continue;
                    }
                    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RParen)) {
                        *i += 1;
                        break;
                    }
                    break;
                }
                lhs = Expr::Call(Box::new(lhs), args);
            }
            Some(TokenKind::Dot) => {
                *i += 1;
                // Special chaining sugar: .then(...)
                if let Some(Token {
                    kind: TokenKind::Ident(name),
                    ..
                }) = tokens.get(*i)
                {
                    if name == "then" {
                        *i += 1;
                        // Expect '(' callee [ '(' args ')' ] ')'
                        if !matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::LParen)) {
                            // If malformed, stop treating as chain and fallback to member parse
                            continue;
                        }
                        *i += 1; // consume '('
                        // Parse a qualified callee name: Ident ('.' Ident)*
                        let mut callee: Option<Expr> = None;
                        if let Some(Token {
                            kind: TokenKind::Ident(first),
                            ..
                        }) = tokens.get(*i)
                        {
                            let mut callee_expr = Expr::Ident(Ident(first.clone()));
                            *i += 1;
                            while matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Dot)) {
                                *i += 1;
                                if let Some(Token {
                                    kind: TokenKind::Ident(nxt),
                                    ..
                                }) = tokens.get(*i)
                                {
                                    callee_expr =
                                        Expr::Member(Box::new(callee_expr), Ident(nxt.clone()));
                                    *i += 1;
                                } else {
                                    break;
                                }
                            }
                            callee = Some(callee_expr);
                        }
                        // Optional inner argument list for the callee
                        let mut extra_args: Vec<Expr> = Vec::new();
                        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::LParen)) {
                            *i += 1;
                            loop {
                                if matches!(
                                    tokens.get(*i).map(|t| &t.kind),
                                    Some(TokenKind::RParen)
                                ) {
                                    *i += 1;
                                    break;
                                }
                                if let Some(arg) = parse_expr(tokens, i) {
                                    extra_args.push(arg);
                                }
                                if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Comma))
                                {
                                    *i += 1;
                                    continue;
                                }
                                if matches!(
                                    tokens.get(*i).map(|t| &t.kind),
                                    Some(TokenKind::RParen)
                                ) {
                                    *i += 1;
                                    break;
                                }
                                break;
                            }
                        }
                        // Expect closing ')' of .then(
                        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RParen)) {
                            *i += 1;
                        }
                        if let Some(callee_expr) = callee {
                            let mut args = Vec::with_capacity(1 + extra_args.len());
                            args.push(lhs);
                            args.extend(extra_args);
                            lhs = Expr::Call(Box::new(callee_expr), args);
                            continue;
                        } else {
                            // malformed, bail out of postfix handling
                            break;
                        }
                    } else {
                        // Regular member access: lhs.name
                        let id = Ident(name.clone());
                        *i += 1;
                        lhs = Expr::Member(Box::new(lhs), id);
                    }
                } else {
                    break;
                }
            }
            Some(TokenKind::QuestionDot) => {
                // Optional chaining: lhs?.field
                *i += 1;
                if let Some(Token {
                    kind: TokenKind::Ident(name),
                    ..
                }) = tokens.get(*i)
                {
                    let id = Ident(name.clone());
                    *i += 1;
                    lhs = Expr::OptionalMember(Box::new(lhs), id);
                } else {
                    break;
                }
            }
            Some(TokenKind::LBracket) => {
                *i += 1;
                if let Some(idx) = parse_expr(tokens, i) {
                    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RBracket)) {
                        *i += 1;
                    }
                    lhs = Expr::Index(Box::new(lhs), Box::new(idx));
                } else {
                    break;
                }
            }
            Some(TokenKind::LBrace) => {
                // Struct construction: TypeName { field: value, ... }
                // Only valid when lhs is an identifier (type name) or member access (qualified type)
                if !matches!(lhs, Expr::Ident(_) | Expr::Member(_, _)) {
                    break;
                }
                *i += 1; // consume '{'
                let mut fields: Vec<(Ident, Expr)> = Vec::new();
                let mut spread: Option<Box<Expr>> = None;
                loop {
                    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RBrace)) {
                        *i += 1;
                        break;
                    }
                    // Check for spread syntax: ..expr
                    if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::DotDot)) {
                        *i += 1;
                        if let Some(spread_expr) = parse_expr(tokens, i) {
                            spread = Some(Box::new(spread_expr));
                        }
                        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Comma)) {
                            *i += 1;
                        }
                        continue;
                    }
                    // Parse field: value
                    if let Some(TokenKind::Ident(field_name)) = tokens.get(*i).map(|t| &t.kind) {
                        let field_ident = Ident(field_name.clone());
                        *i += 1;
                        // Expect ':'
                        if matches!(
                            tokens.get(*i).map(|t| &t.kind),
                            Some(TokenKind::Colon) | Some(TokenKind::Unknown(':'))
                        ) {
                            *i += 1;
                        }
                        if let Some(value) = parse_expr(tokens, i) {
                            fields.push((field_ident, value));
                        }
                        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Comma)) {
                            *i += 1;
                        }
                        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::RBrace)) {
                            *i += 1;
                            break;
                        }
                    } else {
                        // Skip unknown token
                        *i += 1;
                    }
                }
                // Return proper StructLit with preserved type name
                lhs = Expr::StructLit {
                    type_name: Box::new(lhs),
                    fields,
                    spread,
                };
            }
            _ => break,
        }
    }
    lhs
}

fn parse_precedence(tokens: &[Token], i: &mut usize, min_prec: u8) -> Option<Expr> {
    let mut lhs = parse_unary(tokens, i)?;
    lhs = parse_postfix(tokens, i, lhs);
    while let Some(t) = tokens.get(*i) {
        let op_tok = &t.kind;
        let prec = match precedence(op_tok) {
            Some(p) if p >= min_prec => p,
            _ => break,
        };
        if matches!(op_tok, TokenKind::Question) {
            // Ternary conditional: lhs ? then_expr : else_expr
            *i += 1; // consume '?'
            // Parse 'then' expression allowing nested conditionals (right-assoc): use same precedence p
            let then_expr = parse_precedence(tokens, i, prec)?;
            // Expect ':'
            if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::Colon)) {
                *i += 1;
            }
            // Parse 'else' expression (right-assoc): same precedence p
            let else_expr = parse_precedence(tokens, i, prec)?;
            lhs = Expr::Ternary(Box::new(lhs), Box::new(then_expr), Box::new(else_expr));
            continue;
        }
        let op = match op_tok {
            TokenKind::Plus => BinOp::Add,
            TokenKind::Minus => BinOp::Sub,
            TokenKind::Star => BinOp::Mul,
            TokenKind::Slash => BinOp::Div,
            TokenKind::Percent => BinOp::Mod,
            TokenKind::Shl => BinOp::Shl,
            TokenKind::Shr => BinOp::Shr,
            TokenKind::Lt => BinOp::Lt,
            TokenKind::Le => BinOp::Le,
            TokenKind::Gt => BinOp::Gt,
            TokenKind::Ge => BinOp::Ge,
            TokenKind::EqEq => BinOp::Eq,
            TokenKind::BangEq => BinOp::Ne,
            TokenKind::AndAnd => BinOp::And,
            TokenKind::OrOr => BinOp::Or,
            TokenKind::Ampersand => BinOp::BitAnd,
            TokenKind::Pipe => BinOp::BitOr,
            TokenKind::Caret => BinOp::BitXor,
            _ => break,
        };
        *i += 1;
        // Parse the RHS with tighter precedence (prec + 1) to ensure
        // higher-precedence operators bind to the RHS.
        let rhs = parse_precedence(tokens, i, prec + 1)?;
        lhs = Expr::Binary(Box::new(lhs), op, Box::new(rhs));
    }
    Some(lhs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::ast::{BinOp, Expr};
    use crate::compiler::lexer::lex_all;

    fn parse(s: &str) -> Expr {
        let toks = lex_all(s);
        let mut i = 0usize;
        match parse_expr(&toks, &mut i) {
            Some(e) => e.clone(),
            None => {
                eprintln!(
                    "tokens: {:?}",
                    toks.iter().map(|t| &t.kind).collect::<Vec<_>>()
                );
                panic!("expr");
            }
        }
    }

    #[test]
    fn arithmetic_precedence_and_assoc() {
        // 1 + 2 * 3 => 1 + (2 * 3)
        let e = parse("1 + 2 * 3");
        match e {
            Expr::Binary(a, op, b) => {
                assert!(matches!(*a, Expr::Int(1)));
                assert!(matches!(op, BinOp::Add));
                match *b {
                    Expr::Binary(x, op2, y) => {
                        assert!(matches!(*x, Expr::Int(2)));
                        assert!(matches!(op2, BinOp::Mul));
                        assert!(matches!(*y, Expr::Int(3)));
                    }
                    _ => panic!("expected rhs mul"),
                }
            }
            _ => panic!("expected binary add"),
        }

        // (1 + 2) * 3 => (1 + 2) as lhs
        let e = parse("(1 + 2) * 3");
        match e {
            Expr::Binary(a, op, b) => {
                assert!(matches!(op, BinOp::Mul));
                match *a {
                    Expr::Binary(x, op2, y) => {
                        assert!(matches!(op2, BinOp::Add));
                        assert!(matches!(*x, Expr::Int(1)));
                        assert!(matches!(*y, Expr::Int(2)));
                    }
                    _ => panic!("expected lhs add"),
                }
                assert!(matches!(*b, Expr::Int(3)));
            }
            _ => panic!("expected binary mul"),
        }

        // Left associativity: 4 - 2 - 1 => (4 - 2) - 1
        let e = parse("4 - 2 - 1");
        match e {
            Expr::Binary(a, op, b) => {
                assert!(matches!(op, BinOp::Sub));
                match *a {
                    Expr::Binary(x, op2, y) => {
                        assert!(matches!(op2, BinOp::Sub));
                        assert!(matches!(*x, Expr::Int(4)));
                        assert!(matches!(*y, Expr::Int(2)));
                    }
                    _ => panic!("expected lhs sub"),
                }
                assert!(matches!(*b, Expr::Int(1)));
            }
            _ => panic!("expected binary sub"),
        }
    }

    #[test]
    fn parses_await_unary_node() {
        let e = parse("await f(x)");
        match e {
            Expr::Await(inner) => match *inner {
                Expr::Call(inner_callee, ref inner_args) => {
                    match *inner_callee {
                        Expr::Ident(Ident(ref n)) if n == "f" => {}
                        _ => panic!("expected inner call to f"),
                    }
                    assert_eq!(inner_args.len(), 1);
                }
                _ => panic!("expected inner call as await target"),
            },
            _ => panic!("expected await AST node"),
        }
    }

    #[test]
    fn logical_precedence() {
        // a && b || c => (a && b) || c
        let e = parse("a && b || c");
        match e {
            Expr::Binary(ab, op, c) => {
                assert!(matches!(op, BinOp::Or));
                assert!(matches!(*c, Expr::Ident(_)));
                match *ab {
                    Expr::Binary(a, op2, b) => {
                        assert!(matches!(op2, BinOp::And));
                        assert!(matches!(*a, Expr::Ident(_)));
                        assert!(matches!(*b, Expr::Ident(_)));
                    }
                    _ => panic!("expected lhs and"),
                }
            }
            _ => panic!("expected binary or"),
        }
    }

    #[test]
    fn postfix_chains() {
        // a.b(c)[i].d
        let e = parse("a.b(c)[i].d");
        // Expect Member(Index(Call(Member(Ident(a), b), [Ident(c)]), Ident(i)), d)
        match e {
            Expr::Member(root, d) => {
                assert_eq!(d.0, "d");
                match *root {
                    Expr::Index(call, idx) => {
                        assert!(matches!(*idx, Expr::Ident(Ident(ref s)) if s == "i"));
                        match *call {
                            Expr::Call(target, ref args) => {
                                assert_eq!(args.len(), 1);
                                assert!(matches!(args[0], Expr::Ident(Ident(ref s)) if s == "c"));
                                match *target {
                                    Expr::Member(base, b) => {
                                        assert_eq!(b.0, "b");
                                        assert!(
                                            matches!(*base, Expr::Ident(Ident(ref s)) if s == "a")
                                        );
                                    }
                                    _ => panic!("expected member"),
                                }
                            }
                            _ => panic!("expected call"),
                        }
                    }
                    _ => panic!("expected index"),
                }
            }
            _ => panic!("expected member"),
        }
    }

    #[test]
    fn then_chaining_simple() {
        // s.then(trim).then(lower)
        let e = parse("s.then(trim).then(lower)");
        // Expect Call(lower, [Call(trim, [s])])
        match e {
            Expr::Call(callee2, args2) => {
                assert_eq!(args2.len(), 1);
                assert!(matches!(*callee2, Expr::Ident(Ident(ref s)) if s == "lower"));
                match args2[0].clone() {
                    Expr::Call(callee1, args1) => {
                        assert_eq!(args1.len(), 1);
                        assert!(matches!(*callee1, Expr::Ident(Ident(ref s)) if s == "trim"));
                        assert!(matches!(args1[0], Expr::Ident(Ident(ref s)) if s == "s"));
                    }
                    _ => panic!("expected inner call"),
                }
            }
            _ => panic!("expected outer call"),
        }
    }

    #[test]
    fn then_chaining_with_module_and_args() {
        // s.then(strings.replace(a, b))
        let e = parse("s.then(strings.replace(a, b))");
        match e {
            Expr::Call(callee, args) => {
                assert_eq!(args.len(), 3);
                // first arg is original s
                assert!(matches!(args[0], Expr::Ident(Ident(ref s)) if s == "s"));
                assert!(matches!(args[1], Expr::Ident(Ident(ref s)) if s == "a"));
                assert!(matches!(args[2], Expr::Ident(Ident(ref s)) if s == "b"));
                match *callee {
                    Expr::Member(modname, Ident(ref fname)) => {
                        assert_eq!(fname, "replace");
                        assert!(matches!(*modname, Expr::Ident(Ident(ref s)) if s == "strings"));
                    }
                    _ => panic!("expected qualified callee"),
                }
            }
            _ => panic!("expected call"),
        }
    }

    #[test]
    fn string_char_float_literals() {
        // String literal
        let e = parse("\"hi\"");
        match e {
            Expr::Str(s) => assert_eq!(s, "hi"),
            _ => panic!("expected string"),
        }

        // Char literal
        let e2 = parse("'x'");
        match e2 {
            Expr::Char(c) => assert_eq!(c, 'x'),
            _ => panic!("expected char"),
        }

        // Float literal
        let e3 = parse("3.14");
        let expected = 314.0_f64 / 100.0;
        match e3 {
            Expr::Float(x) => assert!((x - expected).abs() < 1e-9),
            _ => panic!("expected float"),
        }
    }

    #[test]
    fn ternary_basic_and_nested() {
        // a ? b : c
        let e = parse("a ? b : c");
        match e {
            Expr::Ternary(c, t, f) => {
                assert!(matches!(*c, Expr::Ident(Ident(ref s)) if s == "a"));
                assert!(matches!(*t, Expr::Ident(Ident(ref s)) if s == "b"));
                assert!(matches!(*f, Expr::Ident(Ident(ref s)) if s == "c"));
            }
            _ => panic!("expected ternary"),
        }

        // Right-associative: a ? b : c ? d : e  => a ? b : (c ? d : e)
        let e2 = parse("a ? b : c ? d : e");
        match e2 {
            Expr::Ternary(c1, t1, f1) => {
                assert!(matches!(*c1, Expr::Ident(Ident(ref s)) if s == "a"));
                assert!(matches!(*t1, Expr::Ident(Ident(ref s)) if s == "b"));
                match *f1 {
                    Expr::Ternary(c2, t2, f2) => {
                        assert!(matches!(*c2, Expr::Ident(Ident(ref s)) if s == "c"));
                        assert!(matches!(*t2, Expr::Ident(Ident(ref s)) if s == "d"));
                        assert!(matches!(*f2, Expr::Ident(Ident(ref s)) if s == "e"));
                    }
                    _ => panic!("expected nested ternary in else"),
                }
            }
            _ => panic!("expected outer ternary"),
        }

        // Precedence relative to +: a ? b + c : d  => then side parses binary
        let e3 = parse("a ? b + c : d");
        match e3 {
            Expr::Ternary(_, t, _) => match *t {
                Expr::Binary(l, op, r) => {
                    assert!(matches!(op, BinOp::Add));
                    assert!(matches!(*l, Expr::Ident(Ident(ref s)) if s == "b"));
                    assert!(matches!(*r, Expr::Ident(Ident(ref s)) if s == "c"));
                }
                _ => panic!("expected binary in then"),
            },
            _ => panic!("expected ternary"),
        }
    }
}
