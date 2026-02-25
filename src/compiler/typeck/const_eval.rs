//! Compile-time constant expression evaluation.
//!
//! This module provides facilities for evaluating constant expressions at compile time,
//! used for:
//! - Enum discriminant validation
//! - Switch case label validation
//!
//! Constant expressions are a subset of expressions that can be evaluated at compile time:
//! - Literals: Int, Float, Bool, Char, Str
//! - Unary operators: +, -, !
//! - Binary operators: arithmetic, bitwise, comparisons, logical
//! - Ternary conditional: cond ? then : else
//! - References to earlier enum discriminants (for expressions like C = A | B)

use crate::compiler::hir::{HirBinOp, HirExpr, HirUnOp};
use std::collections::{HashMap, HashSet};
use std::fmt;

/// Result of constant expression evaluation
#[derive(Clone, Debug, PartialEq)]
pub enum ConstValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Str(String),
}

impl fmt::Display for ConstValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstValue::Int(v) => write!(f, "{}", v),
            ConstValue::Float(v) => write!(f, "{}", v),
            ConstValue::Bool(v) => write!(f, "{}", v),
            ConstValue::Char(v) => write!(f, "'{}'", v),
            ConstValue::Str(v) => write!(f, "\"{}\"", v),
        }
    }
}

/// Error types for constant expression evaluation
#[derive(Clone, Debug, PartialEq)]
pub enum ConstEvalError {
    /// Expression is not constant (e.g., function call)
    NotConstant(String),
    /// Division by zero
    DivisionByZero,
    /// Integer overflow
    Overflow,
    /// Reference to undefined constant
    UndefinedReference(String),
    /// Cyclic constant reference (e.g., A = B, B = A)
    CyclicReference(String),
    /// Type mismatch in constant expression
    TypeMismatch(String),
}

impl fmt::Display for ConstEvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstEvalError::NotConstant(msg) => write!(f, "expression is not constant: {}", msg),
            ConstEvalError::DivisionByZero => write!(f, "division by zero"),
            ConstEvalError::Overflow => write!(f, "integer overflow"),
            ConstEvalError::UndefinedReference(name) => {
                write!(f, "undefined constant '{}'", name)
            }
            ConstEvalError::CyclicReference(name) => {
                write!(f, "cyclic reference detected involving '{}'", name)
            }
            ConstEvalError::TypeMismatch(msg) => write!(f, "type mismatch: {}", msg),
        }
    }
}

/// Context for constant expression evaluation
pub struct ConstEvalContext {
    /// Map of constant names to their evaluated values
    /// Used for enum discriminant references like `C = A | B`
    constants: HashMap<String, ConstValue>,
    /// Track identifiers currently being evaluated to detect cycles
    evaluating: HashSet<String>,
}

impl Default for ConstEvalContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ConstEvalContext {
    pub fn new() -> Self {
        Self {
            constants: HashMap::new(),
            evaluating: HashSet::new(),
        }
    }

    /// Define a constant value
    pub fn define(&mut self, name: &str, value: ConstValue) {
        self.constants.insert(name.to_string(), value);
    }

    /// Set constants from a map of enum variant tags
    pub fn set_enum_variants(&mut self, tags: &std::collections::BTreeMap<String, i64>) {
        for (name, value) in tags {
            self.constants.insert(name.clone(), ConstValue::Int(*value));
        }
    }

    /// Look up a constant by name
    pub fn get(&self, name: &str) -> Option<&ConstValue> {
        self.constants.get(name)
    }

    /// Check if a name is currently being evaluated (for cycle detection)
    fn start_evaluating(&mut self, name: &str) -> bool {
        self.evaluating.insert(name.to_string())
    }

    /// Mark a name as done evaluating
    fn done_evaluating(&mut self, name: &str) {
        self.evaluating.remove(name);
    }

    /// Check if a name is currently being evaluated
    fn is_evaluating(&self, name: &str) -> bool {
        self.evaluating.contains(name)
    }
}

