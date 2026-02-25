//! Generic Constraint Solver for Arth
//!
//! This module implements a constraint-based type inference system for generic types.
//! It handles:
//! - Type variable unification with occurs check
//! - Bound satisfaction checking (e.g., T extends Comparable)
//! - Constraint collection and solving
//! - Error reporting for unsatisfiable constraints

use super::{Ty, same_type};
use crate::compiler::ast as AS;
use std::collections::HashMap;

/// A type constraint that must be satisfied
#[derive(Clone, Debug)]
pub(super) enum Constraint {
    /// Type equality: lhs = rhs
    Eq(Ty, Ty),
    /// Type parameter bound: type_param must satisfy bound
    Bound {
        type_param: String,
        inferred_type: Ty,
        bound: BoundInfo,
    },
}

/// Information about a type bound
#[derive(Clone, Debug)]
pub(super) struct BoundInfo {
    /// The bound interface/trait path
    pub(super) path: Vec<String>,
    /// Type arguments to the bound (e.g., Comparable<T> has T as arg)
    pub(super) type_args: Vec<Ty>,
}

impl BoundInfo {
    pub(super) fn from_namepath(np: &AS::NamePath) -> Self {
        BoundInfo {
            path: np.path.iter().map(|i| i.0.clone()).collect(),
            type_args: np.type_args.iter().map(super::map_name_to_ty).collect(),
        }
    }

    pub(super) fn to_ty(&self) -> Ty {
        if self.type_args.is_empty() {
            Ty::Named(self.path.clone())
        } else {
            Ty::Generic {
                path: self.path.clone(),
                args: self.type_args.clone(),
            }
        }
    }

    pub(super) fn display(&self) -> String {
        if self.type_args.is_empty() {
            self.path.join(".")
        } else {
            format!(
                "{}<{}>",
                self.path.join("."),
                self.type_args
                    .iter()
                    .map(|t| format!("{}", t))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

/// Result of constraint solving
#[derive(Debug)]
pub(super) enum SolveResult {
    /// Successfully solved with substitution map
    Success(HashMap<String, Ty>),
    /// Failed with error messages
    Failure(Vec<ConstraintError>),
}

/// Error during constraint solving
#[derive(Debug, Clone)]
pub(super) enum ConstraintError {
    /// Type mismatch
    TypeMismatch {
        expected: Ty,
        found: Ty,
        context: String,
    },
    /// Occurs check failure (infinite type)
    OccursCheck { type_param: String, in_type: Ty },
    /// Bound not satisfied
    BoundNotSatisfied {
        type_param: String,
        inferred_type: Ty,
        bound: String,
    },
    /// Inconsistent bindings for same type parameter
    InconsistentBinding {
        type_param: String,
        first: Ty,
        second: Ty,
    },
}

impl std::fmt::Display for ConstraintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConstraintError::TypeMismatch {
                expected,
                found,
                context,
            } => write!(
                f,
                "type mismatch: expected '{}', found '{}' ({})",
                expected, found, context
            ),
            ConstraintError::OccursCheck {
                type_param,
                in_type,
            } => write!(
                f,
                "infinite type: type parameter '{}' occurs in type '{}'",
                type_param, in_type
            ),
            ConstraintError::BoundNotSatisfied {
                type_param,
                inferred_type,
                bound,
            } => write!(
                f,
                "type '{}' for generic parameter '{}' does not satisfy bound '{}'",
                inferred_type, type_param, bound
            ),
            ConstraintError::InconsistentBinding {
                type_param,
                first,
                second,
            } => write!(
                f,
                "inconsistent types for '{}': inferred both '{}' and '{}'",
                type_param, first, second
            ),
        }
    }
}

/// The constraint solver
pub(super) struct ConstraintSolver {
    /// Type parameters in scope with their optional bounds
    type_params: HashMap<String, Option<BoundInfo>>,
    /// Collected constraints
    constraints: Vec<Constraint>,
    /// Current substitution map (type param -> concrete type)
    substitution: HashMap<String, Ty>,
    /// Errors encountered during solving
    errors: Vec<ConstraintError>,
}

impl ConstraintSolver {
    /// Create a new constraint solver with the given type parameters
    pub(super) fn new(generics: &[AS::GenericParam]) -> Self {
        let mut type_params = HashMap::new();
        for gp in generics {
            let bound = gp.bound.as_ref().map(BoundInfo::from_namepath);
            type_params.insert(gp.name.0.clone(), bound);
        }
        ConstraintSolver {
            type_params,
            constraints: Vec::new(),
            substitution: HashMap::new(),
            errors: Vec::new(),
        }
    }

