//! File, directory, and console I/O operations
//!
//! All functions use libc directly for C FFI compatibility.

use crate::error::{ErrorCode, from_errno, set_last_error};
use crate::new_handle;
use crate::string::{copy_str_to_buf, make_cstring};

use std::collections::HashMap;
use std::sync::Mutex;

// -----------------------------------------------------------------------------
// File Mode Constants
// -----------------------------------------------------------------------------

/// File open modes (matching Arth's FileMode enum)
pub const FILE_MODE_READ: i32 = 0;
pub const FILE_MODE_WRITE: i32 = 1;
pub const FILE_MODE_APPEND: i32 = 2;
pub const FILE_MODE_READ_WRITE: i32 = 3;

/// Seek whence constants
pub const SEEK_SET: i32 = libc::SEEK_SET;
pub const SEEK_CUR: i32 = libc::SEEK_CUR;
pub const SEEK_END: i32 = libc::SEEK_END;

// -----------------------------------------------------------------------------
// Directory Iterator State
// -----------------------------------------------------------------------------

struct DirIterator {
    dir: *mut libc::DIR,
    #[allow(dead_code)]
    path: String, // Stored for potential future use (e.g., constructing full paths)
}

// Safety: DIR pointers can be sent between threads if access is synchronized
unsafe impl Send for DirIterator {}

lazy_static::lazy_static! {
    static ref DIR_ITERATORS: Mutex<HashMap<i64, DirIterator>> = Mutex::new(HashMap::new());
}

// -----------------------------------------------------------------------------
// File Operations
// -----------------------------------------------------------------------------

/// Open a file
///
/// # Arguments
/// * `path` - Path to the file (UTF-8 encoded)
/// * `path_len` - Length of path in bytes
/// * `mode` - File mode (FILE_MODE_READ, FILE_MODE_WRITE, etc.)
///
/// # Returns
/// * File descriptor (>= 0) on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_file_open(path: *const u8, path_len: usize, mode: i32) -> i64 {
    // Convert path to CString
    let c_path = match unsafe { make_cstring(path, path_len) } {
        Some(p) => p,
        None => {
            set_last_error("Invalid path");
            return ErrorCode::InvalidArgument.as_i32() as i64;
        }
    };

    // Convert mode to libc flags
    let flags = match mode {
        FILE_MODE_READ => libc::O_RDONLY,
        FILE_MODE_WRITE => libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC,
        FILE_MODE_APPEND => libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND,
        FILE_MODE_READ_WRITE => libc::O_RDWR | libc::O_CREAT,
        _ => {
            set_last_error("Invalid file mode");
            return ErrorCode::InvalidArgument.as_i32() as i64;
        }
    };

    // Open the file
    let fd = unsafe { libc::open(c_path.as_ptr(), flags, 0o644) };

    if fd < 0 {
        let errno = unsafe { *libc::__error() };
        set_last_error(format!("Failed to open file: errno {}", errno));
        return from_errno(errno).as_i32() as i64;
    }

    fd as i64
}

/// Close a file
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_file_close(fd: i64) -> i32 {
    let result = unsafe { libc::close(fd as i32) };
    if result < 0 {
        let errno = unsafe { *libc::__error() };
        return from_errno(errno).as_i32();
    }
    0
}

/// Read from a file
///
/// # Arguments
/// * `fd` - File descriptor
/// * `buf` - Buffer to read into
/// * `buf_len` - Maximum bytes to read
///
/// # Returns
/// * Number of bytes read (>= 0) on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_file_read(fd: i64, buf: *mut u8, buf_len: usize) -> i64 {
    if buf.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let result = unsafe { libc::read(fd as i32, buf as *mut libc::c_void, buf_len) };

    if result < 0 {
        let errno = unsafe { *libc::__error() };
        return from_errno(errno).as_i32() as i64;
    }

    result as i64
}

/// Write to a file
///
/// # Arguments
/// * `fd` - File descriptor
/// * `buf` - Buffer to write from
/// * `buf_len` - Number of bytes to write
///
/// # Returns
/// * Number of bytes written (>= 0) on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_file_write(fd: i64, buf: *const u8, buf_len: usize) -> i64 {
    if buf.is_null() && buf_len > 0 {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let result = unsafe { libc::write(fd as i32, buf as *const libc::c_void, buf_len) };

    if result < 0 {
        let errno = unsafe { *libc::__error() };
        return from_errno(errno).as_i32() as i64;
    }

    result as i64
}

