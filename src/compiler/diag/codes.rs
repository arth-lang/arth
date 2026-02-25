//! Error code registry for the Arth compiler.
//!
//! Error codes follow a consistent naming scheme:
//! - E0xxx: Parse errors
//! - E1xxx: Name resolution errors
//! - E2xxx: Type checking errors
//! - E3xxx: Borrow/lifetime errors
//! - W0xxx: Lint warnings
//! - W1xxx: Deprecation warnings

use std::collections::HashMap;
use std::sync::LazyLock;

/// Category of error codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCategory {
    Parse,
    Resolve,
    Type,
    Borrow,
    Lint,
    Deprecation,
}

impl ErrorCategory {
    /// Get the prefix for this category.
    pub fn prefix(self) -> &'static str {
        match self {
            ErrorCategory::Parse => "E0",
            ErrorCategory::Resolve => "E1",
            ErrorCategory::Type => "E2",
            ErrorCategory::Borrow => "E3",
            ErrorCategory::Lint => "W0",
            ErrorCategory::Deprecation => "W1",
        }
    }
}

/// An error code definition.
#[derive(Clone, Debug)]
pub struct ErrorCode {
    /// The code string (e.g., "E0001").
    pub code: &'static str,
    /// The category of this error.
    pub category: ErrorCategory,
    /// Short title for the error.
    pub title: &'static str,
    /// Detailed explanation of the error and how to fix it.
    pub explanation: &'static str,
}

impl ErrorCode {
    const fn new(
        code: &'static str,
        category: ErrorCategory,
        title: &'static str,
        explanation: &'static str,
    ) -> Self {
        Self {
            code,
            category,
            title,
            explanation,
        }
    }
}

// Parse errors (E0xxx)
pub const E0001: ErrorCode = ErrorCode::new(
    "E0001",
    ErrorCategory::Parse,
    "unexpected token",
    "The parser encountered a token that was not expected at this position. \
     Check for missing semicolons, braces, or other syntax errors.",
);

pub const E0002: ErrorCode = ErrorCode::new(
    "E0002",
    ErrorCategory::Parse,
    "unterminated string literal",
    "A string literal was started but not closed. Add a closing quote.",
);

pub const E0003: ErrorCode = ErrorCode::new(
    "E0003",
    ErrorCategory::Parse,
    "invalid escape sequence",
    "An invalid escape sequence was found in a string or character literal. \
     Valid escape sequences include: \\n, \\r, \\t, \\\\, \\\", \\'.",
);

pub const E0004: ErrorCode = ErrorCode::new(
    "E0004",
    ErrorCategory::Parse,
    "expected declaration",
    "Expected a module, struct, interface, enum, or other declaration at this position.",
);

pub const E0005: ErrorCode = ErrorCode::new(
    "E0005",
    ErrorCategory::Parse,
    "expected expression",
    "Expected an expression at this position.",
);

pub const E0006: ErrorCode = ErrorCode::new(
    "E0006",
    ErrorCategory::Parse,
    "expected type",
    "Expected a type annotation at this position.",
);

pub const E0007: ErrorCode = ErrorCode::new(
    "E0007",
    ErrorCategory::Parse,
    "missing semicolon",
    "A semicolon is required at the end of this statement.",
);

pub const E0008: ErrorCode = ErrorCode::new(
    "E0008",
    ErrorCategory::Parse,
    "unbalanced brackets",
    "Opening and closing brackets do not match. Check for missing or extra brackets.",
);

// Resolve errors (E1xxx)
pub const E1001: ErrorCode = ErrorCode::new(
    "E1001",
    ErrorCategory::Resolve,
    "undefined name",
    "The name could not be found in the current scope. Check spelling or add an import.",
);

pub const E1002: ErrorCode = ErrorCode::new(
    "E1002",
    ErrorCategory::Resolve,
    "duplicate definition",
    "A definition with this name already exists in the current scope.",
);

pub const E1003: ErrorCode = ErrorCode::new(
    "E1003",
    ErrorCategory::Resolve,
    "import cycle detected",
    "Circular package imports are not allowed. Refactor to break the cycle.",
);

pub const E1004: ErrorCode = ErrorCode::new(
    "E1004",
    ErrorCategory::Resolve,
    "visibility violation",
    "The item is not accessible from this location due to visibility restrictions.",
);

