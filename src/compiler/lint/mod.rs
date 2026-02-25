//! Linting framework for the Arth compiler.
//!
//! This module provides a lint pass infrastructure for detecting code quality issues,
//! potential bugs, and style violations. Lints can be suppressed using `@allow(lint-name)`.

pub mod passes;
pub mod registry;

use std::collections::HashMap;
use std::path::PathBuf;

pub use registry::{LINT_REGISTRY, LintDef, LintId, LintLevel, get_lint, lookup_lint};

use crate::compiler::diagnostics::{Diagnostic, Reporter, Severity};
use crate::compiler::hir::{HirDecl, HirFile, HirFunc, HirModule};
use crate::compiler::source::Span;
use crate::compiler::typeck::attrs::AllowIndex;

/// Configuration for lint passes.
#[derive(Clone, Debug, Default)]
pub struct LintConfig {
    /// Override levels for specific lints.
    pub level_overrides: HashMap<LintId, LintLevel>,
    /// Whether to treat warnings as errors.
    pub warnings_as_errors: bool,
}

impl LintConfig {
    /// Create a new lint config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the level for a specific lint.
    pub fn set_level(&mut self, lint: LintId, level: LintLevel) {
        self.level_overrides.insert(lint, level);
    }

    /// Get the effective level for a lint.
    pub fn get_level(&self, lint: LintId) -> LintLevel {
        self.level_overrides
            .get(&lint)
            .copied()
            .unwrap_or_else(|| get_lint(lint).default_level)
    }

    /// Check if a lint is enabled.
    pub fn is_enabled(&self, lint: LintId) -> bool {
        self.get_level(lint) != LintLevel::Allow
    }
}

/// Context passed to lint passes.
pub struct LintContext<'a> {
    /// Reporter for emitting diagnostics.
    reporter: &'a mut Reporter,
    /// Allow index from attribute analysis.
    allow_index: &'a AllowIndex,
    /// Lint configuration.
    config: &'a LintConfig,
    /// Current package name.
    current_pkg: String,
    /// Current module name (if inside a module).
    current_module: Option<String>,
    /// Current function name (if inside a function).
    current_function: Option<String>,
    /// Current file path.
    current_file: PathBuf,
}

impl<'a> LintContext<'a> {
    /// Create a new lint context.
    pub fn new(
        reporter: &'a mut Reporter,
        allow_index: &'a AllowIndex,
        config: &'a LintConfig,
        pkg: &str,
        file: PathBuf,
    ) -> Self {
        Self {
            reporter,
            allow_index,
            config,
            current_pkg: pkg.to_string(),
            current_module: None,
            current_function: None,
            current_file: file,
        }
    }

    /// Set the current module.
    pub fn enter_module(&mut self, name: &str) {
        self.current_module = Some(name.to_string());
    }

    /// Clear the current module.
    pub fn exit_module(&mut self) {
        self.current_module = None;
    }

    /// Set the current function.
    pub fn enter_function(&mut self, name: &str) {
        self.current_function = Some(name.to_string());
    }

    /// Clear the current function.
    pub fn exit_function(&mut self) {
        self.current_function = None;
    }

    /// Check if a lint is suppressed at the current location.
    pub fn is_suppressed(&self, lint: LintId) -> bool {
        let lint_name = lint.name();

        // Check function-level suppression
        if let Some(ref func) = self.current_function {
            if self.allow_index.is_suppressed_for_function(
                &self.current_pkg,
                self.current_module.as_deref(),
                func,
                lint_name,
            ) {
                return true;
            }
        }

        // Check module-level suppression
        if let Some(ref module) = self.current_module {
            if self
                .allow_index
                .modules
                .get(&(self.current_pkg.clone(), module.clone()))
                .is_some_and(|lints| lints.contains(lint_name))
            {
                return true;
            }
        }

        false
    }

    /// Emit a lint warning.
    pub fn emit(&mut self, lint: LintId, span: Span, message: String) {
        if self.is_suppressed(lint) {
            return;
        }

        let level = self.config.get_level(lint);
        if level == LintLevel::Allow {
            return;
        }

        let severity = match level {
            LintLevel::Allow => return,
            LintLevel::Warn if self.config.warnings_as_errors => Severity::Error,
            LintLevel::Warn => Severity::Warning,
            LintLevel::Deny => Severity::Error,
        };

        let diag = Diagnostic {
            severity,
            message,
            file: Some(self.current_file.clone()),
            span: Some(span),
            labels: Vec::new(),
            suggestion: None,
            code: Some(lint.code().to_string()),
        };

        self.reporter.emit(diag);
    }

