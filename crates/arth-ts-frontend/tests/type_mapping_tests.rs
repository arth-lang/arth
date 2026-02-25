//! Phase 9: Unit tests for type mappings in isolation.
//!
//! These tests verify that TypeScript types are correctly mapped to Arth types
//! during the lowering process.

use arth_ts_frontend::{TsLoweringOptions, lower_ts_str_to_hir};

/// Helper to extract the return type string from the first function in the HIR.
fn get_first_func_return_type(source: &str) -> Option<String> {
    use arth::compiler::hir::HirDecl;

    let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default()).ok()?;

    for decl in &hir.decls {
        if let HirDecl::Module(m) = decl
            && let Some(func) = m.funcs.first()
        {
            return func.sig.ret.as_ref().map(|t| format!("{:?}", t));
        }
    }
    None
}

/// Helper to extract parameter type from a function.
fn get_first_param_type(source: &str) -> Option<String> {
    use arth::compiler::hir::HirDecl;

    let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default()).ok()?;

    for decl in &hir.decls {
        if let HirDecl::Module(m) = decl
            && let Some(func) = m.funcs.first()
            && let Some(param) = func.sig.params.first()
        {
            // HirParam.ty is HirType, not Option<HirType>
            return Some(format!("{:?}", param.ty));
        }
    }
    None
}

// =============================================================================
// Primitive Type Mappings
// =============================================================================

#[test]
fn test_number_to_float() {
    let source = r#"
        export function getValue(): number {
            return 42;
        }
    "#;
    let ret = get_first_func_return_type(source);
    assert!(ret.is_some(), "should have return type");
    let ret_str = ret.unwrap();
    // number maps to Float in Arth
    assert!(
        ret_str.contains("Float") || ret_str.contains("Int"),
        "number should map to Float or Int, got: {}",
        ret_str
    );
}

#[test]
fn test_string_type() {
    let source = r#"
        export function getName(): string {
            return "hello";
        }
    "#;
    let ret = get_first_func_return_type(source);
    assert!(ret.is_some(), "should have return type");
    let ret_str = ret.unwrap();
    assert!(
        ret_str.contains("String"),
        "string should map to String, got: {}",
        ret_str
    );
}

#[test]
fn test_boolean_type() {
    let source = r#"
        export function isValid(): boolean {
            return true;
        }
    "#;
    let ret = get_first_func_return_type(source);
    assert!(ret.is_some(), "should have return type");
    let ret_str = ret.unwrap();
    assert!(
        ret_str.contains("Bool"),
        "boolean should map to Bool, got: {}",
        ret_str
    );
}

#[test]
fn test_void_return_type() {
    let source = r#"
        export function doNothing(): void {
            return;
        }
    "#;
    let ret = get_first_func_return_type(source);
    // void may be None or explicitly Void
    // Both are acceptable
    assert!(
        ret.is_none()
            || ret
                .as_ref()
                .is_some_and(|s| s.contains("Void") || s.contains("Unit")),
        "void should map to None or Void, got: {:?}",
        ret
    );
}

// =============================================================================
// Array/List Type Mappings
// =============================================================================

#[test]
fn test_array_bracket_syntax() {
    let source = r#"
        export function getNumbers(): number[] {
            return [1, 2, 3];
        }
    "#;
    let ret = get_first_func_return_type(source);
    assert!(ret.is_some(), "should have return type");
    let ret_str = ret.unwrap();
    assert!(
        ret_str.contains("List") || ret_str.contains("Array"),
        "number[] should map to List<Float> or Array, got: {}",
        ret_str
    );
}

#[test]
fn test_array_generic_syntax() {
    let source = r#"
        export function getStrings(): Array<string> {
            return ["a", "b"];
        }
    "#;
    let ret = get_first_func_return_type(source);
    assert!(ret.is_some(), "should have return type");
    let ret_str = ret.unwrap();
    assert!(
        ret_str.contains("List") || ret_str.contains("Array"),
        "Array<string> should map to List<String>, got: {}",
        ret_str
    );
}

// =============================================================================
// Optional/Nullable Type Mappings
// =============================================================================

#[test]
fn test_optional_union_with_null() {
    let source = r#"
        export function findUser(id: number): string | null {
            return null;
        }
    "#;
    let ret = get_first_func_return_type(source);
    assert!(ret.is_some(), "should have return type");
    let ret_str = ret.unwrap();
    assert!(
        ret_str.contains("Optional") || ret_str.contains("Union"),
        "string | null should map to Optional<String>, got: {}",
        ret_str
    );
}

