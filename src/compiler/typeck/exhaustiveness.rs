//! Exhaustiveness and usefulness checking for pattern matching
//!
//! This module implements the classic algorithm from "Warnings for pattern matching"
//! by Luc Maranget. It checks:
//! - Exhaustiveness: whether all possible values are covered by patterns
//! - Usefulness: whether each pattern can match something not covered by previous patterns
//!
//! The algorithm works on a pattern matrix representation and uses recursive
//! specialization to check coverage.

use super::Ty;
use crate::compiler::ast::{self as AS, Pattern};
use crate::compiler::source::Span;
use std::collections::HashMap;

/// A constructor represents the "head" of a pattern
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Constructor {
    /// Wildcard matches anything
    Wildcard,
    /// Boolean literal
    Bool(bool),
    /// Integer literal (for exhaustiveness, we treat all ints as equivalent)
    Int(i64),
    /// String literal
    String(String),
    /// Enum variant: (enum_path, variant_name, arity)
    Variant {
        enum_path: Vec<String>,
        variant: String,
        arity: usize,
    },
    /// Missing constructor (used for witness generation)
    Missing,
}

impl Constructor {
    /// Check if this constructor is a wildcard
    fn is_wildcard(&self) -> bool {
        matches!(self, Constructor::Wildcard)
    }
}

/// A simplified pattern for exhaustiveness checking
#[derive(Clone, Debug)]
pub(super) struct Pat {
    /// The constructor at the head of this pattern
    pub ctor: Constructor,
    /// Sub-patterns for constructors with fields
    pub fields: Vec<Pat>,
    /// The original span for error reporting
    pub span: Option<Span>,
}

impl Pat {
    /// Create a wildcard pattern
    pub fn wildcard() -> Self {
        Pat {
            ctor: Constructor::Wildcard,
            fields: vec![],
            span: None,
        }
    }

    /// Create a boolean pattern
    pub fn bool_pat(b: bool) -> Self {
        Pat {
            ctor: Constructor::Bool(b),
            fields: vec![],
            span: None,
        }
    }

    /// Create a variant pattern
    pub fn variant(enum_path: Vec<String>, variant: String, fields: Vec<Pat>) -> Self {
        let arity = fields.len();
        Pat {
            ctor: Constructor::Variant {
                enum_path,
                variant,
                arity,
            },
            fields,
            span: None,
        }
    }

    /// Create a missing pattern (for witness)
    pub fn missing() -> Self {
        Pat {
            ctor: Constructor::Missing,
            fields: vec![],
            span: None,
        }
    }
}

/// A row in the pattern matrix
#[derive(Clone, Debug)]
struct PatternRow {
    patterns: Vec<Pat>,
}

impl PatternRow {
    fn new(patterns: Vec<Pat>) -> Self {
        PatternRow { patterns }
    }

    fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    fn first(&self) -> Option<&Pat> {
        self.patterns.first()
    }
}

/// The pattern matrix for exhaustiveness checking
#[derive(Clone, Debug)]
struct PatternMatrix {
    rows: Vec<PatternRow>,
}

impl PatternMatrix {
    fn new() -> Self {
        PatternMatrix { rows: vec![] }
    }

    fn push(&mut self, row: PatternRow) {
        self.rows.push(row);
    }

    fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn width(&self) -> usize {
        self.rows.first().map(|r| r.patterns.len()).unwrap_or(0)
    }
}

/// Type context for exhaustiveness checking
pub(super) struct ExhaustivenessCtx<'a> {
    /// Map from (pkg, enum_name) -> list of variant names
    pub enum_variants: &'a HashMap<(String, String, String), Vec<AS::NamePath>>,
    /// Current package for resolving types
    pub current_pkg: Option<String>,
}

