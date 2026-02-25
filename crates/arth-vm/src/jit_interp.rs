//! JIT-Interpreter Integration
//!
//! This module provides the glue code between the JIT compiler and the interpreter.
//! It handles:
//! - Function registration for JIT tracking
//! - Hot path detection and tiered compilation triggering
//! - Native code dispatch for JIT-compiled functions
//!
//! # Tiered Compilation
//!
//! Functions progress through compilation tiers based on call frequency:
//! - **Tier 0 (Interpreter)**: Cold start, full feature support
//! - **Tier 1 (Baseline JIT)**: After `TIER1_THRESHOLD` calls, fast compilation
//! - **Tier 2 (Optimized JIT)**: After `TIER2_THRESHOLD` more calls, full optimization
//!
//! The interpreter falls back to bytecode interpretation for:
//! - Functions that haven't been compiled yet
//! - Functions containing unsupported opcodes
//! - The first N calls before reaching the JIT threshold

use crate::jit::{JitError, init_jit, with_jit};
use crate::program::Program;

/// Result of attempting a JIT call.
pub enum JitCallResult {
    /// Function was executed via JIT, result is the return value
    Executed(i64),
    /// Function should be compiled (threshold reached), fall back to interpreter
    ShouldCompile,
    /// Function is not registered or not hot enough, use interpreter
    UseInterpreter,
    /// JIT execution failed with an error
    Error(JitError),
}

/// Try to execute a function via JIT if available.
///
/// This is the main entry point for the interpreter integration.
/// Handles tiered compilation: functions are compiled to Tier 1 (baseline) first,
/// then recompiled to Tier 2 (optimized) after additional calls.
///
/// # Arguments
/// * `func_offset` - The bytecode offset of the function to call
/// * `args` - The arguments to pass to the function (as i64 values)
/// * `program` - The program containing the bytecode (for compilation)
///
/// # Returns
/// * `JitCallResult::Executed(result)` - Function was JIT-executed
/// * `JitCallResult::ShouldCompile` - Function is hot, trigger compilation
/// * `JitCallResult::UseInterpreter` - Use bytecode interpretation
/// * `JitCallResult::Error(e)` - JIT execution failed
pub fn try_jit_call(func_offset: u32, args: &[i64], program: &Program) -> JitCallResult {
    // Ensure JIT is initialized
    if init_jit().is_err() {
        return JitCallResult::UseInterpreter;
    }

    let result = with_jit(|jit| {
        // Auto-register function if not already registered
        // Use args.len() as param_count since we know how many args were passed
        if !jit.is_function_registered(func_offset) {
            jit.register_function(func_offset, args.len() as u32);
        }

        // Check if function has native code or should be compiled/recompiled
        let (native_ptr, tier_to_compile) = jit.get_function(func_offset);

        if let Some(tier) = tier_to_compile {
            // Function needs compilation (or recompilation) to a higher tier
            match jit.compile_function_at_tier(program, func_offset, tier) {
                Ok(_ptr) => {
                    // Compilation succeeded, call native code
                    jit.increment_cache_hits();
                    match unsafe { jit.call_native(func_offset, args) } {
                        Ok(result) => JitCallResult::Executed(result),
                        Err(e) => JitCallResult::Error(e),
                    }
                }
                Err(_e) => {
                    // Compilation failed (likely unsupported opcode), fall back
                    JitCallResult::UseInterpreter
                }
            }
        } else if native_ptr.is_some() {
            // Function is already compiled, call native code
            jit.increment_cache_hits();
            match unsafe { jit.call_native(func_offset, args) } {
                Ok(result) => JitCallResult::Executed(result),
                Err(e) => JitCallResult::Error(e),
            }
        } else {
            // Not hot enough yet for JIT
            JitCallResult::UseInterpreter
        }
    });

    result.unwrap_or(JitCallResult::UseInterpreter)
}

/// Register a function for JIT tracking.
///
/// Call this when discovering a function in the bytecode to enable JIT tracking.
///
/// # Arguments
/// * `func_offset` - The bytecode offset of the function
/// * `param_count` - The number of parameters the function takes
pub fn register_function(func_offset: u32, param_count: u32) {
    if init_jit().is_err() {
        return;
    }

    with_jit(|jit| {
        jit.register_function(func_offset, param_count);
    });
}

