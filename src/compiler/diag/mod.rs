//! Enhanced diagnostics rendering for the Arth compiler.
//!
//! This module provides production-grade diagnostic output with:
//! - Error codes (E0xxx for parse, E1xxx for resolve, E2xxx for type, etc.)
//! - Source snippets with line numbers and underlines
//! - ANSI color support with automatic terminal detection
//! - Structured JSON output for IDE integration
//!
//! # Example
//!
//! ```ignore
//! use arth::compiler::diag::{DiagnosticRenderer, RenderConfig};
//! use arth::compiler::diagnostics::{Diagnostic, Reporter};
//!
//! let renderer = DiagnosticRenderer::new(RenderConfig::default());
//! let mut reporter = Reporter::new();
//!
//! // ... emit diagnostics ...
//!
//! renderer.render_all(&reporter, &sources);
//! ```

pub mod codes;
pub mod color;
pub mod snippet;

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;

pub use codes::{ERROR_REGISTRY, ErrorCategory, ErrorCode, lookup as lookup_code};
pub use color::{Color, ColorConfig, Style, StyledText};
pub use snippet::{SnippetConfig, SnippetRenderer, render_diagnostic, render_diagnostic_colored};

use crate::compiler::diagnostics::{Diagnostic, Reporter, Severity};
use crate::compiler::source::SourceFile;

/// Output format for diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Human-readable format with colors and snippets.
    #[default]
    Human,
    /// JSON format for IDE integration.
    Json,
    /// Short format: one line per diagnostic.
    Short,
}

/// Configuration for the diagnostic renderer.
#[derive(Clone, Debug)]
pub struct RenderConfig {
    /// Color configuration.
    pub colors: ColorConfig,
    /// Output format.
    pub format: OutputFormat,
    /// Number of context lines in snippets.
    pub context_lines: usize,
    /// Whether to show error codes.
    pub show_codes: bool,
    /// Whether to show source snippets.
    pub show_snippets: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            colors: ColorConfig::auto(),
            format: OutputFormat::Human,
            context_lines: 1,
            show_codes: true,
            show_snippets: true,
        }
    }
}

impl RenderConfig {
    /// Create a config for CI/non-interactive use.
    pub fn ci() -> Self {
        Self {
            colors: ColorConfig::never(),
            format: OutputFormat::Human,
            context_lines: 1,
            show_codes: true,
            show_snippets: true,
        }
    }

    /// Create a config for JSON output.
    pub fn json() -> Self {
        Self {
            colors: ColorConfig::never(),
            format: OutputFormat::Json,
            context_lines: 0,
            show_codes: true,
            show_snippets: false,
        }
    }

    /// Create a config for short output.
    pub fn short() -> Self {
        Self {
            colors: ColorConfig::auto(),
            format: OutputFormat::Short,
            context_lines: 0,
            show_codes: true,
            show_snippets: false,
        }
    }
}

/// Main diagnostic renderer.
pub struct DiagnosticRenderer {
    config: RenderConfig,
}

impl Default for DiagnosticRenderer {
    fn default() -> Self {
        Self::new(RenderConfig::default())
    }
}

impl DiagnosticRenderer {
    /// Create a new diagnostic renderer.
    pub fn new(config: RenderConfig) -> Self {
        Self { config }
    }

    /// Render all diagnostics from a reporter.
    pub fn render_to_stderr(
        &self,
        reporter: &Reporter,
        sources: &HashMap<PathBuf, String>,
    ) -> io::Result<()> {
        let stderr = io::stderr();
        let mut handle = stderr.lock();
        self.render_to_writer(&mut handle, reporter, sources)
    }

    /// Render all diagnostics to a writer.
    pub fn render_to_writer<W: Write>(
        &self,
        writer: &mut W,
        reporter: &Reporter,
        sources: &HashMap<PathBuf, String>,
    ) -> io::Result<()> {
        match self.config.format {
            OutputFormat::Human => self.render_human(writer, reporter, sources),
            OutputFormat::Json => self.render_json(writer, reporter),
            OutputFormat::Short => self.render_short(writer, reporter),
        }
    }