    /// Create a solver from type parameter names (for simpler cases)
    fn from_names(names: &[String]) -> Self {
        let mut type_params = HashMap::new();
        for name in names {
            type_params.insert(name.clone(), None);
        }
        ConstraintSolver {
            type_params,
            constraints: Vec::new(),
            substitution: HashMap::new(),
            errors: Vec::new(),
        }
    }

    /// Add a type parameter with optional bound
    #[allow(dead_code)]
    pub(super) fn add_type_param(&mut self, name: String, bound: Option<BoundInfo>) {
        self.type_params.insert(name, bound);
    }

    /// Check if a name is a type parameter
    fn is_type_param(&self, name: &str) -> bool {
        self.type_params.contains_key(name)
    }

    /// Get the type parameter names
    #[allow(dead_code)]
    fn type_param_names(&self) -> Vec<String> {
        self.type_params.keys().cloned().collect()
    }

    /// Add an equality constraint
    #[allow(dead_code)]
    fn add_eq(&mut self, lhs: Ty, rhs: Ty) {
        self.constraints.push(Constraint::Eq(lhs, rhs));
    }

    /// Try to unify two types, updating the substitution map
    /// Returns true if unification succeeds
    pub(super) fn unify(&mut self, lhs: &Ty, rhs: &Ty) -> bool {
        // Apply current substitution first
        let lhs = self.apply_subst(lhs);
        let rhs = self.apply_subst(rhs);

        // Unknown matches anything
        if matches!(lhs, Ty::Unknown) || matches!(rhs, Ty::Unknown) {
            return true;
        }

        // Never is a subtype of everything
        if matches!(lhs, Ty::Never) || matches!(rhs, Ty::Never) {
            return true;
        }

        // Check if either side is a type parameter
        if let Ty::Named(ref path) = lhs {
            if path.len() == 1 && self.is_type_param(&path[0]) {
                return self.bind_type_param(&path[0], &rhs);
            }
        }
        if let Ty::Named(ref path) = rhs {
            if path.len() == 1 && self.is_type_param(&path[0]) {
                return self.bind_type_param(&path[0], &lhs);
            }
        }

        // Structural unification
        match (&lhs, &rhs) {
            (Ty::Int, Ty::Int)
            | (Ty::Float, Ty::Float)
            | (Ty::Bool, Ty::Bool)
            | (Ty::String, Ty::String)
            | (Ty::Char, Ty::Char)
            | (Ty::Bytes, Ty::Bytes)
            | (Ty::Void, Ty::Void) => true,

            (Ty::Named(a), Ty::Named(b)) => a == b,

            (Ty::Generic { path: pa, args: aa }, Ty::Generic { path: pb, args: ab }) => {
                if pa != pb || aa.len() != ab.len() {
                    return false;
                }
                for (a, b) in aa.iter().zip(ab.iter()) {
                    if !self.unify(a, b) {
                        return false;
                    }
                }
                true
            }

            // Generic with no args can match Named
            (Ty::Generic { path, args }, Ty::Named(other))
            | (Ty::Named(other), Ty::Generic { path, args }) => args.is_empty() && path == other,

            (Ty::Function(pa, ra), Ty::Function(pb, rb)) => {
                if pa.len() != pb.len() {
                    return false;
                }
                if !self.unify(ra, rb) {
                    return false;
                }
                for (a, b) in pa.iter().zip(pb.iter()) {
                    if !self.unify(a, b) {
                        return false;
                    }
                }
                true
            }

            (Ty::Tuple(ea), Ty::Tuple(eb)) => {
                if ea.len() != eb.len() {
                    return false;
                }
                for (a, b) in ea.iter().zip(eb.iter()) {
                    if !self.unify(a, b) {
                        return false;
                    }
                }
                true
            }

            _ => false,
        }
    }

