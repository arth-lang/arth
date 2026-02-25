//! Diagnostics module for the TS frontend.
//!
//! This module provides structured error reporting with source location information,
//! code snippets, and helpful suggestions for fixing common issues.

use std::path::PathBuf;

/// Source span representing a location in the source file.
#[derive(Debug, Clone)]
pub struct SourceSpan {
    /// The source file path.
    pub file: PathBuf,
    /// Byte offset of the start of the span.
    pub start: u32,
    /// Byte offset of the end of the span.
    pub end: u32,
}

impl SourceSpan {
    /// Create a new source span.
    pub fn new(file: PathBuf, start: u32, end: u32) -> Self {
        Self { file, start, end }
    }

    /// Create a span from SWC span info.
    pub fn from_swc(span: swc_common::Span, file: PathBuf) -> Self {
        Self {
            file,
            start: span.lo.0,
            end: span.hi.0,
        }
    }
}

/// Severity level of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    /// Hard error - compilation cannot continue.
    Error,
    /// Warning - compilation continues but there may be issues.
    Warning,
    /// Informational note.
    Note,
    /// Help text for fixing an error.
    Help,
}

impl std::fmt::Display for DiagnosticLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiagnosticLevel::Error => write!(f, "error"),
            DiagnosticLevel::Warning => write!(f, "warning"),
            DiagnosticLevel::Note => write!(f, "note"),
            DiagnosticLevel::Help => write!(f, "help"),
        }
    }
}

/// Category of the diagnostic for filtering and tooling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCategory {
    /// File I/O errors (file not found, permission denied, etc.)
    Io,
    /// SWC parser errors (syntax errors in TS code).
    Parse,
    /// TS subset validation errors (unsupported constructs).
    Validation,
    /// HIR lowering errors.
    Lowering,
    /// Security/sandbox policy violations.
    Security,
    /// Internal compiler errors.
    Internal,
}

impl std::fmt::Display for DiagnosticCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiagnosticCategory::Io => write!(f, "io"),
            DiagnosticCategory::Parse => write!(f, "parse"),
            DiagnosticCategory::Validation => write!(f, "validation"),
            DiagnosticCategory::Lowering => write!(f, "lowering"),
            DiagnosticCategory::Security => write!(f, "security"),
            DiagnosticCategory::Internal => write!(f, "internal"),
        }
    }
}

/// A structured diagnostic message with source location and context.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// Severity level.
    pub level: DiagnosticLevel,
    /// Category for filtering.
    pub category: DiagnosticCategory,
    /// Primary error message.
    pub message: String,
    /// Optional source span where the error occurred.
    pub span: Option<SourceSpan>,
    /// Optional help text with suggestions for fixing.
    pub help: Option<String>,
    /// Optional note with additional context.
    pub note: Option<String>,
    /// Documentation reference (e.g., "see docs/ts-subset.md §3.7").
    pub doc_ref: Option<String>,
}

impl Diagnostic {
    /// Create a new error diagnostic.
    pub fn error(category: DiagnosticCategory, message: impl Into<String>) -> Self {
        Self {
            level: DiagnosticLevel::Error,
            category,
            message: message.into(),
            span: None,
            help: None,
            note: None,
            doc_ref: None,
        }
    }

    /// Create a new warning diagnostic.
    pub fn warning(category: DiagnosticCategory, message: impl Into<String>) -> Self {
        Self {
            level: DiagnosticLevel::Warning,
            category,
            message: message.into(),
            span: None,
            help: None,
            note: None,
            doc_ref: None,
        }
    }

    /// Add a source span to this diagnostic.
    pub fn with_span(mut self, span: SourceSpan) -> Self {
        self.span = Some(span);
        self
    }

    /// Add help text to this diagnostic.
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Add a note to this diagnostic.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Add a documentation reference.
    pub fn with_doc_ref(mut self, doc_ref: impl Into<String>) -> Self {
        self.doc_ref = Some(doc_ref.into());
        self
    }

