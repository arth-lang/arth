//! ANSI color support for diagnostic output.
//!
//! Provides styled text output for terminal diagnostics with automatic
//! detection of terminal color support.

use std::env;
use std::io::{self, IsTerminal, Write};

/// ANSI color codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

impl Color {
    /// Get the ANSI escape code for this color.
    fn code(self) -> &'static str {
        match self {
            Color::Red => "\x1b[31m",
            Color::Green => "\x1b[32m",
            Color::Yellow => "\x1b[33m",
            Color::Blue => "\x1b[34m",
            Color::Magenta => "\x1b[35m",
            Color::Cyan => "\x1b[36m",
            Color::White => "\x1b[37m",
            Color::BrightRed => "\x1b[91m",
            Color::BrightGreen => "\x1b[92m",
            Color::BrightYellow => "\x1b[93m",
            Color::BrightBlue => "\x1b[94m",
            Color::BrightMagenta => "\x1b[95m",
            Color::BrightCyan => "\x1b[96m",
            Color::BrightWhite => "\x1b[97m",
        }
    }
}

/// Text style attributes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Bold,
    Dim,
    Italic,
    Underline,
}

impl Style {
    /// Get the ANSI escape code for this style.
    fn code(self) -> &'static str {
        match self {
            Style::Bold => "\x1b[1m",
            Style::Dim => "\x1b[2m",
            Style::Italic => "\x1b[3m",
            Style::Underline => "\x1b[4m",
        }
    }
}

/// Reset code to clear all formatting.
const RESET: &str = "\x1b[0m";

/// Styled text that can be rendered with or without colors.
#[derive(Clone, Debug)]
pub struct StyledText {
    text: String,
    color: Option<Color>,
    styles: Vec<Style>,
}

impl StyledText {
    /// Create new styled text.
    pub fn new<S: Into<String>>(text: S) -> Self {
        Self {
            text: text.into(),
            color: None,
            styles: Vec::new(),
        }
    }

    /// Set the color.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Add a style.
    pub fn style(mut self, style: Style) -> Self {
        self.styles.push(style);
        self
    }

    /// Make text bold.
    pub fn bold(self) -> Self {
        self.style(Style::Bold)
    }

    /// Render the styled text.
    pub fn render(&self, use_color: bool) -> String {
        if !use_color || (self.color.is_none() && self.styles.is_empty()) {
            return self.text.clone();
        }

        let mut result = String::new();

        // Apply styles
        for style in &self.styles {
            result.push_str(style.code());
        }

        // Apply color
        if let Some(color) = self.color {
            result.push_str(color.code());
        }

        // Add text
        result.push_str(&self.text);

        // Reset
        result.push_str(RESET);

        result
    }
}

/// Color configuration for diagnostic output.
#[derive(Clone, Debug)]
pub struct ColorConfig {
    /// Whether colors are enabled.
    pub enabled: bool,
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self::auto()
    }
}

impl ColorConfig {
    /// Create a new color config with explicit setting.
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Auto-detect color support based on environment.
    pub fn auto() -> Self {
        Self {
            enabled: should_use_color(),
        }
    }

    /// Always enable colors.
    pub fn always() -> Self {
        Self { enabled: true }
    }

    /// Never use colors.
    pub fn never() -> Self {
        Self { enabled: false }
    }

    /// Style for error severity.
    pub fn error<S: Into<String>>(&self, text: S) -> StyledText {
        StyledText::new(text).color(Color::BrightRed).bold()
    }

    /// Style for warning severity.
    pub fn warning<S: Into<String>>(&self, text: S) -> StyledText {
        StyledText::new(text).color(Color::BrightYellow).bold()
    }

    /// Style for note severity.
    pub fn note<S: Into<String>>(&self, text: S) -> StyledText {
        StyledText::new(text).color(Color::BrightCyan).bold()
    }

    /// Style for help messages.
    pub fn help<S: Into<String>>(&self, text: S) -> StyledText {
        StyledText::new(text).color(Color::BrightGreen).bold()
    }