/// Seek in a file
///
/// # Arguments
/// * `fd` - File descriptor
/// * `offset` - Offset to seek to
/// * `whence` - SEEK_SET, SEEK_CUR, or SEEK_END
///
/// # Returns
/// * New file position on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_file_seek(fd: i64, offset: i64, whence: i32) -> i64 {
    let result = unsafe { libc::lseek(fd as i32, offset, whence) };

    if result < 0 {
        let errno = unsafe { *libc::__error() };
        return from_errno(errno).as_i32() as i64;
    }

    result
}

/// Get file size
///
/// # Returns
/// * File size in bytes on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_file_size(fd: i64) -> i64 {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::fstat(fd as i32, &mut stat) };

    if result < 0 {
        let errno = unsafe { *libc::__error() };
        return from_errno(errno).as_i32() as i64;
    }

    stat.st_size
}

/// Flush file buffers to disk
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_file_flush(fd: i64) -> i32 {
    let result = unsafe { libc::fsync(fd as i32) };
    if result < 0 {
        let errno = unsafe { *libc::__error() };
        return from_errno(errno).as_i32();
    }
    0
}

/// Check if a file exists
///
/// # Returns
/// * 1 if file exists
/// * 0 if file does not exist
/// * Negative error code on other errors
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_file_exists(path: *const u8, path_len: usize) -> i32 {
    let c_path = match unsafe { make_cstring(path, path_len) } {
        Some(p) => p,
        None => return ErrorCode::InvalidArgument.as_i32(),
    };

    let result = unsafe { libc::access(c_path.as_ptr(), libc::F_OK) };
    if result == 0 {
        1
    } else {
        let errno = unsafe { *libc::__error() };
        if errno == libc::ENOENT {
            0
        } else {
            from_errno(errno).as_i32()
        }
    }
}

/// Delete a file
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_file_delete(path: *const u8, path_len: usize) -> i32 {
    let c_path = match unsafe { make_cstring(path, path_len) } {
        Some(p) => p,
        None => return ErrorCode::InvalidArgument.as_i32(),
    };

    let result = unsafe { libc::unlink(c_path.as_ptr()) };
    if result < 0 {
        let errno = unsafe { *libc::__error() };
        return from_errno(errno).as_i32();
    }
    0
}

/// Copy a file
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_file_copy(
    src: *const u8,
    src_len: usize,
    dst: *const u8,
    dst_len: usize,
) -> i32 {
    // Open source file
    let src_fd = arth_rt_file_open(src, src_len, FILE_MODE_READ);
    if src_fd < 0 {
        return src_fd as i32;
    }

    // Open destination file
    let dst_fd = arth_rt_file_open(dst, dst_len, FILE_MODE_WRITE);
    if dst_fd < 0 {
        arth_rt_file_close(src_fd);
        return dst_fd as i32;
    }

    // Copy in chunks
    let mut buf = [0u8; 8192];
    loop {
        let n = arth_rt_file_read(src_fd, buf.as_mut_ptr(), buf.len());
        if n < 0 {
            arth_rt_file_close(src_fd);
            arth_rt_file_close(dst_fd);
            return n as i32;
        }
        if n == 0 {
            break; // EOF
        }

        let written = arth_rt_file_write(dst_fd, buf.as_ptr(), n as usize);
        if written < 0 {
            arth_rt_file_close(src_fd);
            arth_rt_file_close(dst_fd);
            return written as i32;
        }
    }

    arth_rt_file_close(src_fd);
    arth_rt_file_close(dst_fd);
    0
}

/// Move/rename a file
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_file_move(
    src: *const u8,
    src_len: usize,
    dst: *const u8,
    dst_len: usize,
) -> i32 {
    let c_src = match unsafe { make_cstring(src, src_len) } {
        Some(p) => p,
        None => return ErrorCode::InvalidArgument.as_i32(),
    };
    let c_dst = match unsafe { make_cstring(dst, dst_len) } {
        Some(p) => p,
        None => return ErrorCode::InvalidArgument.as_i32(),
    };

    let result = unsafe { libc::rename(c_src.as_ptr(), c_dst.as_ptr()) };
    if result < 0 {
        let errno = unsafe { *libc::__error() };
        return from_errno(errno).as_i32();
    }
    0
}