    /// Format this diagnostic for terminal output.
    ///
    /// If `source` is provided, a code snippet with line numbers and
    /// underlines will be included.
    pub fn format(&self, source: Option<&str>) -> String {
        let mut output = String::new();

        // Header: level[category]: message
        output.push_str(&format!(
            "{}[{}]: {}\n",
            self.level, self.category, self.message
        ));

        // Source location and snippet
        if let Some(span) = &self.span {
            if let Some(src) = source {
                let snippet = format_source_snippet(src, span);
                output.push_str(&snippet);
            } else {
                // Just show file and byte offsets
                output.push_str(&format!(
                    "  --> {}:{}..{}\n",
                    span.file.display(),
                    span.start,
                    span.end
                ));
            }
        }

        // Help text
        if let Some(help) = &self.help {
            output.push_str(&format!("  = help: {}\n", help));
        }

        // Note
        if let Some(note) = &self.note {
            output.push_str(&format!("  = note: {}\n", note));
        }

        // Documentation reference
        if let Some(doc_ref) = &self.doc_ref {
            output.push_str(&format!("  = see: {}\n", doc_ref));
        }

        output
    }
}

/// Format a source code snippet with line numbers and underlines.
fn format_source_snippet(source: &str, span: &SourceSpan) -> String {
    let mut output = String::new();

    // Find line and column for start position
    let (start_line, start_col) = byte_to_line_col(source, span.start as usize);
    let (end_line, end_col) = byte_to_line_col(source, span.end as usize);

    // Show file:line:col
    output.push_str(&format!(
        "  --> {}:{}:{}\n",
        span.file.display(),
        start_line,
        start_col
    ));

    // Get the source lines
    let lines: Vec<&str> = source.lines().collect();

    // Calculate gutter width (for line numbers)
    let gutter_width = end_line.to_string().len().max(3);

    // Show context: one line before, the error lines, one line after
    let context_start = start_line.saturating_sub(1).max(1);
    let context_end = (end_line + 1).min(lines.len());

    output.push_str(&format!("{:>gutter_width$} |\n", ""));

    for line_num in context_start..=context_end {
        if line_num == 0 || line_num > lines.len() {
            continue;
        }

        let line = lines[line_num - 1];
        output.push_str(&format!("{:>gutter_width$} | {}\n", line_num, line));

        // If this is a line with the error, show the underline
        if line_num >= start_line && line_num <= end_line {
            let underline = create_underline(
                line,
                if line_num == start_line { start_col } else { 1 },
                if line_num == end_line {
                    end_col
                } else {
                    line.len() + 1
                },
            );
            output.push_str(&format!("{:>gutter_width$} | {}\n", "", underline));
        }
    }

    output.push_str(&format!("{:>gutter_width$} |\n", ""));

    output
}

/// Convert a byte offset to (line, column) position (1-indexed).
fn byte_to_line_col(source: &str, byte_offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    let mut current_offset = 0;

    for ch in source.chars() {
        if current_offset >= byte_offset {
            break;
        }

        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }

        current_offset += ch.len_utf8();
    }

    (line, col)
}

/// Create an underline string for a portion of a line.
fn create_underline(line: &str, start_col: usize, end_col: usize) -> String {
    let mut result = String::new();

    // Add spaces up to the start column
    for (i, ch) in line.chars().enumerate() {
        let col = i + 1;
        if col >= start_col {
            break;
        }
        // Preserve tabs, replace other chars with spaces
        if ch == '\t' {
            result.push('\t');
        } else {
            result.push(' ');
        }
    }

    // Add carets for the underlined portion
    let underline_len = end_col.saturating_sub(start_col).max(1);
    for _ in 0..underline_len {
        result.push('^');
    }

    result
}

/// A collection of diagnostics that can be accumulated during compilation.
#[derive(Debug, Default)]
pub struct DiagnosticBag {
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticBag {
    /// Create a new empty diagnostic bag.
    pub fn new() -> Self {
        Self {
            diagnostics: Vec::new(),
        }
    }

    /// Add a diagnostic to the bag.
    pub fn add(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Check if there are any errors.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.level == DiagnosticLevel::Error)
    }

    /// Get the number of errors.
    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.level == DiagnosticLevel::Error)
            .count()
    }

    /// Get the number of warnings.
    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.level == DiagnosticLevel::Warning)
            .count()
    }

    /// Get all diagnostics.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Take all diagnostics, consuming the bag.
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    /// Format all diagnostics for terminal output.
    pub fn format_all(&self, source: Option<&str>) -> String {
        let mut output = String::new();

        for diagnostic in &self.diagnostics {
            output.push_str(&diagnostic.format(source));
            output.push('\n');
        }

        // Summary
        let errors = self.error_count();
        let warnings = self.warning_count();

        if errors > 0 || warnings > 0 {
            output.push_str(&format!(
                "{}: {} error(s), {} warning(s) emitted\n",
                if errors > 0 { "error" } else { "warning" },
                errors,
                warnings
            ));
        }

        output
    }
}

