//! Phase 9: Event dispatch round-trip tests.
//!
//! These tests verify that:
//! 1. Controllers with event handlers compile correctly
//! 2. Handler metadata is correctly extracted
//! 3. The compiled bytecode can be executed on the VM
//! 4. Event handlers can be dispatched with the correct parameters

use arth_ts_frontend::{
    TsLoweringOptions, compile_ts_string, extract_controller_registry, lower_ts_str_to_hir,
};
use arth_vm::run_program;

// =============================================================================
// Event Handler Compilation Tests
// =============================================================================

#[test]
fn test_click_handler_compiles() {
    let source = r#"
        declare function host_call(payload: string): string;

        export class ButtonController {
            onClick(event: string): void {
                host_call('{"fn":"log","message":"clicked"}');
            }
        }
    "#;

    let result = compile_ts_string(source, "button").expect("should compile");

    // Verify the handler is in the exports
    let has_onclick = result.manifest.exports.iter().any(|e| {
        e.name == "onClick"
            || e.qualified_name
                .as_ref()
                .is_some_and(|q| q.contains("onClick"))
    });
    assert!(has_onclick, "should have onClick export");
}

#[test]
fn test_multiple_handlers_compile() {
    let source = r#"
        declare function host_call(payload: string): string;

        export class FormController {
            onSubmit(data: string): number {
                host_call('{"fn":"submit","data":"' + data + '"}');
                return 0;
            }

            onReset(): void {
                host_call('{"fn":"reset"}');
            }

            onChange(field: string, value: string): void {
                host_call('{"fn":"change","field":"' + field + '","value":"' + value + '"}');
            }
        }
    "#;

    let result = compile_ts_string(source, "form").expect("should compile");

    // Verify all handlers are exported
    let exports: Vec<&str> = result
        .manifest
        .exports
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert!(
        exports.contains(&"onSubmit"),
        "should have onSubmit: {:?}",
        exports
    );
    assert!(
        exports.contains(&"onReset"),
        "should have onReset: {:?}",
        exports
    );
    assert!(
        exports.contains(&"onChange"),
        "should have onChange: {:?}",
        exports
    );
}

// =============================================================================
// Handler Metadata Extraction Tests
// =============================================================================

#[test]
fn test_handler_arity_extracted() {
    let source = r#"
        export class TestController {
            zeroParams(): void {}
            oneParam(a: string): void {}
            twoParams(a: string, b: number): void {}
            threeParams(a: string, b: number, c: boolean): void {}
        }
    "#;

    let hirs = vec![
        lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default()).expect("should lower"),
    ];
    let registry = extract_controller_registry(&hirs);

    let controller = registry
        .controllers
        .get("TestController")
        .expect("should have TestController");

    // Check arities
    assert_eq!(
        controller.handlers.get("zeroParams").map(|h| h.arity),
        Some(0),
        "zeroParams should have arity 0"
    );
    assert_eq!(
        controller.handlers.get("oneParam").map(|h| h.arity),
        Some(1),
        "oneParam should have arity 1"
    );
    assert_eq!(
        controller.handlers.get("twoParams").map(|h| h.arity),
        Some(2),
        "twoParams should have arity 2"
    );
    assert_eq!(
        controller.handlers.get("threeParams").map(|h| h.arity),
        Some(3),
        "threeParams should have arity 3"
    );
}

#[test]
fn test_handler_param_types_extracted() {
    let source = r#"
        export class TypedController {
            handleClick(x: number, y: number): void {}
            handleInput(text: string): string { return text; }
            handleFlag(enabled: boolean): boolean { return enabled; }
        }
    "#;

    let hirs = vec![
        lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default()).expect("should lower"),
    ];
    let registry = extract_controller_registry(&hirs);

    let controller = registry
        .controllers
        .get("TypedController")
        .expect("should have TypedController");

    // Check handleClick params
    let click_handler = controller
        .handlers
        .get("handleClick")
        .expect("should have handleClick");
    assert_eq!(
        click_handler.params.len(),
        2,
        "handleClick should have 2 params"
    );
    assert_eq!(click_handler.params[0].name, "x");
    assert_eq!(click_handler.params[1].name, "y");

    // Check handleInput params
    let input_handler = controller
        .handlers
        .get("handleInput")
        .expect("should have handleInput");
    assert_eq!(
        input_handler.params.len(),
        1,
        "handleInput should have 1 param"
    );
    assert_eq!(input_handler.params[0].name, "text");
}