// -----------------------------------------------------------------------------
// Directory Operations
// -----------------------------------------------------------------------------

/// Create a directory
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_dir_create(path: *const u8, path_len: usize) -> i32 {
    let c_path = match unsafe { make_cstring(path, path_len) } {
        Some(p) => p,
        None => return ErrorCode::InvalidArgument.as_i32(),
    };

    let result = unsafe { libc::mkdir(c_path.as_ptr(), 0o755) };
    if result < 0 {
        let errno = unsafe { *libc::__error() };
        return from_errno(errno).as_i32();
    }
    0
}

/// Create a directory and all parent directories
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_dir_create_all(path: *const u8, path_len: usize) -> i32 {
    let path_str = match unsafe { crate::string::ptr_to_str(path, path_len) } {
        Some(s) => s,
        None => return ErrorCode::InvalidArgument.as_i32(),
    };

    // Create each component of the path
    let mut current = String::new();
    for component in path_str.split('/') {
        if component.is_empty() {
            if current.is_empty() {
                current.push('/');
            }
            continue;
        }
        if !current.is_empty() && !current.ends_with('/') {
            current.push('/');
        }
        current.push_str(component);

        // Try to create this directory (ignore EEXIST)
        let result = arth_rt_dir_create(current.as_ptr(), current.len());
        if result < 0 && result != ErrorCode::AlreadyExists.as_i32() {
            return result;
        }
    }
    0
}

/// Delete an empty directory
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_dir_delete(path: *const u8, path_len: usize) -> i32 {
    let c_path = match unsafe { make_cstring(path, path_len) } {
        Some(p) => p,
        None => return ErrorCode::InvalidArgument.as_i32(),
    };

    let result = unsafe { libc::rmdir(c_path.as_ptr()) };
    if result < 0 {
        let errno = unsafe { *libc::__error() };
        return from_errno(errno).as_i32();
    }
    0
}

/// Check if a directory exists
///
/// # Returns
/// * 1 if directory exists
/// * 0 if directory does not exist
/// * Negative error code on other errors
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_dir_exists(path: *const u8, path_len: usize) -> i32 {
    let c_path = match unsafe { make_cstring(path, path_len) } {
        Some(p) => p,
        None => return ErrorCode::InvalidArgument.as_i32(),
    };

    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::stat(c_path.as_ptr(), &mut stat) };

    if result < 0 {
        let errno = unsafe { *libc::__error() };
        if errno == libc::ENOENT {
            return 0;
        }
        return from_errno(errno).as_i32();
    }

    if (stat.st_mode & libc::S_IFMT) == libc::S_IFDIR {
        1
    } else {
        0
    }
}

/// Check if path is a directory
///
/// # Returns
/// * 1 if path is a directory
/// * 0 if path is not a directory
/// * Negative error code on error
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_is_dir(path: *const u8, path_len: usize) -> i32 {
    arth_rt_dir_exists(path, path_len)
}

/// Check if path is a file
///
/// # Returns
/// * 1 if path is a regular file
/// * 0 if path is not a regular file
/// * Negative error code on error
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_is_file(path: *const u8, path_len: usize) -> i32 {
    let c_path = match unsafe { make_cstring(path, path_len) } {
        Some(p) => p,
        None => return ErrorCode::InvalidArgument.as_i32(),
    };

    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::stat(c_path.as_ptr(), &mut stat) };

    if result < 0 {
        let errno = unsafe { *libc::__error() };
        if errno == libc::ENOENT {
            return 0;
        }
        return from_errno(errno).as_i32();
    }

    if (stat.st_mode & libc::S_IFMT) == libc::S_IFREG {
        1
    } else {
        0
    }
}