    /// Bind a type parameter to a type, with occurs check
    fn bind_type_param(&mut self, param: &str, ty: &Ty) -> bool {
        // Check for existing binding
        if let Some(existing) = self.substitution.get(param) {
            // Already bound - check consistency
            return same_type(existing, ty);
        }

        // Occurs check: param should not appear in ty
        if self.occurs_in(param, ty) {
            self.errors.push(ConstraintError::OccursCheck {
                type_param: param.to_string(),
                in_type: ty.clone(),
            });
            return false;
        }

        // Bind the type parameter
        self.substitution.insert(param.to_string(), ty.clone());
        true
    }

    /// Check if a type parameter occurs in a type (for occurs check)
    fn occurs_in(&self, param: &str, ty: &Ty) -> bool {
        match ty {
            Ty::Named(path) => path.len() == 1 && path[0] == param,
            Ty::Generic { args, .. } => args.iter().any(|a| self.occurs_in(param, a)),
            Ty::Function(params, ret) => {
                params.iter().any(|p| self.occurs_in(param, p)) || self.occurs_in(param, ret)
            }
            Ty::Tuple(elems) => elems.iter().any(|e| self.occurs_in(param, e)),
            _ => false,
        }
    }

    /// Apply the current substitution to a type
    fn apply_subst(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Named(path) => {
                if path.len() == 1 {
                    if let Some(bound) = self.substitution.get(&path[0]) {
                        return self.apply_subst(bound);
                    }
                }
                ty.clone()
            }
            Ty::Generic { path, args } => {
                // Check if the path itself is a type param (rare but possible)
                if path.len() == 1 {
                    if let Some(bound) = self.substitution.get(&path[0]) {
                        // If bound is also generic, merge the args
                        if let Ty::Generic { path: bp, args: ba } = bound {
                            let new_args: Vec<Ty> = if ba.is_empty() {
                                args.iter().map(|a| self.apply_subst(a)).collect()
                            } else {
                                ba.iter().map(|a| self.apply_subst(a)).collect()
                            };
                            return Ty::Generic {
                                path: bp.clone(),
                                args: new_args,
                            };
                        }
                        return self.apply_subst(bound);
                    }
                }
                let new_args: Vec<Ty> = args.iter().map(|a| self.apply_subst(a)).collect();
                Ty::Generic {
                    path: path.clone(),
                    args: new_args,
                }
            }
            Ty::Function(params, ret) => {
                let new_params: Vec<Ty> = params.iter().map(|p| self.apply_subst(p)).collect();
                let new_ret = Box::new(self.apply_subst(ret));
                Ty::Function(new_params, new_ret)
            }
            Ty::Tuple(elems) => {
                let new_elems: Vec<Ty> = elems.iter().map(|e| self.apply_subst(e)).collect();
                Ty::Tuple(new_elems)
            }
            _ => ty.clone(),
        }
    }

    /// Solve all collected constraints
    #[allow(dead_code)]
    fn solve(&mut self) -> bool {
        for constraint in self.constraints.clone() {
            match constraint {
                Constraint::Eq(lhs, rhs) => {
                    if !self.unify(&lhs, &rhs) {
                        self.errors.push(ConstraintError::TypeMismatch {
                            expected: lhs,
                            found: rhs,
                            context: "type parameter inference".to_string(),
                        });
                    }
                }
                Constraint::Bound {
                    type_param,
                    inferred_type,
                    bound,
                } => {
                    // Bound checking is done separately after unification
                    // Store for later checking
                    self.constraints.push(Constraint::Bound {
                        type_param,
                        inferred_type: self.apply_subst(&inferred_type),
                        bound,
                    });
                }
            }
        }
        self.errors.is_empty()
    }

    /// Check all bound constraints
    /// This requires access to the implements index, so it's called separately
    pub(super) fn check_bounds<F>(&mut self, satisfies: F) -> bool
    where
        F: Fn(&Ty, &BoundInfo) -> bool,
    {
        for (param_name, bound_opt) in &self.type_params.clone() {
            if let Some(bound) = bound_opt {
                if let Some(inferred) = self.substitution.get(param_name) {
                    let inferred = self.apply_subst(inferred);
                    if !satisfies(&inferred, bound) {
                        self.errors.push(ConstraintError::BoundNotSatisfied {
                            type_param: param_name.clone(),
                            inferred_type: inferred,
                            bound: bound.display(),
                        });
                    }
                }
            }
        }
        self.errors.is_empty()
    }

