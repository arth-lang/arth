//! Lint registry for the Arth compiler.
//!
//! Defines all available lints with their default severity levels,
//! descriptions, and configuration.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Unique identifier for each lint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LintId {
    /// Variable declared but never used.
    UnusedVariable,
    /// Import statement that is never referenced.
    UnusedImport,
    /// Result of @must_use function is discarded.
    UnusedResult,
    /// Async function with no await points.
    AsyncWithoutAwait,
    /// Code that will never be executed.
    UnreachableCode,
    /// Unnecessary clone on Copy type.
    NeedlessClone,
    /// Usage of deprecated item.
    DeprecatedUsage,
    /// Naming convention violation.
    NamingConvention,
}

impl LintId {
    /// Get the string name of this lint.
    pub fn name(self) -> &'static str {
        match self {
            LintId::UnusedVariable => "unused-variable",
            LintId::UnusedImport => "unused-import",
            LintId::UnusedResult => "unused-result",
            LintId::AsyncWithoutAwait => "async-without-await",
            LintId::UnreachableCode => "unreachable-code",
            LintId::NeedlessClone => "needless-clone",
            LintId::DeprecatedUsage => "deprecated-usage",
            LintId::NamingConvention => "naming-convention",
        }
    }

    /// Get the error code for this lint.
    pub fn code(self) -> &'static str {
        match self {
            LintId::UnusedVariable => "W0001",
            LintId::UnusedImport => "W0002",
            LintId::UnusedResult => "W0003",
            LintId::AsyncWithoutAwait => "W0004",
            LintId::UnreachableCode => "W0005",
            LintId::NeedlessClone => "W0006",
            LintId::DeprecatedUsage => "W1001",
            LintId::NamingConvention => "W0007",
        }
    }

    /// Parse a lint name string to a LintId.
    pub fn from_name(name: &str) -> Option<LintId> {
        match name {
            "unused-variable" | "unused_variable" => Some(LintId::UnusedVariable),
            "unused-import" | "unused_import" => Some(LintId::UnusedImport),
            "unused-result" | "unused_result" => Some(LintId::UnusedResult),
            "async-without-await" | "async_without_await" => Some(LintId::AsyncWithoutAwait),
            "unreachable-code" | "unreachable_code" => Some(LintId::UnreachableCode),
            "needless-clone" | "needless_clone" => Some(LintId::NeedlessClone),
            "deprecated-usage" | "deprecated_usage" => Some(LintId::DeprecatedUsage),
            "deprecated" => Some(LintId::DeprecatedUsage),
            "naming-convention" | "naming_convention" => Some(LintId::NamingConvention),
            _ => None,
        }
    }

    /// Get all lint IDs.
    pub fn all() -> &'static [LintId] {
        &[
            LintId::UnusedVariable,
            LintId::UnusedImport,
            LintId::UnusedResult,
            LintId::AsyncWithoutAwait,
            LintId::UnreachableCode,
            LintId::NeedlessClone,
            LintId::DeprecatedUsage,
            LintId::NamingConvention,
        ]
    }
}

/// Default severity level for lints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LintLevel {
    /// Lint is completely disabled.
    Allow,
    /// Lint emits a warning.
    Warn,
    /// Lint emits an error (compilation fails).
    Deny,
}

impl std::str::FromStr for LintLevel {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "allow" => Ok(LintLevel::Allow),
            "warn" => Ok(LintLevel::Warn),
            "deny" => Ok(LintLevel::Deny),
            _ => Err(()),
        }
    }
}

/// Definition of a lint.
#[derive(Clone, Debug)]
pub struct LintDef {
    /// Unique identifier.
    pub id: LintId,
    /// Default severity level.
    pub default_level: LintLevel,
    /// Short description.
    pub description: &'static str,
    /// Detailed explanation of the lint and how to fix violations.
    pub explanation: &'static str,
}

impl LintDef {
    const fn new(
        id: LintId,
        default_level: LintLevel,
        description: &'static str,
        explanation: &'static str,
    ) -> Self {
        Self {
            id,
            default_level,
            description,
            explanation,
        }
    }
}

// Lint definitions
pub const UNUSED_VARIABLE: LintDef = LintDef::new(
    LintId::UnusedVariable,
    LintLevel::Warn,
    "variable is declared but never used",
    "This variable is declared but its value is never read. Consider removing the variable \
     or prefixing its name with an underscore (_) to suppress this warning.",
);

pub const UNUSED_IMPORT: LintDef = LintDef::new(
    LintId::UnusedImport,
    LintLevel::Warn,
    "import is never used",
    "This import statement brings in a symbol that is never referenced in the code. \
     Consider removing the unused import to keep the code clean.",
);

pub const UNUSED_RESULT: LintDef = LintDef::new(
    LintId::UnusedResult,
    LintLevel::Warn,
    "result of @must_use function is discarded",
    "The return value of a function marked with @must_use is being discarded. \
     This may indicate a bug—the result should typically be used or explicitly ignored.",
);

