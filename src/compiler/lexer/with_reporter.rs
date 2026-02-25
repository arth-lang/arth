use super::{Lexer, Span};
use crate::compiler::diagnostics::{Diagnostic, Reporter};

pub fn lex_all_with_reporter(
    input: &str,
    file: &std::path::Path,
    reporter: &mut Reporter,
) -> Vec<super::Token> {
    let mut lx = Lexer::new(input);
    let mut toks = Vec::new();
    loop {
        let t = lx.next_token();
        let eof = matches!(t.kind, super::TokenKind::Eof);
        toks.push(t);
        if eof {
            break;
        }
    }
    for e in lx.errors() {
        let (sl, sc) = lx.byte_to_line_col(e.start);
        let (el, ec) = lx.byte_to_line_col(e.end);
        reporter.emit(
            Diagnostic::error(e.message.clone())
                .with_file(file.to_path_buf())
                .with_span(Span {
                    start: e.start,
                    end: e.end,
                    start_line: sl,
                    start_col: sc,
                    end_line: el,
                    end_col: ec,
                }),
        );
    }
    toks
}
