//! CLI integration tests for the Arth compiler.
//!
//! These tests verify that each `arth` subcommand works correctly via
//! subprocess invocation, validating argument parsing, exit codes, and
//! basic output for each command.

use std::path::PathBuf;
use std::process::Command;

/// Path to the test fixtures directory.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cli_fixtures")
}

/// Run `cargo run -- <args>` and return (exit_code, stdout, stderr).
fn run_arth(args: &[&str]) -> (i32, String, String) {
    let output = Command::new("cargo")
        .args(["run", "--quiet", "--"])
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to execute cargo run");

    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (code, stdout, stderr)
}

// ────────────────────────────────────────────────────────────────────
// No-args / help
// ────────────────────────────────────────────────────────────────────

#[test]
fn cli_no_args_prints_usage() {
    let (code, _stdout, stderr) = run_arth(&[]);
    // No args prints usage to stderr and exits with code 2 (argument error)
    assert_eq!(code, 2, "no args should exit 2 (usage error)");
    assert!(
        stderr.contains("Usage") || stderr.contains("arth"),
        "should print usage text to stderr"
    );
}

// ────────────────────────────────────────────────────────────────────
// arth lex
// ────────────────────────────────────────────────────────────────────

#[test]
fn cli_lex_valid_file() {
    let path = fixtures_dir().join("valid_hello.arth");
    let (code, stdout, _stderr) = run_arth(&["lex", path.to_str().unwrap()]);
    assert_eq!(code, 0, "lex on valid file should exit 0");
    assert!(
        stdout.contains("PACKAGE") || stdout.contains("IDENT"),
        "lex output should contain token names, got: {}",
        &stdout[..stdout.len().min(200)]
    );
}

#[test]
fn cli_lex_missing_file() {
    let (code, _stdout, stderr) = run_arth(&["lex", "/nonexistent/file.arth"]);
    assert_ne!(code, 0, "lex on missing file should fail");
    assert!(
        stderr.contains("error") || stderr.contains("Error") || stderr.contains("No such file"),
        "should report error for missing file"
    );
}

// ────────────────────────────────────────────────────────────────────
// arth parse
// ────────────────────────────────────────────────────────────────────

#[test]
fn cli_parse_valid_file() {
    let path = fixtures_dir().join("valid_hello.arth");
    let (code, _stdout, _stderr) = run_arth(&["parse", path.to_str().unwrap()]);
    assert_eq!(code, 0, "parse on valid file should exit 0");
}

#[test]
fn cli_parse_invalid_syntax() {
    let path = fixtures_dir().join("invalid_syntax.arth");
    let (code, _stdout, stderr) = run_arth(&["parse", path.to_str().unwrap()]);
    assert_ne!(code, 0, "parse on invalid syntax should fail");
    assert!(
        stderr.contains("error") || stderr.contains("expected"),
        "should report parse error"
    );
}

// ────────────────────────────────────────────────────────────────────
// arth check
// ────────────────────────────────────────────────────────────────────

#[test]
fn cli_check_valid_file() {
    let path = fixtures_dir().join("valid_hello.arth");
    let (code, _stdout, _stderr) = run_arth(&["check", path.to_str().unwrap()]);
    assert_eq!(code, 0, "check on valid file should exit 0");
}

#[test]
fn cli_check_dump_hir() {
    let path = fixtures_dir().join("valid_hello.arth");
    let (code, stdout, _stderr) = run_arth(&["check", "--dump-hir", path.to_str().unwrap()]);
    assert_eq!(code, 0, "check --dump-hir should exit 0");
    assert!(
        stdout.contains("module") || stdout.contains("Module") || stdout.contains("HIR"),
        "should dump HIR output"
    );
}

#[test]
fn cli_check_dump_ir() {
    let path = fixtures_dir().join("valid_hello.arth");
    let (code, stdout, _stderr) = run_arth(&["check", "--dump-ir", path.to_str().unwrap()]);
    assert_eq!(code, 0, "check --dump-ir should exit 0");
    assert!(
        stdout.contains("func") || stdout.contains("block") || stdout.contains("IR"),
        "should dump IR output"
    );
}

// ────────────────────────────────────────────────────────────────────
// arth fmt
// ────────────────────────────────────────────────────────────────────

#[test]
fn cli_fmt_check_well_formatted() {
    let path = fixtures_dir().join("valid_hello.arth");
    let (code, _stdout, _stderr) = run_arth(&["fmt", "--check", path.to_str().unwrap()]);
    // fmt --check exits 0 if already formatted (or returns formatting output)
    // We just verify it doesn't crash
    assert!(
        code == 0 || code == 1,
        "fmt --check should exit 0 or 1, got: {}",
        code
    );
}

// ────────────────────────────────────────────────────────────────────
// arth lint
// ────────────────────────────────────────────────────────────────────

#[test]
fn cli_lint_clean_code() {
    let path = fixtures_dir().join("valid_hello.arth");
    let (code, _stdout, _stderr) = run_arth(&["lint", path.to_str().unwrap()]);
    assert_eq!(code, 0, "lint on clean code should exit 0");
}

#[test]
fn cli_lint_json_output() {
    let path = fixtures_dir().join("valid_hello.arth");
    let (code, _stdout, _stderr) = run_arth(&["lint", "--json", path.to_str().unwrap()]);
    assert_eq!(code, 0, "lint --json should exit 0");
}

// ────────────────────────────────────────────────────────────────────
// arth build
// ────────────────────────────────────────────────────────────────────

#[test]
fn cli_build_vm_valid() {
    let path = fixtures_dir().join("valid_hello.arth");
    let (code, stdout, stderr) = run_arth(&["build", path.to_str().unwrap()]);
    // Build may succeed or fail depending on project structure;
    // at minimum it should not crash
    assert!(
        code == 0 || code == 1,
        "build should exit 0 or 1, got: {} (stdout: {}, stderr: {})",
        code,
        &stdout[..stdout.len().min(200)],
        &stderr[..stderr.len().min(200)]
    );
}

// ────────────────────────────────────────────────────────────────────
// arth test
// ────────────────────────────────────────────────────────────────────

#[test]
fn cli_test_no_tests_found() {
    let path = fixtures_dir().join("valid_hello.arth");
    let (code, stdout, _stderr) = run_arth(&["test", path.to_str().unwrap()]);
    // valid_hello.arth has no @test functions, so "No tests found"
    assert_eq!(code, 0, "test with no @test functions should exit 0");
    assert!(
        stdout.contains("No tests found") || stdout.contains("Found 0"),
        "should report no tests found"
    );
}

// ────────────────────────────────────────────────────────────────────
// arth cache
// ────────────────────────────────────────────────────────────────────

#[test]
fn cli_cache_path() {
    let path = fixtures_dir();
    let (code, stdout, _stderr) = run_arth(&["cache", "path", path.to_str().unwrap()]);
    assert_eq!(code, 0, "cache path should exit 0");
    assert!(
        stdout.contains("arth-cache") || stdout.contains("cache") || !stdout.is_empty(),
        "should print cache path"
    );
}

// ────────────────────────────────────────────────────────────────────
// Unknown command
// ────────────────────────────────────────────────────────────────────

#[test]
fn cli_unknown_command() {
    let (code, _stdout, stderr) = run_arth(&["nonexistent-command"]);
    assert_ne!(code, 0, "unknown command should fail");
    assert!(
        stderr.contains("unknown") || stderr.contains("Unknown") || stderr.contains("Usage"),
        "should report unknown command"
    );
}