/// Open a directory for listing
///
/// # Returns
/// * Handle (>= 0) on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_dir_list(path: *const u8, path_len: usize) -> i64 {
    let path_str = match unsafe { crate::string::ptr_to_str(path, path_len) } {
        Some(s) => s.to_string(),
        None => return ErrorCode::InvalidArgument.as_i32() as i64,
    };

    let c_path = match unsafe { make_cstring(path, path_len) } {
        Some(p) => p,
        None => return ErrorCode::InvalidArgument.as_i32() as i64,
    };

    let dir = unsafe { libc::opendir(c_path.as_ptr()) };
    if dir.is_null() {
        let errno = unsafe { *libc::__error() };
        return from_errno(errno).as_i32() as i64;
    }

    let handle = new_handle();
    let iter = DirIterator {
        dir,
        path: path_str,
    };

    let mut iterators = DIR_ITERATORS.lock().unwrap();
    iterators.insert(handle, iter);

    handle
}

/// Get next entry from directory listing
///
/// # Arguments
/// * `handle` - Directory handle from arth_rt_dir_list
/// * `buf` - Buffer to write entry name
/// * `buf_len` - Buffer size
///
/// # Returns
/// * Length of entry name on success
/// * 0 if no more entries
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_dir_next(handle: i64, buf: *mut u8, buf_len: usize) -> i32 {
    let mut iterators = DIR_ITERATORS.lock().unwrap();
    let iter = match iterators.get_mut(&handle) {
        Some(i) => i,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    loop {
        let entry = unsafe { libc::readdir(iter.dir) };
        if entry.is_null() {
            // Check if it's an error or end of directory
            let errno = unsafe { *libc::__error() };
            if errno != 0 {
                return from_errno(errno).as_i32();
            }
            return 0; // No more entries
        }

        // Get entry name
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
        let name_str = match name.to_str() {
            Ok(s) => s,
            Err(_) => continue, // Skip invalid UTF-8 entries
        };

        // Skip . and ..
        if name_str == "." || name_str == ".." {
            continue;
        }

        // Copy to buffer
        return copy_str_to_buf(name_str, buf, buf_len);
    }
}

/// Close a directory listing handle
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_dir_close(handle: i64) -> i32 {
    let mut iterators = DIR_ITERATORS.lock().unwrap();
    let iter = match iterators.remove(&handle) {
        Some(i) => i,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let result = unsafe { libc::closedir(iter.dir) };
    if result < 0 {
        let errno = unsafe { *libc::__error() };
        return from_errno(errno).as_i32();
    }
    0
}

// -----------------------------------------------------------------------------
// Console Operations
// -----------------------------------------------------------------------------

/// Write to stdout
///
/// # Returns
/// * Number of bytes written on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_console_write(buf: *const u8, buf_len: usize) -> i64 {
    arth_rt_file_write(libc::STDOUT_FILENO as i64, buf, buf_len)
}

/// Write to stderr
///
/// # Returns
/// * Number of bytes written on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_console_write_err(buf: *const u8, buf_len: usize) -> i64 {
    arth_rt_file_write(libc::STDERR_FILENO as i64, buf, buf_len)
}

/// Write a newline to stdout
///
/// # Returns
/// * 1 on success (bytes written)
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_console_write_ln() -> i64 {
    arth_rt_console_write(b"\n".as_ptr(), 1)
}

/// Write a null-terminated C string to stdout
///
/// # Returns
/// * Number of bytes written on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_console_write_str(s: *const u8) -> i64 {
    if s.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }
    let len = unsafe { libc::strlen(s as *const libc::c_char) };
    arth_rt_console_write(s, len)
}

/// Write an i64 value to stdout as decimal string
///
/// # Returns
/// * Number of bytes written on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_console_write_i64(value: i64) -> i64 {
    // Convert i64 to string (max 20 digits + sign + null)
    let mut buf = [0u8; 24];
    let s = format_i64(value, &mut buf);
    arth_rt_console_write(s.as_ptr(), s.len())
}

/// Format an i64 into a byte slice, returns the formatted slice.
fn format_i64(mut value: i64, buf: &mut [u8; 24]) -> &[u8] {
    if value == 0 {
        buf[0] = b'0';
        return &buf[..1];
    }

    let negative = value < 0;
    if negative {
        value = -value;
    }

    let mut idx = buf.len();
    while value > 0 {
        idx -= 1;
        buf[idx] = b'0' + (value % 10) as u8;
        value /= 10;
    }

    if negative {
        idx -= 1;
        buf[idx] = b'-';
    }

    &buf[idx..]
}

