pub mod ast_to_hir;
pub mod hir_to_ir;

/// Optional CPS transformation for async functions.
/// When the `async-cps` feature is enabled, this transforms async body
/// functions into poll-based state machines.
#[cfg(feature = "async-cps")]
pub fn apply_cps_transformation(
    funcs: Vec<crate::compiler::ir::Func>,
) -> Vec<crate::compiler::ir::Func> {
    use crate::compiler::ir::async_lower::{
        is_async_body_func, lower_async_function, should_lower_async,
    };

    let mut result = Vec::new();

    for func in funcs {
        // Check if this is an async body function that should be transformed
        if is_async_body_func(&func.name) && should_lower_async(&func) {
            // Apply CPS transformation
            match lower_async_function(func.clone()) {
                Ok(state_machine) => {
                    // Add the wrapper and poll functions
                    result.push(state_machine.wrapper_func);
                    result.push(state_machine.poll_func);
                    if let Some(drop_func) = state_machine.drop_func {
                        result.push(drop_func);
                    }
                    // Skip the original async_body since we replaced it with poll
                }
                Err(e) => {
                    // Fall back to original function if transformation fails
                    eprintln!(
                        "Warning: CPS transformation failed for {}: {}",
                        func.name, e
                    );
                    result.push(func);
                }
            }
        } else {
            // Keep non-async functions as-is
            result.push(func);
        }
    }

    result
}

/// No-op when CPS feature is disabled
#[cfg(not(feature = "async-cps"))]
pub fn apply_cps_transformation(
    funcs: Vec<crate::compiler::ir::Func>,
) -> Vec<crate::compiler::ir::Func> {
    funcs
}
