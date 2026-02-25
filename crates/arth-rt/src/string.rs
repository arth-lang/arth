//! String and bytes utilities for C interop
//!
//! Provides helpers for converting between Arth strings and C strings.

use std::ffi::{CStr, CString};
use std::ptr;

/// Convert a pointer and length to a Rust string slice
///
/// # Safety
/// - `ptr` must point to valid UTF-8 data of at least `len` bytes
/// - The memory must remain valid for the lifetime of the returned slice
#[inline]
pub unsafe fn ptr_to_str<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    std::str::from_utf8(slice).ok()
}

/// Convert a pointer and length to a Rust byte slice
///
/// # Safety
/// - `ptr` must point to valid memory of at least `len` bytes
/// - The memory must remain valid for the lifetime of the returned slice
#[inline]
pub unsafe fn ptr_to_bytes<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(ptr, len) })
}

/// Convert a null-terminated C string to a Rust string
///
/// # Safety
/// - `ptr` must point to a null-terminated string
#[inline]
pub unsafe fn cstr_to_str<'a>(ptr: *const libc::c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

/// Copy a Rust string into a C buffer
///
/// Returns the number of bytes written (excluding null terminator),
/// or -1 if the buffer is too small.
///
/// The output is always null-terminated if buf_len > 0.
#[inline]
pub fn copy_str_to_buf(s: &str, buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return -1;
    }

    let bytes = s.as_bytes();
    if bytes.len() >= buf_len {
        // Buffer too small, but we still null-terminate
        unsafe {
            *buf = 0;
        }
        return -1;
    }

    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
        *buf.add(bytes.len()) = 0;
    }

    bytes.len() as i32
}

/// Copy bytes into a buffer
///
/// Returns the number of bytes written, or -1 if buffer is too small.
/// Does NOT null-terminate.
#[inline]
pub fn copy_bytes_to_buf(src: &[u8], buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() {
        return -1;
    }

    if src.len() > buf_len {
        return -1;
    }

    unsafe {
        ptr::copy_nonoverlapping(src.as_ptr(), buf, src.len());
    }

    src.len() as i32
}

/// Create a CString from a pointer and length
///
/// Returns None if the string contains interior null bytes.
///
/// # Safety
/// - `ptr` must point to valid memory of at least `len` bytes
pub unsafe fn make_cstring(ptr: *const u8, len: usize) -> Option<CString> {
    if ptr.is_null() {
        return None;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    CString::new(slice).ok()
}

/// Get the length of a string in a buffer
///
/// Counts bytes until null terminator or buf_len, whichever comes first.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_strlen(buf: *const u8, buf_len: usize) -> usize {
    if buf.is_null() {
        return 0;
    }

    for i in 0..buf_len {
        if unsafe { *buf.add(i) } == 0 {
            return i;
        }
    }
    buf_len
}

/// Compare two byte buffers
///
/// Returns:
/// - 0 if equal
/// - negative if a < b
/// - positive if a > b
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_memcmp(a: *const u8, a_len: usize, b: *const u8, b_len: usize) -> i32 {
    if a.is_null() && b.is_null() {
        return 0;
    }
    if a.is_null() {
        return -1;
    }
    if b.is_null() {
        return 1;
    }

    let min_len = a_len.min(b_len);
    for i in 0..min_len {
        let av = unsafe { *a.add(i) };
        let bv = unsafe { *b.add(i) };
        if av != bv {
            return (av as i32) - (bv as i32);
        }
    }

    // Lengths differ
    match a_len.cmp(&b_len) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Greater => 1,
        std::cmp::Ordering::Equal => 0,
    }
}

/// Copy memory between buffers
///
/// Returns number of bytes copied, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_memcpy(
    dst: *mut u8,
    dst_len: usize,
    src: *const u8,
    src_len: usize,
) -> i32 {
    if dst.is_null() || src.is_null() {
        return -1;
    }

    let copy_len = src_len.min(dst_len);
    unsafe {
        ptr::copy_nonoverlapping(src, dst, copy_len);
    }

    copy_len as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ptr_to_str() {
        let s = "hello";
        let result = unsafe { ptr_to_str(s.as_ptr(), s.len()) };
        assert_eq!(result, Some("hello"));
    }

    #[test]
    fn test_ptr_to_str_null() {
        let result = unsafe { ptr_to_str(ptr::null(), 0) };
        assert_eq!(result, None);
    }

    #[test]
    fn test_copy_str_to_buf() {
        let mut buf = [0u8; 16];
        let result = copy_str_to_buf("hello", buf.as_mut_ptr(), buf.len());
        assert_eq!(result, 5);
        assert_eq!(&buf[..6], b"hello\0");
    }

    #[test]
    fn test_copy_str_too_small() {
        let mut buf = [0u8; 3];
        let result = copy_str_to_buf("hello", buf.as_mut_ptr(), buf.len());
        assert_eq!(result, -1);
    }

    #[test]
    fn test_strlen() {
        let s = b"hello\0world";
        let len = arth_rt_strlen(s.as_ptr(), s.len());
        assert_eq!(len, 5);
    }

    #[test]
    fn test_memcmp() {
        let a = b"hello";
        let b = b"hello";
        assert_eq!(arth_rt_memcmp(a.as_ptr(), a.len(), b.as_ptr(), b.len()), 0);

        let c = b"hellp";
        assert!(arth_rt_memcmp(a.as_ptr(), a.len(), c.as_ptr(), c.len()) < 0);
    }
}