impl<'a> ExhaustivenessCtx<'a> {
    /// Get all constructors for a type
    fn type_constructors(&self, ty: &Ty) -> Vec<Constructor> {
        match ty {
            Ty::Bool => vec![Constructor::Bool(true), Constructor::Bool(false)],
            Ty::Named(path) | Ty::Generic { path, .. } => {
                // Check if it's an enum
                let (pkg, enum_name) = if path.len() == 1 {
                    (
                        self.current_pkg.clone().unwrap_or_default(),
                        path[0].clone(),
                    )
                } else {
                    (
                        path[..path.len() - 1].join("."),
                        path.last().unwrap().clone(),
                    )
                };

                // Collect all variants for this enum
                let mut variants = vec![];
                for ((p, e, v), payloads) in self.enum_variants.iter() {
                    if p == &pkg && e == &enum_name {
                        variants.push(Constructor::Variant {
                            enum_path: path.clone(),
                            variant: v.clone(),
                            arity: payloads.len(),
                        });
                    }
                }

                if variants.is_empty() {
                    // Not an enum, treat as opaque (infinite constructors)
                    vec![]
                } else {
                    variants
                }
            }
            // For other types, treat as having infinite constructors
            _ => vec![],
        }
    }

    /// Check if a type has finite constructors
    fn has_finite_constructors(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Bool => true,
            Ty::Named(path) | Ty::Generic { path, .. } => {
                let (pkg, enum_name) = if path.len() == 1 {
                    (
                        self.current_pkg.clone().unwrap_or_default(),
                        path[0].clone(),
                    )
                } else {
                    (
                        path[..path.len() - 1].join("."),
                        path.last().unwrap().clone(),
                    )
                };

                // Check if any variant exists for this enum
                self.enum_variants
                    .keys()
                    .any(|(p, e, _)| p == &pkg && e == &enum_name)
            }
            _ => false,
        }
    }

    /// Get the arity (number of fields) for a constructor
    fn ctor_arity(&self, ctor: &Constructor) -> usize {
        match ctor {
            Constructor::Variant { arity, .. } => *arity,
            _ => 0,
        }
    }

    /// Get sub-field types for a constructor
    fn ctor_field_types(&self, _ctor: &Constructor, _ty: &Ty) -> Vec<Ty> {
        match _ctor {
            Constructor::Variant {
                enum_path,
                variant,
                arity,
            } => {
                // Look up field types
                let (pkg, enum_name) = if enum_path.len() == 1 {
                    (
                        self.current_pkg.clone().unwrap_or_default(),
                        enum_path[0].clone(),
                    )
                } else {
                    (
                        enum_path[..enum_path.len() - 1].join("."),
                        enum_path.last().unwrap().clone(),
                    )
                };
                let key = (pkg, enum_name, variant.clone());
                if let Some(payloads) = self.enum_variants.get(&key) {
                    payloads
                        .iter()
                        .map(|np| Ty::Named(np.path.iter().map(|id| id.0.clone()).collect()))
                        .collect()
                } else {
                    vec![Ty::Unknown; *arity]
                }
            }
            _ => vec![],
        }
    }
}

/// Convert an AST pattern to our Pat representation
pub(super) fn ast_to_pat(pat: &Pattern) -> Pat {
    match pat {
        Pattern::Wildcard => Pat::wildcard(),
        Pattern::Binding(_) => Pat::wildcard(), // Bindings match anything
        Pattern::Literal(expr) => {
            // Extract literal value from expression
            match expr.as_ref() {
                AS::Expr::Bool(b) => Pat::bool_pat(*b),
                AS::Expr::Int(n) => Pat {
                    ctor: Constructor::Int(*n),
                    fields: vec![],
                    span: None,
                },
                AS::Expr::Str(s) => Pat {
                    ctor: Constructor::String(s.clone()),
                    fields: vec![],
                    span: None,
                },
                _ => Pat::wildcard(), // Other expressions treated as wildcards
            }
        }
        Pattern::Variant {
            enum_ty,
            variant,
            payloads,
            span,
        } => {
            let enum_path: Vec<String> = enum_ty.path.iter().map(|i| i.0.clone()).collect();
            let field_pats: Vec<Pat> = payloads.iter().map(ast_to_pat).collect();
            Pat {
                ctor: Constructor::Variant {
                    enum_path,
                    variant: variant.0.clone(),
                    arity: field_pats.len(),
                },
                fields: field_pats,
                span: Some(*span),
            }
        }
    }
}

