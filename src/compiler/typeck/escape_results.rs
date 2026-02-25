// Escape Analysis Results
//
// This module provides data structures for passing escape analysis results
// from the type checker to the HIR-to-IR lowering phase.

use std::collections::{HashMap, HashSet};

/// Move state for a local variable at the end of its scope.
/// Used to determine drop behavior at scope exit.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum MoveState {
    /// Variable is fully available (not moved on any path)
    #[default]
    Available,
    /// Variable is definitely fully moved on all paths
    FullyMoved,
    /// Variable may or may not be moved depending on control flow path
    ConditionallyMoved,
    /// Some fields are moved, others are available.
    /// The set contains the names of moved fields.
    PartiallyMoved(HashSet<String>),
}

/// Drop information for a single struct field.
/// Used for per-field drop tracking in partial moves.
#[derive(Clone, Debug, Default)]
pub struct FieldDropInfo {
    /// Whether this field needs drop
    pub needs_drop: bool,
    /// Type name for drop resolution
    pub drop_ty_name: Option<String>,
}

/// Allocation strategy for a variable (mirrors lifetime.rs AllocStrategy)
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum AllocStrategy {
    /// Stack allocation - value doesn't escape, deterministic drop
    #[default]
    Stack,
    /// Reference counted - value escapes, needs RC for cleanup
    RefCounted,
    /// Unique ownership - single owner, move semantics, deterministic drop
    UniqueOwned,
    /// Region allocation - value lives for a known region
    Region(u32),
}

/// Escape information for a single local variable
#[derive(Clone, Debug)]
pub struct LocalEscapeInfo {
    /// The allocation strategy for this local
    pub alloc_strategy: AllocStrategy,
    /// Whether this local needs drop/cleanup
    pub needs_drop: bool,
    /// Type name for drop resolution (if needs_drop is true)
    pub drop_ty_name: Option<String>,
    /// Move state at end of scope - determines drop strategy
    pub move_state: MoveState,
    /// Per-field drop info for partial moves (field name -> drop info)
    pub field_drop_info: HashMap<String, FieldDropInfo>,
    /// Whether this type has an explicit deinit function.
    /// If true, partial moves are not allowed (deinit might access moved fields).
    /// If false, partial moves are allowed and we drop unmoved fields individually.
    pub has_explicit_deinit: bool,
}

impl Default for LocalEscapeInfo {
    fn default() -> Self {
        LocalEscapeInfo {
            alloc_strategy: AllocStrategy::Stack,
            needs_drop: false,
            drop_ty_name: None,
            move_state: MoveState::Available,
            field_drop_info: HashMap::new(),
            has_explicit_deinit: false,
        }
    }
}

/// Escape analysis results for a single function
#[derive(Clone, Debug, Default)]
pub struct FunctionEscapeInfo {
    /// Per-local escape information: variable name -> escape info
    pub locals: HashMap<String, LocalEscapeInfo>,
}

impl FunctionEscapeInfo {
    pub fn new() -> Self {
        FunctionEscapeInfo {
            locals: HashMap::new(),
        }
    }

    /// Add escape info for a local variable
    pub fn add_local(&mut self, name: &str, info: LocalEscapeInfo) {
        self.locals.insert(name.to_string(), info);
    }

    /// Get escape info for a local variable
    pub fn get_local(&self, name: &str) -> Option<&LocalEscapeInfo> {
        self.locals.get(name)
    }

    /// Get allocation strategy for a local (defaults to Stack if not found)
    pub fn get_alloc_strategy(&self, name: &str) -> AllocStrategy {
        self.locals
            .get(name)
            .map(|info| info.alloc_strategy.clone())
            .unwrap_or(AllocStrategy::Stack)
    }
}

/// Key for identifying a function: (package, module_name, function_name)
/// Module name is None for free functions
pub type FunctionKey = (String, Option<String>, String);

/// Escape analysis results for an entire project
#[derive(Clone, Debug, Default)]
pub struct EscapeAnalysisResults {
    /// Per-function escape information
    pub functions: HashMap<FunctionKey, FunctionEscapeInfo>,
}

impl EscapeAnalysisResults {
    pub fn new() -> Self {
        EscapeAnalysisResults {
            functions: HashMap::new(),
        }
    }

    /// Add escape info for a function
    pub fn add_function(&mut self, key: FunctionKey, info: FunctionEscapeInfo) {
        self.functions.insert(key, info);
    }

    /// Get escape info for a function
    pub fn get_function(&self, key: &FunctionKey) -> Option<&FunctionEscapeInfo> {
        self.functions.get(key)
    }

    /// Get escape info for a function by components
    pub fn get_function_by_parts(
        &self,
        pkg: &str,
        module: Option<&str>,
        func_name: &str,
    ) -> Option<&FunctionEscapeInfo> {
        let key = (
            pkg.to_string(),
            module.map(|s| s.to_string()),
            func_name.to_string(),
        );
        self.functions.get(&key)
    }

    /// Get allocation strategy for a local in a function
    pub fn get_local_alloc_strategy(
        &self,
        pkg: &str,
        module: Option<&str>,
        func_name: &str,
        local_name: &str,
    ) -> AllocStrategy {
        self.get_function_by_parts(pkg, module, func_name)
            .map(|f| f.get_alloc_strategy(local_name))
            .unwrap_or(AllocStrategy::Stack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_results_basic() {
        let mut results = EscapeAnalysisResults::new();

        let mut func_info = FunctionEscapeInfo::new();
        func_info.add_local(
            "x",
            LocalEscapeInfo {
                alloc_strategy: AllocStrategy::Stack,
                needs_drop: false,
                drop_ty_name: None,
                move_state: MoveState::Available,
                field_drop_info: HashMap::new(),
                has_explicit_deinit: false,
            },
        );
        func_info.add_local(
            "y",
            LocalEscapeInfo {
                alloc_strategy: AllocStrategy::RefCounted,
                needs_drop: true,
                drop_ty_name: Some("MyType".to_string()),
                move_state: MoveState::Available,
                field_drop_info: HashMap::new(),
                has_explicit_deinit: true,
            },
        );

        results.add_function(
            (
                "pkg".to_string(),
                Some("Module".to_string()),
                "func".to_string(),
            ),
            func_info,
        );

        // Test retrieval
        assert_eq!(
            results.get_local_alloc_strategy("pkg", Some("Module"), "func", "x"),
            AllocStrategy::Stack
        );
        assert_eq!(
            results.get_local_alloc_strategy("pkg", Some("Module"), "func", "y"),
            AllocStrategy::RefCounted
        );
        // Unknown local defaults to Stack
        assert_eq!(
            results.get_local_alloc_strategy("pkg", Some("Module"), "func", "z"),
            AllocStrategy::Stack
        );
    }
}
