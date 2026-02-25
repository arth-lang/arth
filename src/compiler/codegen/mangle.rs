//! Name Mangling for Arth Native Compilation
//!
//! This module provides name mangling for Arth symbols to ensure unique,
//! linker-compatible names for functions, methods, and types.
//!
//! # Mangling Scheme
//!
//! The Arth name mangling scheme is designed to be:
//! - Unique: Different functions with the same name but different signatures get different symbols
//! - Reversible: Mangled names can be demangled for debugging
//! - Linker-safe: Only uses characters valid in C identifiers ([A-Za-z0-9_])
//!
//! ## Format
//!
//! ```text
//! _A<module_path>F<function_name><params><ret>E
//! ```
//!
//! Where:
//! - `_A` is the Arth mangling prefix
//! - Module path: `<len><name>` pairs for each path segment, or empty for top-level
//! - `F` marks the start of function name
//! - Function name: `<len><name>`
//! - Params: type encodings for each parameter
//! - Ret: `R<type>` for non-void return, or empty for void
//! - `E` marks end of mangled name
//!
//! ## Type Encoding
//!
//! | Type | Encoding |
//! |------|----------|
//! | Int (i64) | `i` |
//! | Float (f64) | `d` |
//! | Bool | `b` |
//! | Void | `v` |
//! | Ptr | `p` |
//! | String | `s` |
//! | Struct | `S<len><name>` |
//! | Enum | `E<len><name>` |
//! | Optional<T> | `O<inner_type>` |
//! | Generic<A,B> | `G<len><name><num_args><arg1><arg2>...` |
//!
//! ## Examples
//!
//! - `main()` → `_AFmainvE` (no module, no params, void return)
//! - `math.add(Int, Int) -> Int` → `_A4mathF3addiiRiE`
//! - `Option.unwrap<T>(Optional<T>) -> T` → `_A6OptionF6unwrapOpRpE` (type-erased to ptr)
//!
//! # ABI Stability: TypeInfo vs Function Mangling
//!
//! **IMPORTANT:** TypeInfo symbol naming is NOT ABI-equivalent to function mangling.
//!
//! The current TypeInfo naming scheme uses a simple format:
//! ```text
//! @_arth_typeinfo_<TypeName>
//! ```
//!
//! This is intentionally different from the function mangling scheme (`_A...E`).
//!
//! ## Versioning Policy
//!
//! TypeInfo symbol names are stable across compiler versions unless the type layout
//! itself changes. If TypeInfo ever needs to include generics or signature encoding:
//!
//! 1. **Introduce a new versioned prefix**: `_arth_typeinfo_v2_...`
//! 2. **Never rewrite `_arth_typeinfo_*` in place**
//! 3. **Maintain backward compatibility** with existing binaries
//!
//! This versioning approach avoids breaking old binaries that were compiled with
//! earlier versions of the compiler. The runtime can support multiple TypeInfo
//! versions simultaneously during a transition period.
//!
//! ## Rationale
//!
//! - TypeInfo is used for RTTI and exception handling at runtime
//! - Changing TypeInfo format in place would break dynamic linking with older libraries
//! - Function mangling can evolve more freely since it's resolved at link time

use crate::compiler::ir::Ty;

/// Mangle a function name for native linking.
///
/// # Arguments
/// * `module_path` - Dot-separated module path (e.g., "app.utils.math")
/// * `function_name` - The function name
/// * `params` - Parameter types
/// * `ret` - Return type
///
/// # Returns
/// A mangled symbol name safe for use in native code.
pub fn mangle_function(module_path: &str, function_name: &str, params: &[Ty], ret: &Ty) -> String {
    let mut result = String::with_capacity(64);
    result.push_str("_A");

    // Encode module path
    if !module_path.is_empty() {
        for segment in module_path.split('.') {
            encode_identifier(&mut result, segment);
        }
    }

    // Function marker and name
    result.push('F');
    encode_identifier(&mut result, function_name);

    // Encode parameters
    for param in params {
        encode_type(&mut result, param);
    }

    // Encode return type (if not void)
    if !matches!(ret, Ty::Void) {
        result.push('R');
        encode_type(&mut result, ret);
    }

    // End marker
    result.push('E');

    result
}