/// Convert an AST pattern to Pat while normalizing unqualified enum paths
/// against the scrutinee type path when they refer to the same enum.
pub(super) fn ast_to_pat_with_scrutinee(pat: &Pattern, scrutinee_ty: &Ty) -> Pat {
    let mut out = ast_to_pat(pat);
    normalize_pat_enum_path(&mut out, scrutinee_ty);
    out
}

fn normalize_pat_enum_path(pat: &mut Pat, scrutinee_ty: &Ty) {
    if let Constructor::Variant { enum_path, .. } = &mut pat.ctor
        && enum_path.len() == 1
    {
        let scrutinee_path_opt = match scrutinee_ty {
            Ty::Named(path) | Ty::Generic { path, .. } => Some(path),
            _ => None,
        };
        if let Some(scrutinee_path) = scrutinee_path_opt
            && !scrutinee_path.is_empty()
            && scrutinee_path.last() == enum_path.last()
        {
            *enum_path = scrutinee_path.clone();
        }
    }

    for field in &mut pat.fields {
        normalize_pat_enum_path(field, scrutinee_ty);
    }
}

/// Result of exhaustiveness checking
#[derive(Debug)]
pub(super) struct ExhaustivenessResult {
    /// Is the match exhaustive?
    pub is_exhaustive: bool,
    /// Witness patterns for non-exhaustiveness (examples of uncovered cases)
    pub witnesses: Vec<WitnessPattern>,
    /// Indices of redundant (unreachable) patterns
    pub redundant_patterns: Vec<usize>,
}

/// A witness pattern representing an uncovered case
#[derive(Debug, Clone)]
pub(super) struct WitnessPattern {
    pub description: String,
}

/// Check exhaustiveness of a set of patterns against a scrutinee type
pub(super) fn check_exhaustiveness(
    patterns: &[Pat],
    scrutinee_ty: &Ty,
    ctx: &ExhaustivenessCtx,
) -> ExhaustivenessResult {
    let mut matrix = PatternMatrix::new();
    let mut redundant_patterns = vec![];

    // Build pattern matrix and check usefulness of each pattern
    for (idx, pat) in patterns.iter().enumerate() {
        let row = PatternRow::new(vec![pat.clone()]);

        // Check if this pattern is useful (can match something not already covered)
        if !is_useful(&matrix, &row, std::slice::from_ref(scrutinee_ty), ctx) {
            redundant_patterns.push(idx);
        }

        matrix.push(row);
    }

    // Check if the match is exhaustive by checking if a wildcard would be useful
    let wildcard_row = PatternRow::new(vec![Pat::wildcard()]);
    let is_exhaustive = !is_useful(
        &matrix,
        &wildcard_row,
        std::slice::from_ref(scrutinee_ty),
        ctx,
    );

    // Generate witnesses for non-exhaustive matches
    let witnesses = if is_exhaustive {
        vec![]
    } else {
        compute_witnesses(&matrix, std::slice::from_ref(scrutinee_ty), ctx)
    };

    ExhaustivenessResult {
        is_exhaustive,
        witnesses,
        redundant_patterns,
    }
}

