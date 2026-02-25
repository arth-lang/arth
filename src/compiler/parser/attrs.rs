use crate::compiler::ast::Attr;
use crate::compiler::lexer::{Token, TokenKind};

// Collect and consume leading doc comments (/// and /** */) and attributes (@Name or @Name(...)).
// Returns (doc_string, attributes).
pub(super) fn take_leading_doc_attrs(
    text: &str,
    tokens: &[Token],
    i: &mut usize,
) -> (Option<String>, Vec<Attr>) {
    // Gather contiguous doc comments first
    let mut docs: Vec<String> = Vec::new();
    loop {
        match tokens.get(*i).map(|t| &t.kind) {
            Some(TokenKind::DocLine(s)) => {
                // Trim a single leading space for line docs
                let line = s.trim_start_matches(' ').to_string();
                docs.push(line);
                *i += 1;
            }
            Some(TokenKind::DocBlock(s)) => {
                let blk = s.trim().to_string();
                docs.push(blk);
                *i += 1;
            }
            _ => break,
        }
    }
    let doc = if docs.is_empty() {
        None
    } else {
        Some(docs.join("\n"))
    };

    // Then zero or more attributes
    let mut attrs: Vec<Attr> = Vec::new();
    loop {
        if !matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::At)) {
            break;
        }
        *i += 1; // '@'
        let mut name_i = *i;
        if let Some(name) = super::types::parse_name_path(tokens, &mut name_i) {
            // Optional argument list: capture raw substring inside outermost (...)
            let mut args: Option<String> = None;
            let mut k = name_i;
            if matches!(tokens.get(k).map(|t| &t.kind), Some(TokenKind::LParen)) {
                let lparen_end = tokens.get(k).map(|t| t.span.end);
                k += 1;
                let mut depth: i32 = 1;
                while let Some(t) = tokens.get(k) {
                    match t.kind {
                        TokenKind::LParen => depth += 1,
                        TokenKind::RParen => {
                            depth -= 1;
                            if depth == 0 {
                                if let Some(start) = lparen_end {
                                    let end = t.span.start;
                                    if start <= end && end <= text.len() {
                                        args = Some(text[start..end].to_string());
                                    }
                                }
                                k += 1; // consume ')'
                                break;
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
            }
            *i = k;
            attrs.push(Attr { name, args });
        } else {
            break;
        }
    }
    (doc, attrs)
}

// Skip zero or more attributes of the form: @Name or @Name(...)
#[allow(dead_code)]
pub(super) fn skip_attribute_list(tokens: &[Token], i: &mut usize) {
    loop {
        if !matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::At)) {
            break;
        }
        *i += 1; // '@'
        // Optional qualified identifier name
        let _ = super::types::parse_name_path(tokens, i);
        // Optional parenthesized arguments; skip balanced parentheses
        if matches!(tokens.get(*i).map(|t| &t.kind), Some(TokenKind::LParen)) {
            let mut depth: i32 = 0;
            while let Some(t) = tokens.get(*i) {
                match t.kind {
                    TokenKind::LParen => {
                        depth += 1;
                        *i += 1;
                    }
                    TokenKind::RParen => {
                        depth -= 1;
                        *i += 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {
                        *i += 1;
                    }
                }
            }
        }
    }
}
