use super::control;
use super::tokens::{Token, TokenKind};
use crate::compiler::source::Span;

pub struct Lexer<'a> {
    input: &'a [u8],
    len: usize,
    pos: usize,
    // line start offsets for byte->(line,col) mapping
    line_starts: Vec<usize>,
    // collected lexing errors
    errors: Vec<LexError>,
}

#[derive(Clone, Debug)]
pub(super) struct LexError {
    pub(super) message: String,
    pub(super) start: usize,
    pub(super) end: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        let bytes = input.as_bytes();
        let mut line_starts = vec![0usize];
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self {
            input: bytes,
            len: input.len(),
            pos: 0,
            line_starts,
            errors: Vec::new(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        if self.pos < self.len {
            let b = self.input[self.pos];
            self.pos += 1;
            Some(b)
        } else {
            None
        }
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while let Some(b) = self.peek() {
                if b == b' ' || b == b'\n' || b == b'\r' || b == b'\t' {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if self.peek() == Some(b'/') && self.input.get(self.pos + 1) == Some(&b'/') {
                // Do not skip doc comments starting with '///'
                if self.input.get(self.pos + 2) == Some(&b'/') {
                    break;
                }
                self.pos += 2;
                while let Some(b) = self.peek() {
                    self.pos += 1;
                    if b == b'\n' {
                        break;
                    }
                }
                continue;
            }
            if self.peek() == Some(b'/') && self.input.get(self.pos + 1) == Some(&b'*') {
                // Do not skip doc block comments starting with '/**'
                if self.input.get(self.pos + 2) == Some(&b'*') {
                    break;
                }
                // Nested block comments: /* ... /* ... */ ... */
                self.pos += 2; // skip /*
                let mut depth = 1usize;
                while self.pos + 1 < self.len {
                    let a = self.input[self.pos];
                    let b = self.input[self.pos + 1];
                    if a == b'/' && b == b'*' {
                        depth += 1;
                        self.pos += 2;
                        continue;
                    }
                    if a == b'*' && b == b'/' {
                        depth -= 1;
                        self.pos += 2;
                        if depth == 0 {
                            break;
                        }
                        continue;
                    }
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
    }

    fn span(&self, start: usize, end: usize) -> Span {
        let (sl, sc) = self.byte_to_line_col(start);
        let (el, ec) = self.byte_to_line_col(end);
        Span {
            start,
            end,
            start_line: sl,
            start_col: sc,
            end_line: el,
            end_col: ec,
        }
    }

    pub(super) fn byte_to_line_col(&self, off: usize) -> (u32, u32) {
        let idx = match self.line_starts.binary_search(&off) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let line = (idx + 1) as u32;
        let col = (off.saturating_sub(*self.line_starts.get(idx).unwrap_or(&0)) + 1) as u32;
        (line, col)
    }

    fn err(&mut self, start: usize, end: usize, msg: impl Into<String>) {
        self.errors.push(LexError {
            message: msg.into(),
            start,
            end,
        });
    }

    pub(super) fn errors(&self) -> &Vec<LexError> {
        &self.errors
    }

    fn is_ident_start(b: u8) -> bool {
        (b as char).is_ascii_alphabetic() || b == b'_'
    }
    fn is_ident_continue(b: u8) -> bool {
        (b as char).is_ascii_alphanumeric() || b == b'_'
    }

    pub fn next_token(&mut self) -> Token {
        self.skip_ws_and_comments();
        let start = self.pos;
        match self.advance() {
            None => Token {
                kind: TokenKind::Eof,
                span: self.span(self.pos, self.pos),
            },
            Some(b'+') => {
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::PlusEq,
                        span: self.span(start, self.pos),
                    }
                } else {
                    Token {
                        kind: TokenKind::Plus,
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b'-') => {
                // '->'
                if self.peek() == Some(b'>') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::Arrow,
                        span: self.span(start, self.pos),
                    }
                } else if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::MinusEq,
                        span: self.span(start, self.pos),
                    }
                } else {
                    Token {
                        kind: TokenKind::Minus,
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b'*') => {
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::StarEq,
                        span: self.span(start, self.pos),
                    }
                } else {
                    Token {
                        kind: TokenKind::Star,
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b'/') => {
                match (self.input.get(self.pos), self.input.get(self.pos + 1)) {
                    (Some(b'='), _) => {
                        self.pos += 1; // consume '='
                        Token {
                            kind: TokenKind::SlashEq,
                            span: self.span(start, self.pos),
                        }
                    }
                    (Some(b'/'), Some(b'/')) => {
                        // Doc line: '/// ...\n'
                        // Consume the two we've peeked: we already consumed first '/'; pos points at second.
                        self.pos += 2; // now after '///'
                        let mut buf = String::new();
                        while let Some(b) = self.peek() {
                            if b == b'\n' {
                                break;
                            }
                            buf.push(b as char);
                            self.pos += 1;
                        }
                        Token {
                            kind: TokenKind::DocLine(buf),
                            span: self.span(start, self.pos),
                        }
                    }
                    (Some(b'*'), Some(b'*')) => {
                        // Doc block: '/** ... */'
                        self.pos += 2; // after '/**'
                        let mut buf = String::new();
                        while self.pos + 1 < self.len {
                            let a = self.input[self.pos];
                            let b = self.input[self.pos + 1];
                            if a == b'*' && b == b'/' {
                                self.pos += 2; // consume closing */
                                break;
                            }
                            buf.push(a as char);
                            self.pos += 1;
                        }
                        Token {
                            kind: TokenKind::DocBlock(buf),
                            span: self.span(start, self.pos),
                        }
                    }
                    (Some(b'/'), _) => {
                        // Regular line comment; skip and get next token
                        self.pos += 1; // skip second '/'
                        while let Some(b) = self.peek() {
                            self.pos += 1;
                            if b == b'\n' {
                                break;
                            }
                        }
                        self.next_token()
                    }
                    (Some(b'*'), _) => {
                        // Regular block comment; support nesting
                        self.pos += 1; // now after '/*'
                        let mut depth = 1usize;
                        while self.pos + 1 < self.len {
                            let a = self.input[self.pos];
                            let b = self.input[self.pos + 1];
                            if a == b'/' && b == b'*' {
                                depth += 1;
                                self.pos += 2;
                                continue;
                            }
                            if a == b'*' && b == b'/' {
                                depth -= 1;
                                self.pos += 2;
                                if depth == 0 {
                                    break;
                                }
                                continue;
                            }
                            self.pos += 1;
                        }
                        self.next_token()
                    }
                    _ => Token {
                        kind: TokenKind::Slash,
                        span: self.span(start, self.pos),
                    },
                }
            }
            Some(b'<') => {
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::Le,
                        span: self.span(start, self.pos),
                    }
                } else if self.peek() == Some(b'<') {
                    self.pos += 1;
                    if self.peek() == Some(b'=') {
                        self.pos += 1;
                        Token {
                            kind: TokenKind::ShlEq,
                            span: self.span(start, self.pos),
                        }
                    } else {
                        Token {
                            kind: TokenKind::Shl,
                            span: self.span(start, self.pos),
                        }
                    }
                } else {
                    Token {
                        kind: TokenKind::Lt,
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b'>') => {
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::Ge,
                        span: self.span(start, self.pos),
                    }
                } else if self.peek() == Some(b'>') {
                    self.pos += 1;
                    if self.peek() == Some(b'=') {
                        self.pos += 1;
                        Token {
                            kind: TokenKind::ShrEq,
                            span: self.span(start, self.pos),
                        }
                    } else {
                        Token {
                            kind: TokenKind::Shr,
                            span: self.span(start, self.pos),
                        }
                    }
                } else {
                    Token {
                        kind: TokenKind::Gt,
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b'=') => {
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::EqEq,
                        span: self.span(start, self.pos),
                    }
                } else {
                    Token {
                        kind: TokenKind::Eq,
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b'&') => {
                if self.peek() == Some(b'&') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::AndAnd,
                        span: self.span(start, self.pos),
                    }
                } else if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::AndEq,
                        span: self.span(start, self.pos),
                    }
                } else {
                    Token {
                        kind: TokenKind::Ampersand,
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b'|') => {
                if self.peek() == Some(b'|') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::OrOr,
                        span: self.span(start, self.pos),
                    }
                } else if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::OrEq,
                        span: self.span(start, self.pos),
                    }
                } else {
                    Token {
                        kind: TokenKind::Pipe,
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b'^') => {
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::XorEq,
                        span: self.span(start, self.pos),
                    }
                } else {
                    Token {
                        kind: TokenKind::Caret,
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b'!') => {
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::BangEq,
                        span: self.span(start, self.pos),
                    }
                } else {
                    Token {
                        kind: TokenKind::Bang,
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b'%') => {
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::PercentEq,
                        span: self.span(start, self.pos),
                    }
                } else {
                    Token {
                        kind: TokenKind::Percent,
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b':') => Token {
                kind: TokenKind::Colon,
                span: self.span(start, self.pos),
            },
            Some(b'?') => {
                if self.peek() == Some(b'.') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::QuestionDot,
                        span: self.span(start, self.pos),
                    }
                } else {
                    Token {
                        kind: TokenKind::Question,
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b'(') => Token {
                kind: TokenKind::LParen,
                span: self.span(start, self.pos),
            },
            Some(b')') => Token {
                kind: TokenKind::RParen,
                span: self.span(start, self.pos),
            },
            Some(b'{') => Token {
                kind: TokenKind::LBrace,
                span: self.span(start, self.pos),
            },
            Some(b'}') => Token {
                kind: TokenKind::RBrace,
                span: self.span(start, self.pos),
            },
            Some(b'[') => Token {
                kind: TokenKind::LBracket,
                span: self.span(start, self.pos),
            },
            Some(b']') => Token {
                kind: TokenKind::RBracket,
                span: self.span(start, self.pos),
            },
            Some(b',') => Token {
                kind: TokenKind::Comma,
                span: self.span(start, self.pos),
            },
            Some(b'@') => Token {
                kind: TokenKind::At,
                span: self.span(start, self.pos),
            },
            Some(b'.') => {
                if self.peek() == Some(b'.') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::DotDot,
                        span: self.span(start, self.pos),
                    }
                } else {
                    Token {
                        kind: TokenKind::Dot,
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b';') => Token {
                kind: TokenKind::Semicolon,
                span: self.span(start, self.pos),
            },
            Some(b) if (b as char).is_ascii_digit() => {
                // Numeric literals: 0x.. (hex), 0b.. (bin), decimal; allow '_' separators.
                let mut end = self.pos;
                let mut base = 10u32;
                let mut idx = start;
                let mut is_float = false;
                if self.input[start] == b'0'
                    && let Some(nx) = self.peek()
                {
                    if nx == b'x' || nx == b'X' {
                        base = 16;
                        self.pos += 1;
                        idx = self.pos;
                        end = self.pos;
                    } else if nx == b'b' || nx == b'B' {
                        base = 2;
                        self.pos += 1;
                        idx = self.pos;
                        end = self.pos;
                    }
                }
                if base == 10 {
                    // First, consume digits/underscores
                    while let Some(nb) = self.peek() {
                        let c = nb as char;
                        if c == '_' || c.is_ascii_digit() {
                            self.pos += 1;
                            end = self.pos;
                        } else {
                            break;
                        }
                    }
                    // Fractional part
                    if self.peek() == Some(b'.') {
                        // Only treat as float if followed by a digit
                        if self
                            .input
                            .get(self.pos + 1)
                            .map(|b| (*b as char).is_ascii_digit())
                            .unwrap_or(false)
                        {
                            is_float = true;
                            self.pos += 1;
                            end = self.pos;
                            while let Some(nb) = self.peek() {
                                let c = nb as char;
                                if c == '_' || c.is_ascii_digit() {
                                    self.pos += 1;
                                    end = self.pos;
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                    // Exponent part
                    if matches!(self.peek(), Some(b'e' | b'E')) {
                        // Require at least one digit after optional sign
                        let mut look = self.pos + 1;
                        if matches!(self.input.get(look), Some(b'+' | b'-')) {
                            look += 1;
                        }
                        if self
                            .input
                            .get(look)
                            .map(|b| (*b as char).is_ascii_digit())
                            .unwrap_or(false)
                        {
                            is_float = true;
                            self.pos = look;
                            end = self.pos;
                            while let Some(nb) = self.peek() {
                                let c = nb as char;
                                if c == '_' || c.is_ascii_digit() {
                                    self.pos += 1;
                                    end = self.pos;
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                    if is_float {
                        let s = std::str::from_utf8(&self.input[start..end]).unwrap_or("");
                        let clean: String = s.chars().filter(|&ch| ch != '_').collect();
                        let val = clean.parse::<f64>().unwrap_or(0.0);
                        return Token {
                            kind: TokenKind::Float(val),
                            span: self.span(start, end),
                        };
                    }
                    // else fall through as integer decimal
                    idx = start;
                    base = 10;
                } else {
                    // Non-decimal bases: consume valid digits
                    while let Some(nb) = self.peek() {
                        let c = nb as char;
                        let ok = match base {
                            2 => c == '_' || c == '0' || c == '1',
                            16 => {
                                c == '_'
                                    || c.is_ascii_digit()
                                    || (('a'..='f').contains(&c) || ('A'..='F').contains(&c))
                            }
                            _ => false,
                        };
                        if ok {
                            self.pos += 1;
                            end = self.pos;
                        } else {
                            break;
                        }
                    }
                }
                // Integer literal parsing by stripping underscores
                let s = std::str::from_utf8(&self.input[idx..end]).unwrap_or("");
                let clean: String = s.chars().filter(|&ch| ch != '_').collect();
                let val = i64::from_str_radix(&clean, base).unwrap_or(0);
                Token {
                    kind: TokenKind::Number(val),
                    span: self.span(start, end),
                }
            }
            Some(b'"') => {
                // Check for triple-quoted multiline/raw string: """..."""
                if self.input.get(self.pos) == Some(&b'"')
                    && self.input.get(self.pos + 1) == Some(&b'"')
                {
                    // Triple-quoted string
                    self.pos += 2; // skip the two additional quotes
                    let mut s = String::new();
                    let mut terminated = false;
                    let literal_start = start;

                    // Consume characters until we find closing """
                    while self.pos < self.len {
                        // Check for closing """
                        if self.input.get(self.pos) == Some(&b'"')
                            && self.input.get(self.pos + 1) == Some(&b'"')
                            && self.input.get(self.pos + 2) == Some(&b'"')
                        {
                            self.pos += 3; // skip closing """
                            terminated = true;
                            break;
                        }
                        // No escape processing - everything is literal
                        s.push(self.input[self.pos] as char);
                        self.pos += 1;
                    }

                    return if terminated {
                        Token {
                            kind: TokenKind::StringLit(s),
                            span: self.span(start, self.pos),
                        }
                    } else {
                        self.err(
                            literal_start,
                            self.pos,
                            "unterminated triple-quoted string literal",
                        );
                        Token {
                            kind: TokenKind::Unknown('"'),
                            span: self.span(start, self.pos),
                        }
                    };
                }

                // Regular single-quoted string with escape processing
                let mut s = String::new();
                let mut terminated = false;
                let literal_start = start;
                while let Some(b) = self.peek() {
                    self.pos += 1;
                    match b {
                        b'"' => {
                            terminated = true;
                            break;
                        }
                        b'\\' => {
                            if let Some(nb) = self.peek() {
                                self.pos += 1;
                                match nb {
                                    b'\\' => s.push('\\'),
                                    b'"' => s.push('"'),
                                    b'\'' => s.push('\''),
                                    b'n' => s.push('\n'),
                                    b'r' => s.push('\r'),
                                    b't' => s.push('\t'),
                                    b'0' => s.push('\0'),
                                    b'x' => {
                                        // \xNN (hex byte)
                                        let mut hex = String::new();
                                        for _ in 0..2 {
                                            if let Some(h) = self.peek() {
                                                self.pos += 1;
                                                hex.push(h as char);
                                            }
                                        }
                                        match u8::from_str_radix(&hex, 16) {
                                            Ok(byte) => s.push(byte as char),
                                            Err(_) => self.err(
                                                self.pos.saturating_sub(hex.len()),
                                                self.pos,
                                                "invalid hex escape",
                                            ),
                                        }
                                    }
                                    b'u' => {
                                        // \uXXXX
                                        let mut hex = String::new();
                                        for _ in 0..4 {
                                            if let Some(h) = self.peek() {
                                                self.pos += 1;
                                                hex.push(h as char);
                                            }
                                        }
                                        if let Ok(cp) = u32::from_str_radix(&hex, 16) {
                                            if let Some(ch) = std::char::from_u32(cp) {
                                                s.push(ch);
                                            } else {
                                                self.err(
                                                    self.pos.saturating_sub(4),
                                                    self.pos,
                                                    "invalid Unicode escape",
                                                );
                                            }
                                        } else {
                                            self.err(
                                                self.pos.saturating_sub(4),
                                                self.pos,
                                                "invalid Unicode escape",
                                            );
                                        }
                                    }
                                    b'U' => {
                                        // \UXXXXXXXX
                                        let mut hex = String::new();
                                        for _ in 0..8 {
                                            if let Some(h) = self.peek() {
                                                self.pos += 1;
                                                hex.push(h as char);
                                            }
                                        }
                                        if let Ok(cp) = u32::from_str_radix(&hex, 16) {
                                            if let Some(ch) = std::char::from_u32(cp) {
                                                s.push(ch);
                                            } else {
                                                self.err(
                                                    self.pos.saturating_sub(8),
                                                    self.pos,
                                                    "invalid Unicode escape",
                                                );
                                            }
                                        } else {
                                            self.err(
                                                self.pos.saturating_sub(8),
                                                self.pos,
                                                "invalid Unicode escape",
                                            );
                                        }
                                    }
                                    other => {
                                        self.err(
                                            self.pos - 2,
                                            self.pos,
                                            format!("invalid escape: \\{}", other as char),
                                        );
                                    }
                                }
                            } else {
                                break;
                            }
                        }
                        other => s.push(other as char),
                    }
                }
                if terminated {
                    Token {
                        kind: TokenKind::StringLit(s),
                        span: self.span(start, self.pos),
                    }
                } else {
                    self.err(literal_start, self.pos, "unterminated string literal");
                    Token {
                        kind: TokenKind::Unknown('"'),
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b'\'') => {
                // Char literal
                let literal_start = start;
                let ch = match self.peek() {
                    Some(b'\\') => {
                        // Escaped
                        self.pos += 1;
                        match self.peek() {
                            Some(nb) => {
                                self.pos += 1;
                                match nb {
                                    b'\\' => '\\',
                                    b'\'' => '\'',
                                    b'"' => '"',
                                    b'n' => '\n',
                                    b'r' => '\r',
                                    b't' => '\t',
                                    b'0' => '\0',
                                    b'x' => {
                                        // \xNN (hex byte)
                                        let mut hex = String::new();
                                        for _ in 0..2 {
                                            if let Some(h) = self.peek() {
                                                self.pos += 1;
                                                hex.push(h as char);
                                            }
                                        }
                                        match u8::from_str_radix(&hex, 16) {
                                            Ok(byte) => byte as char,
                                            Err(_) => {
                                                self.err(
                                                    self.pos.saturating_sub(hex.len()),
                                                    self.pos,
                                                    "invalid hex escape",
                                                );
                                                '\0'
                                            }
                                        }
                                    }
                                    b'u' => {
                                        let mut hex = String::new();
                                        for _ in 0..4 {
                                            if let Some(h) = self.peek() {
                                                self.pos += 1;
                                                hex.push(h as char);
                                            }
                                        }
                                        match u32::from_str_radix(&hex, 16)
                                            .ok()
                                            .and_then(std::char::from_u32)
                                        {
                                            Some(c) => c,
                                            None => {
                                                self.err(
                                                    self.pos.saturating_sub(4),
                                                    self.pos,
                                                    "invalid Unicode escape",
                                                );
                                                '\0'
                                            }
                                        }
                                    }
                                    b'U' => {
                                        let mut hex = String::new();
                                        for _ in 0..8 {
                                            if let Some(h) = self.peek() {
                                                self.pos += 1;
                                                hex.push(h as char);
                                            }
                                        }
                                        match u32::from_str_radix(&hex, 16)
                                            .ok()
                                            .and_then(std::char::from_u32)
                                        {
                                            Some(c) => c,
                                            None => {
                                                self.err(
                                                    self.pos.saturating_sub(8),
                                                    self.pos,
                                                    "invalid Unicode escape",
                                                );
                                                '\0'
                                            }
                                        }
                                    }
                                    other => other as char,
                                }
                            }
                            None => '\0',
                        }
                    }
                    Some(b) => {
                        self.pos += 1;
                        b as char
                    }
                    None => '\0',
                };
                // Expect closing quote
                if self.peek() == Some(b'\'') {
                    self.pos += 1;
                    Token {
                        kind: TokenKind::CharLit(ch),
                        span: self.span(start, self.pos),
                    }
                } else {
                    self.err(literal_start, self.pos, "unterminated char literal");
                    Token {
                        kind: TokenKind::Unknown('\''),
                        span: self.span(start, self.pos),
                    }
                }
            }
            Some(b) if Self::is_ident_start(b) => {
                let mut end = self.pos;
                while let Some(nb) = self.peek() {
                    if Self::is_ident_continue(nb) {
                        self.pos += 1;
                        end = self.pos;
                    } else {
                        break;
                    }
                }
                let s = std::str::from_utf8(&self.input[start..end]).unwrap_or("");
                let kind = match s {
                    "package" => TokenKind::Package,
                    "module" => TokenKind::Module,
                    "public" => TokenKind::Public,
                    "import" => TokenKind::Import,
                    "internal" => TokenKind::Internal,
                    "private" => TokenKind::Private,
                    "export" => TokenKind::Export,
                    "struct" => TokenKind::Struct,
                    "interface" => TokenKind::Interface,
                    "enum" => TokenKind::Enum,
                    "sealed" => TokenKind::Sealed,
                    "provider" => TokenKind::Provider,
                    "shared" => TokenKind::Shared,
                    "type" => TokenKind::Type,
                    "extends" => TokenKind::Extends,
                    "implements" => TokenKind::Implements,
                    "as" => TokenKind::As,
                    "static" => TokenKind::Static,
                    "final" => TokenKind::Final,
                    "void" => TokenKind::Void,
                    "async" => TokenKind::Async,
                    "fn" => TokenKind::Fn,
                    "await" => TokenKind::Await,
                    "unsafe" => TokenKind::Unsafe,
                    "extern" => TokenKind::Extern,
                    "throws" => TokenKind::Throws,
                    "var" => TokenKind::Var,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    _ => match control::keyword_kind(s) {
                        Some(k) => k,
                        None => TokenKind::Ident(s.to_string()),
                    },
                };
                Token {
                    kind,
                    span: self.span(start, end),
                }
            }
            Some(other) => Token {
                kind: TokenKind::Unknown(other as char),
                span: self.span(start, self.pos),
            },
        }
    }
}

pub fn lex_all(input: &str) -> Vec<Token> {
    let mut lx = Lexer::new(input);
    let mut toks = Vec::new();
    loop {
        let t = lx.next_token();
        let eof = matches!(t.kind, TokenKind::Eof);
        toks.push(t);
        if eof {
            break;
        }
    }
    toks
}
// moved to parent module
