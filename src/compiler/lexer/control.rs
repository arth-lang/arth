use super::TokenKind;

pub fn keyword_kind(s: &str) -> Option<TokenKind> {
    Some(match s {
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "while" => TokenKind::While,
        "for" => TokenKind::For,
        "switch" => TokenKind::Switch,
        "case" => TokenKind::Case,
        "default" => TokenKind::Default,
        "try" => TokenKind::Try,
        "catch" => TokenKind::Catch,
        "finally" => TokenKind::Finally,
        "break" => TokenKind::Break,
        "continue" => TokenKind::Continue,
        "return" => TokenKind::Return,
        "throw" => TokenKind::Throw,
        "panic" => TokenKind::Panic,
        _ => return None,
    })
}