/// Evaluate a constant expression
pub fn eval_const_expr(
    expr: &HirExpr,
    ctx: &mut ConstEvalContext,
) -> Result<ConstValue, ConstEvalError> {
    match expr {
        // Literals
        HirExpr::Int { value, .. } => Ok(ConstValue::Int(*value)),
        HirExpr::Float { value, .. } => Ok(ConstValue::Float(*value)),
        HirExpr::Bool { value, .. } => Ok(ConstValue::Bool(*value)),
        HirExpr::Char { value, .. } => Ok(ConstValue::Char(*value)),
        HirExpr::Str { value, .. } => Ok(ConstValue::Str(value.clone())),

        // Identifier - look up in context (for enum discriminant references)
        HirExpr::Ident { name, .. } => {
            if ctx.is_evaluating(name) {
                return Err(ConstEvalError::CyclicReference(name.clone()));
            }
            match ctx.get(name) {
                Some(v) => Ok(v.clone()),
                None => Err(ConstEvalError::UndefinedReference(name.clone())),
            }
        }

        // Unary operators
        HirExpr::Unary { op, expr, .. } => {
            let val = eval_const_expr(expr, ctx)?;
            match (op, val) {
                (HirUnOp::Neg, ConstValue::Int(v)) => v
                    .checked_neg()
                    .map(ConstValue::Int)
                    .ok_or(ConstEvalError::Overflow),
                (HirUnOp::Neg, ConstValue::Float(v)) => Ok(ConstValue::Float(-v)),
                (HirUnOp::Not, ConstValue::Bool(v)) => Ok(ConstValue::Bool(!v)),
                (HirUnOp::Not, ConstValue::Int(v)) => Ok(ConstValue::Int(!v)), // Bitwise not
                (op, v) => Err(ConstEvalError::TypeMismatch(format!(
                    "cannot apply {:?} to {}",
                    op, v
                ))),
            }
        }

        // Binary operators
        HirExpr::Binary {
            left, op, right, ..
        } => {
            let lhs = eval_const_expr(left, ctx)?;
            let rhs = eval_const_expr(right, ctx)?;
            eval_binary_op(op.clone(), lhs, rhs)
        }

        // Conditional (ternary)
        HirExpr::Conditional {
            cond,
            then_expr,
            else_expr,
            ..
        } => {
            let cond_val = eval_const_expr(cond, ctx)?;
            match cond_val {
                ConstValue::Bool(true) => eval_const_expr(then_expr, ctx),
                ConstValue::Bool(false) => eval_const_expr(else_expr, ctx),
                _ => Err(ConstEvalError::TypeMismatch(
                    "ternary condition must be boolean".to_string(),
                )),
            }
        }

        // Cast expressions - basic support for primitive casts
        HirExpr::Cast { to, expr, .. } => {
            let val = eval_const_expr(expr, ctx)?;
            eval_cast(to, val)
        }

        // Everything else is not a constant expression
        HirExpr::Call { .. } => Err(ConstEvalError::NotConstant(
            "function calls not allowed".to_string(),
        )),
        HirExpr::Member { .. } => Err(ConstEvalError::NotConstant(
            "member access not allowed".to_string(),
        )),
        HirExpr::OptionalMember { .. } => Err(ConstEvalError::NotConstant(
            "optional chaining not allowed".to_string(),
        )),
        HirExpr::Index { .. } => Err(ConstEvalError::NotConstant(
            "indexing not allowed".to_string(),
        )),
        HirExpr::Await { .. } => Err(ConstEvalError::NotConstant("await not allowed".to_string())),
        HirExpr::Lambda { .. } => Err(ConstEvalError::NotConstant(
            "lambda not allowed".to_string(),
        )),
        HirExpr::ListLit { .. } => Err(ConstEvalError::NotConstant(
            "list literals not allowed".to_string(),
        )),
        HirExpr::MapLit { .. } => Err(ConstEvalError::NotConstant(
            "map literals not allowed".to_string(),
        )),
        HirExpr::StructLit { .. } => Err(ConstEvalError::NotConstant(
            "struct literals not allowed".to_string(),
        )),
        HirExpr::EnumVariant { .. } => Err(ConstEvalError::NotConstant(
            "enum variants not allowed".to_string(),
        )),
    }
}