// ============================================================================
// OSR (On-Stack Replacement) Integration
// ============================================================================

/// Result of attempting an OSR call.
pub enum OsrCallResult {
    /// Loop was executed via OSR, result is the return value
    Executed(i64),
    /// Loop should be compiled (threshold reached), fall back to interpreter
    ShouldCompile,
    /// Loop is not hot enough or not registered, use interpreter
    UseInterpreter,
    /// OSR execution failed with an error
    Error(JitError),
}

/// Register a loop back edge for OSR tracking.
///
/// Call this when a backward Jump is detected (loop back edge).
///
/// # Arguments
/// * `back_edge_ip` - The IP of the backward Jump instruction
/// * `header_ip` - The target of the jump (loop header)
/// * `local_count` - Number of locals in scope
pub fn register_loop(back_edge_ip: u32, header_ip: u32, local_count: u32) {
    if init_jit().is_err() {
        return;
    }

    with_jit(|jit| {
        jit.register_loop(back_edge_ip, header_ip, local_count);
    });
}

/// Try to execute a loop via OSR if available.
///
/// This is called when a backward Jump is encountered (loop iteration).
/// If the loop is hot enough, it will be compiled and executed via JIT.
///
/// # Arguments
/// * `back_edge_ip` - The IP of the backward Jump instruction
/// * `header_ip` - The target of the jump (loop header)
/// * `locals` - The current locals as i64 values
/// * `local_count` - Number of locals in scope (for registration)
/// * `program` - The program containing the bytecode
///
/// # Returns
/// * `OsrCallResult::Executed(result)` - Loop was JIT-executed
/// * `OsrCallResult::ShouldCompile` - Loop is hot, trigger compilation
/// * `OsrCallResult::UseInterpreter` - Use bytecode interpretation
/// * `OsrCallResult::Error(e)` - OSR execution failed
pub fn try_osr_call(
    back_edge_ip: u32,
    header_ip: u32,
    locals: &[i64],
    local_count: u32,
    program: &Program,
) -> OsrCallResult {
    // Ensure JIT is initialized
    if init_jit().is_err() {
        return OsrCallResult::UseInterpreter;
    }

    let result = with_jit(|jit| {
        // Auto-register loop if not already registered
        if !jit.is_loop_registered(back_edge_ip) {
            jit.register_loop(back_edge_ip, header_ip, local_count);
        }

        // Check if loop is hot enough or has OSR entry
        let (should_compile, has_entry) = jit.increment_loop_iteration(back_edge_ip);

        if has_entry {
            // Loop is already compiled, call OSR entry
            jit.increment_osr_entries();
            match unsafe { jit.call_osr(back_edge_ip, locals) } {
                Ok(result) => OsrCallResult::Executed(result),
                Err(e) => OsrCallResult::Error(e),
            }
        } else if should_compile {
            // Loop is hot, compile it
            match jit.compile_osr(program, back_edge_ip) {
                Ok(_ptr) => {
                    // Compilation succeeded, call OSR entry
                    jit.increment_osr_entries();
                    match unsafe { jit.call_osr(back_edge_ip, locals) } {
                        Ok(result) => OsrCallResult::Executed(result),
                        Err(e) => OsrCallResult::Error(e),
                    }
                }
                Err(_e) => {
                    // Compilation failed (likely unsupported opcode), fall back
                    OsrCallResult::UseInterpreter
                }
            }
        } else {
            // Not hot enough yet
            OsrCallResult::UseInterpreter
        }
    });

    result.unwrap_or(OsrCallResult::UseInterpreter)
}

// ============================================================================
// Deoptimization Integration
// ============================================================================

use crate::jit::{CompilationTier, DeoptInfo, DeoptReason};

/// Result of a deoptimization operation.
#[derive(Debug)]
pub enum DeoptResult {
    /// Successfully deoptimized to the specified tier
    Success(CompilationTier),
    /// Function not found in JIT
    FunctionNotFound,
    /// JIT not initialized
    JitNotAvailable,
}

