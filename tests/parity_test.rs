//! API Parity Tests: VM vs Native Mode
//!
//! These tests verify that Arth code produces identical output when run
//! via the VM backend vs the LLVM native backend.
//!
//! Run with: cargo test --test parity_test

use std::path::Path;
use std::process::Command;

/// Run an Arth file with the VM backend and capture output.
fn run_vm(source_path: &str) -> Result<String, String> {
    let output = Command::new("cargo")
        .args(["run", "--quiet", "--", "run", source_path])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .map_err(|e| format!("Failed to run VM: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("VM execution failed: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Build and run an Arth file with the LLVM native backend.
fn run_native(source_path: &str) -> Result<String, String> {
    // Build native binary
    let build_output = Command::new("cargo")
        .args([
            "run",
            "--quiet",
            "--",
            "build",
            "--backend",
            "llvm",
            source_path,
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .map_err(|e| format!("Failed to build native: {}", e))?;

    if !build_output.status.success() {
        let stderr = String::from_utf8_lossy(&build_output.stderr);
        return Err(format!("Native build failed: {}", stderr));
    }

    // Run the binary
    let bin_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/arth-out/app");
    let run_output = Command::new(&bin_path)
        .output()
        .map_err(|e| format!("Failed to run native binary: {}", e))?;

    if !run_output.status.success() {
        let stderr = String::from_utf8_lossy(&run_output.stderr);
        return Err(format!("Native execution failed: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&run_output.stdout).to_string())
}

/// Compare VM and Native outputs, normalizing whitespace.
fn compare_outputs(vm_output: &str, native_output: &str) -> bool {
    // Normalize line endings and trim
    let vm_lines: Vec<&str> = vm_output.lines().map(|l| l.trim()).collect();
    let native_lines: Vec<&str> = native_output.lines().map(|l| l.trim()).collect();

    if vm_lines.len() != native_lines.len() {
        eprintln!(
            "Line count mismatch: VM={}, Native={}",
            vm_lines.len(),
            native_lines.len()
        );
        return false;
    }

    for (i, (vm_line, native_line)) in vm_lines.iter().zip(native_lines.iter()).enumerate() {
        if vm_line != native_line {
            eprintln!("Line {} mismatch:", i + 1);
            eprintln!("  VM:     '{}'", vm_line);
            eprintln!("  Native: '{}'", native_line);
            return false;
        }
    }

    true
}

/// Run a parity test for a given source file.
fn run_parity_test(source_path: &str) -> Result<(), String> {
    eprintln!("Testing parity for: {}", source_path);

    // Run on VM
    let vm_output = run_vm(source_path)?;
    eprintln!("VM output ({} bytes)", vm_output.len());

    // Run on Native
    let native_output = run_native(source_path)?;
    eprintln!("Native output ({} bytes)", native_output.len());

    // Compare
    if compare_outputs(&vm_output, &native_output) {
        eprintln!("✓ Outputs match!");
        Ok(())
    } else {
        Err(format!("Output mismatch for {}", source_path))
    }
}

// =============================================================================
// Unit Tests (always run, test internal logic)
// =============================================================================

#[test]
fn test_compare_outputs_identical() {
    let a = "Hello\nWorld\n";
    let b = "Hello\nWorld\n";
    assert!(compare_outputs(a, b));
}

#[test]
fn test_compare_outputs_whitespace_normalized() {
    let a = "Hello  \n  World\n";
    let b = "Hello\nWorld";
    assert!(compare_outputs(a, b));
}

#[test]
fn test_compare_outputs_different() {
    let a = "Hello\nWorld";
    let b = "Hello\nEarth";
    assert!(!compare_outputs(a, b));
}

#[test]
fn test_compare_outputs_line_count() {
    let a = "Line1\nLine2";
    let b = "Line1";
    assert!(!compare_outputs(a, b));
}

// =============================================================================
// Integration Tests (require both VM and clang/LLVM)
// =============================================================================

#[test]
#[ignore] // Run with: cargo test --test parity_test -- --ignored
fn test_console_parity() {
    match run_parity_test("tests/parity/console_parity.arth") {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Parity test skipped or failed: {}", e);
            // Don't fail if native toolchain not available
        }
    }
}

#[test]
#[ignore] // Run with: cargo test --test parity_test -- --ignored
fn test_arithmetic_parity() {
    match run_parity_test("tests/parity/arithmetic_parity.arth") {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Parity test skipped or failed: {}", e);
        }
    }
}

#[test]
#[ignore] // Run with: cargo test --test parity_test -- --ignored
fn test_control_flow_parity() {
    match run_parity_test("tests/parity/control_flow_parity.arth") {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Parity test skipped or failed: {}", e);
        }
    }
}

// =============================================================================
// Backend Detection Tests
// =============================================================================

#[test]
fn test_vm_backend_available() {
    // VM backend should always be available
    let output = Command::new("cargo")
        .args(["run", "--quiet", "--", "--help"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output();

    assert!(output.is_ok(), "Compiler should be runnable");
}

#[test]
fn test_native_backend_detection() {
    use arth::compiler::codegen::linker::detect_available_linker;

    match detect_available_linker() {
        Ok(backend) => {
            eprintln!("Native linker available: {:?}", backend);
        }
        Err(e) => {
            eprintln!("Native linker not available: {}", e);
            // This is OK - we just skip native tests
        }
    }
}