/// Evaluate a binary operation on constant values
fn eval_binary_op(
    op: HirBinOp,
    lhs: ConstValue,
    rhs: ConstValue,
) -> Result<ConstValue, ConstEvalError> {
    use ConstValue::*;
    use HirBinOp::*;

    match (op, lhs, rhs) {
        // Integer arithmetic
        (Add, Int(a), Int(b)) => a.checked_add(b).map(Int).ok_or(ConstEvalError::Overflow),
        (Sub, Int(a), Int(b)) => a.checked_sub(b).map(Int).ok_or(ConstEvalError::Overflow),
        (Mul, Int(a), Int(b)) => a.checked_mul(b).map(Int).ok_or(ConstEvalError::Overflow),
        (Div, Int(a), Int(b)) => {
            if b == 0 {
                Err(ConstEvalError::DivisionByZero)
            } else {
                a.checked_div(b).map(Int).ok_or(ConstEvalError::Overflow)
            }
        }
        (Mod, Int(a), Int(b)) => {
            if b == 0 {
                Err(ConstEvalError::DivisionByZero)
            } else {
                a.checked_rem(b).map(Int).ok_or(ConstEvalError::Overflow)
            }
        }

        // Float arithmetic
        (Add, Float(a), Float(b)) => Ok(Float(a + b)),
        (Sub, Float(a), Float(b)) => Ok(Float(a - b)),
        (Mul, Float(a), Float(b)) => Ok(Float(a * b)),
        (Div, Float(a), Float(b)) => Ok(Float(a / b)),
        (Mod, Float(a), Float(b)) => Ok(Float(a % b)),

        // Bitwise operations (integers only)
        (Shl, Int(a), Int(b)) => {
            if !(0..=63).contains(&b) {
                Err(ConstEvalError::TypeMismatch(
                    "shift amount out of range".to_string(),
                ))
            } else {
                Ok(Int(a << b))
            }
        }
        (Shr, Int(a), Int(b)) => {
            if !(0..=63).contains(&b) {
                Err(ConstEvalError::TypeMismatch(
                    "shift amount out of range".to_string(),
                ))
            } else {
                Ok(Int(a >> b))
            }
        }
        (BitAnd, Int(a), Int(b)) => Ok(Int(a & b)),
        (BitOr, Int(a), Int(b)) => Ok(Int(a | b)),
        (Xor, Int(a), Int(b)) => Ok(Int(a ^ b)),

        // Comparisons (integers)
        (Lt, Int(a), Int(b)) => Ok(Bool(a < b)),
        (Le, Int(a), Int(b)) => Ok(Bool(a <= b)),
        (Gt, Int(a), Int(b)) => Ok(Bool(a > b)),
        (Ge, Int(a), Int(b)) => Ok(Bool(a >= b)),
        (Eq, Int(a), Int(b)) => Ok(Bool(a == b)),
        (Ne, Int(a), Int(b)) => Ok(Bool(a != b)),

        // Comparisons (floats)
        (Lt, Float(a), Float(b)) => Ok(Bool(a < b)),
        (Le, Float(a), Float(b)) => Ok(Bool(a <= b)),
        (Gt, Float(a), Float(b)) => Ok(Bool(a > b)),
        (Ge, Float(a), Float(b)) => Ok(Bool(a >= b)),
        (Eq, Float(a), Float(b)) => Ok(Bool(a == b)),
        (Ne, Float(a), Float(b)) => Ok(Bool(a != b)),

        // Boolean operations
        (And, Bool(a), Bool(b)) => Ok(Bool(a && b)),
        (Or, Bool(a), Bool(b)) => Ok(Bool(a || b)),
        (Eq, Bool(a), Bool(b)) => Ok(Bool(a == b)),
        (Ne, Bool(a), Bool(b)) => Ok(Bool(a != b)),

        // String operations
        (Add, Str(a), Str(b)) => Ok(Str(format!("{}{}", a, b))),
        (Eq, Str(a), Str(b)) => Ok(Bool(a == b)),
        (Ne, Str(a), Str(b)) => Ok(Bool(a != b)),

        // Type mismatches
        (op, l, r) => Err(ConstEvalError::TypeMismatch(format!(
            "cannot apply {:?} to {} and {}",
            op, l, r
        ))),
    }
}

