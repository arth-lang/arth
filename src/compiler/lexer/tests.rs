use super::*;
use crate::compiler::diagnostics::Reporter;
use std::path::Path;

fn kinds_without_eof(input: &str) -> Vec<TokenKind> {
    lex_all(input)
        .into_iter()
        .filter(|t| !matches!(t.kind, TokenKind::Eof))
        .map(|t| t.kind)
        .collect()
}

#[test]
fn lexes_new_keywords_and_ops() {
    let src = "import internal private export struct interface enum sealed provider shared extends static final await \
                   : ? <= >= << >> ->";
    let ks = kinds_without_eof(src);
    assert!(ks.contains(&TokenKind::Import));
    assert!(ks.contains(&TokenKind::Internal));
    assert!(ks.contains(&TokenKind::Private));
    assert!(ks.contains(&TokenKind::Export));
    assert!(ks.contains(&TokenKind::Struct));
    assert!(ks.contains(&TokenKind::Interface));
    assert!(ks.contains(&TokenKind::Enum));
    assert!(ks.contains(&TokenKind::Sealed));
    assert!(ks.contains(&TokenKind::Provider));
    assert!(ks.contains(&TokenKind::Shared));
    assert!(ks.contains(&TokenKind::Extends));
    assert!(ks.contains(&TokenKind::Static));
    assert!(ks.contains(&TokenKind::Final));
    assert!(ks.contains(&TokenKind::Await));
    assert!(ks.contains(&TokenKind::Colon));
    assert!(ks.contains(&TokenKind::Question));
    assert!(ks.contains(&TokenKind::Le));
    assert!(ks.contains(&TokenKind::Ge));
    assert!(ks.contains(&TokenKind::Shl));
    assert!(ks.contains(&TokenKind::Shr));
    assert!(ks.contains(&TokenKind::Arrow));
}

#[test]
fn lexes_numeric_radixes_and_underscores() {
    let toks = lex_all("0 123 1_000 0xFF 0x1_FF 0b1010 0b10_10");
    let nums: Vec<i64> = toks
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::Number(n) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(nums, vec![0, 123, 1000, 255, 511, 10, 10]);
}

#[test]
fn lexes_floats_and_exponents() {
    let toks = lex_all("0.0 1.5 10e2 3.14e-2 6.02e23 1_000.0 1.0_5");
    let floats: Vec<f64> = toks
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::Float(f) => Some(f),
            _ => None,
        })
        .collect();
    assert!(floats.iter().any(|&f| (f - 0.0).abs() < 1e-12));
    assert!(floats.iter().any(|&f| (f - 1.5).abs() < 1e-12));
    assert!(floats.iter().any(|&f| (f - 1000.0).abs() < 1e-6));
    assert!(floats.iter().any(|&f| (f - 0.0314).abs() < 1e-6));
    assert!(floats.iter().any(|&f| f > 1e23));
}

#[test]
fn lexes_char_and_string_escapes() {
    let toks = lex_all("'a' '\\n' '\\u0041' '\\x41' \"hi\\n\\u0042\\x43\"");
    let mut got_char_a = false;
    let mut got_char_nl = false;
    let mut a_count = 0;
    let mut got_string = false;
    for t in toks {
        match t.kind {
            TokenKind::CharLit('a') => got_char_a = true,
            TokenKind::CharLit('\n') => got_char_nl = true,
            TokenKind::CharLit('A') => a_count += 1,
            TokenKind::StringLit(ref s) if s.contains("\nBC") => got_string = true,
            _ => {}
        }
    }
    assert!(got_char_a && got_char_nl && a_count >= 2 && got_string);
}

#[test]
fn lexes_doc_comments() {
    let toks = lex_all("/// hello\n/** world */ 1");
    let mut saw_line = false;
    let mut saw_block = false;
    let mut saw_num = false;
    for t in toks {
        match t.kind {
            TokenKind::DocLine(ref s) if s.trim() == "hello" => saw_line = true,
            TokenKind::DocBlock(ref s) if s.contains("world") => saw_block = true,
            TokenKind::Number(1) => saw_num = true,
            _ => {}
        }
    }
    assert!(saw_line && saw_block && saw_num);
}

#[test]
fn skips_nested_block_comments() {
    let toks = lex_all("/* a /* b */ c */ 42");
    let nums: Vec<i64> = toks
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::Number(n) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(nums, vec![42]);
}

