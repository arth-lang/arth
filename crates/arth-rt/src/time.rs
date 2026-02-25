//! Time operations using libc/POSIX APIs
//!
//! Provides wall-clock time, monotonic instants, and sleep functionality.

use crate::error::{ErrorCode, from_errno, set_last_error};
use crate::new_handle;

use std::collections::HashMap;
use std::sync::Mutex;

// -----------------------------------------------------------------------------
// Instant Tracking
// -----------------------------------------------------------------------------

/// Stores the start time of each instant handle
struct InstantData {
    #[cfg(target_os = "macos")]
    start: u64, // mach_absolute_time value
    #[cfg(not(target_os = "macos"))]
    start: libc::timespec,
}

lazy_static::lazy_static! {
    static ref INSTANTS: Mutex<HashMap<i64, InstantData>> = Mutex::new(HashMap::new());
}

// macOS-specific: mach_absolute_time for monotonic clock
#[cfg(target_os = "macos")]
mod mach {
    #[repr(C)]
    pub struct MachTimebaseInfo {
        pub numer: u32,
        pub denom: u32,
    }

    unsafe extern "C" {
        pub fn mach_absolute_time() -> u64;
        pub fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    }

    pub fn nanos_since(start: u64) -> u64 {
        use std::sync::OnceLock;

        static INFO: OnceLock<MachTimebaseInfo> = OnceLock::new();

        let now = unsafe { mach_absolute_time() };
        let elapsed = now - start;

        // Get timebase info for conversion to nanoseconds
        let info = INFO.get_or_init(|| {
            let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
            unsafe { mach_timebase_info(&mut info as *mut _) };
            info
        });

        elapsed * info.numer as u64 / info.denom as u64
    }
}

// -----------------------------------------------------------------------------
// Wall-Clock Time
// -----------------------------------------------------------------------------

/// Get current wall-clock time as milliseconds since Unix epoch
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_time_now() -> i64 {
    let mut tv: libc::timeval = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::gettimeofday(&mut tv, std::ptr::null_mut()) };

    if result < 0 {
        return -1;
    }

    (tv.tv_sec as i64) * 1000 + (tv.tv_usec as i64) / 1000
}

/// Get current wall-clock time as nanoseconds since Unix epoch
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_time_now_nanos() -> i64 {
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };

    #[cfg(target_os = "macos")]
    let result = unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts) };

    #[cfg(not(target_os = "macos"))]
    let result = unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts) };

    if result < 0 {
        return -1;
    }

    (ts.tv_sec as i64) * 1_000_000_000 + (ts.tv_nsec as i64)
}

// -----------------------------------------------------------------------------
// Monotonic Instants
// -----------------------------------------------------------------------------

/// Create a new monotonic instant
///
/// # Returns
/// * Handle (>= 0) for the instant
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_instant_now() -> i64 {
    #[cfg(target_os = "macos")]
    let data = InstantData {
        start: unsafe { mach::mach_absolute_time() },
    };

    #[cfg(not(target_os = "macos"))]
    let data = {
        let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        if result < 0 {
            return from_errno(unsafe { *libc::__error() }).as_i32() as i64;
        }
        InstantData { start: ts }
    };

    let handle = new_handle();
    let mut instants = INSTANTS.lock().unwrap();
    instants.insert(handle, data);

    handle
}

/// Get elapsed time in milliseconds since instant was created
///
/// # Returns
/// * Elapsed milliseconds (>= 0) on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_instant_elapsed(handle: i64) -> i64 {
    let instants = INSTANTS.lock().unwrap();
    let data = match instants.get(&handle) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    #[cfg(target_os = "macos")]
    {
        let nanos = mach::nanos_since(data.start);
        (nanos / 1_000_000) as i64
    }

    #[cfg(not(target_os = "macos"))]
    {
        let mut now: libc::timespec = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
        if result < 0 {
            return from_errno(unsafe { *libc::__error() }).as_i32() as i64;
        }

        let secs = now.tv_sec - data.start.tv_sec;
        let nsecs = now.tv_nsec - data.start.tv_nsec;
        let total_nanos = secs as i64 * 1_000_000_000 + nsecs as i64;
        total_nanos / 1_000_000
    }
}

/// Get elapsed time in nanoseconds since instant was created
///
/// # Returns
/// * Elapsed nanoseconds (>= 0) on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_instant_elapsed_nanos(handle: i64) -> i64 {
    let instants = INSTANTS.lock().unwrap();
    let data = match instants.get(&handle) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    #[cfg(target_os = "macos")]
    {
        mach::nanos_since(data.start) as i64
    }

    #[cfg(not(target_os = "macos"))]
    {
        let mut now: libc::timespec = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
        if result < 0 {
            return from_errno(unsafe { *libc::__error() }).as_i32() as i64;
        }

        let secs = now.tv_sec - data.start.tv_sec;
        let nsecs = now.tv_nsec - data.start.tv_nsec;
        secs as i64 * 1_000_000_000 + nsecs as i64
    }
}

/// Free an instant handle
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_instant_free(handle: i64) -> i32 {
    let mut instants = INSTANTS.lock().unwrap();
    if instants.remove(&handle).is_some() {
        0
    } else {
        ErrorCode::InvalidHandle.as_i32()
    }
}

// -----------------------------------------------------------------------------
// Sleep
// -----------------------------------------------------------------------------

