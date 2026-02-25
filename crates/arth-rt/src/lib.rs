//! arth-rt: Native Runtime Library for Arth
//!
//! This crate provides C FFI wrappers for Arth's standard library functions.
//! All functions use the C ABI and can be called from:
//! - Native-compiled Arth code (via LLVM backend)
//! - The Arth VM (via host function dispatch)
//!
//! # Design Principles
//!
//! 1. **C ABI**: All public functions use `extern "C"` for portability
//! 2. **Handle-based**: Resources (files, connections) use opaque i64 handles
//! 3. **Error codes**: Functions return error codes; errno provides details
//! 4. **No allocations returned**: Caller provides buffers for string/byte output
//! 5. **Thread-safe**: Global state protected by appropriate synchronization
//!
//! # Naming Convention
//!
//! All exported functions follow the pattern: `arth_rt_<module>_<operation>`
//! - `arth_rt_file_open` - file operations
//! - `arth_rt_dir_create` - directory operations
//! - `arth_rt_sqlite_query` - SQLite operations
//! - `arth_rt_pg_connect` - PostgreSQL operations
//!
//! # Error Handling
//!
//! - Return value of -1 or negative typically indicates error
//! - Check errno for detailed error information
//! - Use `arth_rt_errno()` and `arth_rt_strerror()` for error details

#![allow(clippy::missing_safety_doc)]
#![allow(clippy::manual_strip)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::redundant_field_names)]
#![allow(clippy::needless_return)]
#![allow(clippy::single_match)]
#![allow(clippy::needless_borrows_for_generic_args)]
#![allow(clippy::unnecessary_to_owned)]
#![allow(clippy::useless_format)]
#![allow(clippy::match_single_binding)]
#![allow(clippy::not_unsafe_ptr_arg_deref)] // FFI functions are called from C, which has no safety guarantees

use std::sync::atomic::{AtomicI64, Ordering};

// Module declarations
#[cfg(feature = "io")]
pub mod io;

#[cfg(feature = "time")]
pub mod time;

#[cfg(feature = "sqlite")]
pub mod sqlite;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "net")]
pub mod net;

#[cfg(feature = "tls")]
pub mod tls;

#[cfg(feature = "http")]
pub mod http;

#[cfg(feature = "html")]
pub mod html;

#[cfg(feature = "async-rt")]
pub mod async_rt;

#[cfg(feature = "crypto")]
pub mod crypto;

pub mod alloc;
pub mod closure;
pub mod closure_variadic;
pub mod encoding;
pub mod enum_rt;
pub mod error;
pub mod exception;
pub mod executor_rt;
pub mod panic;
pub mod provider;
pub mod shared;
pub mod string;
pub mod struct_rt;

// Re-export commonly used items
pub use error::*;
pub use string::*;

// -----------------------------------------------------------------------------
// Global Handle Management
// -----------------------------------------------------------------------------

/// Global counter for generating unique handles
static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

/// Generate a new unique handle
#[inline]
pub fn new_handle() -> i64 {
    NEXT_HANDLE.fetch_add(1, Ordering::Relaxed)
}

// -----------------------------------------------------------------------------
// Error Handling
// -----------------------------------------------------------------------------

/// Get the last error code set by arth-rt functions
/// Returns 0 if no error, negative values for errors
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_errno() -> i32 {
    // For now, use the system errno
    // In the future, we may maintain our own thread-local error
    unsafe { *libc::__error() }
}

/// Get error message for the last error
/// Returns the number of bytes written (excluding null terminator)
/// Returns -1 if buffer is too small
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_strerror(buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return -1;
    }

    let errno = unsafe { *libc::__error() };
    let msg = unsafe { libc::strerror(errno) };

    if msg.is_null() {
        return -1;
    }

    let msg_len = unsafe { libc::strlen(msg) };
    if msg_len >= buf_len {
        return -1;
    }

    unsafe {
        std::ptr::copy_nonoverlapping(msg as *const u8, buf, msg_len);
        *buf.add(msg_len) = 0; // null terminate
    }

    msg_len as i32
}

// -----------------------------------------------------------------------------
// Version Information
// -----------------------------------------------------------------------------

/// Get the arth-rt version as a string
/// Returns pointer to static string (do not free)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_version() -> *const libc::c_char {
    static VERSION: &[u8] = b"0.1.0\0";
    VERSION.as_ptr() as *const libc::c_char
}

// -----------------------------------------------------------------------------
// Initialization
// -----------------------------------------------------------------------------

/// Initialize the arth runtime
/// Must be called before any other arth_rt_* functions
/// Returns 0 on success, -1 on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_init() -> i32 {
    // Currently a no-op, but reserved for future initialization
    // (e.g., setting up async runtime, TLS contexts, etc.)
    0
}

/// Shutdown the arth runtime
/// Should be called before program exit to clean up resources
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_shutdown() {
    // Clean up any global resources
    // Currently a no-op
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_generation() {
        let h1 = new_handle();
        let h2 = new_handle();
        assert!(h2 > h1);
    }

    #[test]
    fn test_version() {
        let ver = arth_rt_version();
        assert!(!ver.is_null());
        let s = unsafe { std::ffi::CStr::from_ptr(ver) };
        assert_eq!(s.to_str().unwrap(), "0.1.0");
    }

    #[test]
    fn test_init_shutdown() {
        assert_eq!(arth_rt_init(), 0);
        arth_rt_shutdown();
    }
}