    /// Render in human-readable format.
    fn render_human<W: Write>(
        &self,
        writer: &mut W,
        reporter: &Reporter,
        sources: &HashMap<PathBuf, String>,
    ) -> io::Result<()> {
        let snippet_config = SnippetConfig {
            colors: self.config.colors.clone(),
            context_lines: self.config.context_lines,
            show_gutter: true,
        };
        let renderer = SnippetRenderer::new(snippet_config);

        for diag in reporter.diagnostics() {
            // Get source for this diagnostic
            let source = diag
                .file
                .as_ref()
                .and_then(|f| sources.get(f))
                .map(|s| s.as_str());

            let rendered = renderer.render(diag, source);
            for line in rendered.lines() {
                writeln!(writer, "{}", line)?;
            }
            writeln!(writer)?;
        }

        // Print summary
        let (errors, warnings) = self.count_diagnostics(reporter);
        if errors > 0 || warnings > 0 {
            let colors = &self.config.colors;
            if errors > 0 {
                let styled = colors.error(format!("{} error(s)", errors));
                write!(writer, "{}", colors.render(&styled))?;
            }
            if warnings > 0 {
                if errors > 0 {
                    write!(writer, ", ")?;
                }
                let styled = colors.warning(format!("{} warning(s)", warnings));
                write!(writer, "{}", colors.render(&styled))?;
            }
            writeln!(writer, " emitted")?;
        }

        Ok(())
    }

    /// Render in JSON format.
    fn render_json<W: Write>(&self, writer: &mut W, reporter: &Reporter) -> io::Result<()> {
        let diagnostics: Vec<JsonDiagnostic> = reporter
            .diagnostics()
            .iter()
            .map(|d| JsonDiagnostic::from_diagnostic(d))
            .collect();

        let output = JsonOutput { diagnostics };
        let json = serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string());
        writeln!(writer, "{}", json)
    }

    /// Render in short format (one line per diagnostic).
    fn render_short<W: Write>(&self, writer: &mut W, reporter: &Reporter) -> io::Result<()> {
        let colors = &self.config.colors;

        for diag in reporter.diagnostics() {
            let severity = match diag.severity {
                Severity::Error => colors.render(&colors.error("error")),
                Severity::Warning => colors.render(&colors.warning("warning")),
                Severity::Note => colors.render(&colors.note("note")),
            };

            let code = diag
                .code
                .as_ref()
                .map(|c| format!("[{}]", c))
                .unwrap_or_default();

            let location = if let Some(file) = &diag.file {
                if let Some(span) = &diag.span {
                    if span.start_line > 0 {
                        format!(
                            "{}:{}:{}: ",
                            file.display(),
                            span.start_line,
                            span.start_col
                        )
                    } else {
                        format!("{}: ", file.display())
                    }
                } else {
                    format!("{}: ", file.display())
                }
            } else {
                String::new()
            };

            writeln!(writer, "{}{}{}: {}", location, severity, code, diag.message)?;
        }

        Ok(())
    }

    /// Count errors and warnings.
    fn count_diagnostics(&self, reporter: &Reporter) -> (usize, usize) {
        let mut errors = 0;
        let mut warnings = 0;
        for diag in reporter.diagnostics() {
            match diag.severity {
                Severity::Error => errors += 1,
                Severity::Warning => warnings += 1,
                Severity::Note => {}
            }
        }
        (errors, warnings)
    }
}

/// JSON output structure.
#[derive(serde::Serialize)]
struct JsonOutput {
    diagnostics: Vec<JsonDiagnostic>,
}

/// JSON representation of a diagnostic.
#[derive(serde::Serialize)]
struct JsonDiagnostic {
    severity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span: Option<JsonSpan>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    labels: Vec<JsonLabel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggestion: Option<String>,
}

impl JsonDiagnostic {
    fn from_diagnostic(d: &Diagnostic) -> Self {
        Self {
            severity: match d.severity {
                Severity::Error => "error".to_string(),
                Severity::Warning => "warning".to_string(),
                Severity::Note => "note".to_string(),
            },
            code: d.code.clone(),
            message: d.message.clone(),
            file: d.file.as_ref().map(|f| f.display().to_string()),
            span: d.span.as_ref().map(|s| JsonSpan {
                start_line: s.start_line,
                start_col: s.start_col,
                end_line: s.end_line,
                end_col: s.end_col,
                start_byte: s.start,
                end_byte: s.end,
            }),
            labels: d
                .labels
                .iter()
                .map(|l| JsonLabel {
                    message: l.message.clone(),
                    file: l.file.as_ref().map(|f| f.display().to_string()),
                    span: JsonSpan {
                        start_line: l.span.start_line,
                        start_col: l.span.start_col,
                        end_line: l.span.end_line,
                        end_col: l.span.end_col,
                        start_byte: l.span.start,
                        end_byte: l.span.end,
                    },
                })
                .collect(),
            suggestion: d.suggestion.clone(),
        }
    }
}

