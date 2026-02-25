//! Lint pass for unused variables.
//!
//! Detects variables that are declared but never used.

use std::collections::HashMap;

use crate::compiler::hir::{HirBlock, HirExpr, HirFunc, HirStmt};
use crate::compiler::lint::{LintContext, LintId, LintPass};
use crate::compiler::source::Span;

/// Pass that detects unused variables.
pub struct UnusedVariablePass {
    /// Variables declared in the current function.
    /// Maps variable name to (span, used).
    declared: HashMap<String, (Span, bool)>,
}

impl UnusedVariablePass {
    /// Create a new unused variable pass.
    pub fn new() -> Self {
        Self {
            declared: HashMap::new(),
        }
    }

    /// Reset state for a new function.
    fn reset(&mut self) {
        self.declared.clear();
    }

    /// Record a variable declaration.
    fn record_decl(&mut self, name: &str, span: Span) {
        // Skip variables starting with underscore
        if name.starts_with('_') {
            return;
        }
        self.declared.insert(name.to_string(), (span, false));
    }

    /// Mark a variable as used.
    fn mark_used(&mut self, name: &str) {
        if let Some((_, used)) = self.declared.get_mut(name) {
            *used = true;
        }
    }

    /// Check a block for variable declarations and usages.
    fn check_block(&mut self, block: &HirBlock) {
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
    }

    /// Check a statement.
    fn check_stmt(&mut self, stmt: &HirStmt) {
        match stmt {
            HirStmt::VarDecl {
                name, init, span, ..
            } => {
                // Convert HirSpan to source::Span
                let source_span = Span {
                    start: span.start as usize,
                    end: span.end as usize,
                    start_line: 0,
                    start_col: 0,
                    end_line: 0,
                    end_col: 0,
                };
                self.record_decl(name, source_span);
                if let Some(expr) = init {
                    self.check_expr_for_usages(expr);
                }
            }
            HirStmt::Assign { expr, .. } => {
                self.check_expr_for_usages(expr);
            }
            HirStmt::Expr { expr, .. } => {
                self.check_expr_for_usages(expr);
            }
            HirStmt::Return { expr: Some(e), .. } => {
                self.check_expr_for_usages(e);
            }
            HirStmt::Throw { expr, .. } => {
                self.check_expr_for_usages(expr);
            }
            HirStmt::If {
                cond,
                then_blk,
                else_blk,
                ..
            } => {
                self.check_expr_for_usages(cond);
                self.check_block(then_blk);
                if let Some(else_blk) = else_blk {
                    self.check_block(else_blk);
                }
            }
            HirStmt::While { cond, body, .. } => {
                self.check_expr_for_usages(cond);
                self.check_block(body);
            }
            HirStmt::Block(block) => {
                self.check_block(block);
            }
            HirStmt::Switch {
                expr,
                cases,
                default,
                ..
            } => {
                self.check_expr_for_usages(expr);
                for (case_expr, case_block) in cases {
                    self.check_expr_for_usages(case_expr);
                    self.check_block(case_block);
                }
                if let Some(def) = default {
                    self.check_block(def);
                }
            }
            HirStmt::Try {
                try_blk,
                catches,
                finally_blk,
                ..
            } => {
                self.check_block(try_blk);
                for catch in catches {
                    self.check_block(&catch.block);
                }
                if let Some(fin) = finally_blk {
                    self.check_block(fin);
                }
            }
            HirStmt::FieldAssign { object, expr, .. } => {
                self.check_expr_for_usages(object);
                self.check_expr_for_usages(expr);
            }
            _ => {}
        }
    }

    /// Check an expression for variable usages.
    fn check_expr_for_usages(&mut self, expr: &HirExpr) {
        match expr {
            HirExpr::Ident { name, .. } => {
                self.mark_used(name);
            }
            HirExpr::Binary { left, right, .. } => {
                self.check_expr_for_usages(left);
                self.check_expr_for_usages(right);
            }
            HirExpr::Unary { expr: e, .. } | HirExpr::Await { expr: e, .. } => {
                self.check_expr_for_usages(e);
            }
            HirExpr::Call { callee, args, .. } => {
                self.check_expr_for_usages(callee);
                for arg in args {
                    self.check_expr_for_usages(arg);
                }
            }
            HirExpr::Member { object, .. } => {
                self.check_expr_for_usages(object);
            }
            HirExpr::Index { object, index, .. } => {
                self.check_expr_for_usages(object);
                self.check_expr_for_usages(index);
            }
            HirExpr::Conditional {
                cond,
                then_expr,
                else_expr,
                ..
            } => {
                self.check_expr_for_usages(cond);
                self.check_expr_for_usages(then_expr);
                self.check_expr_for_usages(else_expr);
            }
            HirExpr::ListLit { elements, .. } => {
                for item in elements {
                    self.check_expr_for_usages(item);
                }
            }
            HirExpr::MapLit { pairs, spread, .. } => {
                for (k, v) in pairs {
                    self.check_expr_for_usages(k);
                    self.check_expr_for_usages(v);
                }
                if let Some(s) = spread {
                    self.check_expr_for_usages(s);
                }
            }
            HirExpr::StructLit { fields, spread, .. } => {
                for (_, val) in fields {
                    self.check_expr_for_usages(val);
                }
                if let Some(s) = spread {
                    self.check_expr_for_usages(s);
                }
            }
            HirExpr::Lambda { body, .. } => {
                self.check_block(body);
            }
            HirExpr::Cast { expr: e, .. } => {
                self.check_expr_for_usages(e);
            }
            _ => {}
        }
    }

    /// Emit warnings for unused variables.
    fn emit_unused(&self, ctx: &mut LintContext) {
        for (name, (span, used)) in &self.declared {
            if !*used {
                ctx.emit_with_suggestion(
                    LintId::UnusedVariable,
                    *span,
                    format!("unused variable '{}'", name),
                    format!("prefix with underscore to suppress: '_{}'", name),
                );
            }
        }
    }
}

impl Default for UnusedVariablePass {
    fn default() -> Self {
        Self::new()
    }
}

impl LintPass for UnusedVariablePass {
    fn name(&self) -> &'static str {
        "unused-variable"
    }

    fn lints(&self) -> &[LintId] {
        &[LintId::UnusedVariable]
    }

    fn check_function(&mut self, func: &HirFunc, ctx: &mut LintContext) {
        self.reset();

        // Check the function body
        if let Some(body) = &func.body {
            self.check_block(body);
        }

        // Emit warnings for unused variables
        self.emit_unused(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_span() -> Span {
        Span {
            start: 0,
            end: 10,
            start_line: 1,
            start_col: 1,
            end_line: 1,
            end_col: 11,
        }
    }

    #[test]
    fn test_unused_variable_detection() {
        let mut pass = UnusedVariablePass::new();

        // Record a declaration
        pass.record_decl("x", make_span());

        // Should be unused
        assert_eq!(pass.declared.get("x").map(|(_, used)| *used), Some(false));

        // Mark as used
        pass.mark_used("x");
        assert_eq!(pass.declared.get("x").map(|(_, used)| *used), Some(true));
    }

    #[test]
    fn test_underscore_prefix_ignored() {
        let mut pass = UnusedVariablePass::new();

        // Underscore-prefixed variables should be ignored
        pass.record_decl("_unused", make_span());
        assert!(pass.declared.get("_unused").is_none());
    }

    #[test]
    fn test_reset_clears_state() {
        let mut pass = UnusedVariablePass::new();

        pass.record_decl("x", make_span());
        assert!(!pass.declared.is_empty());

        pass.reset();
        assert!(pass.declared.is_empty());
    }
}
