//! Debug utilities for Arth VM.
//!
//! Provides conditional verbose logging that can be enabled with:
//! - ARTH_DEBUG=1 environment variable
//! - Programmatically via `enable_debug()`

use std::sync::atomic::{AtomicBool, Ordering};

/// Global debug mode flag
static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable debug mode
pub fn enable_debug() {
    DEBUG_ENABLED.store(true, Ordering::SeqCst);
    eprintln!("[ARTH_DEBUG] Debug mode enabled");
}

/// Check if debug mode is enabled
pub fn is_debug_enabled() -> bool {
    DEBUG_ENABLED.load(Ordering::SeqCst)
}

/// Initialize debug mode from environment
pub fn init_from_env() {
    if std::env::var("ARTH_DEBUG")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
    {
        enable_debug();
    }
}

/// Log a debug message (only when debug mode is enabled)
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug_enabled() {
            eprintln!("[ARTH_DEBUG] {}", format!($($arg)*));
        }
    };
}

/// Log an Arth VM host call
pub fn log_host_call(fn_name: &str, args: &str, result: &str) {
    if is_debug_enabled() {
        let args_preview = if args.len() > 80 {
            format!("{}...", &args[..80])
        } else {
            args.to_string()
        };
        let result_preview = if result.len() > 80 {
            format!("{}...", &result[..80])
        } else {
            result.to_string()
        };
        eprintln!(
            "[ARTH_HOST] {}({}) -> {}",
            fn_name, args_preview, result_preview
        );
    }
}

/// Log a VM execution event
pub fn log_exec(phase: &str, message: &str) {
    if is_debug_enabled() {
        eprintln!("[ARTH_EXEC | {}] {}", phase, message);
    }
}

/// Log an error (always logged, not just in debug mode)
pub fn log_error(context: &str, error: &str) {
    eprintln!("[ARTH_ERROR | {}] {}", context, error);
}

/// Log a warning (only when debug mode is enabled)
pub fn log_warn(context: &str, message: &str) {
    if is_debug_enabled() {
        eprintln!("[ARTH_WARN | {}] {}", context, message);
    }
}
