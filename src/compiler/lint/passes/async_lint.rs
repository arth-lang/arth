//! Lint pass for async-related issues.
//!
//! Detects async functions that have no await points.

use crate::compiler::hir::{HirBlock, HirExpr, HirFunc, HirStmt};
use crate::compiler::lint::{LintContext, LintId, LintPass};
use crate::compiler::source::Span;

/// Pass that detects async functions without await points.
pub struct AsyncWithoutAwaitPass {
    /// Whether we found an await in the current function.
    found_await: bool,
}

impl AsyncWithoutAwaitPass {
    /// Create a new async lint pass.
    pub fn new() -> Self {
        Self { found_await: false }
    }

    /// Reset state for a new function.
    fn reset(&mut self) {
        self.found_await = false;
    }

    /// Check a block for await expressions.
    fn check_block(&mut self, block: &HirBlock) {
        for stmt in &block.stmts {
            if self.found_await {
                return;
            }
            self.check_stmt(stmt);
        }
    }

    /// Check a statement for await expressions.
    fn check_stmt(&mut self, stmt: &HirStmt) {
        if self.found_await {
            return;
        }

        match stmt {
            HirStmt::Expr { expr, .. } => {
                self.check_expr(expr);
            }
            HirStmt::Return { expr: Some(e), .. } => {
                self.check_expr(e);
            }
            HirStmt::Throw { expr, .. } => {
                self.check_expr(expr);
            }
            HirStmt::VarDecl { init: Some(e), .. } | HirStmt::Assign { expr: e, .. } => {
                self.check_expr(e);
            }
            HirStmt::If {
                cond,
                then_blk,
                else_blk,
                ..
            } => {
                self.check_expr(cond);
                if !self.found_await {
                    self.check_block(then_blk);
                }
                if !self.found_await {
                    if let Some(else_blk) = else_blk {
                        self.check_block(else_blk);
                    }
                }
            }
            HirStmt::While { cond, body, .. } => {
                self.check_expr(cond);
                if !self.found_await {
                    self.check_block(body);
                }
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
                self.check_expr(expr);
                if !self.found_await {
                    for (case_expr, case_block) in cases {
                        self.check_expr(case_expr);
                        if self.found_await {
                            return;
                        }
                        self.check_block(case_block);
                        if self.found_await {
                            return;
                        }
                    }
                }
                if !self.found_await {
                    if let Some(def) = default {
                        self.check_block(def);
                    }
                }
            }
            HirStmt::Try {
                try_blk,
                catches,
                finally_blk,
                ..
            } => {
                self.check_block(try_blk);
                if !self.found_await {
                    for catch in catches {
                        self.check_block(&catch.block);
                        if self.found_await {
                            return;
                        }
                    }
                }
                if !self.found_await {
                    if let Some(fin) = finally_blk {
                        self.check_block(fin);
                    }
                }
            }
            HirStmt::FieldAssign { object, expr, .. } => {
                self.check_expr(object);
                if !self.found_await {
                    self.check_expr(expr);
                }
            }
            _ => {}
        }
    }

    /// Check an expression for await.
    fn check_expr(&mut self, expr: &HirExpr) {
        if self.found_await {
            return;
        }

        match expr {
            HirExpr::Await { .. } => {
                self.found_await = true;
            }
            HirExpr::Binary { left, right, .. } => {
                self.check_expr(left);
                if !self.found_await {
                    self.check_expr(right);
                }
            }
            HirExpr::Unary { expr: e, .. } => {
                self.check_expr(e);
            }
            HirExpr::Call { callee, args, .. } => {
                self.check_expr(callee);
                if !self.found_await {
                    for arg in args {
                        self.check_expr(arg);
                        if self.found_await {
                            return;
                        }
                    }
                }
            }
            HirExpr::Member { object, .. } => {
                self.check_expr(object);
            }
            HirExpr::Index { object, index, .. } => {
                self.check_expr(object);
                if !self.found_await {
                    self.check_expr(index);
                }
            }
            HirExpr::Conditional {
                cond,
                then_expr,
                else_expr,
                ..
            } => {
                self.check_expr(cond);
                if !self.found_await {
                    self.check_expr(then_expr);
                }
                if !self.found_await {
                    self.check_expr(else_expr);
                }
            }
            HirExpr::ListLit { elements, .. } => {
                for item in elements {
                    self.check_expr(item);
                    if self.found_await {
                        return;
                    }
                }
            }
            HirExpr::MapLit { pairs, spread, .. } => {
                for (k, v) in pairs {
                    self.check_expr(k);
                    if self.found_await {
                        return;
                    }
                    self.check_expr(v);
                    if self.found_await {
                        return;
                    }
                }
                if let Some(s) = spread {
                    self.check_expr(s);
                }
            }
            HirExpr::StructLit { fields, spread, .. } => {
                for (_, val) in fields {
                    self.check_expr(val);
                    if self.found_await {
                        return;
                    }
                }
                if let Some(s) = spread {
                    self.check_expr(s);
                }
            }
            HirExpr::Lambda { .. } => {
                // Don't check nested function bodies - they have their own async context
            }
            HirExpr::Cast { expr: e, .. } => {
                self.check_expr(e);
            }
            _ => {}
        }
    }
}

impl Default for AsyncWithoutAwaitPass {
    fn default() -> Self {
        Self::new()
    }
}

impl LintPass for AsyncWithoutAwaitPass {
    fn name(&self) -> &'static str {
        "async-without-await"
    }

    fn lints(&self) -> &[LintId] {
        &[LintId::AsyncWithoutAwait]
    }

    fn check_function(&mut self, func: &HirFunc, ctx: &mut LintContext) {
        // Only check async functions
        if !func.sig.is_async {
            return;
        }

        self.reset();

        // Check the function body for await expressions
        if let Some(body) = &func.body {
            self.check_block(body);
        }

        // Emit warning if no await found
        if !self.found_await {
            // Get the span from func.sig or use a default
            let span = func
                .sig
                .span
                .as_ref()
                .map(|s| Span {
                    start: s.start as usize,
                    end: s.end as usize,
                    start_line: 0,
                    start_col: 0,
                    end_line: 0,
                    end_col: 0,
                })
                .unwrap_or_default();

            ctx.emit_with_suggestion(
                LintId::AsyncWithoutAwait,
                span,
                format!("async function '{}' has no await points", func.sig.name),
                "consider removing the async modifier or adding await expressions".to_string(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::hir::core::{HirId, Span as HirSpan};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn make_span() -> HirSpan {
        HirSpan {
            file: Arc::new(PathBuf::from("test.arth")),
            start: 0,
            end: 10,
        }
    }

    #[test]
    fn test_finds_await() {
        let mut pass = AsyncWithoutAwaitPass::new();

        // Simulate checking an await expression
        let await_expr = HirExpr::Await {
            id: HirId(0),
            span: make_span(),
            expr: Box::new(HirExpr::Ident {
                id: HirId(1),
                span: make_span(),
                name: "task".to_string(),
            }),
        };
        pass.check_expr(&await_expr);

        assert!(pass.found_await);
    }

    #[test]
    fn test_no_await() {
        let mut pass = AsyncWithoutAwaitPass::new();

        // Check expression without await
        let ident = HirExpr::Ident {
            id: HirId(0),
            span: make_span(),
            name: "x".to_string(),
        };
        pass.check_expr(&ident);

        assert!(!pass.found_await);
    }

    #[test]
    fn test_reset() {
        let mut pass = AsyncWithoutAwaitPass::new();

        pass.found_await = true;
        pass.reset();

        assert!(!pass.found_await);
    }
}