/// Mangle a method name (function on a type).
///
/// Methods are mangled as: `_A<module>M<type>F<method><params><ret>E`
pub fn mangle_method(
    module_path: &str,
    type_name: &str,
    method_name: &str,
    params: &[Ty],
    ret: &Ty,
) -> String {
    let mut result = String::with_capacity(64);
    result.push_str("_A");

    // Encode module path
    if !module_path.is_empty() {
        for segment in module_path.split('.') {
            encode_identifier(&mut result, segment);
        }
    }

    // Type marker and name
    result.push('M');
    encode_identifier(&mut result, type_name);

    // Function marker and name
    result.push('F');
    encode_identifier(&mut result, method_name);

    // Encode parameters (excluding self)
    for param in params {
        encode_type(&mut result, param);
    }

    // Encode return type
    if !matches!(ret, Ty::Void) {
        result.push('R');
        encode_type(&mut result, ret);
    }

    // End marker
    result.push('E');

    result
}

/// Mangle a type name for type info globals.
///
/// Format: `_A<module>T<name>E` (consistent with function/method mangling)
pub fn mangle_type(module_path: &str, type_name: &str) -> String {
    let mut result = String::with_capacity(32);
    result.push_str("_A");

    // Encode module path
    if !module_path.is_empty() {
        for segment in module_path.split('.') {
            encode_identifier(&mut result, segment);
        }
    }

    // Type marker and name
    result.push('T');
    encode_identifier(&mut result, type_name);
    result.push('E');

    result
}

/// Encode an identifier with its length prefix.
fn encode_identifier(buf: &mut String, name: &str) {
    // Replace any non-alphanumeric characters with underscores
    let safe_name: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();

    buf.push_str(&safe_name.len().to_string());
    buf.push_str(&safe_name);
}

/// Encode a type for the mangled signature.
fn encode_type(buf: &mut String, ty: &Ty) {
    match ty {
        Ty::I64 => buf.push('i'),
        Ty::F64 => buf.push('d'),
        Ty::I1 => buf.push('b'),
        Ty::Ptr => buf.push('p'),
        Ty::Void => buf.push('v'),
        Ty::String => buf.push('s'),
        Ty::Struct(name) => {
            buf.push('S');
            encode_identifier(buf, name);
        }
        Ty::Enum(name) => {
            buf.push('E');
            encode_identifier(buf, name);
        }
        Ty::Optional(inner) => {
            buf.push('O');
            encode_type(buf, inner);
        }
    }
}

// =============================================================================
// Demangling
// =============================================================================

/// Demangled symbol information.
#[derive(Clone, Debug, PartialEq)]
pub struct DemangledSymbol {
    /// Module path segments
    pub module_path: Vec<String>,
    /// Type name (for methods)
    pub type_name: Option<String>,
    /// Function/method name
    pub function_name: String,
    /// Parameter types (as strings)
    pub params: Vec<String>,
    /// Return type (as string)
    pub return_type: String,
}

impl std::fmt::Display for DemangledSymbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Module path
        if !self.module_path.is_empty() {
            write!(f, "{}.", self.module_path.join("."))?;
        }

        // Type name (for methods)
        if let Some(ref type_name) = self.type_name {
            write!(f, "{}.", type_name)?;
        }

        // Function name
        write!(f, "{}", self.function_name)?;

        // Parameters
        write!(f, "({})", self.params.join(", "))?;

        // Return type
        if self.return_type != "void" {
            write!(f, " -> {}", self.return_type)?;
        }

        Ok(())
    }
}

