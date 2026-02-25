// Definite Assignment Analysis for Arth
//
// This module implements path-sensitive dataflow analysis to ensure that all
// local variables are definitely initialized before use. Unlike the basic
// `initialized: bool` flag in LocalInfo, this analysis tracks initialization
// state across all control flow paths and enforces that variables are initialized
// on ALL paths before they can be used.
//
// Key concepts:
// - **Definitely initialized**: Variable is initialized on all paths reaching this point
// - **Possibly initialized**: Variable is initialized on some but not all paths
// - **Definitely uninitialized**: Variable is not initialized on any path
//
// Algorithm:
// 1. Track initialization state for each variable at each program point
// 2. At merge points (after if/else, loops), join the states from all incoming paths
// 3. Report error if a variable is used when not definitely initialized
//
// Join rules (for merging states from multiple paths):
// - definitely_init ∧ definitely_init = definitely_init
// - definitely_init ∧ not_init = possibly_init
// - possibly_init ∧ * = possibly_init
// - not_init ∧ not_init = not_init

use std::collections::{HashMap, HashSet};

/// Initialization state for a single variable
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitState {
    /// Variable is definitely initialized (on all control flow paths)
    DefinitelyInit,
    /// Variable is possibly initialized (on some but not all paths)
    PossiblyInit,
    /// Variable is definitely not initialized (on no paths)
    NotInit,
}

impl InitState {
    /// Join two initialization states from different control flow paths
    ///
    /// Examples:
    /// - DefinitelyInit ∧ DefinitelyInit = DefinitelyInit (both paths init)
    /// - DefinitelyInit ∧ NotInit = PossiblyInit (only one path inits)
    /// - PossiblyInit ∧ anything = PossiblyInit (some paths init)
    /// - NotInit ∧ NotInit = NotInit (neither path inits)
    pub fn join(self, other: Self) -> Self {
        use InitState::*;
        match (self, other) {
            (DefinitelyInit, DefinitelyInit) => DefinitelyInit,
            (NotInit, NotInit) => NotInit,
            _ => PossiblyInit, // Any other combination = possibly initialized
        }
    }

    /// Check if this state represents "definitely initialized"
    pub fn is_definitely_init(&self) -> bool {
        matches!(self, InitState::DefinitelyInit)
    }

    /// Check if this state represents "any initialization" (definitely or possibly)
    pub fn is_any_init(&self) -> bool {
        !matches!(self, InitState::NotInit)
    }
}

/// Tracks initialization state for all variables at a program point
#[derive(Debug, Clone)]
pub struct InitEnv {
    /// Map from variable name to its initialization state
    states: HashMap<String, InitState>,
}

impl InitEnv {
    /// Create a new empty initialization environment
    pub fn new() -> Self {
        InitEnv {
            states: HashMap::new(),
        }
    }

    /// Declare a new variable as uninitialized
    pub fn declare(&mut self, name: String) {
        self.states.insert(name, InitState::NotInit);
    }

    /// Declare a new variable as already initialized (for parameters, etc.)
    pub fn declare_initialized(&mut self, name: String) {
        self.states.insert(name, InitState::DefinitelyInit);
    }

    /// Mark a variable as definitely initialized (e.g., after assignment)
    pub fn initialize(&mut self, name: &str) {
        if let Some(state) = self.states.get_mut(name) {
            *state = InitState::DefinitelyInit;
        }
    }

    /// Get the initialization state of a variable
    pub fn get(&self, name: &str) -> Option<InitState> {
        self.states.get(name).copied()
    }

    /// Check if a variable is definitely initialized
    pub fn is_definitely_init(&self, name: &str) -> bool {
        self.get(name)
            .map(|s| s.is_definitely_init())
            .unwrap_or(false)
    }

    /// Check if a variable is declared (exists in the environment)
    pub fn is_declared(&self, name: &str) -> bool {
        self.states.contains_key(name)
    }

    /// Join this environment with another, creating a new environment
    /// where each variable's state is the join of states from both inputs.
    ///
    /// This is used at control flow merge points (e.g., after if/else branches).
    pub fn join(&self, other: &Self) -> Self {
        let mut result = InitEnv::new();

        // Collect all variables from both environments
        let all_vars: HashSet<&String> = self.states.keys().chain(other.states.keys()).collect();

        for var in all_vars {
            let state1 = self.states.get(var).copied().unwrap_or(InitState::NotInit);
            let state2 = other.states.get(var).copied().unwrap_or(InitState::NotInit);
            result.states.insert(var.clone(), state1.join(state2));
        }

        result
    }

    /// Join this environment with another in-place (mutates self)
    pub fn join_in_place(&mut self, other: &Self) {
        let all_vars: HashSet<String> = self
            .states
            .keys()
            .chain(other.states.keys())
            .cloned()
            .collect();

        for var in all_vars {
            let state1 = self.states.get(&var).copied().unwrap_or(InitState::NotInit);
            let state2 = other
                .states
                .get(&var)
                .copied()
                .unwrap_or(InitState::NotInit);
            self.states.insert(var, state1.join(state2));
        }
    }