/// Check if a pattern row is useful given a pattern matrix
/// A pattern is useful if it can match some value not matched by any row in the matrix
fn is_useful(
    matrix: &PatternMatrix,
    row: &PatternRow,
    types: &[Ty],
    ctx: &ExhaustivenessCtx,
) -> bool {
    let rest_types = types.get(1..).unwrap_or(&[]);

    // Base case: empty row
    if row.is_empty() {
        // Useful iff the matrix has no complete rows
        return matrix.is_empty();
    }

    let first_pat = row.first().unwrap();
    let first_ty = types.first().cloned().unwrap_or(Ty::Unknown);

    match &first_pat.ctor {
        Constructor::Wildcard => {
            // For wildcard, we need to check all possible constructors
            let ctors = ctx.type_constructors(&first_ty);

            if ctors.is_empty() || !ctx.has_finite_constructors(&first_ty) {
                // Infinite type or unknown constructors - check default matrix
                let default_matrix = default_specialize(matrix);
                let rest_row = PatternRow::new(row.patterns[1..].to_vec());
                is_useful(&default_matrix, &rest_row, rest_types, ctx)
            } else {
                // Finite constructors - check if matrix has a wildcard that covers all
                let has_wildcard_in_matrix = matrix
                    .rows
                    .iter()
                    .any(|r| r.first().map(|p| p.ctor.is_wildcard()).unwrap_or(false));

                if has_wildcard_in_matrix {
                    // Matrix has a wildcard - it covers all constructors
                    // Check if the default specialization still has room for our row
                    let default_matrix = default_specialize(matrix);
                    let rest_row = PatternRow::new(row.patterns[1..].to_vec());
                    is_useful(&default_matrix, &rest_row, rest_types, ctx)
                } else {
                    // No wildcard in matrix - check each constructor
                    let covered_ctors = collect_head_constructors(matrix);

                    // Find constructors not covered by the matrix
                    let missing_ctors: Vec<&Constructor> = ctors
                        .iter()
                        .filter(|c| !covered_ctors.iter().any(|cc| constructors_equal(c, cc)))
                        .collect();

                    if missing_ctors.is_empty() {
                        // All constructors covered - check each specialization
                        for ctor in &ctors {
                            let spec_matrix = specialize(matrix, ctor, ctx);
                            let spec_row = specialize_row(row, ctor, ctx);
                            let field_types = ctx.ctor_field_types(ctor, &first_ty);
                            let mut new_types = field_types;
                            new_types.extend(rest_types.iter().cloned());
                            if is_useful(&spec_matrix, &spec_row, &new_types, ctx) {
                                return true;
                            }
                        }
                        false
                    } else {
                        // Some constructors missing - wildcard is useful
                        true
                    }
                }
            }
        }
        _ => {
            // Specific constructor - specialize on it
            let spec_matrix = specialize(matrix, &first_pat.ctor, ctx);
            let spec_row = specialize_row(row, &first_pat.ctor, ctx);
            let field_types = ctx.ctor_field_types(&first_pat.ctor, &first_ty);
            let mut new_types = field_types;
            new_types.extend(rest_types.iter().cloned());
            is_useful(&spec_matrix, &spec_row, &new_types, ctx)
        }
    }
}

/// Collect all head constructors from the first column of a matrix
fn collect_head_constructors(matrix: &PatternMatrix) -> Vec<Constructor> {
    let mut ctors = vec![];
    for row in &matrix.rows {
        if let Some(first) = row.first() {
            if !first.ctor.is_wildcard() {
                // Avoid duplicates
                if !ctors.iter().any(|c| constructors_equal(c, &first.ctor)) {
                    ctors.push(first.ctor.clone());
                }
            }
        }
    }
    ctors
}

/// Check if two constructors are equal (for deduplication)
fn constructors_equal(a: &Constructor, b: &Constructor) -> bool {
    match (a, b) {
        (Constructor::Wildcard, Constructor::Wildcard) => true,
        (Constructor::Bool(x), Constructor::Bool(y)) => x == y,
        (Constructor::Int(x), Constructor::Int(y)) => x == y,
        (Constructor::String(x), Constructor::String(y)) => x == y,
        (
            Constructor::Variant {
                variant: v1,
                enum_path: p1,
                ..
            },
            Constructor::Variant {
                variant: v2,
                enum_path: p2,
                ..
            },
        ) => v1 == v2 && p1 == p2,
        _ => false,
    }
}