/// Demangle an Arth symbol name.
///
/// Returns `None` if the symbol is not a valid Arth mangled name.
pub fn demangle(symbol: &str) -> Option<DemangledSymbol> {
    if !symbol.starts_with("_A") || !symbol.ends_with('E') {
        return None;
    }

    let mut chars = symbol[2..symbol.len() - 1].chars().peekable();
    let mut module_path = Vec::new();
    let mut type_name = None;
    let mut params = Vec::new();
    let mut return_type = "void".to_string();

    // Parse module path segments until we hit 'F' (function) or 'M' (method) or 'T' (type)
    while let Some(&c) = chars.peek() {
        if c == 'F' || c == 'M' || c == 'T' {
            break;
        }
        if c.is_ascii_digit() {
            if let Some(ident) = parse_identifier(&mut chars) {
                module_path.push(ident);
            } else {
                return None;
            }
        } else {
            return None;
        }
    }

    // Check for type marker (method on type)
    if chars.peek() == Some(&'M') {
        chars.next(); // consume 'M'
        type_name = parse_identifier(&mut chars);
        type_name.as_ref()?;
    }

    // Check for type-only symbol
    if chars.peek() == Some(&'T') {
        // This is a type symbol, not a function
        return None;
    }

    // Parse function marker and name
    if chars.next() != Some('F') {
        return None;
    }
    let function_name = parse_identifier(&mut chars)?;

    // Parse parameter types until 'R' (return) or end
    while let Some(&c) = chars.peek() {
        if c == 'R' {
            break;
        }
        if let Some(ty) = parse_type(&mut chars) {
            params.push(ty);
        } else {
            break;
        }
    }

    // Parse return type
    if chars.peek() == Some(&'R') {
        chars.next(); // consume 'R'
        if let Some(ty) = parse_type(&mut chars) {
            return_type = ty;
        }
    }

    Some(DemangledSymbol {
        module_path,
        type_name,
        function_name,
        params,
        return_type,
    })
}

/// Parse a length-prefixed identifier.
fn parse_identifier(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
    let mut len_str = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            len_str.push(c);
            chars.next();
        } else {
            break;
        }
    }

    if len_str.is_empty() {
        return None;
    }

    let len: usize = len_str.parse().ok()?;
    let mut result = String::with_capacity(len);
    for _ in 0..len {
        result.push(chars.next()?);
    }

    Some(result)
}

/// Parse a type encoding and return a human-readable type name.
fn parse_type(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
    let c = chars.next()?;
    match c {
        'i' => Some("Int".to_string()),
        'd' => Some("Float".to_string()),
        'b' => Some("Bool".to_string()),
        'p' => Some("Ptr".to_string()),
        'v' => Some("void".to_string()),
        's' => Some("String".to_string()),
        'S' => {
            let name = parse_identifier(chars)?;
            Some(name)
        }
        'E' => {
            let name = parse_identifier(chars)?;
            Some(name)
        }
        'O' => {
            let inner = parse_type(chars)?;
            Some(format!("Optional<{}>", inner))
        }
        _ => None,
    }
}

/// Check if a symbol name is a mangled Arth symbol.
pub fn is_mangled(symbol: &str) -> bool {
    symbol.starts_with("_A") && symbol.ends_with('E')
}