/// Read a line from stdin
///
/// Reads until newline or buffer is full.
/// The newline is included in the output if present.
///
/// # Returns
/// * Number of bytes read on success
/// * 0 on EOF
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_console_read_line(buf: *mut u8, buf_len: usize) -> i64 {
    if buf.is_null() || buf_len == 0 {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let mut total = 0usize;
    let mut one_byte = [0u8; 1];

    while total < buf_len - 1 {
        let n = arth_rt_file_read(libc::STDIN_FILENO as i64, one_byte.as_mut_ptr(), 1);
        if n < 0 {
            return n;
        }
        if n == 0 {
            break; // EOF
        }

        unsafe {
            *buf.add(total) = one_byte[0];
        }
        total += 1;

        if one_byte[0] == b'\n' {
            break;
        }
    }

    // Null-terminate
    unsafe {
        *buf.add(total) = 0;
    }

    total as i64
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn test_file_open_close() {
        let path = "/tmp/arth_rt_test_file.txt";
        let path_bytes = path.as_bytes();

        // Create file
        let fd = arth_rt_file_open(path_bytes.as_ptr(), path_bytes.len(), FILE_MODE_WRITE);
        assert!(fd >= 0, "Failed to open file: {}", fd);

        // Close file
        let result = arth_rt_file_close(fd);
        assert_eq!(result, 0);

        // Cleanup
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_file_read_write() {
        let path = "/tmp/arth_rt_test_rw.txt";
        let path_bytes = path.as_bytes();
        let data = b"Hello, arth-rt!";

        // Write
        let fd = arth_rt_file_open(path_bytes.as_ptr(), path_bytes.len(), FILE_MODE_WRITE);
        assert!(fd >= 0);

        let written = arth_rt_file_write(fd, data.as_ptr(), data.len());
        assert_eq!(written, data.len() as i64);

        arth_rt_file_close(fd);

        // Read back
        let fd = arth_rt_file_open(path_bytes.as_ptr(), path_bytes.len(), FILE_MODE_READ);
        assert!(fd >= 0);

        let mut buf = [0u8; 32];
        let read = arth_rt_file_read(fd, buf.as_mut_ptr(), buf.len());
        assert_eq!(read, data.len() as i64);
        assert_eq!(&buf[..data.len()], data);

        arth_rt_file_close(fd);

        // Cleanup
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_file_exists() {
        let path = "/tmp/arth_rt_test_exists.txt";
        let path_bytes = path.as_bytes();

        // Should not exist yet
        assert_eq!(
            arth_rt_file_exists(path_bytes.as_ptr(), path_bytes.len()),
            0
        );

        // Create file
        fs::File::create(path).unwrap();

        // Should exist now
        assert_eq!(
            arth_rt_file_exists(path_bytes.as_ptr(), path_bytes.len()),
            1
        );

        // Cleanup
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_dir_create_delete() {
        let path = "/tmp/arth_rt_test_dir";
        let path_bytes = path.as_bytes();

        // Create
        let result = arth_rt_dir_create(path_bytes.as_ptr(), path_bytes.len());
        assert!(result == 0 || result == ErrorCode::AlreadyExists.as_i32());

        // Check exists
        assert_eq!(arth_rt_dir_exists(path_bytes.as_ptr(), path_bytes.len()), 1);
        assert_eq!(arth_rt_is_dir(path_bytes.as_ptr(), path_bytes.len()), 1);

        // Delete
        let result = arth_rt_dir_delete(path_bytes.as_ptr(), path_bytes.len());
        assert_eq!(result, 0);
    }

    #[test]
    fn test_dir_list() {
        let path = "/tmp";
        let path_bytes = path.as_bytes();

        let handle = arth_rt_dir_list(path_bytes.as_ptr(), path_bytes.len());
        assert!(handle >= 0);

        let mut buf = [0u8; 256];
        let mut count = 0;
        loop {
            let len = arth_rt_dir_next(handle, buf.as_mut_ptr(), buf.len());
            if len == 0 {
                break;
            }
            assert!(len > 0);
            count += 1;
        }

        assert!(count > 0, "Expected at least one entry in /tmp");

        arth_rt_dir_close(handle);
    }
}