#[test]
fn test_async_handler_detected() {
    let source = r#"
        export class AsyncController {
            async fetchData(): Promise<string> {
                return "data";
            }

            syncMethod(): string {
                return "sync";
            }
        }
    "#;

    let hirs = vec![
        lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default()).expect("should lower"),
    ];
    let registry = extract_controller_registry(&hirs);

    let controller = registry
        .controllers
        .get("AsyncController")
        .expect("should have AsyncController");

    assert!(
        controller
            .handlers
            .get("fetchData")
            .map(|h| h.is_async)
            .unwrap_or(false),
        "fetchData should be async"
    );
    assert!(
        !controller
            .handlers
            .get("syncMethod")
            .map(|h| h.is_async)
            .unwrap_or(true),
        "syncMethod should NOT be async"
    );
}

// =============================================================================
// VM Execution Tests
// =============================================================================

#[test]
fn test_handler_executes_on_vm() {
    // Note: Classes compile to modules in Arth, so we call functions directly
    // from the module (not as instance methods)
    let source = r#"
        declare function host_call(payload: string): string;

        export function increment(n: number): number {
            return n + 1;
        }

        export function main(): void {
            const result = increment(5);
            host_call('{"result":' + result + '}');
        }
    "#;

    let result = compile_ts_string(source, "counter").expect("should compile");
    let exit_code = run_program(&result.program);

    assert_eq!(exit_code, 0, "should execute successfully");
}

#[test]
fn test_handler_with_string_param_executes() {
    let source = r#"
        declare function host_call(payload: string): string;

        export function log(message: string): void {
            host_call('{"fn":"log","msg":"' + message + '"}');
        }

        export function main(): void {
            log("hello world");
        }
    "#;

    let result = compile_ts_string(source, "logger").expect("should compile");
    let exit_code = run_program(&result.program);

    assert_eq!(exit_code, 0, "should execute successfully");
}

#[test]
fn test_handler_with_return_value() {
    let source = r#"
        declare function host_call(payload: string): string;

        export function add(a: number, b: number): number {
            return a + b;
        }

        export function multiply(a: number, b: number): number {
            return a * b;
        }

        export function main(): void {
            const sum = add(3, 4);
            const product = multiply(sum, 2);
            host_call('{"result":' + product + '}');
        }
    "#;

    let result = compile_ts_string(source, "calc").expect("should compile");
    let exit_code = run_program(&result.program);

    assert_eq!(exit_code, 0, "should execute successfully");
}

// =============================================================================
// Event Dispatch Integration Tests
// =============================================================================

#[test]
fn test_event_handler_dispatch_pattern() {
    // This tests the pattern used by the Rune runtime to dispatch events
    // Functions are called directly (classes compile to modules)
    let source = r#"
        declare function host_call(payload: string): string;

        interface Event {
            type: string;
            x: number;
            y: number;
        }

        // Simplified handler without string comparison (VM string comparison is WIP)
        export function handleEvent(x: number, y: number): number {
            if (x > 0) {
                host_call('{"fn":"click","x":' + x + ',"y":' + y + '}');
                return 1;
            }
            return 0;
        }

        export function main(): void {
            handleEvent(100, 200);
        }
    "#;

    let result = compile_ts_string(source, "ui").expect("should compile");
    let exit_code = run_program(&result.program);

    assert_eq!(exit_code, 0, "should execute successfully");
}

#[test]
fn test_controller_registry_matches_exports() {
    let source = r#"
        export class EventHandler {
            onClick(): void {}
            onHover(): void {}
            onScroll(delta: number): void {}
        }
    "#;

    let result = compile_ts_string(source, "events").expect("should compile");

    // Controller registry handlers should match manifest exports
    let export_names: std::collections::HashSet<String> = result
        .manifest
        .exports
        .iter()
        .filter(|e| e.kind == "func")
        .map(|e| e.name.clone())
        .collect();

    let registry_handlers: std::collections::HashSet<String> = result
        .controller_registry
        .controllers
        .values()
        .flat_map(|c| c.handlers.keys().cloned())
        .collect();

    // All registry handlers should be in exports
    for handler in &registry_handlers {
        assert!(
            export_names.contains(handler),
            "Registry handler '{}' should be in exports. Exports: {:?}",
            handler,
            export_names
        );
    }
}

// =============================================================================
// Error Case Tests
// =============================================================================

#[test]
fn test_handler_with_invalid_this_access_fails() {
    let source = r#"
        export class BadController {
            state = { count: 0 };

            increment(): void {
                this.state.count = this.state.count + 1;
            }
        }
    "#;

    let result = compile_ts_string(source, "bad");
    assert!(
        result.is_err(),
        "Controller with this.state should fail to compile"
    );
}

#[test]
fn test_handler_with_class_fields_fails() {
    let source = r#"
        export class BadController {
            cache: Map<string, number>;

            lookup(key: string): number {
                return 0;
            }
        }
    "#;

    let result = compile_ts_string(source, "bad");
    assert!(
        result.is_err(),
        "Controller with class fields should fail to compile"
    );
}
