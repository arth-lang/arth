//! Source snippet rendering for diagnostics.
//!
//! Renders source code snippets with line numbers, underlines, and annotations
//! in a format similar to rustc's error output.

use std::cmp;
use std::path::Path;

use super::color::{ColorConfig, StyledText};
use crate::compiler::diagnostics::{Diagnostic, Label, Severity};
use crate::compiler::source::Span;

/// Number of context lines to show before and after the error.
const CONTEXT_LINES: usize = 1;

/// Configuration for snippet rendering.
#[derive(Clone, Debug)]
pub struct SnippetConfig {
    /// Color configuration.
    pub colors: ColorConfig,
    /// Number of context lines before/after.
    pub context_lines: usize,
    /// Whether to show the gutter (line numbers).
    pub show_gutter: bool,
}

impl Default for SnippetConfig {
    fn default() -> Self {
        Self {
            colors: ColorConfig::auto(),
            context_lines: CONTEXT_LINES,
            show_gutter: true,
        }
    }
}

/// A rendered source snippet.
#[derive(Clone, Debug, Default)]
pub struct RenderedSnippet {
    lines: Vec<String>,
}

impl RenderedSnippet {
    /// Create an empty snippet.
    pub fn new() -> Self {
        Self { lines: Vec::new() }
    }

    /// Add a line to the snippet.
    pub fn push<S: Into<String>>(&mut self, line: S) {
        self.lines.push(line.into());
    }

    /// Get the rendered lines.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }
}

impl std::fmt::Display for RenderedSnippet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.lines.join("\n"))
    }
}

/// Renders source snippets for diagnostics.
pub struct SnippetRenderer {
    config: SnippetConfig,
}

impl Default for SnippetRenderer {
    fn default() -> Self {
        Self::new(SnippetConfig::default())
    }
}

impl SnippetRenderer {
    /// Create a new snippet renderer.
    pub fn new(config: SnippetConfig) -> Self {
        Self { config }
    }

    /// Render a diagnostic with source snippet.
    pub fn render(&self, diag: &Diagnostic, source: Option<&str>) -> RenderedSnippet {
        let mut snippet = RenderedSnippet::new();

        // Render the header line: "error[E0001]: message"
        snippet.push(self.render_header(diag));

        // Render the location line: " --> file:line:col"
        if let Some(file) = &diag.file {
            snippet.push(self.render_location(file, diag.span.as_ref()));
        }

        // Render source snippet if we have both source and span
        if let (Some(src), Some(span)) = (source, &diag.span) {
            if span.start_line > 0 {
                self.render_source_lines(&mut snippet, src, span, &diag.labels, diag.severity);
            }
        }

        // Render secondary labels
        for label in &diag.labels {
            let label_file = label.file.as_ref().or(diag.file.as_ref());
            if label_file != diag.file.as_ref() {
                // Label is in a different file, show abbreviated location
                if let Some(f) = label_file {
                    snippet.push(self.render_note_location(f, &label.span, &label.message));
                }
            }
        }

        // Render suggestion if present
        if let Some(suggestion) = &diag.suggestion {
            snippet.push(self.render_help(suggestion));
        }

        snippet
    }

    /// Render the header line.
    fn render_header(&self, diag: &Diagnostic) -> String {
        let colors = &self.config.colors;
        let severity_text = match diag.severity {
            Severity::Error => colors.render(&colors.error("error")),
            Severity::Warning => colors.render(&colors.warning("warning")),
            Severity::Note => colors.render(&colors.note("note")),
        };

        let code_text = if let Some(code) = &diag.code {
            format!("[{}]", code)
        } else {
            String::new()
        };

        format!("{}{}: {}", severity_text, code_text, diag.message)
    }

    /// Render the location line.
    fn render_location(&self, file: &Path, span: Option<&Span>) -> String {
        let colors = &self.config.colors;
        let arrow = colors.render(&colors.line_number("-->"));

        match span {
            Some(s) if s.start_line > 0 => {
                format!(
                    " {} {}:{}:{}",
                    arrow,
                    file.display(),
                    s.start_line,
                    s.start_col
                )
            }
            _ => format!(" {} {}", arrow, file.display()),
        }
    }

    /// Render a note location for a secondary label.
    fn render_note_location(&self, file: &Path, span: &Span, message: &str) -> String {
        let colors = &self.config.colors;
        let note = colors.render(&colors.note("note"));

        if span.start_line > 0 {
            format!(
                " = {}: {} ({}:{}:{})",
                note,
                message,
                file.display(),
                span.start_line,
                span.start_col
            )
        } else {
            format!(" = {}: {} ({})", note, message, file.display())
        }
    }