#[test]
fn test_optional_union_with_undefined() {
    let source = r#"
        export function findItem(key: string): number | undefined {
            return undefined;
        }
    "#;
    let ret = get_first_func_return_type(source);
    assert!(ret.is_some(), "should have return type");
    let ret_str = ret.unwrap();
    assert!(
        ret_str.contains("Optional") || ret_str.contains("Union"),
        "number | undefined should map to Optional<Float>, got: {}",
        ret_str
    );
}

// =============================================================================
// Map/Record Type Mappings
// =============================================================================

#[test]
fn test_map_type() {
    let source = r#"
        export function getCache(): Map<string, number> {
            return new Map();
        }
    "#;
    let ret = get_first_func_return_type(source);
    assert!(ret.is_some(), "should have return type");
    let ret_str = ret.unwrap();
    assert!(
        ret_str.contains("Map"),
        "Map<string, number> should map to Map<String, Float>, got: {}",
        ret_str
    );
}

#[test]
fn test_set_type() {
    let source = r#"
        export function getUniqueIds(): Set<string> {
            return new Set();
        }
    "#;
    let ret = get_first_func_return_type(source);
    assert!(ret.is_some(), "should have return type");
    let ret_str = ret.unwrap();
    assert!(
        ret_str.contains("Set"),
        "Set<string> should map to Set<String>, got: {}",
        ret_str
    );
}

// =============================================================================
// Promise/Task Type Mappings
// =============================================================================

#[test]
fn test_promise_type() {
    let source = r#"
        export async function fetchData(): Promise<string> {
            return "data";
        }
    "#;
    let ret = get_first_func_return_type(source);
    assert!(ret.is_some(), "should have return type");
    let ret_str = ret.unwrap();
    // Promise maps to Task in Arth
    assert!(
        ret_str.contains("Task") || ret_str.contains("Promise"),
        "Promise<string> should map to Task<String>, got: {}",
        ret_str
    );
}

// =============================================================================
// Parameter Type Mappings
// =============================================================================

#[test]
fn test_number_param() {
    let source = r#"
        export function increment(value: number): number {
            return value + 1;
        }
    "#;
    let param_type = get_first_param_type(source);
    assert!(param_type.is_some(), "should have param type");
    let type_str = param_type.unwrap();
    assert!(
        type_str.contains("Float") || type_str.contains("Int"),
        "number param should map to Float, got: {}",
        type_str
    );
}

#[test]
fn test_string_param() {
    let source = r#"
        export function greet(name: string): void {
            return;
        }
    "#;
    let param_type = get_first_param_type(source);
    assert!(param_type.is_some(), "should have param type");
    let type_str = param_type.unwrap();
    assert!(
        type_str.contains("String"),
        "string param should map to String, got: {}",
        type_str
    );
}

#[test]
fn test_boolean_param() {
    let source = r#"
        export function toggle(enabled: boolean): boolean {
            return !enabled;
        }
    "#;
    let param_type = get_first_param_type(source);
    assert!(param_type.is_some(), "should have param type");
    let type_str = param_type.unwrap();
    assert!(
        type_str.contains("Bool"),
        "boolean param should map to Bool, got: {}",
        type_str
    );
}

#[test]
fn test_array_param() {
    let source = r#"
        export function sum(numbers: number[]): number {
            return 0;
        }
    "#;
    let param_type = get_first_param_type(source);
    assert!(param_type.is_some(), "should have param type");
    let type_str = param_type.unwrap();
    assert!(
        type_str.contains("List") || type_str.contains("Array"),
        "number[] param should map to List<Float>, got: {}",
        type_str
    );
}

// =============================================================================
// Custom Type References
// =============================================================================

#[test]
fn test_custom_type_reference() {
    let source = r#"
        interface User {
            name: string;
        }

        export function createUser(): User {
            return { name: "test" };
        }
    "#;
    let ret = get_first_func_return_type(source);
    assert!(ret.is_some(), "should have return type");
    let ret_str = ret.unwrap();
    assert!(
        ret_str.contains("User"),
        "User type should be preserved, got: {}",
        ret_str
    );
}

#[test]
fn test_generic_custom_type() {
    let source = r#"
        interface Response<T> {
            data: T;
            status: number;
        }

        export function getResponse(): Response<string> {
            return { data: "ok", status: 200 };
        }
    "#;
    let ret = get_first_func_return_type(source);
    assert!(ret.is_some(), "should have return type");
    let ret_str = ret.unwrap();
    assert!(
        ret_str.contains("Response"),
        "Response<string> type should be preserved, got: {}",
        ret_str
    );
}