pub const E1005: ErrorCode = ErrorCode::new(
    "E1005",
    ErrorCategory::Resolve,
    "ambiguous import",
    "Multiple items match this import. Use a more specific import path.",
);

pub const E1006: ErrorCode = ErrorCode::new(
    "E1006",
    ErrorCategory::Resolve,
    "undefined import",
    "The imported item does not exist in the specified package.",
);

// Type errors (E2xxx)
pub const E2001: ErrorCode = ErrorCode::new(
    "E2001",
    ErrorCategory::Type,
    "type mismatch",
    "The expected type does not match the actual type. Check the expression type.",
);

pub const E2002: ErrorCode = ErrorCode::new(
    "E2002",
    ErrorCategory::Type,
    "undefined type",
    "The specified type does not exist. Check spelling or add an import.",
);

pub const E2003: ErrorCode = ErrorCode::new(
    "E2003",
    ErrorCategory::Type,
    "invalid argument count",
    "The function was called with the wrong number of arguments.",
);

pub const E2004: ErrorCode = ErrorCode::new(
    "E2004",
    ErrorCategory::Type,
    "not callable",
    "This expression cannot be called as a function.",
);

pub const E2005: ErrorCode = ErrorCode::new(
    "E2005",
    ErrorCategory::Type,
    "missing field",
    "A required field is missing from this struct literal.",
);

pub const E2006: ErrorCode = ErrorCode::new(
    "E2006",
    ErrorCategory::Type,
    "duplicate field",
    "This field appears more than once in the struct literal.",
);

pub const E2007: ErrorCode = ErrorCode::new(
    "E2007",
    ErrorCategory::Type,
    "unknown field",
    "This field does not exist on the struct type.",
);

pub const E2008: ErrorCode = ErrorCode::new(
    "E2008",
    ErrorCategory::Type,
    "generic constraint not satisfied",
    "The type argument does not satisfy the required constraint.",
);

pub const E2009: ErrorCode = ErrorCode::new(
    "E2009",
    ErrorCategory::Type,
    "missing return type",
    "A return type annotation is required for this function.",
);

pub const E2010: ErrorCode = ErrorCode::new(
    "E2010",
    ErrorCategory::Type,
    "unreachable pattern",
    "This pattern will never be matched because previous patterns are exhaustive.",
);

pub const E2011: ErrorCode = ErrorCode::new(
    "E2011",
    ErrorCategory::Type,
    "non-exhaustive patterns",
    "The match expression does not cover all possible cases.",
);

pub const E2012: ErrorCode = ErrorCode::new(
    "E2012",
    ErrorCategory::Type,
    "uncaught exception",
    "This function call may throw an exception that is not caught or declared.",
);

pub const E2013: ErrorCode = ErrorCode::new(
    "E2013",
    ErrorCategory::Type,
    "undeclared throws",
    "The thrown exception type is not declared in the function signature.",
);

pub const E2014: ErrorCode = ErrorCode::new(
    "E2014",
    ErrorCategory::Type,
    "null not allowed",
    "Arth does not have null. Use Optional<T> instead.",
);

pub const E2015: ErrorCode = ErrorCode::new(
    "E2015",
    ErrorCategory::Type,
    "await outside async",
    "The await expression can only be used inside async functions.",
);

// Borrow/lifetime errors (E3xxx)
pub const E3001: ErrorCode = ErrorCode::new(
    "E3001",
    ErrorCategory::Borrow,
    "use after move",
    "The value was moved and is no longer available. Consider cloning or restructuring.",
);

pub const E3002: ErrorCode = ErrorCode::new(
    "E3002",
    ErrorCategory::Borrow,
    "borrow of moved value",
    "Cannot borrow a value that has been moved.",
);

pub const E3003: ErrorCode = ErrorCode::new(
    "E3003",
    ErrorCategory::Borrow,
    "conflicting borrows",
    "Cannot have multiple mutable borrows or a mutable borrow with an immutable borrow.",
);

pub const E3004: ErrorCode = ErrorCode::new(
    "E3004",
    ErrorCategory::Borrow,
    "borrow escapes scope",
    "The borrowed reference would outlive the value it borrows from.",
);

pub const E3005: ErrorCode = ErrorCode::new(
    "E3005",
    ErrorCategory::Borrow,
    "partial move not allowed",
    "Cannot partially move from types with destructors (deinit).",
);