    /// Style for line numbers.
    pub fn line_number<S: Into<String>>(&self, text: S) -> StyledText {
        StyledText::new(text).color(Color::BrightBlue).bold()
    }

    /// Style for code.
    pub fn code<S: Into<String>>(&self, text: S) -> StyledText {
        StyledText::new(text).bold()
    }

    /// Style for primary span underline.
    pub fn primary<S: Into<String>>(&self, text: S) -> StyledText {
        StyledText::new(text).color(Color::BrightRed).bold()
    }

    /// Style for secondary span underline.
    pub fn secondary<S: Into<String>>(&self, text: S) -> StyledText {
        StyledText::new(text).color(Color::BrightBlue)
    }

    /// Render styled text according to color config.
    pub fn render(&self, styled: &StyledText) -> String {
        styled.render(self.enabled)
    }
}

/// Determine whether to use colors based on environment.
fn should_use_color() -> bool {
    // Check NO_COLOR environment variable (https://no-color.org/)
    if env::var("NO_COLOR").is_ok() {
        return false;
    }

    // Check CLICOLOR_FORCE
    if env::var("CLICOLOR_FORCE").is_ok_and(|v| v != "0") {
        return true;
    }

    // Check if stderr is a terminal
    if !io::stderr().is_terminal() {
        return false;
    }

    // Check TERM
    if let Ok(term) = env::var("TERM") {
        if term == "dumb" {
            return false;
        }
    }

    true
}

/// A writer that can optionally colorize output.
pub struct ColorWriter<W: Write> {
    writer: W,
    config: ColorConfig,
}

impl<W: Write> ColorWriter<W> {
    /// Create a new color writer.
    pub fn new(writer: W, config: ColorConfig) -> Self {
        Self { writer, config }
    }

    /// Write styled text.
    pub fn write_styled(&mut self, styled: &StyledText) -> io::Result<()> {
        write!(self.writer, "{}", self.config.render(styled))
    }

    /// Write plain text.
    pub fn write_str(&mut self, s: &str) -> io::Result<()> {
        write!(self.writer, "{}", s)
    }

    /// Write a newline.
    pub fn newline(&mut self) -> io::Result<()> {
        writeln!(self.writer)
    }

    /// Get the underlying writer.
    pub fn into_inner(self) -> W {
        self.writer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_styled_text_no_color() {
        let styled = StyledText::new("hello").color(Color::Red).bold();
        assert_eq!(styled.render(false), "hello");
    }

    #[test]
    fn test_styled_text_with_color() {
        let styled = StyledText::new("hello").color(Color::Red);
        let rendered = styled.render(true);
        assert!(rendered.contains("\x1b[31m"));
        assert!(rendered.contains("hello"));
        assert!(rendered.contains("\x1b[0m"));
    }

    #[test]
    fn test_styled_text_bold() {
        let styled = StyledText::new("hello").bold();
        let rendered = styled.render(true);
        assert!(rendered.contains("\x1b[1m"));
    }

    #[test]
    fn test_color_config_error() {
        let config = ColorConfig::always();
        let styled = config.error("error");
        let rendered = config.render(&styled);
        assert!(rendered.contains("\x1b[91m")); // BrightRed
        assert!(rendered.contains("\x1b[1m")); // Bold
    }

    #[test]
    fn test_color_config_warning() {
        let config = ColorConfig::always();
        let styled = config.warning("warning");
        let rendered = config.render(&styled);
        assert!(rendered.contains("\x1b[93m")); // BrightYellow
    }

    #[test]
    fn test_color_config_never() {
        let config = ColorConfig::never();
        let styled = config.error("error");
        let rendered = config.render(&styled);
        assert_eq!(rendered, "error");
    }

    #[test]
    fn test_plain_text() {
        let styled = StyledText::new("plain");
        assert_eq!(styled.render(true), "plain");
        assert_eq!(styled.render(false), "plain");
    }
}
