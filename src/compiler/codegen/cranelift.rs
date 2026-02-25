//! Cranelift Backend for Arth Compiler
//!
//! This module provides Cranelift-based code generation.
//!
//! NOTE: The JIT compiler is implemented in the arth-vm crate (crates/arth-vm/src/jit.rs).
//! This module is for ahead-of-time Cranelift compilation, which is not yet implemented.
//!
//! For JIT compilation at runtime, use the `jit` module in `arth-vm` with
//! the `jit` feature enabled:
//!
//! ```toml
//! [dependencies]
//! arth-vm = { features = ["jit"] }
//! ```

use crate::compiler::ir::Module;

/// Compile an IR module to native code using Cranelift.
///
/// NOTE: This is a placeholder. The actual JIT implementation is in arth-vm.
/// This function is for ahead-of-time compilation which is not yet implemented.
#[cfg(feature = "cranelift")]
pub fn compile_module_with_message(_m: &Module, message: Option<&str>) -> Result<(), String> {
    let _ = message;
    Err(String::from(
        "Cranelift AOT compilation is not yet implemented. \
         For JIT compilation, use arth-vm with the 'jit' feature.",
    ))
}

#[cfg(not(feature = "cranelift"))]
pub fn compile_module_with_message(_m: &Module, _message: Option<&str>) -> Result<(), String> {
    Err(String::from(
        "Cranelift backend is not enabled. Rebuild with `--features cranelift`.",
    ))
}