    /// Get the current substitution map
    #[allow(dead_code)]
    fn get_substitution(&self) -> &HashMap<String, Ty> {
        &self.substitution
    }

    /// Take the substitution map (consumes the solver)
    pub(super) fn into_substitution(self) -> HashMap<String, Ty> {
        self.substitution
    }

    /// Get collected errors
    fn errors(&self) -> &[ConstraintError] {
        &self.errors
    }

    /// Take errors (consumes them)
    pub(super) fn take_errors(&mut self) -> Vec<ConstraintError> {
        std::mem::take(&mut self.errors)
    }

    /// Check if there are any errors
    pub(super) fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Get the final result
    #[allow(dead_code)]
    fn result(self) -> SolveResult {
        if self.errors.is_empty() {
            SolveResult::Success(self.substitution)
        } else {
            SolveResult::Failure(self.errors)
        }
    }
}

/// Infer type arguments for a generic function call
/// Returns the substitution map if successful, or error messages
#[allow(dead_code)]
pub(super) fn infer_type_args(
    generics: &[AS::GenericParam],
    param_types: &[Ty],
    arg_types: &[Ty],
) -> Result<HashMap<String, Ty>, Vec<ConstraintError>> {
    let mut solver = ConstraintSolver::new(generics);

    // Add equality constraints for each parameter-argument pair
    for (param_ty, arg_ty) in param_types.iter().zip(arg_types.iter()) {
        if !solver.unify(param_ty, arg_ty) {
            // Error already recorded in solver
        }
    }

    if solver.has_errors() {
        Err(solver.take_errors())
    } else {
        Ok(solver.into_substitution())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_unification() {
        let mut solver = ConstraintSolver::from_names(&["T".to_string()]);

        // T = Int
        let param = Ty::Named(vec!["T".to_string()]);
        let arg = Ty::Int;

        assert!(solver.unify(&param, &arg));
        assert_eq!(solver.substitution.get("T"), Some(&Ty::Int));
    }

    #[test]
    fn test_consistent_bindings() {
        let mut solver = ConstraintSolver::from_names(&["T".to_string()]);

        // First: T = Int
        let param = Ty::Named(vec!["T".to_string()]);
        assert!(solver.unify(&param, &Ty::Int));

        // Second: T = Int (same type - should succeed)
        assert!(solver.unify(&param, &Ty::Int));
    }

    #[test]
    fn test_inconsistent_bindings() {
        let mut solver = ConstraintSolver::from_names(&["T".to_string()]);

        // First: T = Int
        let param = Ty::Named(vec!["T".to_string()]);
        assert!(solver.unify(&param, &Ty::Int));

        // Second: T = String (different type - should fail)
        assert!(!solver.unify(&param, &Ty::String));
    }

    #[test]
    fn test_generic_unification() {
        let mut solver = ConstraintSolver::from_names(&["T".to_string()]);

        // List<T> = List<Int>
        let param = Ty::Generic {
            path: vec!["List".to_string()],
            args: vec![Ty::Named(vec!["T".to_string()])],
        };
        let arg = Ty::Generic {
            path: vec!["List".to_string()],
            args: vec![Ty::Int],
        };

        assert!(solver.unify(&param, &arg));
        assert_eq!(solver.substitution.get("T"), Some(&Ty::Int));
    }

    #[test]
    fn test_nested_generic_unification() {
        let mut solver = ConstraintSolver::from_names(&["T".to_string()]);

        // Map<String, T> = Map<String, List<Int>>
        let param = Ty::Generic {
            path: vec!["Map".to_string()],
            args: vec![Ty::String, Ty::Named(vec!["T".to_string()])],
        };
        let arg = Ty::Generic {
            path: vec!["Map".to_string()],
            args: vec![
                Ty::String,
                Ty::Generic {
                    path: vec!["List".to_string()],
                    args: vec![Ty::Int],
                },
            ],
        };

        assert!(solver.unify(&param, &arg));

        let expected = Ty::Generic {
            path: vec!["List".to_string()],
            args: vec![Ty::Int],
        };
        assert_eq!(solver.substitution.get("T"), Some(&expected));
    }

    #[test]
    fn test_occurs_check() {
        let mut solver = ConstraintSolver::from_names(&["T".to_string()]);

        // T = List<T> should fail (infinite type)
        let param = Ty::Named(vec!["T".to_string()]);
        let arg = Ty::Generic {
            path: vec!["List".to_string()],
            args: vec![Ty::Named(vec!["T".to_string()])],
        };

        assert!(!solver.unify(&param, &arg));
        assert!(solver.has_errors());
        assert!(matches!(
            &solver.errors()[0],
            ConstraintError::OccursCheck { .. }
        ));
    }

    #[test]
    fn test_multiple_type_params() {
        let mut solver = ConstraintSolver::from_names(&["K".to_string(), "V".to_string()]);

        // Map<K, V> = Map<String, Int>
        let param = Ty::Generic {
            path: vec!["Map".to_string()],
            args: vec![
                Ty::Named(vec!["K".to_string()]),
                Ty::Named(vec!["V".to_string()]),
            ],
        };
        let arg = Ty::Generic {
            path: vec!["Map".to_string()],
            args: vec![Ty::String, Ty::Int],
        };

        assert!(solver.unify(&param, &arg));
        assert_eq!(solver.substitution.get("K"), Some(&Ty::String));
        assert_eq!(solver.substitution.get("V"), Some(&Ty::Int));
    }

    #[test]
    fn test_apply_substitution() {
        let mut solver = ConstraintSolver::from_names(&["T".to_string()]);
        solver.substitution.insert("T".to_string(), Ty::Int);

        // Apply to List<T> should give List<Int>
        let ty = Ty::Generic {
            path: vec!["List".to_string()],
            args: vec![Ty::Named(vec!["T".to_string()])],
        };
        let result = solver.apply_subst(&ty);

        let expected = Ty::Generic {
            path: vec!["List".to_string()],
            args: vec![Ty::Int],
        };
        assert_eq!(result, expected);
    }

    #[test]
    fn test_function_type_unification() {
        let mut solver = ConstraintSolver::from_names(&["T".to_string()]);

        // (T) -> T = (Int) -> Int
        let param = Ty::Function(
            vec![Ty::Named(vec!["T".to_string()])],
            Box::new(Ty::Named(vec!["T".to_string()])),
        );
        let arg = Ty::Function(vec![Ty::Int], Box::new(Ty::Int));

        assert!(solver.unify(&param, &arg));
        assert_eq!(solver.substitution.get("T"), Some(&Ty::Int));
    }

    #[test]
    fn test_tuple_unification() {
        let mut solver = ConstraintSolver::from_names(&["T".to_string(), "U".to_string()]);

        // (T, U) = (Int, String)
        let param = Ty::Tuple(vec![
            Ty::Named(vec!["T".to_string()]),
            Ty::Named(vec!["U".to_string()]),
        ]);
        let arg = Ty::Tuple(vec![Ty::Int, Ty::String]);

        assert!(solver.unify(&param, &arg));
        assert_eq!(solver.substitution.get("T"), Some(&Ty::Int));
        assert_eq!(solver.substitution.get("U"), Some(&Ty::String));
    }

    #[test]
    fn test_unknown_unifies_with_anything() {
        let mut solver = ConstraintSolver::from_names(&["T".to_string()]);

        // T = Unknown should succeed (no binding)
        let param = Ty::Named(vec!["T".to_string()]);
        assert!(solver.unify(&param, &Ty::Unknown));

        // Unknown = Int should also succeed
        assert!(solver.unify(&Ty::Unknown, &Ty::Int));
    }

    #[test]
    fn test_never_unifies_with_anything() {
        let mut solver = ConstraintSolver::from_names(&[]);

        // Never = Int should succeed
        assert!(solver.unify(&Ty::Never, &Ty::Int));

        // String = Never should also succeed
        assert!(solver.unify(&Ty::String, &Ty::Never));
    }
}
