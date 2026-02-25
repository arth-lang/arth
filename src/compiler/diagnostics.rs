use std::path::PathBuf;

use crate::compiler::source::Span;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// A label attached to a diagnostic, pointing to a span with a message.
/// Used for secondary annotations like "borrow created here" or "move occurred here".
#[derive(Clone, Debug)]
pub struct Label {
    pub span: Span,
    pub message: String,
    pub file: Option<PathBuf>,
}

impl Label {
    pub fn new<M: Into<String>>(span: Span, message: M) -> Self {
        Self {
            span,
            message: message.into(),
            file: None,
        }
    }

    pub fn with_file(mut self, file: PathBuf) -> Self {
        self.file = Some(file);
        self
    }
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub file: Option<PathBuf>,
    pub span: Option<Span>,
    /// Secondary labels for additional context (e.g., "borrow created here")
    pub labels: Vec<Label>,
    /// Optional suggestion for fixing the issue
    pub suggestion: Option<String>,
    /// Error code (e.g., "E0001", "W0001")
    pub code: Option<String>,
}

impl Diagnostic {
    pub fn error<M: Into<String>>(message: M) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            file: None,
            span: None,
            labels: Vec::new(),
            suggestion: None,
            code: None,
        }
    }

    pub fn warning<M: Into<String>>(message: M) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            file: None,
            span: None,
            labels: Vec::new(),
            suggestion: None,
            code: None,
        }
    }

    pub fn note<M: Into<String>>(message: M) -> Self {
        Self {
            severity: Severity::Note,
            message: message.into(),
            file: None,
            span: None,
            labels: Vec::new(),
            suggestion: None,
            code: None,
        }
    }

    /// Create an error with a specific error code.
    pub fn error_with_code<M: Into<String>>(code: &str, message: M) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            file: None,
            span: None,
            labels: Vec::new(),
            suggestion: None,
            code: Some(code.to_string()),
        }
    }

    /// Create a warning with a specific error code.
    pub fn warning_with_code<M: Into<String>>(code: &str, message: M) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            file: None,
            span: None,
            labels: Vec::new(),
            suggestion: None,
            code: Some(code.to_string()),
        }
    }

    pub fn with_file(mut self, file: PathBuf) -> Self {
        self.file = Some(file);
        self
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    /// Add a secondary label to the diagnostic
    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    /// Add multiple secondary labels to the diagnostic
    pub fn with_labels(mut self, labels: Vec<Label>) -> Self {
        self.labels.extend(labels);
        self
    }

    /// Add a suggestion for fixing the issue
    pub fn with_suggestion<M: Into<String>>(mut self, suggestion: M) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Add an error code
    pub fn with_code<S: Into<String>>(mut self, code: S) -> Self {
        self.code = Some(code.into());
        self
    }
}

#[derive(Default)]
pub struct Reporter {
    diagnostics: Vec<Diagnostic>,
}

impl Reporter {
    pub fn new() -> Self {
        Self {
            diagnostics: vec![],
        }
    }

    pub fn emit(&mut self, mut d: Diagnostic) {
        // Ensure a primary span exists when a secondary label carries source location.
        if d.span.is_none()
            && let Some(first_label) = d.labels.first()
        {
            d.span = Some(first_label.span);
        }
        // Assign stable fallback diagnostic codes when not explicitly provided.
        if d.code.is_none() {
            d.code = Some(stable_code_for(d.severity, &d.message));
        }
        self.diagnostics.push(d);
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| matches!(d.severity, Severity::Error))
    }

    pub fn has_warnings(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| matches!(d.severity, Severity::Warning))
    }

    pub fn drain_to_stderr(&mut self) {
        for d in self.diagnostics.drain(..) {
            let code_str = d
                .code
                .as_ref()
                .map(|c| format!("[{}]", c))
                .unwrap_or_default();
            match d.severity {
                Severity::Error => eprint!("error{}: ", code_str),
                Severity::Warning => eprint!("warning{}: ", code_str),
                Severity::Note => eprint!("note{}: ", code_str),
            }
            eprintln!("{}", d.message);
            if let Some(ref file) = d.file {
                match d.span {
                    Some(ref span) => {
                        if span.start_line > 0 {
                            eprintln!(
                                "  --> {}:{}:{}..{}:{} (bytes {}..{})",
                                file.display(),
                                span.start_line,
                                span.start_col,
                                span.end_line,
                                span.end_col,
                                span.start,
                                span.end
                            );
                        } else {
                            eprintln!("  --> {}:{}..{}", file.display(), span.start, span.end);
                        }
                    }
                    None => {
                        eprintln!("  --> {}", file.display());
                    }
                }
            }

            // Print secondary labels
            for label in &d.labels {
                let file_display = label
                    .file
                    .as_ref()
                    .or(d.file.as_ref())
                    .map(|f| f.display().to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());

                if label.span.start_line > 0 {
                    eprintln!(
                        "  note: {} ({}:{}:{})",
                        label.message, file_display, label.span.start_line, label.span.start_col
                    );
                } else {
                    eprintln!(
                        "  note: {} ({}:{}..{})",
                        label.message, file_display, label.span.start, label.span.end
                    );
                }
            }

            // Print suggestion if present
            if let Some(ref suggestion) = d.suggestion {
                eprintln!("  help: {}", suggestion);
            }
        }
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

fn stable_code_for(severity: Severity, message: &str) -> String {
    // FNV-1a hash for deterministic code assignment across runs/platforms.
    let mut hash: u32 = 0x811C9DC5;
    for &b in message.as_bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    let prefix = match severity {
        Severity::Error => "E",
        Severity::Warning => "W",
        Severity::Note => "N",
    };
    format!("{}{:04}", prefix, hash % 10_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reporter_assigns_stable_codes_for_missing_code() {
        let mut r1 = Reporter::new();
        let mut r2 = Reporter::new();

        r1.emit(Diagnostic::error("same diagnostic message"));
        r2.emit(Diagnostic::error("same diagnostic message"));

        let c1 = r1.diagnostics()[0].code.clone();
        let c2 = r2.diagnostics()[0].code.clone();

        assert!(c1.is_some());
        assert_eq!(c1, c2, "fallback codes must be deterministic");
    }

    #[test]
    fn reporter_keeps_explicit_code_when_present() {
        let mut r = Reporter::new();
        r.emit(Diagnostic::error_with_code("E4242", "custom code"));
        assert_eq!(r.diagnostics()[0].code.as_deref(), Some("E4242"));
    }

    #[test]
    fn reporter_promotes_first_label_to_primary_span_when_missing() {
        let mut r = Reporter::new();
        let label_span = Span {
            start: 10,
            end: 12,
            start_line: 2,
            start_col: 3,
            end_line: 2,
            end_col: 5,
        };
        r.emit(
            Diagnostic::error("label-only span").with_label(Label::new(label_span, "points here")),
        );
        assert_eq!(r.diagnostics()[0].span, Some(label_span));
    }
}