    /// Get all variable names tracked in this environment
    pub fn all_vars(&self) -> Vec<String> {
        self.states.keys().cloned().collect()
    }

    /// Get variables that are not definitely initialized
    pub fn uninitialized_vars(&self) -> Vec<String> {
        self.states
            .iter()
            .filter(|(_, state)| !state.is_definitely_init())
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Invalidate a variable's initialization (e.g., after a move)
    pub fn uninitialize(&mut self, name: &str) {
        if let Some(state) = self.states.get_mut(name) {
            *state = InitState::NotInit;
        }
    }

    /// Remove a variable from tracking (when it goes out of scope)
    pub fn remove(&mut self, name: &str) {
        self.states.remove(name);
    }

    /// Merge initialization state from inner scope back to outer scope
    ///
    /// This is used when exiting a block scope to propagate initialization
    /// changes to the enclosing scope. Variables declared in the inner scope
    /// are not propagated (they're local to that scope).
    pub fn merge_from_inner(&mut self, inner: &Self, outer_vars: &HashSet<String>) {
        for var in outer_vars {
            if let Some(inner_state) = inner.states.get(var) {
                self.states.insert(var.clone(), *inner_state);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_state_join() {
        use InitState::*;

        // Definitely + Definitely = Definitely
        assert_eq!(DefinitelyInit.join(DefinitelyInit), DefinitelyInit);

        // Definitely + NotInit = Possibly
        assert_eq!(DefinitelyInit.join(NotInit), PossiblyInit);
        assert_eq!(NotInit.join(DefinitelyInit), PossiblyInit);

        // Possibly + anything = Possibly
        assert_eq!(PossiblyInit.join(DefinitelyInit), PossiblyInit);
        assert_eq!(PossiblyInit.join(NotInit), PossiblyInit);
        assert_eq!(PossiblyInit.join(PossiblyInit), PossiblyInit);

        // NotInit + NotInit = NotInit
        assert_eq!(NotInit.join(NotInit), NotInit);
    }

    #[test]
    fn test_init_env_basic() {
        let mut env = InitEnv::new();

        // Declare uninitialized variable
        env.declare("x".to_string());
        assert_eq!(env.get("x"), Some(InitState::NotInit));
        assert!(!env.is_definitely_init("x"));

        // Initialize variable
        env.initialize("x");
        assert_eq!(env.get("x"), Some(InitState::DefinitelyInit));
        assert!(env.is_definitely_init("x"));

        // Uninitialize (e.g., after move)
        env.uninitialize("x");
        assert_eq!(env.get("x"), Some(InitState::NotInit));
    }

    #[test]
    fn test_init_env_join_both_paths_init() {
        let mut then_env = InitEnv::new();
        then_env.declare("x".to_string());
        then_env.initialize("x");

        let mut else_env = InitEnv::new();
        else_env.declare("x".to_string());
        else_env.initialize("x");

        let merged = then_env.join(&else_env);
        assert_eq!(merged.get("x"), Some(InitState::DefinitelyInit));
    }

    #[test]
    fn test_init_env_join_one_path_init() {
        let mut then_env = InitEnv::new();
        then_env.declare("x".to_string());
        then_env.initialize("x");

        let mut else_env = InitEnv::new();
        else_env.declare("x".to_string());
        // else_env does NOT initialize x

        let merged = then_env.join(&else_env);
        assert_eq!(merged.get("x"), Some(InitState::PossiblyInit));
    }

    #[test]
    fn test_init_env_join_neither_path_init() {
        let mut then_env = InitEnv::new();
        then_env.declare("x".to_string());

        let mut else_env = InitEnv::new();
        else_env.declare("x".to_string());

        let merged = then_env.join(&else_env);
        assert_eq!(merged.get("x"), Some(InitState::NotInit));
    }

    #[test]
    fn test_declare_initialized() {
        let mut env = InitEnv::new();
        env.declare_initialized("param".to_string());
        assert!(env.is_definitely_init("param"));
    }

    #[test]
    fn test_uninitialized_vars() {
        let mut env = InitEnv::new();
        env.declare("x".to_string());
        env.declare("y".to_string());
        env.initialize("x");

        let uninit = env.uninitialized_vars();
        assert_eq!(uninit, vec!["y"]);
    }

    #[test]
    fn test_merge_from_inner_scope() {
        let mut outer = InitEnv::new();
        outer.declare("x".to_string());
        outer.declare("y".to_string());

        let mut inner = InitEnv::new();
        inner.declare("x".to_string());
        inner.initialize("x");
        inner.declare("z".to_string()); // Local to inner scope
        inner.initialize("z");

        let outer_vars: HashSet<String> =
            vec!["x".to_string(), "y".to_string()].into_iter().collect();
        outer.merge_from_inner(&inner, &outer_vars);

        // x should be initialized (merged from inner)
        assert!(outer.is_definitely_init("x"));
        // y should still be uninitialized
        assert!(!outer.is_definitely_init("y"));
        // z should NOT be in outer (it was local to inner)
        assert!(!outer.is_declared("z"));
    }
}