/// Specialize a pattern matrix on a constructor
/// This filters and expands rows that match the given constructor
fn specialize(
    matrix: &PatternMatrix,
    ctor: &Constructor,
    ctx: &ExhaustivenessCtx,
) -> PatternMatrix {
    let mut result = PatternMatrix::new();
    let arity = ctx.ctor_arity(ctor);

    for row in &matrix.rows {
        if let Some(first) = row.first() {
            if let Some(specialized) = specialize_pattern(first, ctor, arity) {
                let mut new_patterns = specialized;
                new_patterns.extend(row.patterns[1..].iter().cloned());
                result.push(PatternRow::new(new_patterns));
            }
        }
    }

    result
}

/// Specialize a single pattern on a constructor
fn specialize_pattern(pat: &Pat, ctor: &Constructor, arity: usize) -> Option<Vec<Pat>> {
    match &pat.ctor {
        Constructor::Wildcard => {
            // Wildcard expands to arity wildcards
            Some(vec![Pat::wildcard(); arity])
        }
        _ if constructors_equal(&pat.ctor, ctor) => {
            // Same constructor - return fields
            Some(pat.fields.clone())
        }
        _ => {
            // Different constructor - row doesn't match
            None
        }
    }
}

/// Specialize a pattern row on a constructor
fn specialize_row(row: &PatternRow, ctor: &Constructor, ctx: &ExhaustivenessCtx) -> PatternRow {
    let arity = ctx.ctor_arity(ctor);
    if let Some(first) = row.first() {
        if let Some(specialized) = specialize_pattern(first, ctor, arity) {
            let mut new_patterns = specialized;
            new_patterns.extend(row.patterns[1..].iter().cloned());
            return PatternRow::new(new_patterns);
        }
    }
    PatternRow::new(vec![])
}

/// Compute the default matrix (rows with wildcard in first column)
fn default_specialize(matrix: &PatternMatrix) -> PatternMatrix {
    let mut result = PatternMatrix::new();

    for row in &matrix.rows {
        if let Some(first) = row.first() {
            if first.ctor.is_wildcard() {
                result.push(PatternRow::new(row.patterns[1..].to_vec()));
            }
        }
    }

    result
}

/// Compute witness patterns for non-exhaustive matches
fn compute_witnesses(
    matrix: &PatternMatrix,
    types: &[Ty],
    ctx: &ExhaustivenessCtx,
) -> Vec<WitnessPattern> {
    let mut witnesses = vec![];

    if types.is_empty() {
        return witnesses;
    }

    let first_ty = &types[0];
    let ctors = ctx.type_constructors(first_ty);
    let covered_ctors = collect_head_constructors(matrix);

    // Find missing constructors
    for ctor in &ctors {
        if !covered_ctors.iter().any(|c| constructors_equal(c, ctor)) {
            witnesses.push(WitnessPattern {
                description: format_constructor(ctor),
            });
        }
    }

    // If no specific constructors missing but wildcards can match more
    if witnesses.is_empty() && !ctx.has_finite_constructors(first_ty) {
        witnesses.push(WitnessPattern {
            description: "_".to_string(),
        });
    }

    witnesses
}

