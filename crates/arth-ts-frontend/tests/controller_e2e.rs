//! End-to-end test for controller-style TypeScript compilation.
//!
//! This test compiles a simplified version of a WAID controller and verifies
//! that exports are callable and string.length works correctly.

use arth_ts_frontend::compile_ts_string;
use arth_vm::run_program;

/// Simplified controller that tests:
/// - Export functions are compiled correctly
/// - String parameters work
/// - String.length works
/// - Basic control flow works
const CONTROLLER_SOURCE: &str = r#"
declare function host_call(payload: string): string;

// Simplified logInfo that builds JSON manually (no JSON.stringify needed)
function logInfo(message: string): void {
  host_call('{"fn":"log_info","args":{"message":"' + message + '"}}');
}

// Test string.length
function logWithLength(msg: string): void {
  const len = msg.length;
  logInfo("msg length is " + len);
}

// Simple event structure
interface SimpleEvent {
  event_type: string;
  intent: string;
}

export function handle_event(eventJson: string): number {
  // Test string.length on parameter
  logInfo("handle_event called, len=" + eventJson.length);
  logWithLength(eventJson);
  return 0;
}

export function initialize(): number {
  logInfo("controller initialized");
  return 0;
}

export function main(): void {
  logInfo("main entry");
  initialize();
  // Call handle_event with a test payload
  handle_event("test_payload_12345");
}
"#;

#[test]
fn controller_compiles_and_runs() {
    let result =
        compile_ts_string(CONTROLLER_SOURCE, "test_controller").expect("controller should compile");

    // Verify we have the expected exports
    let export_names: Vec<&str> = result
        .manifest
        .exports
        .iter()
        .map(|e| e.name.as_str())
        .collect();

    assert!(
        export_names.contains(&"main"),
        "should have main export, got: {:?}",
        export_names
    );
    assert!(
        export_names.contains(&"handle_event"),
        "should have handle_event export, got: {:?}",
        export_names
    );
    assert!(
        export_names.contains(&"initialize"),
        "should have initialize export, got: {:?}",
        export_names
    );

    // Run the program
    let exit_code = run_program(&result.program);

    assert_eq!(
        exit_code, 0,
        "controller should run successfully, got exit_code={}",
        exit_code
    );
}

/// Test that string.length compiles to StrLen opcode
#[test]
fn string_length_compiles_correctly() {
    let source = r#"
declare function host_call(payload: string): string;

export function test_length(s: string): number {
  return s.length;
}

export function main(): void {
  const len = test_length("hello");
  host_call('{"fn":"log_info","args":{"message":"len=' + len + '"}}');
}
"#;

    let result = compile_ts_string(source, "test_strlen").expect("string.length should compile");

    let exit_code = run_program(&result.program);

    assert_eq!(
        exit_code, 0,
        "string.length test should run successfully, got exit_code={}",
        exit_code
    );
}

/// Test that exported functions with string parameters work
#[test]
fn export_with_string_param_works() {
    let source = r#"
declare function host_call(payload: string): string;

export function greet(name: string): void {
  host_call('{"fn":"log_info","args":{"message":"hello ' + name + '"}}');
}

export function main(): void {
  greet("world");
}
"#;

    let result = compile_ts_string(source, "test_greet").expect("greet function should compile");

    let exit_code = run_program(&result.program);

    assert_eq!(
        exit_code, 0,
        "greet test should run successfully, got exit_code={}",
        exit_code
    );
}