    /// Emit a lint with a suggestion.
    pub fn emit_with_suggestion(
        &mut self,
        lint: LintId,
        span: Span,
        message: String,
        suggestion: String,
    ) {
        if self.is_suppressed(lint) || !self.config.is_enabled(lint) {
            return;
        }

        let level = self.config.get_level(lint);
        let severity = match level {
            LintLevel::Allow => return,
            LintLevel::Warn if self.config.warnings_as_errors => Severity::Error,
            LintLevel::Warn => Severity::Warning,
            LintLevel::Deny => Severity::Error,
        };

        let diag = Diagnostic {
            severity,
            message,
            file: Some(self.current_file.clone()),
            span: Some(span),
            labels: Vec::new(),
            suggestion: Some(suggestion),
            code: Some(lint.code().to_string()),
        };

        self.reporter.emit(diag);
    }

    /// Get the current package.
    pub fn package(&self) -> &str {
        &self.current_pkg
    }

    /// Get the current module, if any.
    pub fn module(&self) -> Option<&str> {
        self.current_module.as_deref()
    }

    /// Get the current function, if any.
    pub fn function(&self) -> Option<&str> {
        self.current_function.as_deref()
    }
}

/// Trait for implementing lint passes.
pub trait LintPass {
    /// Name of this lint pass.
    fn name(&self) -> &'static str;

    /// Lints checked by this pass.
    fn lints(&self) -> &[LintId];

    /// Check a module declaration.
    fn check_module(&mut self, _module: &HirModule, _ctx: &mut LintContext) {}

    /// Check a function.
    fn check_function(&mut self, _func: &HirFunc, _ctx: &mut LintContext) {}
}

/// Run all lint passes on the given HIR files.
pub fn run_lints(files: &[HirFile], allow_index: &AllowIndex, config: &LintConfig) -> Reporter {
    let mut reporter = Reporter::new();

    // Create all lint passes
    let mut passes: Vec<Box<dyn LintPass>> = vec![
        Box::new(passes::UnusedVariablePass::new()),
        Box::new(passes::AsyncWithoutAwaitPass::new()),
    ];

    // Run passes on each file
    for file in files {
        let pkg = file
            .package
            .as_ref()
            .map(|p| p.0.join("."))
            .unwrap_or_default();

        let mut ctx = LintContext::new(&mut reporter, allow_index, config, &pkg, file.path.clone());

        for decl in &file.decls {
            check_decl(decl, &mut passes, &mut ctx);
        }
    }

    reporter
}

/// Check a single declaration with all lint passes.
fn check_decl(decl: &HirDecl, passes: &mut [Box<dyn LintPass>], ctx: &mut LintContext) {
    match decl {
        HirDecl::Module(m) => {
            ctx.enter_module(&m.name);

            for pass in passes.iter_mut() {
                pass.check_module(m, ctx);
            }

            // Check functions in the module
            for func in &m.funcs {
                ctx.enter_function(&func.sig.name);

                for pass in passes.iter_mut() {
                    pass.check_function(func, ctx);
                }

                ctx.exit_function();
            }

            ctx.exit_module();
        }
        HirDecl::Function(func) => {
            ctx.enter_function(&func.sig.name);

            for pass in passes.iter_mut() {
                pass.check_function(func, ctx);
            }

            ctx.exit_function();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lint_config_default() {
        let config = LintConfig::default();
        assert_eq!(config.get_level(LintId::UnusedVariable), LintLevel::Warn);
        assert!(config.is_enabled(LintId::UnusedVariable));
    }

    #[test]
    fn test_lint_config_override() {
        let mut config = LintConfig::new();
        config.set_level(LintId::UnusedVariable, LintLevel::Allow);

        assert_eq!(config.get_level(LintId::UnusedVariable), LintLevel::Allow);
        assert!(!config.is_enabled(LintId::UnusedVariable));

        // Other lints should still use default
        assert_eq!(config.get_level(LintId::UnusedImport), LintLevel::Warn);
    }

    #[test]
    fn test_lint_config_deny() {
        let mut config = LintConfig::new();
        config.set_level(LintId::UnusedVariable, LintLevel::Deny);

        assert_eq!(config.get_level(LintId::UnusedVariable), LintLevel::Deny);
        assert!(config.is_enabled(LintId::UnusedVariable));
    }
}