pub const ASYNC_WITHOUT_AWAIT: LintDef = LintDef::new(
    LintId::AsyncWithoutAwait,
    LintLevel::Warn,
    "async function has no await points",
    "This function is marked as async but does not contain any await expressions. \
     Consider removing the async modifier or adding await points.",
);

pub const UNREACHABLE_CODE: LintDef = LintDef::new(
    LintId::UnreachableCode,
    LintLevel::Warn,
    "code will never be executed",
    "This code is unreachable because control flow never reaches this point. \
     This often indicates dead code that should be removed.",
);

pub const NEEDLESS_CLONE: LintDef = LintDef::new(
    LintId::NeedlessClone,
    LintLevel::Warn,
    "unnecessary clone",
    "This clone is unnecessary because the type is Copy or the value is only used once. \
     Consider removing the clone call.",
);

pub const DEPRECATED_USAGE: LintDef = LintDef::new(
    LintId::DeprecatedUsage,
    LintLevel::Warn,
    "use of deprecated item",
    "This item is marked as deprecated and may be removed in a future version. \
     Check the deprecation note for migration guidance.",
);

pub const NAMING_CONVENTION: LintDef = LintDef::new(
    LintId::NamingConvention,
    LintLevel::Warn,
    "naming convention violation",
    "The name does not follow the recommended naming convention. \
     Types should use UpperCamelCase, functions and variables should use lowerCamelCase.",
);

/// Global lint registry.
pub static LINT_REGISTRY: LazyLock<HashMap<LintId, &'static LintDef>> = LazyLock::new(|| {
    let defs: &[&LintDef] = &[
        &UNUSED_VARIABLE,
        &UNUSED_IMPORT,
        &UNUSED_RESULT,
        &ASYNC_WITHOUT_AWAIT,
        &UNREACHABLE_CODE,
        &NEEDLESS_CLONE,
        &DEPRECATED_USAGE,
        &NAMING_CONVENTION,
    ];
    let mut map = HashMap::with_capacity(defs.len());
    for def in defs {
        map.insert(def.id, *def);
    }
    map
});

/// Get the definition for a lint.
pub fn get_lint(id: LintId) -> &'static LintDef {
    LINT_REGISTRY
        .get(&id)
        .expect("internal error: lint ID not found in registry (this is a compiler bug)")
}

/// Get all lint definitions.
pub fn all_lints() -> Vec<&'static LintDef> {
    LINT_REGISTRY.values().copied().collect()
}

/// Lookup a lint by name.
pub fn lookup_lint(name: &str) -> Option<&'static LintDef> {
    LintId::from_name(name).and_then(|id| LINT_REGISTRY.get(&id).copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lint_id_name() {
        assert_eq!(LintId::UnusedVariable.name(), "unused-variable");
        assert_eq!(LintId::AsyncWithoutAwait.name(), "async-without-await");
    }

    #[test]
    fn test_lint_id_code() {
        assert_eq!(LintId::UnusedVariable.code(), "W0001");
        assert_eq!(LintId::DeprecatedUsage.code(), "W1001");
    }

    #[test]
    fn test_lint_id_from_name() {
        assert_eq!(
            LintId::from_name("unused-variable"),
            Some(LintId::UnusedVariable)
        );
        assert_eq!(
            LintId::from_name("unused_variable"),
            Some(LintId::UnusedVariable)
        );
        assert_eq!(
            LintId::from_name("deprecated"),
            Some(LintId::DeprecatedUsage)
        );
        assert_eq!(LintId::from_name("nonexistent"), None);
    }

    #[test]
    fn test_lint_level_from_str() {
        use std::str::FromStr;
        assert_eq!(LintLevel::from_str("allow"), Ok(LintLevel::Allow));
        assert_eq!(LintLevel::from_str("WARN"), Ok(LintLevel::Warn));
        assert_eq!(LintLevel::from_str("Deny"), Ok(LintLevel::Deny));
        assert!(LintLevel::from_str("invalid").is_err());
    }

    #[test]
    fn test_get_lint() {
        let def = get_lint(LintId::UnusedVariable);
        assert_eq!(def.id, LintId::UnusedVariable);
        assert_eq!(def.default_level, LintLevel::Warn);
    }

    #[test]
    fn test_all_lints() {
        let lints = all_lints();
        assert!(!lints.is_empty());
        assert!(lints.iter().any(|l| l.id == LintId::UnusedVariable));
    }

    #[test]
    fn test_lookup_lint() {
        let def = lookup_lint("unused-import");
        assert!(def.is_some());
        assert_eq!(def.unwrap().id, LintId::UnusedImport);

        assert!(lookup_lint("nonexistent").is_none());
    }

    #[test]
    fn test_all_lint_ids() {
        let ids = LintId::all();
        assert_eq!(ids.len(), 8);
    }
}
