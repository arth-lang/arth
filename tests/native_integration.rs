//! Integration tests for native compilation via LLVM backend.
//!
//! These tests verify that the LLVM backend correctly links with arth-rt
//! and produces working native binaries.

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

fn native_build_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Helper to build a native binary from an Arth source file.
fn build_native(source_path: &str) -> Result<PathBuf, String> {
    // Run arth build --backend llvm
    let output = Command::new("cargo")
        .args(["run", "--", "build", "--backend", "llvm", source_path])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .map_err(|e| format!("Failed to execute cargo run: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Build failed: {}", stderr));
    }

    Ok(PathBuf::from("target/arth-out/app"))
}

/// Helper to run a native binary and capture output.
fn run_native(bin_path: &PathBuf) -> Result<String, String> {
    let output = Command::new(bin_path)
        .output()
        .map_err(|e| format!("Failed to execute binary: {}", e))?;

    let mut combined = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.stderr.is_empty() {
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
    }

    Ok(combined)
}

fn run_native_case(source_path: &str, required_markers: &[&str]) {
    let _guard = native_build_lock().lock().expect("native lock poisoned");
    let bin_path = build_native(source_path)
        .unwrap_or_else(|e| panic!("native build failed for {source_path}: {e}"));
    let output = run_native(&bin_path)
        .unwrap_or_else(|e| panic!("native execution failed for {source_path}: {e}"));
    for marker in required_markers {
        assert!(
            output.contains(marker),
            "expected marker '{marker}' in output for {source_path}, got:\n{output}"
        );
    }
}

#[test]
#[ignore] // Run with: cargo test --test native_integration -- --ignored
fn test_hello_native() {
    run_native_case(
        "tests/native/hello_native.arth",
        &["Hello from native Arth!"],
    );
}

#[test]
#[ignore] // Run with: cargo test --test native_integration test_native_struct_field_integration -- --ignored
fn test_native_struct_field_integration() {
    run_native_case("tests/native/phase1_struct.arth", &["PHASE1_STRUCT_OK"]);
}

#[test]
#[ignore] // Run with: cargo test --test native_integration test_native_control_flow_integration -- --ignored
fn test_native_control_flow_integration() {
    run_native_case(
        "tests/native/phase1_control_flow.arth",
        &["PHASE1_CONTROL_FLOW_OK"],
    );
}

#[test]
#[ignore] // Run with: cargo test --test native_integration test_native_enum_pattern_integration -- --ignored
fn test_native_enum_pattern_integration() {
    run_native_case("tests/native/phase1_enum.arth", &["PHASE1_ENUM_OK"]);
}

#[test]
#[ignore] // Run with: cargo test --test native_integration test_native_exception_try_catch_integration -- --ignored
fn test_native_exception_try_catch_integration() {
    run_native_case(
        "tests/native/phase1_exception.arth",
        &["PHASE1_EXCEPTION_OK"],
    );
}

#[test]
#[ignore] // Run with: cargo test --test native_integration test_native_closure_capture_integration -- --ignored
fn test_native_closure_capture_integration() {
    run_native_case("tests/native/phase1_closure.arth", &["PHASE1_CLOSURE_OK"]);
}

#[test]
#[ignore] // Run with: cargo test --test native_integration test_native_provider_field_integration -- --ignored
fn test_native_provider_field_integration() {
    run_native_case("tests/native/phase1_provider.arth", &["PHASE1_PROVIDER_OK"]);
}

#[test]
#[ignore] // Run with: cargo test --test native_integration test_native_async_task_integration -- --ignored
fn test_native_async_task_integration() {
    run_native_case("tests/native/phase1_async_task.arth", &["PHASE1_ASYNC_OK"]);
}

#[test]
#[ignore] // Run with: cargo test --test native_integration test_native_executor_spawn_join_integration -- --ignored
fn test_native_executor_spawn_join_integration() {
    run_native_case(
        "tests/native/phase1_executor_spawn.arth",
        &["PHASE1_SPAWN_OK"],
    );
}

#[test]
#[ignore] // Run with: cargo test --test native_integration test_native_executor_cancel_integration -- --ignored
fn test_native_executor_cancel_integration() {
    run_native_case(
        "tests/native/phase1_executor_cancel.arth",
        &["PHASE1_CANCEL_OK"],
    );
}

#[test]
#[ignore] // Run with: cargo test --test native_integration test_native_region_scope_integration -- --ignored --test-threads=1
fn test_native_region_scope_integration() {
    run_native_case("tests/native/phase1_region.arth", &["PHASE1_REGION_OK"]);
}

#[test]
#[ignore] // Run with: cargo test --test native_integration test_native_abi_struct_passing_integration -- --ignored --test-threads=1
fn test_native_abi_struct_passing_integration() {
    run_native_case(
        "tests/native/phase1_abi_struct_passing.arth",
        &["PHASE1_ABI_STRUCT_OK"],
    );
}