#[test]
fn lexes_package_clause() {
    let src = "package demo.logging;";
    let toks = lex_all(src);
    let kinds: Vec<&TokenKind> = toks.iter().map(|t| &t.kind).collect();
    assert!(matches!(kinds.first(), Some(TokenKind::Package)));
    assert!(matches!(kinds.get(1), Some(TokenKind::Ident(s)) if *s == "demo"));
    assert!(matches!(kinds.get(2), Some(TokenKind::Dot)));
    assert!(matches!(kinds.get(3), Some(TokenKind::Ident(s)) if *s == "logging"));
    assert!(matches!(kinds.get(4), Some(TokenKind::Semicolon)));
    assert!(matches!(kinds.last(), Some(TokenKind::Eof)));
}

#[test]
fn lexes_triple_quoted_string_basic() {
    let src = r#""""hello world""""#;
    let toks = lex_all(src);
    let strings: Vec<String> = toks
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::StringLit(s) => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(strings, vec!["hello world"]);
}

#[test]
fn lexes_triple_quoted_multiline_string() {
    let src = "\"\"\"line1\nline2\nline3\"\"\"";
    let toks = lex_all(src);
    let strings: Vec<String> = toks
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::StringLit(s) => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(strings.len(), 1);
    assert!(strings[0].contains("line1\nline2\nline3"));
}

#[test]
fn lexes_triple_quoted_raw_no_escapes() {
    // Triple-quoted strings are raw - backslashes are literal
    let src = r#""""hello\nworld\t""""#;
    let toks = lex_all(src);
    let strings: Vec<String> = toks
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::StringLit(s) => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(strings.len(), 1);
    // Should contain literal backslash-n, not newline
    assert_eq!(strings[0], r#"hello\nworld\t"#);
}

#[test]
fn lexes_triple_quoted_with_embedded_quotes() {
    // Single and double quotes inside triple-quoted strings
    let src = "\"\"\"He said \"hello\" and 'goodbye'\"\"\"";
    let toks = lex_all(src);
    let strings: Vec<String> = toks
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::StringLit(s) => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(strings.len(), 1);
    assert!(strings[0].contains("\"hello\""));
    assert!(strings[0].contains("'goodbye'"));
}

#[test]
fn lexes_empty_triple_quoted_string() {
    let src = "\"\"\"\"\"\"";
    let toks = lex_all(src);
    let strings: Vec<String> = toks
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::StringLit(s) => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(strings, vec![""]);
}

#[test]
fn lexes_triple_quoted_with_windows_newlines() {
    // Windows-style CRLF should be preserved
    let src = "\"\"\"line1\r\nline2\"\"\"";
    let toks = lex_all(src);
    let strings: Vec<String> = toks
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::StringLit(s) => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(strings.len(), 1);
    assert!(strings[0].contains("\r\n"));
}

#[test]
fn reports_invalid_unicode_escape_in_string_and_recovers() {
    let src = "\"bad \\uD800\" 42";
    let mut reporter = Reporter::new();
    let toks = lex_all_with_reporter(
        src,
        Path::new("/mem/lexer/unicode_string.arth"),
        &mut reporter,
    );

    assert!(
        reporter.has_errors(),
        "invalid unicode escape should be reported"
    );
    assert!(
        reporter
            .diagnostics()
            .iter()
            .any(|d| d.message.contains("invalid Unicode escape"))
    );
    assert!(
        toks.iter().any(|t| matches!(t.kind, TokenKind::Number(42))),
        "lexer should recover and continue tokenizing after unicode escape error"
    );
}

#[test]
fn reports_invalid_unicode_escape_in_char_and_recovers() {
    let src = "'\\uD800' 'a'";
    let mut reporter = Reporter::new();
    let toks = lex_all_with_reporter(
        src,
        Path::new("/mem/lexer/unicode_char.arth"),
        &mut reporter,
    );

    assert!(
        reporter.has_errors(),
        "invalid unicode escape should be reported"
    );
    assert!(
        reporter
            .diagnostics()
            .iter()
            .any(|d| d.message.contains("invalid Unicode escape"))
    );
    assert!(
        toks.iter()
            .any(|t| matches!(t.kind, TokenKind::CharLit('a'))),
        "lexer should keep lexing subsequent character literals after error"
    );
}

#[test]
fn skips_deeply_nested_block_comments() {
    let toks = lex_all("/* l1 /* l2 /* l3 */ l2 end */ l1 end */ 7");
    let nums: Vec<i64> = toks
        .into_iter()
        .filter_map(|t| match t.kind {
            TokenKind::Number(n) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(nums, vec![7]);
}