/// Sleep for specified milliseconds
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure (e.g., interrupted)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sleep(millis: i64) -> i32 {
    if millis <= 0 {
        return 0;
    }

    let ts = libc::timespec {
        tv_sec: (millis / 1000) as libc::time_t,
        tv_nsec: ((millis % 1000) * 1_000_000) as libc::c_long,
    };

    let result = unsafe { libc::nanosleep(&ts, std::ptr::null_mut()) };
    if result < 0 {
        let errno = unsafe { *libc::__error() };
        // EINTR is not really an error for sleep
        if errno == libc::EINTR {
            return 0;
        }
        return from_errno(errno).as_i32();
    }
    0
}

/// Sleep for specified nanoseconds
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sleep_nanos(nanos: i64) -> i32 {
    if nanos <= 0 {
        return 0;
    }

    let ts = libc::timespec {
        tv_sec: (nanos / 1_000_000_000) as libc::time_t,
        tv_nsec: (nanos % 1_000_000_000) as libc::c_long,
    };

    let result = unsafe { libc::nanosleep(&ts, std::ptr::null_mut()) };
    if result < 0 {
        let errno = unsafe { *libc::__error() };
        if errno == libc::EINTR {
            return 0;
        }
        return from_errno(errno).as_i32();
    }
    0
}

// -----------------------------------------------------------------------------
// DateTime Formatting (using strftime)
// -----------------------------------------------------------------------------

/// Format a timestamp as a string
///
/// # Arguments
/// * `millis` - Milliseconds since Unix epoch
/// * `fmt` - Format string (strftime format)
/// * `fmt_len` - Length of format string
/// * `buf` - Output buffer
/// * `buf_len` - Output buffer size
///
/// # Returns
/// * Length of formatted string on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_time_format(
    millis: i64,
    fmt: *const u8,
    fmt_len: usize,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    if fmt.is_null() || buf.is_null() || buf_len == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    // Convert format to CString
    let fmt_slice = unsafe { std::slice::from_raw_parts(fmt, fmt_len) };
    let fmt_cstr = match std::ffi::CString::new(fmt_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };

    // Convert millis to time_t
    let time_t = (millis / 1000) as libc::time_t;

    // Get local time
    let tm = unsafe { libc::localtime(&time_t) };
    if tm.is_null() {
        set_last_error("Failed to convert time");
        return ErrorCode::Error.as_i32();
    }

    // Format
    let result =
        unsafe { libc::strftime(buf as *mut libc::c_char, buf_len, fmt_cstr.as_ptr(), tm) };

    if result == 0 {
        // Buffer too small or format error
        return ErrorCode::BufferTooSmall.as_i32();
    }

    result as i32
}

/// Parse a datetime string to milliseconds since epoch
///
/// # Arguments
/// * `str` - Input string
/// * `str_len` - Length of input string
/// * `fmt` - Format string (strptime format)
/// * `fmt_len` - Length of format string
///
/// # Returns
/// * Milliseconds since epoch on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_time_parse(
    s: *const u8,
    str_len: usize,
    fmt: *const u8,
    fmt_len: usize,
) -> i64 {
    if s.is_null() || fmt.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    // Convert to CStrings
    let str_slice = unsafe { std::slice::from_raw_parts(s, str_len) };
    let str_cstr = match std::ffi::CString::new(str_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
    };

    let fmt_slice = unsafe { std::slice::from_raw_parts(fmt, fmt_len) };
    let fmt_cstr = match std::ffi::CString::new(fmt_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
    };

    // Parse
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::strptime(str_cstr.as_ptr(), fmt_cstr.as_ptr(), &mut tm) };

    if result.is_null() {
        set_last_error("Failed to parse datetime");
        return ErrorCode::Error.as_i32() as i64;
    }

    // Convert to time_t
    let time_t = unsafe { libc::mktime(&mut tm) };
    if time_t == -1 {
        set_last_error("Failed to convert parsed time");
        return ErrorCode::Error.as_i32() as i64;
    }

    (time_t as i64) * 1000
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_now() {
        let now = arth_rt_time_now();
        assert!(now > 0);
        // Should be after year 2020 (in millis)
        assert!(now > 1577836800000);
    }

    #[test]
    fn test_instant_elapsed() {
        let handle = arth_rt_instant_now();
        assert!(handle >= 0);

        // Sleep a bit
        arth_rt_sleep(10);

        let elapsed = arth_rt_instant_elapsed(handle);
        assert!(elapsed >= 0);
        // Should be at least a few milliseconds
        assert!(elapsed >= 5, "Elapsed: {}", elapsed);

        arth_rt_instant_free(handle);
    }

    #[test]
    fn test_sleep() {
        let start = arth_rt_instant_now();

        arth_rt_sleep(50);

        let elapsed = arth_rt_instant_elapsed(start);
        assert!(elapsed >= 40, "Elapsed: {}", elapsed);

        arth_rt_instant_free(start);
    }

    #[test]
    fn test_time_format() {
        let millis = 1704067200000i64; // 2024-01-01 00:00:00 UTC
        let fmt = b"%Y-%m-%d";
        let mut buf = [0u8; 32];

        let len = arth_rt_time_format(millis, fmt.as_ptr(), fmt.len(), buf.as_mut_ptr(), buf.len());

        assert!(len > 0);
        let s = std::str::from_utf8(&buf[..len as usize]).unwrap();
        // Note: result depends on local timezone
        assert!(s.contains("2024") || s.contains("2023")); // Could be Dec 31 in some timezones
    }
}