#[test]
fn test_llvm_ir_emission() {
    // Test that LLVM IR can be emitted correctly
    use arth::compiler::codegen::llvm_text::emit_module_text;
    use arth::compiler::ir::{
        BlockData, Func, Inst, InstKind, Linkage, Module as IrModule, Terminator, Ty, Value,
    };

    // Create a minimal module with a main function that calls console output
    let mut module = IrModule::new("test");
    module.strings = vec!["Hello, World!".to_string()];

    // Build a simple main function
    let blocks = vec![BlockData {
        name: "entry".to_string(),
        insts: vec![
            // Load string constant
            Inst {
                kind: InstKind::ConstStr(0),
                result: Value(0),
                span: None,
            },
            // Get string length (for demo, just use a constant)
            Inst {
                kind: InstKind::ConstI64(13),
                result: Value(1),
                span: None,
            },
            // Call console write (intrinsic)
            Inst {
                kind: InstKind::Call {
                    name: "__arth_console_write".to_string(),
                    args: vec![Value(0), Value(1)],
                    ret: Ty::I64,
                },
                result: Value(2),
                span: None,
            },
        ],
        term: Terminator::Ret(Some(Value(2))),
        span: None,
    }];

    let func = Func {
        name: "main".to_string(),
        params: vec![],
        ret: Ty::I64,
        blocks,
        linkage: Linkage::External,
        span: None,
    };
    module.funcs.push(func);

    // Emit LLVM IR
    let ir = emit_module_text(&module);

    // Verify the IR contains expected elements
    assert!(
        ir.contains("@.s0 = private unnamed_addr constant"),
        "Missing string constant"
    );
    assert!(ir.contains("define i64 @main()"), "Missing main function");
    assert!(
        ir.contains("call i64 @arth_rt_console_write"),
        "Missing arth_rt call"
    );
    assert!(
        ir.contains("declare i64 @arth_rt_console_write"),
        "Missing arth_rt declaration"
    );
}

#[test]
fn test_linker_config() {
    use arth::compiler::codegen::linker::{LinkerBackend, LinkerConfig};

    // Test default linker configuration
    let config = LinkerConfig::default();
    assert_eq!(config.optimization_level, 2);
    assert_eq!(config.backend, LinkerBackend::Clang);
    assert!(!config.debug_info);
    assert!(!config.static_link);
}

#[test]
fn test_cfg_backend_for_native() {
    use arth::compiler::attrs::CfgBackend;

    // Test that CfgBackend has LLVM option for native compilation
    assert_eq!(CfgBackend::from_name("llvm"), Some(CfgBackend::Llvm));
    assert_eq!(CfgBackend::from_name("native"), Some(CfgBackend::Llvm));
    assert_eq!(CfgBackend::from_name("vm"), Some(CfgBackend::Vm));
    assert_eq!(
        CfgBackend::from_name("cranelift"),
        Some(CfgBackend::Cranelift)
    );
}

/// Test that emit_module_text_with_debug produces LLVM IR with DWARF metadata.
/// This is a unit-level end-to-end test that verifies the full debug emission pipeline.
#[test]
fn test_debug_info_emits_dwarf_metadata() {
    use arth::compiler::codegen::llvm_debug::{DebugInfoBuilder, SourceLineTable};
    use arth::compiler::codegen::llvm_text::emit_module_text_with_debug;
    use arth::compiler::ir::{
        BinOp, BlockData, Func, Inst, InstKind, Linkage, Module, Span, Terminator, Ty, Value,
    };
    use std::sync::Arc;

    let src = "package demo;\nfun main(): Int {\n  val x = 1 + 2;\n  return x;\n}\n";
    let file = Arc::new(PathBuf::from("/project/src/demo/Main.arth"));

    let a = Value(0);
    let b = Value(1);
    let res = Value(2);

    let inst_a = Inst {
        result: a,
        kind: InstKind::ConstI64(1),
        span: Some(Span::new(file.clone(), 34, 35)),
    };
    let inst_b = Inst {
        result: b,
        kind: InstKind::ConstI64(2),
        span: Some(Span::new(file.clone(), 38, 39)),
    };
    let inst_add = Inst {
        result: res,
        kind: InstKind::Binary(BinOp::Add, a, b),
        span: Some(Span::new(file.clone(), 34, 39)),
    };

    let block = BlockData {
        name: "entry".into(),
        insts: vec![inst_a, inst_b, inst_add],
        term: Terminator::Ret(Some(res)),
        span: Some(Span::new(file.clone(), 14, 58)),
    };

    let func = Func {
        name: "main".into(),
        params: vec![],
        ret: Ty::I64,
        blocks: vec![block],
        linkage: Linkage::External,
        span: Some(Span::new(file.clone(), 14, 58)),
    };

    let mut m = Module::new("demo");
    m.funcs.push(func);

    let table =
        SourceLineTable::from_sources(&[(PathBuf::from("/project/src/demo/Main.arth"), src)]);
    let mut debug = DebugInfoBuilder::new("arth 0.1.0", "/project", table);
    let ir = emit_module_text_with_debug(&m, Some("aarch64-apple-darwin"), &mut debug);

    // Verify essential DWARF metadata is present
    assert!(ir.contains("!llvm.dbg.cu"), "missing compile unit");
    assert!(ir.contains("!llvm.module.flags"), "missing module flags");
    assert!(ir.contains("Dwarf Version"), "missing DWARF version flag");
    assert!(
        ir.contains("Debug Info Version"),
        "missing debug info version"
    );
    assert!(ir.contains("DICompileUnit"), "missing DICompileUnit");
    assert!(ir.contains("DIFile"), "missing DIFile");
    assert!(ir.contains("DISubprogram"), "missing DISubprogram");
    assert!(ir.contains("DILocation"), "missing DILocation");
    assert!(ir.contains("DIBasicType"), "missing DIBasicType");

    // Function should have debug attachment
    assert!(ir.contains("!dbg"), "no !dbg annotations");

    // Subprogram should reference "main"
    assert!(ir.contains("name: \"main\""), "missing main subprogram");
}