/// Sanitize a name for use in LLVM IR (replace invalid characters).
/// This is for unmangled names that need to be safe for LLVM.
pub fn sanitize_llvm_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mangle_simple_function() {
        // main() -> void
        let mangled = mangle_function("", "main", &[], &Ty::Void);
        assert_eq!(mangled, "_AF4mainE");

        // add(Int, Int) -> Int
        let mangled = mangle_function("", "add", &[Ty::I64, Ty::I64], &Ty::I64);
        assert_eq!(mangled, "_AF3addiiRiE");
    }

    #[test]
    fn test_mangle_with_module() {
        // math.add(Int, Int) -> Int
        let mangled = mangle_function("math", "add", &[Ty::I64, Ty::I64], &Ty::I64);
        assert_eq!(mangled, "_A4mathF3addiiRiE");

        // app.utils.format(String) -> String
        let mangled = mangle_function("app.utils", "format", &[Ty::String], &Ty::String);
        assert_eq!(mangled, "_A3app5utilsF6formatsRsE");
    }

    #[test]
    fn test_mangle_method() {
        // Point.distance(Point, Point) -> Float
        let mangled = mangle_method(
            "",
            "Point",
            "distance",
            &[
                Ty::Struct("Point".to_string()),
                Ty::Struct("Point".to_string()),
            ],
            &Ty::F64,
        );
        assert_eq!(mangled, "_AM5PointF8distanceS5PointS5PointRdE");
    }

    #[test]
    fn test_mangle_complex_types() {
        // process(Optional<Int>) -> Bool
        let mangled = mangle_function("", "process", &[Ty::Optional(Box::new(Ty::I64))], &Ty::I1);
        assert_eq!(mangled, "_AF7processOiRbE");

        // handle(Error) -> void (struct param)
        let mangled = mangle_function("", "handle", &[Ty::Struct("Error".to_string())], &Ty::Void);
        assert_eq!(mangled, "_AF6handleS5ErrorE");
    }

    #[test]
    fn test_mangle_type() {
        let mangled = mangle_type("", "Point");
        assert_eq!(mangled, "_AT5PointE");

        let mangled = mangle_type("geometry", "Point");
        assert_eq!(mangled, "_A8geometryT5PointE");
    }

    #[test]
    fn test_demangle_simple() {
        let symbol = "_AF4mainE";
        let demangled = demangle(symbol).unwrap();
        assert_eq!(demangled.function_name, "main");
        assert!(demangled.module_path.is_empty());
        assert!(demangled.params.is_empty());
        assert_eq!(demangled.return_type, "void");
    }

    #[test]
    fn test_demangle_with_params() {
        let symbol = "_AF3addiiRiE";
        let demangled = demangle(symbol).unwrap();
        assert_eq!(demangled.function_name, "add");
        assert_eq!(demangled.params, vec!["Int", "Int"]);
        assert_eq!(demangled.return_type, "Int");
    }

    #[test]
    fn test_demangle_with_module() {
        let symbol = "_A4mathF3addiiRiE";
        let demangled = demangle(symbol).unwrap();
        assert_eq!(demangled.module_path, vec!["math"]);
        assert_eq!(demangled.function_name, "add");
    }

    #[test]
    fn test_demangle_method() {
        let symbol = "_AM5PointF8distanceS5PointS5PointRdE";
        let demangled = demangle(symbol).unwrap();
        assert_eq!(demangled.type_name, Some("Point".to_string()));
        assert_eq!(demangled.function_name, "distance");
        assert_eq!(demangled.params, vec!["Point", "Point"]);
        assert_eq!(demangled.return_type, "Float");
    }

    #[test]
    fn test_demangle_optional() {
        let symbol = "_AF7processOiRbE";
        let demangled = demangle(symbol).unwrap();
        assert_eq!(demangled.params, vec!["Optional<Int>"]);
        assert_eq!(demangled.return_type, "Bool");
    }

    #[test]
    fn test_demangle_to_string() {
        let symbol = "_A4mathF3addiiRiE";
        let demangled = demangle(symbol).unwrap();
        assert_eq!(demangled.to_string(), "math.add(Int, Int) -> Int");
    }

    #[test]
    fn test_roundtrip() {
        // Verify that mangle -> demangle produces consistent results
        let mangled = mangle_function("app.utils", "format", &[Ty::String], &Ty::String);
        let demangled = demangle(&mangled).unwrap();
        assert_eq!(demangled.module_path, vec!["app", "utils"]);
        assert_eq!(demangled.function_name, "format");
        assert_eq!(demangled.params, vec!["String"]);
        assert_eq!(demangled.return_type, "String");
    }

    #[test]
    fn test_is_mangled() {
        assert!(is_mangled("_AF4mainE"));
        assert!(is_mangled("_A4mathF3addiiRiE"));
        assert!(!is_mangled("main"));
        assert!(!is_mangled("_Amain")); // missing E
        assert!(!is_mangled("AF4mainE")); // missing _
    }

    #[test]
    fn test_sanitize_llvm_name() {
        assert_eq!(sanitize_llvm_name("hello"), "hello");
        assert_eq!(sanitize_llvm_name("hello.world"), "hello_world");
        assert_eq!(sanitize_llvm_name("foo::bar"), "foo__bar");
        assert_eq!(sanitize_llvm_name("func<T>"), "func_T_");
    }
}