/// Evaluate a cast operation
fn eval_cast(
    to: &crate::compiler::hir::HirType,
    val: ConstValue,
) -> Result<ConstValue, ConstEvalError> {
    use crate::compiler::hir::HirType;
    use ConstValue::*;

    // Get the target type name
    let target_name = match to {
        HirType::Name { path } | HirType::Generic { path, .. } => {
            path.last().map(|s| s.as_str()).unwrap_or("")
        }
        HirType::TypeParam { name } => name.as_str(),
    };

    match (target_name, val) {
        // Int casts
        ("Int" | "int", Int(v)) => Ok(Int(v)),
        ("Int" | "int", Float(v)) => Ok(Int(v as i64)),
        ("Int" | "int", Bool(v)) => Ok(Int(if v { 1 } else { 0 })),
        ("Int" | "int", Char(v)) => Ok(Int(v as i64)),

        // Float casts
        ("Float" | "float", Float(v)) => Ok(Float(v)),
        ("Float" | "float", Int(v)) => Ok(Float(v as f64)),

        // Bool casts
        ("Bool" | "bool", Bool(v)) => Ok(Bool(v)),
        ("Bool" | "bool", Int(v)) => Ok(Bool(v != 0)),

        // Char casts
        ("Char" | "char", Char(v)) => Ok(Char(v)),
        ("Char" | "char", Int(v)) => {
            if let Some(c) = char::from_u32(v as u32) {
                Ok(Char(c))
            } else {
                Err(ConstEvalError::TypeMismatch(format!(
                    "invalid character code {}",
                    v
                )))
            }
        }

        // String casts
        ("String", Str(v)) => Ok(Str(v)),
        ("String", Int(v)) => Ok(Str(v.to_string())),
        ("String", Float(v)) => Ok(Str(v.to_string())),
        ("String", Bool(v)) => Ok(Str(v.to_string())),
        ("String", Char(v)) => Ok(Str(v.to_string())),

        _ => Err(ConstEvalError::TypeMismatch(format!(
            "unsupported cast to '{}'",
            target_name
        ))),
    }
}

