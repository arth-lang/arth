//! Panic and abort handling for native mode
//!
//! Provides panic handlers and abort functionality for native-compiled Arth programs.
//! In native mode, these functions handle unrecoverable errors.

use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

// -----------------------------------------------------------------------------
// Panic Handler Registration
// -----------------------------------------------------------------------------

/// Function signature for custom panic handlers
pub type PanicHandler =
    extern "C" fn(msg: *const u8, msg_len: usize, file: *const u8, file_len: usize, line: u32);

/// Global custom panic handler
static PANIC_HANDLER: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

/// Whether we're currently in a panic (to prevent recursive panics)
static IN_PANIC: AtomicBool = AtomicBool::new(false);

/// Register a custom panic handler
///
/// # Arguments
/// * `handler` - Function to call on panic, or null to use default
///
/// # Returns
/// * Previous handler (or null if none was set)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_set_panic_handler(handler: PanicHandler) -> *mut () {
    PANIC_HANDLER.swap(handler as *mut (), Ordering::SeqCst)
}

/// Get the current panic handler
///
/// # Returns
/// * Current handler, or null if using default
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_get_panic_handler() -> *mut () {
    PANIC_HANDLER.load(Ordering::SeqCst)
}

// -----------------------------------------------------------------------------
// Panic Functions
// -----------------------------------------------------------------------------

/// Trigger a panic with a message
///
/// This function will:
/// 1. Call the custom panic handler if set
/// 2. Print the panic message to stderr
/// 3. Abort the program
///
/// # Arguments
/// * `msg` - Panic message (UTF-8)
/// * `msg_len` - Length of message
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_panic(msg: *const u8, msg_len: usize) -> ! {
    arth_rt_panic_at(msg, msg_len, std::ptr::null(), 0, 0)
}

/// Trigger a panic with a message and location
///
/// # Arguments
/// * `msg` - Panic message (UTF-8)
/// * `msg_len` - Length of message
/// * `file` - Source file name (UTF-8, may be null)
/// * `file_len` - Length of file name
/// * `line` - Line number (0 if unknown)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_panic_at(
    msg: *const u8,
    msg_len: usize,
    file: *const u8,
    file_len: usize,
    line: u32,
) -> ! {
    // Prevent recursive panics
    if IN_PANIC.swap(true, Ordering::SeqCst) {
        // Already panicking, just abort immediately
        unsafe { libc::abort() };
    }

    // Call custom handler if set
    let handler = PANIC_HANDLER.load(Ordering::SeqCst);
    if !handler.is_null() {
        let handler_fn: PanicHandler = unsafe { std::mem::transmute(handler) };
        handler_fn(msg, msg_len, file, file_len, line);
    }

    // Print panic message to stderr
    unsafe {
        let prefix = b"panic: ";
        libc::write(
            libc::STDERR_FILENO,
            prefix.as_ptr() as *const libc::c_void,
            prefix.len(),
        );

        if !msg.is_null() && msg_len > 0 {
            libc::write(libc::STDERR_FILENO, msg as *const libc::c_void, msg_len);
        } else {
            let unknown = b"<unknown>";
            libc::write(
                libc::STDERR_FILENO,
                unknown.as_ptr() as *const libc::c_void,
                unknown.len(),
            );
        }

        // Print location if available
        if !file.is_null() && file_len > 0 {
            let at = b"\n  at ";
            libc::write(
                libc::STDERR_FILENO,
                at.as_ptr() as *const libc::c_void,
                at.len(),
            );
            libc::write(libc::STDERR_FILENO, file as *const libc::c_void, file_len);

            if line > 0 {
                let colon = b":";
                libc::write(
                    libc::STDERR_FILENO,
                    colon.as_ptr() as *const libc::c_void,
                    colon.len(),
                );

                // Convert line number to string
                let mut line_buf = [0u8; 16];
                let line_str = format_u32(line, &mut line_buf);
                libc::write(
                    libc::STDERR_FILENO,
                    line_str.as_ptr() as *const libc::c_void,
                    line_str.len(),
                );
            }
        }

        let newline = b"\n";
        libc::write(
            libc::STDERR_FILENO,
            newline.as_ptr() as *const libc::c_void,
            newline.len(),
        );
    }

    // Abort the program
    unsafe { libc::abort() };
}

/// Abort the program immediately without cleanup
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_abort() -> ! {
    unsafe { libc::abort() };
}

/// Exit the program with a status code
///
/// # Arguments
/// * `status` - Exit status (0 for success)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_exit(status: i32) -> ! {
    unsafe { libc::exit(status) };
}

// -----------------------------------------------------------------------------
// Assertion Support
// -----------------------------------------------------------------------------

/// Assert a condition, panic if false
///
/// # Arguments
/// * `condition` - Condition that must be true
/// * `msg` - Message to display if assertion fails
/// * `msg_len` - Length of message
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_assert(condition: bool, msg: *const u8, msg_len: usize) {
    if !condition {
        arth_rt_panic(msg, msg_len);
    }
}