    /// Render help/suggestion line.
    fn render_help(&self, suggestion: &str) -> String {
        let colors = &self.config.colors;
        let help = colors.render(&colors.help("help"));
        format!(" = {}: {}", help, suggestion)
    }

    /// Render source lines with underlines and annotations.
    fn render_source_lines(
        &self,
        snippet: &mut RenderedSnippet,
        source: &str,
        span: &Span,
        labels: &[Label],
        severity: Severity,
    ) {
        let lines: Vec<&str> = source.lines().collect();
        let line_count = lines.len();

        // Calculate line range to display
        let start_line = span.start_line as usize;
        let end_line = span.end_line as usize;

        let display_start = start_line.saturating_sub(self.config.context_lines).max(1);
        let display_end = cmp::min(end_line + self.config.context_lines, line_count);

        // Calculate gutter width
        let gutter_width = display_end.to_string().len();

        // Add empty gutter line
        snippet.push(self.render_gutter_separator(gutter_width));

        for line_num in display_start..=display_end {
            if line_num > line_count {
                break;
            }

            let line = lines.get(line_num - 1).unwrap_or(&"");

            // Render the source line
            snippet.push(self.render_source_line(line_num, line, gutter_width));

            // If this line is within the span, render underlines
            if line_num >= start_line && line_num <= end_line {
                if let Some(underline) =
                    self.render_underline(line_num, line, span, labels, gutter_width, severity)
                {
                    snippet.push(underline);
                }
            }
        }

        // Add empty gutter line at end
        snippet.push(self.render_gutter_separator(gutter_width));
    }

    /// Render a single source line with gutter.
    fn render_source_line(&self, line_num: usize, line: &str, gutter_width: usize) -> String {
        let colors = &self.config.colors;
        let num_str = format!("{:>width$}", line_num, width = gutter_width);
        let styled_num = colors.render(&colors.line_number(&num_str));
        let pipe = colors.render(&colors.line_number("|"));
        format!("{} {} {}", styled_num, pipe, line)
    }

    /// Render the gutter separator line.
    fn render_gutter_separator(&self, gutter_width: usize) -> String {
        let colors = &self.config.colors;
        let spaces = " ".repeat(gutter_width);
        let pipe = colors.render(&colors.line_number("|"));
        format!("{} {}", spaces, pipe)
    }

    /// Render underline for a source line.
    fn render_underline(
        &self,
        line_num: usize,
        line: &str,
        span: &Span,
        labels: &[Label],
        gutter_width: usize,
        severity: Severity,
    ) -> Option<String> {
        let colors = &self.config.colors;

        // Calculate underline position
        let start_line = span.start_line as usize;
        let end_line = span.end_line as usize;

        let (start_col, end_col) = if line_num == start_line && line_num == end_line {
            // Single-line span
            (span.start_col as usize, span.end_col as usize)
        } else if line_num == start_line {
            // First line of multi-line span
            (span.start_col as usize, line.len() + 1)
        } else if line_num == end_line {
            // Last line of multi-line span
            (1, span.end_col as usize)
        } else {
            // Middle line of multi-line span
            (1, line.len() + 1)
        };

        // Adjust for 1-based columns
        let start_idx = start_col.saturating_sub(1);
        let end_idx = end_col.saturating_sub(1);

        if end_idx <= start_idx {
            return None;
        }

        // Build the underline
        let padding = " ".repeat(start_idx);
        let underline_char = '^';
        let underline = underline_char.to_string().repeat(end_idx - start_idx);

        let styled_underline = match severity {
            Severity::Error => colors.render(&colors.primary(&underline)),
            Severity::Warning => colors.render(&colors.warning(&underline)),
            Severity::Note => colors.render(&colors.secondary(&underline)),
        };

        // Check for any labels on this line
        let label_msg = labels
            .iter()
            .find(|l| l.span.start_line as usize == line_num)
            .map(|l| format!(" {}", l.message));

        let spaces = " ".repeat(gutter_width);
        let pipe = colors.render(&colors.line_number("|"));

        Some(format!(
            "{} {} {}{}{}",
            spaces,
            pipe,
            padding,
            styled_underline,
            label_msg.unwrap_or_default()
        ))
    }
}

/// Render a diagnostic to a string.
pub fn render_diagnostic(diag: &Diagnostic, source: Option<&str>) -> String {
    let renderer = SnippetRenderer::default();
    renderer.render(diag, source).to_string()
}