/// JSON representation of a span.
#[derive(serde::Serialize)]
struct JsonSpan {
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
    start_byte: usize,
    end_byte: usize,
}

/// JSON representation of a label.
#[derive(serde::Serialize)]
struct JsonLabel {
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    span: JsonSpan,
}

/// Helper to build a source map from source files.
pub fn build_source_map(files: &[SourceFile]) -> HashMap<PathBuf, String> {
    files
        .iter()
        .map(|f| (f.path.clone(), f.text.clone()))
        .collect()
}

/// Convenience function to render diagnostics to stderr.
pub fn emit_diagnostics(reporter: &Reporter, sources: &HashMap<PathBuf, String>) {
    let renderer = DiagnosticRenderer::default();
    if let Err(e) = renderer.render_to_stderr(reporter, sources) {
        eprintln!("Error writing diagnostics: {}", e);
    }
}

/// Convenience function to render diagnostics to a string.
pub fn render_diagnostics_to_string(
    reporter: &Reporter,
    sources: &HashMap<PathBuf, String>,
) -> String {
    let mut buffer = Vec::new();
    let renderer = DiagnosticRenderer::new(RenderConfig {
        colors: ColorConfig::never(),
        ..Default::default()
    });
    if renderer
        .render_to_writer(&mut buffer, reporter, sources)
        .is_ok()
    {
        String::from_utf8_lossy(&buffer).to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::source::Span;

    fn make_diag(message: &str) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            message: message.to_string(),
            file: Some(PathBuf::from("test.arth")),
            span: Some(Span {
                start: 0,
                end: 5,
                start_line: 1,
                start_col: 1,
                end_line: 1,
                end_col: 6,
            }),
            labels: Vec::new(),
            suggestion: None,
            code: Some("E2001".to_string()),
        }
    }

    #[test]
    fn test_render_human() {
        let mut reporter = Reporter::new();
        reporter.emit(make_diag("test error"));

        let mut sources = HashMap::new();
        sources.insert(PathBuf::from("test.arth"), "hello world".to_string());

        let output = render_diagnostics_to_string(&reporter, &sources);
        assert!(output.contains("error[E2001]: test error"));
        assert!(output.contains("test.arth:1:1"));
    }

    #[test]
    fn test_render_json() {
        let mut reporter = Reporter::new();
        reporter.emit(make_diag("test error"));

        let sources = HashMap::new();
        let config = RenderConfig::json();
        let renderer = DiagnosticRenderer::new(config);

        let mut buffer = Vec::new();
        renderer
            .render_to_writer(&mut buffer, &reporter, &sources)
            .unwrap();

        let output = String::from_utf8_lossy(&buffer);
        assert!(output.contains("\"severity\": \"error\""));
        assert!(output.contains("\"code\": \"E2001\""));
        assert!(output.contains("\"message\": \"test error\""));
    }

    #[test]
    fn test_render_short() {
        let mut reporter = Reporter::new();
        reporter.emit(make_diag("test error"));

        let sources = HashMap::new();
        let config = RenderConfig::short();
        let renderer = DiagnosticRenderer::new(config);

        let mut buffer = Vec::new();
        renderer
            .render_to_writer(&mut buffer, &reporter, &sources)
            .unwrap();

        let output = String::from_utf8_lossy(&buffer);
        assert!(output.contains("test.arth:1:1: error[E2001]: test error"));
    }

    #[test]
    fn test_build_source_map() {
        let files = vec![
            SourceFile {
                path: PathBuf::from("a.arth"),
                text: "content a".to_string(),
            },
            SourceFile {
                path: PathBuf::from("b.arth"),
                text: "content b".to_string(),
            },
        ];

        let map = build_source_map(&files);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get(&PathBuf::from("a.arth")),
            Some(&"content a".to_string())
        );
    }

    #[test]
    fn test_count_diagnostics() {
        let mut reporter = Reporter::new();
        reporter.emit(Diagnostic::error("error 1"));
        reporter.emit(Diagnostic::error("error 2"));
        reporter.emit(Diagnostic::warning("warning 1"));
        reporter.emit(Diagnostic::note("note 1"));

        let renderer = DiagnosticRenderer::default();
        let (errors, warnings) = renderer.count_diagnostics(&reporter);
        assert_eq!(errors, 2);
        assert_eq!(warnings, 1);
    }
}
