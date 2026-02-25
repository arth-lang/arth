//! Tests for untrusted/adversarial input handling.
//!
//! The compiler must produce diagnostics (not panics) for all inputs,
//! including pathologically nested, oversized, or malformed sources.

use std::fs;
use std::process::Command;

use tempfile::TempDir;

fn arth_bin() -> &'static str {
    env!("CARGO_BIN_EXE_arth")
}

fn assert_no_ice(output: &std::process::Output, context: &str) {
    let code = output.status.code().unwrap_or(-1);
    assert_ne!(
        code,
        101,
        "ICE detected (exit 101) for {context}.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}

fn write_temp_arth(content: &str) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let f = dir.path().join("input.arth");
    fs::write(&f, content).unwrap();
    (dir, f)
}

// -----------------------------------------------------------------------
// Deep nesting: expressions
// -----------------------------------------------------------------------

#[test]
fn deeply_nested_parens_does_not_panic() {
    let depth = 500;
    let open: String = "(".repeat(depth);
    let close: String = ")".repeat(depth);
    let src = format!(
        "package test;\nmodule M {{ public void f() {{ Int x = {}1{}; }} }}",
        open, close
    );
    let (_dir, f) = write_temp_arth(&src);
    let out = Command::new(arth_bin())
        .args(["check", f.to_str().unwrap()])
        .output()
        .expect("failed to run arth");
    assert_no_ice(&out, "deeply nested parentheses");
}

// -----------------------------------------------------------------------
// Deep nesting: blocks
// -----------------------------------------------------------------------

#[test]
fn deeply_nested_blocks_does_not_panic() {
    let depth = 500;
    let open: String = "{ if (true) ".repeat(depth);
    let close: String = " }".repeat(depth);
    let src = format!(
        "package test;\nmodule M {{ public void f() {}{}{} }}",
        open, close, ""
    );
    let (_dir, f) = write_temp_arth(&src);
    let out = Command::new(arth_bin())
        .args(["check", f.to_str().unwrap()])
        .output()
        .expect("failed to run arth");
    assert_no_ice(&out, "deeply nested blocks");
}

// -----------------------------------------------------------------------
// Long identifier
// -----------------------------------------------------------------------

#[test]
fn extremely_long_identifier_does_not_panic() {
    let ident = "a".repeat(1_000_000);
    let src = format!(
        "package test;\nmodule M {{ public void f() {{ Int {} = 1; }} }}",
        ident
    );
    let (_dir, f) = write_temp_arth(&src);
    let out = Command::new(arth_bin())
        .args(["check", f.to_str().unwrap()])
        .output()
        .expect("failed to run arth");
    assert_no_ice(&out, "extremely long identifier");
}

// -----------------------------------------------------------------------
// Binary file as source
// -----------------------------------------------------------------------

#[test]
fn binary_file_as_source_does_not_panic() {
    let dir = TempDir::new().unwrap();
    let f = dir.path().join("binary.arth");
    let data: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
    fs::write(&f, &data).unwrap();
    let out = Command::new(arth_bin())
        .args(["check", f.to_str().unwrap()])
        .output()
        .expect("failed to run arth");
    assert_no_ice(&out, "binary file as source");
}

// -----------------------------------------------------------------------
// Zero-byte / whitespace-only
// -----------------------------------------------------------------------

#[test]
fn zero_byte_file_does_not_panic() {
    let (_dir, f) = write_temp_arth("");
    let out = Command::new(arth_bin())
        .args(["check", f.to_str().unwrap()])
        .output()
        .expect("failed to run arth");
    assert_no_ice(&out, "zero-byte file");
}

#[test]
fn whitespace_only_file_does_not_panic() {
    let (_dir, f) = write_temp_arth("   \n\n\t\t\n   ");
    let out = Command::new(arth_bin())
        .args(["check", f.to_str().unwrap()])
        .output()
        .expect("failed to run arth");
    assert_no_ice(&out, "whitespace-only file");
}

// -----------------------------------------------------------------------
// Pathological repetition
// -----------------------------------------------------------------------

#[test]
fn million_semicolons_does_not_panic() {
    let src = format!("package test;\n{}", ";".repeat(1_000_000));
    let (_dir, f) = write_temp_arth(&src);
    let out = Command::new(arth_bin())
        .args(["check", f.to_str().unwrap()])
        .output()
        .expect("failed to run arth");
    assert_no_ice(&out, "million semicolons");
}

// -----------------------------------------------------------------------
// Oversized source file
// -----------------------------------------------------------------------

#[test]
fn oversized_source_file_rejected() {
    let dir = TempDir::new().unwrap();
    let f = dir.path().join("huge.arth");
    // Create 11 MB file (over default 10 MB limit)
    let data = "// padding\n".repeat(1_100_000);
    fs::write(&f, &data).unwrap();
    let out = Command::new(arth_bin())
        .args(["check", f.to_str().unwrap()])
        .output()
        .expect("failed to run arth");
    assert_no_ice(&out, "oversized source file");
    // Should produce an error (not succeed)
    assert!(
        !out.status.success(),
        "should reject oversized file but exited successfully"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("too large"),
        "error should mention file is too large, got: {}",
        stderr
    );
}
