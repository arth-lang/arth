use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn arth_bin() -> Option<PathBuf> {
    if let Some(bin) = std::env::var_os("CARGO_BIN_EXE_arth") {
        return Some(PathBuf::from(bin));
    }

    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join("arth");
    if fallback.exists() {
        Some(fallback)
    } else {
        None
    }
}

fn run_arth(args: &[OsString]) -> Output {
    let bin = arth_bin().expect("arth binary not found; run `cargo build --bin arth` first");
    Command::new(bin)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .expect("failed to execute arth binary")
}

#[test]
fn stdlib_api_reference_has_explicit_i_h_a_classification() {
    let docs = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("docs")
        .join("reference")
        .join("stdlib-api-reference.md");
    let text = fs::read_to_string(&docs).expect("failed to read stdlib-api-reference.md");

    assert!(
        text.contains("| `[I]` | Core intrinsic - VM-backed implementation |"),
        "missing `[I]` legend entry"
    );
    assert!(
        text.contains("| `[H]` | Host-provided intrinsic - requires host capability domain(s) |"),
        "missing `[H]` legend entry"
    );
    assert!(
        text.contains("| `[A]` | Pure Arth helper - source-defined helper body |"),
        "missing `[A]` legend entry"
    );

    // Sanity-check one representative API row for each class.
    assert!(text.contains("| `Task.spawn` | `[I]` |"));
    assert!(text.contains("| `File.open` | `[H]` |"));
    assert!(text.contains("| `Option.some` | `[A]` |"));
}

#[test]
fn task_primitives_execute_in_normal_run_flow() {
    let test_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("conformance")
        .join("concurrency")
        .join("task_primitives_sleep_yield_checkcancel.arth");

    let output = run_arth(&[OsString::from("run"), test_file.as_os_str().to_os_string()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stdout.contains("vm: exit code 0"),
        "expected successful VM run, got stdout:\n{}",
        stdout
    );
    assert!(
        !stderr.contains("undefined symbol"),
        "task primitives should not hit unresolved symbols, stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("\"type\":\"panic\""),
        "task primitives should not panic at runtime, stderr:\n{}",
        stderr
    );
}

#[test]
fn pure_stdlib_helpers_are_not_linked_in_default_run_flow() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock went backwards")
        .as_nanos();
    let source = std::env::temp_dir().join(format!("arth_pure_helper_unlinked_{unique}.arth"));
    let program = r#"package demo;

module Main {
    public static void main() {
        String joined = Path.join("a", "b");
        println(joined);
    }
}
"#;
    fs::write(&source, program).expect("failed to write temporary Arth source");

    let check_output = run_arth(&[OsString::from("check"), source.as_os_str().to_os_string()]);
    let check_stdout = String::from_utf8_lossy(&check_output.stdout);
    let check_stderr = String::from_utf8_lossy(&check_output.stderr);
    assert!(
        check_output.status.success(),
        "check should pass because stdlib signatures are seeded\nstdout:\n{}\nstderr:\n{}",
        check_stdout,
        check_stderr
    );

    let run_output = run_arth(&[OsString::from("run"), source.as_os_str().to_os_string()]);
    let run_stdout = String::from_utf8_lossy(&run_output.stdout);
    let run_stderr = String::from_utf8_lossy(&run_output.stderr);

    // Current VM CLI exits with process status 0 even when VM reports a non-zero exit code.
    // Assert against VM output and panic diagnostics to validate runtime behavior.
    assert!(
        run_stdout.contains("vm: exit code 2"),
        "expected runtime failure for unresolved pure stdlib helper\nstdout:\n{}\nstderr:\n{}",
        run_stdout,
        run_stderr
    );
    assert!(
        run_stderr.contains("undefined symbol 'Path.join'"),
        "expected unresolved symbol for pure helper call\nstdout:\n{}\nstderr:\n{}",
        run_stdout,
        run_stderr
    );

    let _ = fs::remove_file(&source);
}