/// Assert a condition with location info
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_assert_at(
    condition: bool,
    msg: *const u8,
    msg_len: usize,
    file: *const u8,
    file_len: usize,
    line: u32,
) {
    if !condition {
        arth_rt_panic_at(msg, msg_len, file, file_len, line);
    }
}

// -----------------------------------------------------------------------------
// Unreachable Code
// -----------------------------------------------------------------------------

/// Mark code as unreachable - panics if reached
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_unreachable() -> ! {
    let msg = b"entered unreachable code";
    arth_rt_panic(msg.as_ptr(), msg.len())
}

/// Mark code as unreachable with location
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_unreachable_at(file: *const u8, file_len: usize, line: u32) -> ! {
    let msg = b"entered unreachable code";
    arth_rt_panic_at(msg.as_ptr(), msg.len(), file, file_len, line)
}

// -----------------------------------------------------------------------------
// Helper Functions
// -----------------------------------------------------------------------------

/// Format a u32 as decimal string into a buffer
/// Returns a slice of the buffer containing the formatted number
fn format_u32(mut n: u32, buf: &mut [u8; 16]) -> &[u8] {
    if n == 0 {
        buf[0] = b'0';
        return &buf[0..1];
    }

    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }

    &buf[i..]
}

// -----------------------------------------------------------------------------
// Rust Panic Hook Integration
// -----------------------------------------------------------------------------

/// Install a Rust panic hook that calls arth_rt_panic
///
/// This allows Rust panics within arth-rt to be handled uniformly.
/// Call this during initialization if desired.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = info.to_string();
        let msg_bytes = msg.as_bytes();

        let (file, line) = if let Some(loc) = info.location() {
            (loc.file(), loc.line())
        } else {
            ("", 0)
        };

        // Don't use arth_rt_panic_at as it would abort
        // Instead, just print and let Rust handle the abort
        unsafe {
            let prefix = b"rust panic: ";
            libc::write(
                libc::STDERR_FILENO,
                prefix.as_ptr() as *const libc::c_void,
                prefix.len(),
            );
            libc::write(
                libc::STDERR_FILENO,
                msg_bytes.as_ptr() as *const libc::c_void,
                msg_bytes.len(),
            );

            if !file.is_empty() {
                let at = b"\n  at ";
                libc::write(
                    libc::STDERR_FILENO,
                    at.as_ptr() as *const libc::c_void,
                    at.len(),
                );
                libc::write(
                    libc::STDERR_FILENO,
                    file.as_ptr() as *const libc::c_void,
                    file.len(),
                );

                if line > 0 {
                    let colon = b":";
                    libc::write(
                        libc::STDERR_FILENO,
                        colon.as_ptr() as *const libc::c_void,
                        colon.len(),
                    );

                    let mut line_buf = [0u8; 16];
                    let line_str = format_u32(line, &mut line_buf);
                    libc::write(
                        libc::STDERR_FILENO,
                        line_str.as_ptr() as *const libc::c_void,
                        line_str.len(),
                    );
                }
            }

            let newline = b"\n";
            libc::write(
                libc::STDERR_FILENO,
                newline.as_ptr() as *const libc::c_void,
                newline.len(),
            );
        }
    }));
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn test_format_u32() {
        let mut buf = [0u8; 16];

        assert_eq!(format_u32(0, &mut buf), b"0");
        assert_eq!(format_u32(1, &mut buf), b"1");
        assert_eq!(format_u32(42, &mut buf), b"42");
        assert_eq!(format_u32(12345, &mut buf), b"12345");
        assert_eq!(format_u32(u32::MAX, &mut buf), b"4294967295");
    }

    #[test]
    fn test_handler_registration() {
        // Should start as null
        let old = arth_rt_get_panic_handler();
        assert!(old.is_null());

        // Set a handler
        static CALLED: AtomicU32 = AtomicU32::new(0);

        extern "C" fn test_handler(
            _msg: *const u8,
            _msg_len: usize,
            _file: *const u8,
            _file_len: usize,
            _line: u32,
        ) {
            CALLED.fetch_add(1, Ordering::SeqCst);
        }

        let prev = arth_rt_set_panic_handler(test_handler);
        assert!(prev.is_null());

        let current = arth_rt_get_panic_handler();
        assert!(!current.is_null());

        // Restore null handler
        arth_rt_set_panic_handler(unsafe { std::mem::transmute(std::ptr::null::<()>()) });
    }

    #[test]
    fn test_assert_pass() {
        // Should not panic
        let msg = b"should not see this";
        arth_rt_assert(true, msg.as_ptr(), msg.len());
    }

    // Note: We can't easily test panic/abort in unit tests since they terminate the process.
    // These would need integration tests that fork a child process.
}
