//! Error handling utilities for arth-rt
//!
//! Provides consistent error codes and conversion from system errors.

use std::cell::RefCell;

/// Error codes used by arth-rt functions
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Success (no error)
    Success = 0,
    /// Generic error
    Error = -1,
    /// Invalid argument
    InvalidArgument = -2,
    /// Resource not found
    NotFound = -3,
    /// Permission denied
    PermissionDenied = -4,
    /// Resource already exists
    AlreadyExists = -5,
    /// Buffer too small
    BufferTooSmall = -6,
    /// Invalid handle
    InvalidHandle = -7,
    /// Operation would block (for non-blocking IO)
    WouldBlock = -8,
    /// Connection refused
    ConnectionRefused = -9,
    /// Connection reset
    ConnectionReset = -10,
    /// Timeout
    Timeout = -11,
    /// End of file / no more data
    Eof = -12,
    /// Interrupted system call
    Interrupted = -13,
    /// Resource busy
    Busy = -14,
    /// Not supported
    NotSupported = -15,
    /// IO error
    IoError = -16,
    /// Database error
    DbError = -17,
    /// Network error
    NetError = -18,
    /// TLS/SSL error
    TlsError = -19,
    /// Capability denied (sandboxed)
    CapabilityDenied = -20,
}

impl ErrorCode {
    /// Convert to i32 for C ABI
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

impl From<std::io::Error> for ErrorCode {
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind;
        match e.kind() {
            ErrorKind::NotFound => ErrorCode::NotFound,
            ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
            ErrorKind::AlreadyExists => ErrorCode::AlreadyExists,
            ErrorKind::WouldBlock => ErrorCode::WouldBlock,
            ErrorKind::ConnectionRefused => ErrorCode::ConnectionRefused,
            ErrorKind::ConnectionReset => ErrorCode::ConnectionReset,
            ErrorKind::TimedOut => ErrorCode::Timeout,
            ErrorKind::UnexpectedEof => ErrorCode::Eof,
            ErrorKind::Interrupted => ErrorCode::Interrupted,
            ErrorKind::InvalidInput | ErrorKind::InvalidData => ErrorCode::InvalidArgument,
            ErrorKind::Unsupported => ErrorCode::NotSupported,
            _ => ErrorCode::IoError,
        }
    }
}

/// Convert errno to ErrorCode
pub fn from_errno(errno: i32) -> ErrorCode {
    match errno {
        libc::ENOENT => ErrorCode::NotFound,
        libc::EACCES | libc::EPERM => ErrorCode::PermissionDenied,
        libc::EEXIST => ErrorCode::AlreadyExists,
        libc::EAGAIN => ErrorCode::WouldBlock,
        libc::ECONNREFUSED => ErrorCode::ConnectionRefused,
        libc::ECONNRESET => ErrorCode::ConnectionReset,
        libc::ETIMEDOUT => ErrorCode::Timeout,
        libc::EINTR => ErrorCode::Interrupted,
        libc::EBUSY => ErrorCode::Busy,
        libc::EBADF => ErrorCode::InvalidHandle,
        libc::EINVAL => ErrorCode::InvalidArgument,
        libc::ENOTSUP | libc::EOPNOTSUPP => ErrorCode::NotSupported,
        _ => ErrorCode::Error,
    }
}

// Thread-local storage for the last error message
thread_local! {
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Set the last error message (thread-local)
pub fn set_last_error(msg: impl Into<String>) {
    LAST_ERROR.with(|e| {
        *e.borrow_mut() = Some(msg.into());
    });
}

/// Get the last error message (thread-local)
pub fn get_last_error() -> Option<String> {
    LAST_ERROR.with(|e| e.borrow().clone())
}

/// Clear the last error message
pub fn clear_last_error() {
    LAST_ERROR.with(|e| {
        *e.borrow_mut() = None;
    });
}

/// Get the last error message as a C function
/// Copies the message into the provided buffer
/// Returns the length written, or -1 if buffer too small
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_get_last_error(buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return -1;
    }

    let msg = get_last_error();
    match msg {
        None => {
            // No error, write empty string
            unsafe { *buf = 0 };
            0
        }
        Some(s) => {
            let bytes = s.as_bytes();
            if bytes.len() >= buf_len {
                return ErrorCode::BufferTooSmall.as_i32();
            }
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
                *buf.add(bytes.len()) = 0;
            }
            bytes.len() as i32
        }
    }
}

/// Clear the last error
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_clear_last_error() {
    clear_last_error();
}

// ─────────────────────────────────────────────────────────────────────────────
// FFI errno capture functions for CError support
// ─────────────────────────────────────────────────────────────────────────────

/// Capture current errno value (thread-local).
/// This should be called immediately after a C function returns an error indicator.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_capture_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
}

/// Get error message for a specific errno value.
/// Unlike arth_rt_strerror in lib.rs which captures current errno,
/// this takes the errno value as a parameter.
/// Copies the message into the provided buffer (null-terminated).
/// Returns the number of bytes written (excluding null), or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_strerror_for_errno(errno: i32, buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return -1;
    }

    let msg = std::io::Error::from_raw_os_error(errno).to_string();
    let bytes = msg.as_bytes();
    let copy_len = bytes.len().min(buf_len.saturating_sub(1));

    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, copy_len);
        *buf.add(copy_len) = 0;
    }

    copy_len as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_codes() {
        assert_eq!(ErrorCode::Success.as_i32(), 0);
        assert!(ErrorCode::Error.as_i32() < 0);
    }

    #[test]
    fn test_from_io_error() {
        let e = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        assert_eq!(ErrorCode::from(e), ErrorCode::NotFound);
    }

    #[test]
    fn test_last_error() {
        set_last_error("test error");
        assert_eq!(get_last_error(), Some("test error".to_string()));
        clear_last_error();
        assert_eq!(get_last_error(), None);
    }

    #[test]
    fn test_strerror_for_errno() {
        let mut buf = [0u8; 256];
        let len = arth_rt_strerror_for_errno(libc::ENOENT, buf.as_mut_ptr(), buf.len());
        assert!(len > 0, "strerror should return positive length");
        let msg = std::str::from_utf8(&buf[..len as usize]).unwrap();
        // The message should contain something meaningful (varies by platform)
        assert!(!msg.is_empty(), "strerror message should not be empty");
    }

    #[test]
    fn test_strerror_for_errno_null_buf() {
        let result = arth_rt_strerror_for_errno(libc::ENOENT, std::ptr::null_mut(), 256);
        assert_eq!(result, -1, "null buffer should return -1");
    }

    #[test]
    fn test_strerror_for_errno_zero_len() {
        let mut buf = [0u8; 1];
        let result = arth_rt_strerror_for_errno(libc::ENOENT, buf.as_mut_ptr(), 0);
        assert_eq!(result, -1, "zero length should return -1");
    }
}
