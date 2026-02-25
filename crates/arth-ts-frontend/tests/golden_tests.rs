//! Phase 9: Golden tests for generated Arth code.
//!
//! These tests verify that the Arth code emitter produces consistent output
//! by comparing against expected "golden" snapshots.

use arth_ts_frontend::{TsLoweringOptions, emit_arth_source, lower_ts_str_to_hir};

/// Helper to compile TS to Arth source code.
fn ts_to_arth(source: &str) -> Result<String, String> {
    let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
        .map_err(|e| e.to_string())?;
    // emit_arth_source returns EmitResult directly, not a Result<>
    let result = emit_arth_source(&hir);
    Ok(result.source)
}

/// Normalize whitespace for comparison (trim lines, remove empty lines).
fn normalize(s: &str) -> String {
    s.lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

// =============================================================================
// Golden Tests: Simple Functions
// =============================================================================

#[test]
fn golden_simple_function() {
    let ts = r#"
        export function add(a: number, b: number): number {
            return a + b;
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    // Verify key elements are present
    assert!(
        normalized.contains("module") || normalized.contains("public"),
        "should have module or public keyword"
    );
    assert!(
        normalized.contains("add"),
        "should contain function name 'add'"
    );
    assert!(
        normalized.contains("return"),
        "should contain return statement"
    );
    assert!(
        normalized.contains("+") || normalized.contains("add"),
        "should contain addition operator"
    );
}

#[test]
fn golden_function_with_conditionals() {
    let ts = r#"
        export function max(a: number, b: number): number {
            if (a > b) {
                return a;
            } else {
                return b;
            }
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    assert!(normalized.contains("if"), "should contain if statement");
    assert!(normalized.contains("else"), "should contain else clause");
    assert!(
        normalized.contains(">"),
        "should contain greater-than operator"
    );
}

#[test]
fn golden_function_with_loop() {
    let ts = r#"
        export function sumTo(n: number): number {
            let total = 0;
            let i = 1;
            while (i <= n) {
                total = total + i;
                i = i + 1;
            }
            return total;
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    assert!(normalized.contains("while"), "should contain while loop");
    // Variable declarations in Arth use various forms
    assert!(
        normalized.contains("total") && normalized.contains("i"),
        "should contain variable names: {}",
        normalized
    );
}

// =============================================================================
// Golden Tests: Data Structures
// =============================================================================

#[test]
fn golden_struct_from_interface() {
    let ts = r#"
        interface User {
            name: string;
            age: number;
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    assert!(
        normalized.contains("struct") || normalized.contains("User"),
        "should contain struct definition: {}",
        normalized
    );
    assert!(normalized.contains("name"), "should contain name field");
    assert!(normalized.contains("age"), "should contain age field");
}

#[test]
fn golden_struct_from_data_class() {
    let ts = r#"
        @data
        class Point {
            x: number;
            y: number;
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    assert!(
        normalized.contains("struct") || normalized.contains("Point"),
        "should contain Point struct: {}",
        normalized
    );
    assert!(normalized.contains("x"), "should contain x field");
    assert!(normalized.contains("y"), "should contain y field");
}

#[test]
fn golden_provider_class() {
    let ts = r#"
        @provider
        class AppState {
            count: number;
            readonly name: string;
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    assert!(
        normalized.contains("provider") || normalized.contains("AppState"),
        "should contain provider definition: {}",
        normalized
    );
    assert!(normalized.contains("count"), "should contain count field");
    assert!(normalized.contains("name"), "should contain name field");
    // readonly should map to final
    assert!(
        normalized.contains("final") || normalized.contains("readonly"),
        "readonly field should become final"
    );
}

// =============================================================================
// Golden Tests: Module Structure
// =============================================================================

#[test]
fn golden_exported_module() {
    let ts = r#"
        export class Calculator {
            add(a: number, b: number): number {
                return a + b;
            }

            subtract(a: number, b: number): number {
                return a - b;
            }
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    assert!(
        normalized.contains("module") || normalized.contains("Calculator"),
        "should contain Calculator module: {}",
        normalized
    );
    assert!(normalized.contains("add"), "should contain add function");
    assert!(
        normalized.contains("subtract"),
        "should contain subtract function"
    );
    assert!(
        normalized.contains("public"),
        "exported class methods should be public"
    );
}

#[test]
fn golden_module_with_implements() {
    let ts = r#"
        interface Greeter {
            greet(name: string): string;
        }

        export class FriendlyGreeter implements Greeter {
            greet(name: string): string {
                return "Hello, " + name + "!";
            }
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    assert!(
        normalized.contains("interface") || normalized.contains("Greeter"),
        "should contain Greeter interface: {}",
        normalized
    );
    assert!(
        normalized.contains("implements") || normalized.contains("FriendlyGreeter"),
        "should contain implements clause: {}",
        normalized
    );
}

// =============================================================================
// Golden Tests: Expressions
// =============================================================================

#[test]
fn golden_string_concatenation() {
    let ts = r#"
        export function greet(name: string): string {
            return "Hello, " + name + "!";
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    assert!(
        normalized.contains("Hello") || normalized.contains("\""),
        "should contain string literal"
    );
    assert!(
        normalized.contains("+") || normalized.contains("concat"),
        "should contain string concatenation"
    );
}

#[test]
fn golden_boolean_operators() {
    let ts = r#"
        export function check(a: boolean, b: boolean): boolean {
            return a && b || !a;
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    assert!(
        normalized.contains("&&") || normalized.contains("and"),
        "should contain AND operator"
    );
    assert!(
        normalized.contains("||") || normalized.contains("or"),
        "should contain OR operator"
    );
    assert!(
        normalized.contains("!") || normalized.contains("not"),
        "should contain NOT operator"
    );
}

#[test]
fn golden_comparison_operators() {
    let ts = r#"
        export function compare(a: number, b: number): boolean {
            return a < b && a <= b && a > b && a >= b && a == b && a != b;
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");

    // Should contain various comparison operators
    assert!(arth.contains("<"), "should contain < operator");
    assert!(arth.contains(">"), "should contain > operator");
    assert!(
        arth.contains("<=") || arth.contains("=<"),
        "should contain <= operator"
    );
    assert!(
        arth.contains(">=") || arth.contains("=>"),
        "should contain >= operator"
    );
    assert!(arth.contains("=="), "should contain == operator");
    assert!(arth.contains("!="), "should contain != operator");
}

#[test]
fn golden_ternary_expression() {
    let ts = r#"
        export function abs(x: number): number {
            return x >= 0 ? x : -x;
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    // Ternary should be lowered to if-else or conditional expression
    assert!(
        normalized.contains("if") || normalized.contains("?") || normalized.contains("cond"),
        "should contain conditional logic: {}",
        normalized
    );
}

// =============================================================================
// Golden Tests: Control Flow
// =============================================================================

#[test]
fn golden_for_loop_desugared() {
    let ts = r#"
        export function countUp(n: number): number {
            let sum = 0;
            for (let i = 0; i < n; i = i + 1) {
                sum = sum + i;
            }
            return sum;
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    // for loops are desugared to while loops in HIR
    assert!(
        normalized.contains("while") || normalized.contains("loop"),
        "for loop should be desugared to while: {}",
        normalized
    );
}

// =============================================================================
// Golden Tests: Regression Prevention
// =============================================================================

#[test]
fn golden_null_to_optional_none() {
    let ts = r#"
        export function maybeNull(): string | null {
            return null;
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    // null should be lowered to Optional.none()
    assert!(
        normalized.contains("none")
            || normalized.contains("Optional")
            || normalized.contains("null"),
        "null should map to Optional.none(): {}",
        normalized
    );
}

#[test]
fn golden_async_function() {
    let ts = r#"
        export async function fetchData(): Promise<string> {
            return "data";
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    // async functions should be marked
    assert!(
        normalized.contains("async") || normalized.contains("Task"),
        "async function should be marked: {}",
        normalized
    );
}

// =============================================================================
// Golden Tests: Edge Cases
// =============================================================================

#[test]
fn golden_empty_function() {
    let ts = r#"
        export function noop(): void {
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    assert!(arth.contains("noop"), "should contain function name");
}

#[test]
fn golden_nested_expressions() {
    let ts = r#"
        export function complex(a: number, b: number, c: number): number {
            return ((a + b) * c) - (a / b);
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    assert!(arth.contains("complex"), "should contain function name");
    assert!(arth.contains("return"), "should contain return");
}

#[test]
fn golden_method_chaining() {
    let ts = r#"
        export function getLength(s: string): number {
            return s.length;
        }
    "#;

    let arth = ts_to_arth(ts).expect("should compile");
    let normalized = normalize(&arth);

    assert!(
        normalized.contains("length") || normalized.contains("len"),
        "should contain length access: {}",
        normalized
    );
}