/// Specialized function for evaluating integral constant expressions
/// (used for enum discriminants and switch case labels)
pub fn eval_integral_const(
    expr: &HirExpr,
    ctx: &mut ConstEvalContext,
) -> Result<i64, ConstEvalError> {
    match eval_const_expr(expr, ctx)? {
        ConstValue::Int(v) => Ok(v),
        ConstValue::Bool(v) => Ok(if v { 1 } else { 0 }),
        other => Err(ConstEvalError::TypeMismatch(format!(
            "expected integral type, got {}",
            other
        ))),
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
            end: 0,
        }
    }

    fn make_id() -> HirId {
        HirId(1)
    }

    fn int_expr(value: i64) -> HirExpr {
        HirExpr::Int {
            id: make_id(),
            span: make_span(),
            value,
        }
    }

    fn bool_expr(value: bool) -> HirExpr {
        HirExpr::Bool {
            id: make_id(),
            span: make_span(),
            value,
        }
    }

    fn ident_expr(name: &str) -> HirExpr {
        HirExpr::Ident {
            id: make_id(),
            span: make_span(),
            name: name.to_string(),
        }
    }

    fn binary_expr(left: HirExpr, op: HirBinOp, right: HirExpr) -> HirExpr {
        HirExpr::Binary {
            id: make_id(),
            span: make_span(),
            left: Box::new(left),
            op,
            right: Box::new(right),
        }
    }

    #[test]
    fn eval_integer_literal() {
        let mut ctx = ConstEvalContext::new();
        let expr = int_expr(42);
        let result = eval_const_expr(&expr, &mut ctx);
        assert_eq!(result, Ok(ConstValue::Int(42)));
    }

    #[test]
    fn eval_integer_addition() {
        let mut ctx = ConstEvalContext::new();
        let expr = binary_expr(int_expr(10), HirBinOp::Add, int_expr(20));
        let result = eval_const_expr(&expr, &mut ctx);
        assert_eq!(result, Ok(ConstValue::Int(30)));
    }

    #[test]
    fn eval_integer_subtraction() {
        let mut ctx = ConstEvalContext::new();
        let expr = binary_expr(int_expr(50), HirBinOp::Sub, int_expr(20));
        let result = eval_const_expr(&expr, &mut ctx);
        assert_eq!(result, Ok(ConstValue::Int(30)));
    }

    #[test]
    fn eval_bitwise_or() {
        let mut ctx = ConstEvalContext::new();
        let expr = binary_expr(int_expr(1), HirBinOp::BitOr, int_expr(2));
        let result = eval_const_expr(&expr, &mut ctx);
        assert_eq!(result, Ok(ConstValue::Int(3)));
    }

    #[test]
    fn eval_bitwise_and() {
        let mut ctx = ConstEvalContext::new();
        let expr = binary_expr(int_expr(7), HirBinOp::BitAnd, int_expr(3));
        let result = eval_const_expr(&expr, &mut ctx);
        assert_eq!(result, Ok(ConstValue::Int(3)));
    }

    #[test]
    fn eval_shift_left() {
        let mut ctx = ConstEvalContext::new();
        let expr = binary_expr(int_expr(1), HirBinOp::Shl, int_expr(4));
        let result = eval_const_expr(&expr, &mut ctx);
        assert_eq!(result, Ok(ConstValue::Int(16)));
    }

    #[test]
    fn eval_variant_reference() {
        let mut ctx = ConstEvalContext::new();
        ctx.define("A", ConstValue::Int(1));
        ctx.define("B", ConstValue::Int(2));

        // C = A | B should be 3
        let expr = binary_expr(ident_expr("A"), HirBinOp::BitOr, ident_expr("B"));
        let result = eval_const_expr(&expr, &mut ctx);
        assert_eq!(result, Ok(ConstValue::Int(3)));
    }

    #[test]
    fn eval_undefined_reference_error() {
        let mut ctx = ConstEvalContext::new();
        let expr = ident_expr("Unknown");
        let result = eval_const_expr(&expr, &mut ctx);
        assert!(matches!(result, Err(ConstEvalError::UndefinedReference(_))));
    }

    #[test]
    fn eval_division_by_zero_error() {
        let mut ctx = ConstEvalContext::new();
        let expr = binary_expr(int_expr(10), HirBinOp::Div, int_expr(0));
        let result = eval_const_expr(&expr, &mut ctx);
        assert_eq!(result, Err(ConstEvalError::DivisionByZero));
    }

    #[test]
    fn eval_integral_const_from_bool() {
        let mut ctx = ConstEvalContext::new();
        let expr = bool_expr(true);
        let result = eval_integral_const(&expr, &mut ctx);
        assert_eq!(result, Ok(1));
    }

    #[test]
    fn eval_complex_expression() {
        let mut ctx = ConstEvalContext::new();
        // (1 << 0) | (1 << 1) | (1 << 2) = 7
        let a = binary_expr(int_expr(1), HirBinOp::Shl, int_expr(0));
        let b = binary_expr(int_expr(1), HirBinOp::Shl, int_expr(1));
        let c = binary_expr(int_expr(1), HirBinOp::Shl, int_expr(2));
        let ab = binary_expr(a, HirBinOp::BitOr, b);
        let abc = binary_expr(ab, HirBinOp::BitOr, c);
        let result = eval_const_expr(&abc, &mut ctx);
        assert_eq!(result, Ok(ConstValue::Int(7)));
    }
}
