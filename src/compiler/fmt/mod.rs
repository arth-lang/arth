//! Code formatter for the Arth language.
//!
//! This module provides a formatter that prints AST nodes to formatted source code.
//! The formatter preserves semantics while normalizing style according to configuration.
//!
//! # Example
//!
//! ```ignore
//! use arth::compiler::fmt::{format, FormatConfig};
//! let formatted = format(&ast, &FormatConfig::default());
//! ```

mod printer;

pub use printer::Printer;

/// Brace style for function and control flow blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BraceStyle {
    /// K&R style: opening brace on same line.
    /// ```text
    /// fn foo() {
    /// ```
    #[default]
    SameLine,
    /// Allman style: opening brace on new line.
    /// ```text
    /// fn foo()
    /// {
    /// ```
    NextLine,
}

/// Configuration for the formatter.
#[derive(Clone, Debug)]
pub struct FormatConfig {
    /// Number of spaces per indentation level.
    pub indent: usize,
    /// Maximum line width before wrapping.
    pub max_line_width: usize,
    /// Brace placement style.
    pub brace_style: BraceStyle,
    /// Whether to use trailing commas in multiline constructs.
    pub trailing_comma: bool,
    /// Whether to add spaces inside braces: `{ foo }` vs `{foo}`.
    pub spaces_in_braces: bool,
    /// Whether to add spaces inside brackets: `[ a, b ]` vs `[a, b]`.
    pub spaces_in_brackets: bool,
    /// Whether to add spaces inside parentheses: `( a, b )` vs `(a, b)`.
    pub spaces_in_parens: bool,
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self {
            indent: 4,
            max_line_width: 100,
            brace_style: BraceStyle::SameLine,
            trailing_comma: true,
            spaces_in_braces: true,
            spaces_in_brackets: false,
            spaces_in_parens: false,
        }
    }
}

impl FormatConfig {
    /// Create a new formatter config with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the indentation width.
    pub fn with_indent(mut self, indent: usize) -> Self {
        self.indent = indent;
        self
    }

    /// Set the maximum line width.
    pub fn with_max_width(mut self, width: usize) -> Self {
        self.max_line_width = width;
        self
    }

    /// Set the brace style.
    pub fn with_brace_style(mut self, style: BraceStyle) -> Self {
        self.brace_style = style;
        self
    }

    /// Set whether to use trailing commas.
    pub fn with_trailing_comma(mut self, trailing: bool) -> Self {
        self.trailing_comma = trailing;
        self
    }
}

/// Result of formatting.
#[derive(Clone, Debug)]
pub struct FormatResult {
    /// The formatted source code.
    pub output: String,
    /// Whether any changes were made from the original.
    pub changed: bool,
}

/// Format an AST file to a string.
pub fn format(file: &crate::compiler::ast::FileAst, config: &FormatConfig) -> FormatResult {
    let mut printer = Printer::new(config.clone());
    let output = printer.print_file(file);
    FormatResult {
        output,
        changed: true, // We don't track changes yet
    }
}

/// Check if source is properly formatted without modifying it.
pub fn check(source: &str, config: &FormatConfig) -> Result<bool, String> {
    // Parse the source
    use crate::compiler::diagnostics::Reporter;
    use crate::compiler::parser;
    use crate::compiler::source::SourceFile;
    use std::path::PathBuf;

    let sf = SourceFile {
        path: PathBuf::from("<check>"),
        text: source.to_string(),
    };

    let mut reporter = Reporter::new();
    let ast = parser::parse_file(&sf, &mut reporter);

    if reporter.has_errors() {
        return Err("Parse errors encountered".to_string());
    }

    let formatted = format(&ast, config);

    // Normalize both for comparison (trim trailing whitespace, normalize line endings)
    let normalized_source = normalize_whitespace(source);
    let normalized_output = normalize_whitespace(&formatted.output);

    Ok(normalized_source == normalized_output)
}

/// Normalize whitespace for comparison.
fn normalize_whitespace(s: &str) -> String {
    s.lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_config_default() {
        let config = FormatConfig::default();
        assert_eq!(config.indent, 4);
        assert_eq!(config.max_line_width, 100);
        assert_eq!(config.brace_style, BraceStyle::SameLine);
        assert!(config.trailing_comma);
    }

    #[test]
    fn test_format_config_builder() {
        let config = FormatConfig::new()
            .with_indent(2)
            .with_max_width(80)
            .with_brace_style(BraceStyle::NextLine)
            .with_trailing_comma(false);

        assert_eq!(config.indent, 2);
        assert_eq!(config.max_line_width, 80);
        assert_eq!(config.brace_style, BraceStyle::NextLine);
        assert!(!config.trailing_comma);
    }

    #[test]
    fn test_normalize_whitespace() {
        let input = "foo  \nbar\n  baz  \n";
        let normalized = normalize_whitespace(input);
        assert_eq!(normalized, "foo\nbar\n  baz");
    }
}
