//! Reproducibility regression tests.
//!
//! Verifies that the Arth compiler produces identical output for
//! identical inputs across multiple runs.

use std::fs;
use std::process::Command;

use tempfile::TempDir;

fn arth_bin() -> &'static str {
    env!("CARGO_BIN_EXE_arth")
}

/// Compile a single .arth file and return the .abc bytecode bytes.
///
/// Sets the working directory to the temp dir so `target/arth-out/app.abc`
/// lands in a predictable location per invocation.
fn compile_to_abc(work_dir: &std::path::Path, source_file: &std::path::Path) -> Vec<u8> {
    let out = Command::new(arth_bin())
        .args(["build", "--no-incremental", source_file.to_str().unwrap()])
        .current_dir(work_dir)
        .output()
        .expect("failed to run arth build");
    assert!(
        out.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let abc_path = work_dir.join("target/arth-out/app.abc");
    assert!(abc_path.exists(), "expected .abc at {}", abc_path.display());
    fs::read(&abc_path).expect("failed to read .abc")
}

#[test]
fn same_source_produces_identical_bytecode() {
    let src_dir = TempDir::new().unwrap();
    let source = src_dir.path().join("Main.arth");
    fs::write(
        &source,
        "package test;\n\nmodule Main {\n  public void main() {\n    Int x = 42;\n  }\n}\n",
    )
    .unwrap();

    // First compilation
    let work1 = TempDir::new().unwrap();
    let bytes1 = compile_to_abc(work1.path(), &source);

    // Second compilation (clean output dir)
    let work2 = TempDir::new().unwrap();
    let bytes2 = compile_to_abc(work2.path(), &source);

    assert_eq!(
        bytes1, bytes2,
        "bytecode differs between identical compilations"
    );
}

#[test]
fn different_source_produces_different_bytecode() {
    let src_dir = TempDir::new().unwrap();
    let source_a = src_dir.path().join("A.arth");
    let source_b = src_dir.path().join("B.arth");
    fs::write(
        &source_a,
        "package test;\n\nmodule Main {\n  public void main() {\n    Int x = 1;\n  }\n}\n",
    )
    .unwrap();
    fs::write(
        &source_b,
        "package test;\n\nmodule Main {\n  public void main() {\n    Int x = 2;\n  }\n}\n",
    )
    .unwrap();

    let work1 = TempDir::new().unwrap();
    let bytes_a = compile_to_abc(work1.path(), &source_a);

    let work2 = TempDir::new().unwrap();
    let bytes_b = compile_to_abc(work2.path(), &source_b);

    assert_ne!(
        bytes_a, bytes_b,
        "different sources should produce different bytecode"
    );
}