/// Record a deoptimization event for a function.
///
/// This should be called by the interpreter when a JIT-compiled function
/// needs to fall back to interpretation due to a runtime condition.
///
/// # Arguments
/// * `func_offset` - The bytecode offset of the function
/// * `reason` - The reason for deoptimization
///
/// # Returns
/// The tier that was deoptimized to, or an error.
pub fn record_deopt(func_offset: u32, reason: DeoptReason) -> DeoptResult {
    if init_jit().is_err() {
        return DeoptResult::JitNotAvailable;
    }

    let result = with_jit(|jit| {
        let info = DeoptInfo::new(reason, func_offset);
        match jit.record_deopt(func_offset, reason, Some(info)) {
            Some(tier) => DeoptResult::Success(tier),
            None => DeoptResult::FunctionNotFound,
        }
    });

    result.unwrap_or(DeoptResult::JitNotAvailable)
}

/// Deoptimize a function with full state capture.
///
/// This is used when deoptimizing mid-execution with state that needs
/// to be transferred to the interpreter.
///
/// # Arguments
/// * `func_offset` - The bytecode offset of the function
/// * `reason` - The reason for deoptimization
/// * `bytecode_offset` - The bytecode offset where deopt occurred
/// * `locals` - Current local values to transfer
/// * `stack` - Current stack values to transfer
///
/// # Returns
/// The tier that was deoptimized to, or an error.
pub fn deoptimize(
    func_offset: u32,
    reason: DeoptReason,
    bytecode_offset: u32,
    locals: Vec<i64>,
    stack: Vec<i64>,
) -> DeoptResult {
    if init_jit().is_err() {
        return DeoptResult::JitNotAvailable;
    }

    let result = with_jit(|jit| {
        let info = DeoptInfo::with_state(reason, bytecode_offset, locals, stack);
        match jit.record_deopt(func_offset, reason, Some(info)) {
            Some(tier) => DeoptResult::Success(tier),
            None => DeoptResult::FunctionNotFound,
        }
    });

    result.unwrap_or(DeoptResult::JitNotAvailable)
}

/// Check if a function has JIT disabled due to too many deopts.
///
/// # Arguments
/// * `func_offset` - The bytecode offset of the function
///
/// # Returns
/// True if JIT is disabled for this function.
pub fn is_jit_disabled(func_offset: u32) -> bool {
    if init_jit().is_err() {
        return false;
    }

    with_jit(|jit| jit.is_jit_disabled(func_offset)).unwrap_or(false)
}