/// Helper functions for creating common diagnostics.
#[allow(dead_code)] // Infrastructure for future diagnostic creation
pub mod helpers {
    use std::path::Path;

    use super::*;

    /// Create an IO error diagnostic.
    pub fn io_error(file: &Path, message: &str) -> Diagnostic {
        Diagnostic::error(DiagnosticCategory::Io, message).with_span(SourceSpan::new(
            file.to_path_buf(),
            0,
            0,
        ))
    }

    /// Create a parse error diagnostic.
    pub fn parse_error(file: &Path, span: swc_common::Span, message: &str) -> Diagnostic {
        Diagnostic::error(DiagnosticCategory::Parse, message)
            .with_span(SourceSpan::from_swc(span, file.to_path_buf()))
    }

    /// Create a validation error for unsupported constructs.
    pub fn unsupported_construct(
        file: &Path,
        span: swc_common::Span,
        construct: &str,
        help: Option<&str>,
        doc_ref: Option<&str>,
    ) -> Diagnostic {
        let mut diag = Diagnostic::error(
            DiagnosticCategory::Validation,
            format!("unsupported TS construct: {}", construct),
        )
        .with_span(SourceSpan::from_swc(span, file.to_path_buf()));

        if let Some(h) = help {
            diag = diag.with_help(h);
        }

        if let Some(d) = doc_ref {
            diag = diag.with_doc_ref(d);
        }

        diag
    }

    /// Create a security policy violation diagnostic.
    pub fn security_violation(
        file: &Path,
        span: swc_common::Span,
        message: &str,
        doc_ref: Option<&str>,
    ) -> Diagnostic {
        let mut diag = Diagnostic::error(DiagnosticCategory::Security, message)
            .with_span(SourceSpan::from_swc(span, file.to_path_buf()));

        if let Some(d) = doc_ref {
            diag = diag.with_doc_ref(d);
        }

        diag
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_to_line_col() {
        let source = "line one\nline two\nline three";
        assert_eq!(byte_to_line_col(source, 0), (1, 1));
        assert_eq!(byte_to_line_col(source, 5), (1, 6));
        assert_eq!(byte_to_line_col(source, 9), (2, 1));
        assert_eq!(byte_to_line_col(source, 14), (2, 6));
    }

    #[test]
    fn test_diagnostic_format_without_source() {
        let diag = Diagnostic::error(DiagnosticCategory::Validation, "test error message")
            .with_help("try doing something else")
            .with_doc_ref("docs/test.md");

        let output = diag.format(None);
        assert!(output.contains("error[validation]: test error message"));
        assert!(output.contains("help: try doing something else"));
        assert!(output.contains("see: docs/test.md"));
    }

    #[test]
    fn test_diagnostic_format_with_source() {
        let source = "export function* gen() { }";
        let span = SourceSpan::new(PathBuf::from("test.ts"), 7, 16);
        let diag = Diagnostic::error(
            DiagnosticCategory::Validation,
            "generators are not supported",
        )
        .with_span(span);

        let output = diag.format(Some(source));
        assert!(output.contains("test.ts:1:8"));
        assert!(output.contains("function*"));
        assert!(output.contains("^"));
    }

    #[test]
    fn test_diagnostic_bag() {
        let mut bag = DiagnosticBag::new();
        bag.add(Diagnostic::error(DiagnosticCategory::Parse, "error 1"));
        bag.add(Diagnostic::warning(
            DiagnosticCategory::Validation,
            "warning 1",
        ));
        bag.add(Diagnostic::error(DiagnosticCategory::Lowering, "error 2"));

        assert!(bag.has_errors());
        assert_eq!(bag.error_count(), 2);
        assert_eq!(bag.warning_count(), 1);
    }

    #[test]
    fn test_create_underline() {
        let line = "  let x = 42;";
        let underline = create_underline(line, 7, 8);
        assert_eq!(underline, "      ^");
    }
}