/// Render a diagnostic to a string with explicit color configuration.
pub fn render_diagnostic_colored(
    diag: &Diagnostic,
    source: Option<&str>,
    colors: ColorConfig,
) -> String {
    let config = SnippetConfig {
        colors,
        ..Default::default()
    };
    let renderer = SnippetRenderer::new(config);
    renderer.render(diag, source).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_diag(message: &str, span: Span) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            message: message.to_string(),
            file: Some(PathBuf::from("test.arth")),
            span: Some(span),
            labels: Vec::new(),
            suggestion: None,
            code: None,
        }
    }

    #[test]
    fn test_render_simple_error() {
        let span = Span {
            start: 0,
            end: 5,
            start_line: 1,
            start_col: 1,
            end_line: 1,
            end_col: 6,
        };
        let diag = make_diag("test error", span);
        let source = "hello world";

        let config = SnippetConfig {
            colors: ColorConfig::never(),
            ..Default::default()
        };
        let renderer = SnippetRenderer::new(config);
        let rendered = renderer.render(&diag, Some(source)).to_string();

        assert!(rendered.contains("error: test error"));
        assert!(rendered.contains("test.arth:1:1"));
        assert!(rendered.contains("hello world"));
        assert!(rendered.contains("^^^^^"));
    }

    #[test]
    fn test_render_with_code() {
        let span = Span {
            start: 0,
            end: 3,
            start_line: 1,
            start_col: 1,
            end_line: 1,
            end_col: 4,
        };
        let mut diag = make_diag("type mismatch", span);
        diag.code = Some("E2001".to_string());

        let config = SnippetConfig {
            colors: ColorConfig::never(),
            ..Default::default()
        };
        let renderer = SnippetRenderer::new(config);
        let rendered = renderer
            .render(&diag, Some("int x = \"hello\""))
            .to_string();

        assert!(rendered.contains("error[E2001]: type mismatch"));
    }

    #[test]
    fn test_render_warning() {
        let span = Span {
            start: 4,
            end: 5,
            start_line: 1,
            start_col: 5,
            end_line: 1,
            end_col: 6,
        };
        let diag = Diagnostic {
            severity: Severity::Warning,
            message: "unused variable 'x'".to_string(),
            file: Some(PathBuf::from("test.arth")),
            span: Some(span),
            labels: Vec::new(),
            suggestion: Some("prefix with _ to suppress".to_string()),
            code: Some("W0001".to_string()),
        };

        let config = SnippetConfig {
            colors: ColorConfig::never(),
            ..Default::default()
        };
        let renderer = SnippetRenderer::new(config);
        let rendered = renderer.render(&diag, Some("int x = 1")).to_string();

        assert!(rendered.contains("warning[W0001]: unused variable"));
        assert!(rendered.contains("help: prefix with _ to suppress"));
    }

    #[test]
    fn test_render_with_label() {
        let span = Span {
            start: 8,
            end: 13,
            start_line: 1,
            start_col: 9,
            end_line: 1,
            end_col: 14,
        };
        let label = Label {
            span: Span {
                start: 8,
                end: 13,
                start_line: 1,
                start_col: 9,
                end_line: 1,
                end_col: 14,
            },
            message: "expected String".to_string(),
            file: None,
        };
        let mut diag = make_diag("type mismatch", span);
        diag.labels.push(label);

        let config = SnippetConfig {
            colors: ColorConfig::never(),
            ..Default::default()
        };
        let renderer = SnippetRenderer::new(config);
        let rendered = renderer.render(&diag, Some("String s = 12345")).to_string();

        assert!(rendered.contains("expected String"));
    }

    #[test]
    fn test_render_multiline() {
        let span = Span {
            start: 0,
            end: 20,
            start_line: 1,
            start_col: 1,
            end_line: 2,
            end_col: 5,
        };
        let diag = make_diag("multiline error", span);
        let source = "line one\nline two";

        let config = SnippetConfig {
            colors: ColorConfig::never(),
            ..Default::default()
        };
        let renderer = SnippetRenderer::new(config);
        let rendered = renderer.render(&diag, Some(source)).to_string();

        assert!(rendered.contains("line one"));
        assert!(rendered.contains("line two"));
    }

    #[test]
    fn test_render_no_source() {
        let span = Span {
            start: 0,
            end: 5,
            start_line: 1,
            start_col: 1,
            end_line: 1,
            end_col: 6,
        };
        let diag = make_diag("error without source", span);

        let config = SnippetConfig {
            colors: ColorConfig::never(),
            ..Default::default()
        };
        let renderer = SnippetRenderer::new(config);
        let rendered = renderer.render(&diag, None).to_string();

        assert!(rendered.contains("error: error without source"));
        assert!(rendered.contains("test.arth:1:1"));
    }
}