/// Get deoptimization statistics for a function.
///
/// # Arguments
/// * `func_offset` - The bytecode offset of the function
///
/// # Returns
/// A tuple of (deopt_count, last_reason, jit_disabled), or None if function not found.
pub fn get_deopt_stats(func_offset: u32) -> Option<(u32, DeoptReason, bool)> {
    if init_jit().is_err() {
        return None;
    }

    with_jit(|jit| jit.get_function_deopt_stats(func_offset)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Op;
    use crate::runtime::run_program;
    use crate::{JIT_THRESHOLD, Program, get_jit_stats};

    #[test]
    fn test_jit_call_unregistered_function() {
        // Calling an unregistered function should fall back to interpreter
        let p = Program::new(vec![], vec![]);
        let result = try_jit_call(999, &[], &p);
        assert!(matches!(result, JitCallResult::UseInterpreter));
    }

    #[test]
    fn test_register_function() {
        // Registering a function should succeed
        register_function(100, 2);
        // Second registration should be idempotent
        register_function(100, 2);
    }

    #[test]
    fn test_jit_dispatch_in_interpreter() {
        // Create a program that calls a simple function many times
        // This tests the Op::Call JIT dispatch integration
        //
        // After TIER1_THRESHOLD calls, the function should be compiled to Tier 1
        // and subsequent calls should use the JIT-compiled version

        let mut code = Vec::new();

        // Simple function at offset 500 that does: local0 + local1
        // Offset must be > main code size (110 calls × 4 = 440 + 1 halt = 441)
        let add_func_offset = 500u32;

        // Build 110+ sequential calls (above JIT_THRESHOLD/TIER1_THRESHOLD)
        // Each call: push args, call function, pop result
        for _ in 0..(JIT_THRESHOLD + 10) {
            code.push(Op::PushI64(10)); // arg0
            code.push(Op::PushI64(5)); // arg1
            code.push(Op::Call(add_func_offset));
            code.push(Op::Pop); // discard result
        }

        code.push(Op::Halt);

        // Pad to function offset
        while code.len() < add_func_offset as usize {
            code.push(Op::Halt);
        }

        // Add function with interpreter calling convention:
        // - LocalSet(1), LocalSet(0) prologue pops args from stack
        // - LocalGet reads from locals
        // - JIT skips prologue and initializes locals from function params
        assert_eq!(code.len(), add_func_offset as usize);
        code.push(Op::LocalSet(1)); // pop arg1 (5) into local 1
        code.push(Op::LocalSet(0)); // pop arg0 (10) into local 0
        code.push(Op::LocalGet(0)); // push 10
        code.push(Op::LocalGet(1)); // push 5
        code.push(Op::AddI64); // 10 + 5 = 15
        code.push(Op::Ret);

        let program = Program::new(vec![], code);

        // Run the program (always returns 0, but we check JIT stats)
        let _ = run_program(&program);

        // Check JIT stats - the add function should have been compiled and executed
        if let Some(stats) = get_jit_stats() {
            // After 100 calls, JIT should compile to Tier 1
            assert!(
                stats.functions_compiled > 0,
                "JIT should have compiled at least one function. Stats: {:?}",
                stats
            );
            assert!(
                stats.tier1_compilations > 0,
                "JIT should have at least one Tier 1 compilation. Stats: {:?}",
                stats
            );
            // cache_hits tracks JIT executions after initial compile
            assert!(
                stats.cache_hits > 0,
                "JIT should have had cache hits for subsequent calls. Stats: {:?}",
                stats
            );
        } else {
            panic!("get_jit_stats() returned None - JIT not initialized");
        }
    }

    #[test]
    fn test_osr_loop_meta() {
        use crate::jit::{OSR_THRESHOLD, OsrLoopMeta};

        let mut meta = OsrLoopMeta::new(100, 50, 2);
        assert_eq!(meta.back_edge_ip, 100);
        assert_eq!(meta.header_ip, 50);
        assert_eq!(meta.local_count, 2);
        assert_eq!(meta.iteration_count, 0);
        assert!(!meta.has_osr_entry());

        // Increment up to threshold - 1
        for _ in 0..OSR_THRESHOLD - 1 {
            assert!(!meta.increment_iteration());
        }

        // At threshold, should trigger compilation
        assert!(meta.increment_iteration());

        // Simulate starting compilation (would be done by compile_osr)
        meta.compiling = true;

        // While compiling, no more triggers
        assert!(!meta.increment_iteration());

        // After compilation complete, set osr_entry
        meta.compiling = false;
        meta.osr_entry = Some(0x1000 as *const u8);

        // With entry set, no more triggers
        assert!(!meta.increment_iteration());
        assert!(meta.has_osr_entry());
    }

    #[test]
    fn test_osr_compile_simple_loop() {
        use crate::jit::{JitContext, OSR_THRESHOLD};

        // Simple loop: sum = 0; while (i < 10) { sum = sum + i; i = i + 1 }
        // Bytecode layout:
        // 0: PushI64(0)      ; sum = 0
        // 1: LocalSet(0)     ; store sum in local 0
        // 2: PushI64(0)      ; i = 0
        // 3: LocalSet(1)     ; store i in local 1
        // 4: LocalGet(1)     ; loop header: push i
        // 5: PushI64(10)     ; push 10
        // 6: LtI64           ; i < 10
        // 7: JumpIfFalse(14) ; if false, exit loop (forward jump)
        // 8: LocalGet(0)     ; push sum
        // 9: LocalGet(1)     ; push i
        // 10: AddI64         ; sum + i
        // 11: LocalSet(0)    ; sum = sum + i
        // 12: LocalGet(1)    ; push i
        // 13: PushI64(1)     ; push 1
        // 14: AddI64         ; i + 1
        // 15: LocalSet(1)    ; i = i + 1
        // 16: Jump(4)        ; back edge to loop header
        // 17: LocalGet(0)    ; push sum (result)
        // 18: Ret

        let code = vec![
            Op::PushI64(0),      // 0
            Op::LocalSet(0),     // 1
            Op::PushI64(0),      // 2
            Op::LocalSet(1),     // 3
            Op::LocalGet(1),     // 4 - loop header
            Op::PushI64(10),     // 5
            Op::LtI64,           // 6
            Op::JumpIfFalse(17), // 7 - exit loop
            Op::LocalGet(0),     // 8
            Op::LocalGet(1),     // 9
            Op::AddI64,          // 10
            Op::LocalSet(0),     // 11
            Op::LocalGet(1),     // 12
            Op::PushI64(1),      // 13
            Op::AddI64,          // 14
            Op::LocalSet(1),     // 15
            Op::Jump(4),         // 16 - back edge
            Op::LocalGet(0),     // 17
            Op::Ret,             // 18
        ];
        let program = Program::new(vec![], code);

        let mut jit = JitContext::new().expect("JIT context creation failed");

        // Register the loop (back_edge_ip=16, header_ip=4, 2 locals)
        jit.register_loop(16, 4, 2);

        // Simulate enough iterations to trigger OSR
        for _ in 0..OSR_THRESHOLD {
            let (should_compile, _) = jit.increment_loop_iteration(16);
            if should_compile {
                break;
            }
        }

        // Now compile OSR
        let result = jit.compile_osr(&program, 16);
        assert!(result.is_ok(), "OSR compilation failed: {:?}", result.err());

        // Call the OSR entry with initial locals [sum=0, i=0]
        // The loop should compute sum = 0+1+2+...+9 = 45
        let call_result = unsafe { jit.call_osr(16, &[0, 0]) };
        assert!(
            call_result.is_ok(),
            "OSR call failed: {:?}",
            call_result.err()
        );

        // Verify stats
        assert_eq!(jit.stats.osr_compilations, 1);
    }

    #[test]
    fn test_record_deopt() {
        use crate::jit::{CompilationTier, DeoptReason};

        // Register a function
        register_function(5000, 2);

        // Record a deopt
        let result = record_deopt(5000, DeoptReason::TypeMismatch);
        match result {
            DeoptResult::Success(tier) => {
                // Should fall back to interpreter since no native code
                assert_eq!(tier, CompilationTier::Interpreter);
            }
            _ => panic!("Expected Success, got {:?}", result),
        }
    }

    #[test]
    fn test_deoptimize_with_state() {
        use crate::jit::{CompilationTier, DeoptReason};

        // Register a function
        register_function(5001, 2);

        // Deoptimize with state
        let result = deoptimize(5001, DeoptReason::Overflow, 100, vec![1, 2], vec![3, 4, 5]);
        match result {
            DeoptResult::Success(tier) => {
                assert_eq!(tier, CompilationTier::Interpreter);
            }
            _ => panic!("Expected Success, got {:?}", result),
        }
    }

    #[test]
    fn test_is_jit_disabled() {
        use crate::jit::{DeoptReason, MAX_DEOPTS_PER_FUNCTION};

        // Register a function
        register_function(5002, 0);

        // Initially not disabled
        assert!(!is_jit_disabled(5002));

        // Record many deopts to trigger disable
        for _ in 0..MAX_DEOPTS_PER_FUNCTION {
            record_deopt(5002, DeoptReason::Unknown);
        }

        // Now should be disabled
        assert!(is_jit_disabled(5002));
    }

    #[test]
    fn test_get_deopt_stats() {
        use crate::jit::DeoptReason;

        // Register a function
        register_function(5003, 0);

        // Initially no deopts
        let stats = get_deopt_stats(5003);
        assert_eq!(stats, Some((0, DeoptReason::None, false)));

        // After a deopt
        record_deopt(5003, DeoptReason::DivisionByZero);
        let stats = get_deopt_stats(5003);
        assert_eq!(stats, Some((1, DeoptReason::DivisionByZero, false)));

        // Non-existent function
        let stats = get_deopt_stats(99999);
        assert!(stats.is_none());
    }
}