/// Format a constructor for error messages
fn format_constructor(ctor: &Constructor) -> String {
    match ctor {
        Constructor::Wildcard => "_".to_string(),
        Constructor::Bool(b) => b.to_string(),
        Constructor::Int(n) => n.to_string(),
        Constructor::String(s) => format!("\"{}\"", s),
        Constructor::Variant {
            enum_path,
            variant,
            arity,
        } => {
            let base = format!("{}.{}", enum_path.join("."), variant);
            if *arity > 0 {
                format!("{}({})", base, vec!["_"; *arity].join(", "))
            } else {
                base
            }
        }
        Constructor::Missing => "<missing>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> HashMap<(String, String, String), Vec<AS::NamePath>> {
        let mut variants = HashMap::new();
        // Create a simple enum: enum Status { Ok, Error(String) }
        variants.insert(
            ("demo".to_string(), "Status".to_string(), "Ok".to_string()),
            vec![],
        );
        variants.insert(
            (
                "demo".to_string(),
                "Status".to_string(),
                "Error".to_string(),
            ),
            vec![AS::NamePath {
                path: vec![AS::Ident("String".to_string())],
                type_args: vec![],
            }],
        );
        variants
    }

    #[test]
    fn test_exhaustive_bool_match() {
        let variants = HashMap::new();
        let ctx = ExhaustivenessCtx {
            enum_variants: &variants,
            current_pkg: Some("demo".to_string()),
        };

        let patterns = vec![Pat::bool_pat(true), Pat::bool_pat(false)];

        let result = check_exhaustiveness(&patterns, &Ty::Bool, &ctx);
        assert!(result.is_exhaustive);
        assert!(result.witnesses.is_empty());
    }

    #[test]
    fn test_non_exhaustive_bool_match() {
        let variants = HashMap::new();
        let ctx = ExhaustivenessCtx {
            enum_variants: &variants,
            current_pkg: Some("demo".to_string()),
        };

        let patterns = vec![Pat::bool_pat(true)];

        let result = check_exhaustiveness(&patterns, &Ty::Bool, &ctx);
        assert!(!result.is_exhaustive);
        assert_eq!(result.witnesses.len(), 1);
        assert_eq!(result.witnesses[0].description, "false");
    }

    #[test]
    fn test_wildcard_is_exhaustive() {
        let variants = HashMap::new();
        let ctx = ExhaustivenessCtx {
            enum_variants: &variants,
            current_pkg: Some("demo".to_string()),
        };

        let patterns = vec![Pat::wildcard()];

        let result = check_exhaustiveness(&patterns, &Ty::Bool, &ctx);
        assert!(result.is_exhaustive);
    }

    #[test]
    fn test_exhaustive_enum_match() {
        let variants = make_ctx();
        let ctx = ExhaustivenessCtx {
            enum_variants: &variants,
            current_pkg: Some("demo".to_string()),
        };

        let patterns = vec![
            Pat::variant(vec!["Status".to_string()], "Ok".to_string(), vec![]),
            Pat::variant(
                vec!["Status".to_string()],
                "Error".to_string(),
                vec![Pat::wildcard()],
            ),
        ];

        let scrutinee_ty = Ty::Named(vec!["Status".to_string()]);
        let result = check_exhaustiveness(&patterns, &scrutinee_ty, &ctx);
        assert!(result.is_exhaustive);
    }

    #[test]
    fn test_non_exhaustive_enum_match() {
        let variants = make_ctx();
        let ctx = ExhaustivenessCtx {
            enum_variants: &variants,
            current_pkg: Some("demo".to_string()),
        };

        let patterns = vec![Pat::variant(
            vec!["Status".to_string()],
            "Ok".to_string(),
            vec![],
        )];

        let scrutinee_ty = Ty::Named(vec!["Status".to_string()]);
        let result = check_exhaustiveness(&patterns, &scrutinee_ty, &ctx);
        assert!(!result.is_exhaustive);
        assert_eq!(result.witnesses.len(), 1);
        assert!(result.witnesses[0].description.contains("Error"));
    }

    #[test]
    fn test_redundant_pattern() {
        let variants = HashMap::new();
        let ctx = ExhaustivenessCtx {
            enum_variants: &variants,
            current_pkg: Some("demo".to_string()),
        };

        // First wildcard covers everything, second is redundant
        let patterns = vec![Pat::wildcard(), Pat::bool_pat(true)];

        let result = check_exhaustiveness(&patterns, &Ty::Bool, &ctx);
        assert!(result.is_exhaustive);
        assert_eq!(result.redundant_patterns, vec![1]);
    }

    #[test]
    fn test_duplicate_pattern_is_redundant() {
        let variants = HashMap::new();
        let ctx = ExhaustivenessCtx {
            enum_variants: &variants,
            current_pkg: Some("demo".to_string()),
        };

        // Same pattern twice
        let patterns = vec![
            Pat::bool_pat(true),
            Pat::bool_pat(true),
            Pat::bool_pat(false),
        ];

        let result = check_exhaustiveness(&patterns, &Ty::Bool, &ctx);
        assert!(result.is_exhaustive);
        assert_eq!(result.redundant_patterns, vec![1]); // Second 'true' is redundant
    }

    #[test]
    fn test_is_useful_handles_missing_tail_types_without_panicking() {
        let variants = HashMap::new();
        let ctx = ExhaustivenessCtx {
            enum_variants: &variants,
            current_pkg: Some("demo".to_string()),
        };

        // Malformed arity shape: row has more pattern columns than available type columns.
        // This can happen on invalid input in earlier phases and must not panic here.
        let matrix = PatternMatrix::new();
        let row = PatternRow::new(vec![Pat::wildcard(), Pat::wildcard()]);
        let result = std::panic::catch_unwind(|| is_useful(&matrix, &row, &[Ty::Int], &ctx));
        assert!(
            result.is_ok(),
            "is_useful panicked on malformed arity state"
        );
    }

    #[test]
    fn test_nested_payload_patterns_detect_non_exhaustive_inner_variants() {
        let mut variants = HashMap::new();
        // enum Inner { A, B }
        variants.insert(
            ("demo".to_string(), "Inner".to_string(), "A".to_string()),
            vec![],
        );
        variants.insert(
            ("demo".to_string(), "Inner".to_string(), "B".to_string()),
            vec![],
        );
        // enum Outer { Some(Inner), None }
        variants.insert(
            ("demo".to_string(), "Outer".to_string(), "Some".to_string()),
            vec![AS::NamePath::new(vec![AS::Ident("Inner".to_string())])],
        );
        variants.insert(
            ("demo".to_string(), "Outer".to_string(), "None".to_string()),
            vec![],
        );

        let ctx = ExhaustivenessCtx {
            enum_variants: &variants,
            current_pkg: Some("demo".to_string()),
        };

        let patterns = vec![
            Pat::variant(
                vec!["Outer".to_string()],
                "Some".to_string(),
                vec![Pat::variant(
                    vec!["Inner".to_string()],
                    "A".to_string(),
                    vec![],
                )],
            ),
            Pat::variant(vec!["Outer".to_string()], "None".to_string(), vec![]),
        ];
        let result = check_exhaustiveness(&patterns, &Ty::Named(vec!["Outer".to_string()]), &ctx);

        assert!(
            !result.is_exhaustive,
            "missing Outer.Some(Inner.B) should be detected as non-exhaustive"
        );
    }

    #[test]
    fn test_nested_payload_patterns_with_inner_wildcard_are_exhaustive() {
        let mut variants = HashMap::new();
        // enum Inner { A, B }
        variants.insert(
            ("demo".to_string(), "Inner".to_string(), "A".to_string()),
            vec![],
        );
        variants.insert(
            ("demo".to_string(), "Inner".to_string(), "B".to_string()),
            vec![],
        );
        // enum Outer { Some(Inner), None }
        variants.insert(
            ("demo".to_string(), "Outer".to_string(), "Some".to_string()),
            vec![AS::NamePath::new(vec![AS::Ident("Inner".to_string())])],
        );
        variants.insert(
            ("demo".to_string(), "Outer".to_string(), "None".to_string()),
            vec![],
        );

        let ctx = ExhaustivenessCtx {
            enum_variants: &variants,
            current_pkg: Some("demo".to_string()),
        };

        let patterns = vec![
            Pat::variant(
                vec!["Outer".to_string()],
                "Some".to_string(),
                vec![Pat::wildcard()],
            ),
            Pat::variant(vec!["Outer".to_string()], "None".to_string(), vec![]),
        ];
        let result = check_exhaustiveness(&patterns, &Ty::Named(vec!["Outer".to_string()]), &ctx);

        assert!(
            result.is_exhaustive,
            "Outer.Some(_) plus Outer.None should be exhaustive"
        );
    }
}
