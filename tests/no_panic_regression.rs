//! Regression tests that verify the Arth compiler never exits with code 101
//! (internal compiler error / panic) on adversarial inputs.
//!
//! Exit code 101 means a panic slipped past the `catch_unwind` boundary in
//! `main.rs` -- or worse, there was no boundary at all.  These tests ensure
//! that every CLI command handles bad inputs gracefully and returns a normal
//! (non-ICE) exit code.

use std::fs;
use std::process::Command;

use tempfile::TempDir;

/// Helper: the arth binary path, resolved by Cargo at compile time.
fn arth_bin() -> &'static str {
    env!("CARGO_BIN_EXE_arth")
}

/// Assert that the command did NOT exit with code 101 (ICE).
fn assert_no_ice(output: &std::process::Output, context: &str) {
    let code = output.status.code().unwrap_or(-1);
    assert_ne!(
        code,
        101,
        "ICE detected (exit 101) for {context}.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}

// ---------------------------------------------------------------------------
// lex
// ---------------------------------------------------------------------------

#[test]
fn lex_nonexistent_file() {
    let output = Command::new(arth_bin())
        .args(["lex", "/tmp/__arth_no_such_file_ever__.arth"])
        .output()
        .expect("failed to run arth");

    assert_no_ice(&output, "lex on nonexistent file");
    // Should fail, but not with 101
    assert!(!output.status.success());
}

// ---------------------------------------------------------------------------
// parse
// ---------------------------------------------------------------------------

#[test]
fn parse_empty_file() {
    let dir = TempDir::new().expect("failed to create temp dir");
    let file = dir.path().join("empty.arth");
    fs::write(&file, "").expect("failed to write empty file");

    let output = Command::new(arth_bin())
        .args(["parse", file.to_str().unwrap()])
        .output()
        .expect("failed to run arth");

    assert_no_ice(&output, "parse on empty file");
}

// ---------------------------------------------------------------------------
// check
// ---------------------------------------------------------------------------

#[test]
fn check_binary_garbage() {
    let dir = TempDir::new().expect("failed to create temp dir");
    let file = dir.path().join("garbage.arth");
    // Write non-UTF-8 binary garbage
    let garbage: Vec<u8> = (0..256).map(|i| i as u8).collect();
    fs::write(&file, &garbage).expect("failed to write garbage file");

    let output = Command::new(arth_bin())
        .args(["check", file.to_str().unwrap()])
        .output()
        .expect("failed to run arth");

    assert_no_ice(&output, "check on binary garbage");
}

// ---------------------------------------------------------------------------
// build
// ---------------------------------------------------------------------------

#[test]
fn build_invalid_syntax() {
    let dir = TempDir::new().expect("failed to create temp dir");
    let file = dir.path().join("bad.arth");
    fs::write(&file, "{{{{{{ not valid arth code !!!!! }}}}}}").expect("failed to write file");

    let output = Command::new(arth_bin())
        .args(["build", file.to_str().unwrap()])
        .output()
        .expect("failed to run arth");

    assert_no_ice(&output, "build on invalid syntax");
    assert!(!output.status.success());
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

#[test]
fn run_nonexistent_file() {
    let output = Command::new(arth_bin())
        .args(["run", "/tmp/__arth_no_such_file_ever__.arth"])
        .output()
        .expect("failed to run arth");

    assert_no_ice(&output, "run on nonexistent file");
    assert!(!output.status.success());
}