pub const E3006: ErrorCode = ErrorCode::new(
    "E3006",
    ErrorCategory::Borrow,
    "borrow crosses await",
    "Exclusive borrows cannot be held across await points.",
);

// Lint warnings (W0xxx)
pub const W0001: ErrorCode = ErrorCode::new(
    "W0001",
    ErrorCategory::Lint,
    "unused variable",
    "This variable is declared but never used. Consider removing it or prefixing with _.",
);

pub const W0002: ErrorCode = ErrorCode::new(
    "W0002",
    ErrorCategory::Lint,
    "unused import",
    "This import is not used. Consider removing it.",
);

pub const W0003: ErrorCode = ErrorCode::new(
    "W0003",
    ErrorCategory::Lint,
    "unused result",
    "The result of this expression is marked @must_use but is being discarded.",
);

pub const W0004: ErrorCode = ErrorCode::new(
    "W0004",
    ErrorCategory::Lint,
    "async without await",
    "This async function has no await points. Consider removing the async modifier.",
);

pub const W0005: ErrorCode = ErrorCode::new(
    "W0005",
    ErrorCategory::Lint,
    "unreachable code",
    "This code will never be executed.",
);

pub const W0006: ErrorCode = ErrorCode::new(
    "W0006",
    ErrorCategory::Lint,
    "needless clone",
    "This clone is unnecessary because the value is Copy or only used once.",
);

// Deprecation warnings (W1xxx)
pub const W1001: ErrorCode = ErrorCode::new(
    "W1001",
    ErrorCategory::Deprecation,
    "deprecated item",
    "This item is deprecated and may be removed in a future version.",
);

/// Global registry of all error codes.
pub static ERROR_REGISTRY: LazyLock<HashMap<&'static str, &'static ErrorCode>> =
    LazyLock::new(|| {
        let codes: &[&ErrorCode] = &[
            // Parse errors
            &E0001, &E0002, &E0003, &E0004, &E0005, &E0006, &E0007, &E0008,
            // Resolve errors
            &E1001, &E1002, &E1003, &E1004, &E1005, &E1006, // Type errors
            &E2001, &E2002, &E2003, &E2004, &E2005, &E2006, &E2007, &E2008, &E2009, &E2010, &E2011,
            &E2012, &E2013, &E2014, &E2015, // Borrow errors
            &E3001, &E3002, &E3003, &E3004, &E3005, &E3006, // Lint warnings
            &W0001, &W0002, &W0003, &W0004, &W0005, &W0006, // Deprecation warnings
            &W1001,
        ];
        let mut map = HashMap::with_capacity(codes.len());
        for code in codes {
            map.insert(code.code, *code);
        }
        map
    });

/// Look up an error code by its string identifier.
pub fn lookup(code: &str) -> Option<&'static ErrorCode> {
    ERROR_REGISTRY.get(code).copied()
}

/// Get all error codes in a category.
pub fn codes_in_category(category: ErrorCategory) -> Vec<&'static ErrorCode> {
    ERROR_REGISTRY
        .values()
        .filter(|c| c.category == category)
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_e0001() {
        let code = lookup("E0001").expect("E0001 should exist");
        assert_eq!(code.code, "E0001");
        assert_eq!(code.category, ErrorCategory::Parse);
        assert_eq!(code.title, "unexpected token");
    }

    #[test]
    fn test_lookup_w0001() {
        let code = lookup("W0001").expect("W0001 should exist");
        assert_eq!(code.code, "W0001");
        assert_eq!(code.category, ErrorCategory::Lint);
        assert_eq!(code.title, "unused variable");
    }

    #[test]
    fn test_lookup_nonexistent() {
        assert!(lookup("X9999").is_none());
    }

    #[test]
    fn test_codes_in_category() {
        let parse_codes = codes_in_category(ErrorCategory::Parse);
        assert!(!parse_codes.is_empty());
        for code in &parse_codes {
            assert!(code.code.starts_with("E0"));
        }
    }

    #[test]
    fn test_category_prefix() {
        assert_eq!(ErrorCategory::Parse.prefix(), "E0");
        assert_eq!(ErrorCategory::Resolve.prefix(), "E1");
        assert_eq!(ErrorCategory::Type.prefix(), "E2");
        assert_eq!(ErrorCategory::Borrow.prefix(), "E3");
        assert_eq!(ErrorCategory::Lint.prefix(), "W0");
        assert_eq!(ErrorCategory::Deprecation.prefix(), "W1");
    }
}
