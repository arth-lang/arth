//! Memory allocation wrappers for C FFI
//!
//! Provides malloc/free wrappers and memory utilities for native compilation.
//! These functions use libc directly for maximum portability.

use crate::error::{ErrorCode, set_last_error};

// -----------------------------------------------------------------------------
// Basic Allocation
// -----------------------------------------------------------------------------

/// Allocate memory of specified size
///
/// # Arguments
/// * `size` - Number of bytes to allocate
///
/// # Returns
/// * Pointer to allocated memory on success
/// * Null pointer on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_alloc(size: usize) -> *mut u8 {
    if size == 0 {
        return std::ptr::null_mut();
    }

    let ptr = unsafe { libc::malloc(size) as *mut u8 };
    if ptr.is_null() {
        set_last_error("Memory allocation failed");
    }
    ptr
}

/// Allocate zero-initialized memory
///
/// # Arguments
/// * `count` - Number of elements
/// * `size` - Size of each element in bytes
///
/// # Returns
/// * Pointer to allocated memory on success
/// * Null pointer on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_calloc(count: usize, size: usize) -> *mut u8 {
    if count == 0 || size == 0 {
        return std::ptr::null_mut();
    }

    let ptr = unsafe { libc::calloc(count, size) as *mut u8 };
    if ptr.is_null() {
        set_last_error("Memory allocation failed");
    }
    ptr
}

/// Reallocate memory to new size
///
/// # Arguments
/// * `ptr` - Pointer to existing allocation (or null for new allocation)
/// * `new_size` - New size in bytes
///
/// # Returns
/// * Pointer to reallocated memory on success
/// * Null pointer on failure (original memory unchanged)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_realloc(ptr: *mut u8, new_size: usize) -> *mut u8 {
    if new_size == 0 {
        if !ptr.is_null() {
            unsafe { libc::free(ptr as *mut libc::c_void) };
        }
        return std::ptr::null_mut();
    }

    let new_ptr = unsafe { libc::realloc(ptr as *mut libc::c_void, new_size) as *mut u8 };
    if new_ptr.is_null() {
        set_last_error("Memory reallocation failed");
    }
    new_ptr
}

/// Free allocated memory
///
/// # Arguments
/// * `ptr` - Pointer to memory to free (null is safe)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_free(ptr: *mut u8) {
    if !ptr.is_null() {
        unsafe { libc::free(ptr as *mut libc::c_void) };
    }
}

// -----------------------------------------------------------------------------
// Aligned Allocation
// -----------------------------------------------------------------------------

/// Allocate aligned memory
///
/// # Arguments
/// * `alignment` - Required alignment (must be power of 2)
/// * `size` - Number of bytes to allocate
///
/// # Returns
/// * Pointer to aligned memory on success
/// * Null pointer on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_alloc_aligned(alignment: usize, size: usize) -> *mut u8 {
    if size == 0 {
        return std::ptr::null_mut();
    }

    // Alignment must be power of 2 and at least sizeof(void*)
    if alignment == 0 || (alignment & (alignment - 1)) != 0 {
        set_last_error("Alignment must be a power of 2");
        return std::ptr::null_mut();
    }

    let align = alignment.max(std::mem::size_of::<*mut u8>());

    let mut ptr: *mut libc::c_void = std::ptr::null_mut();
    let result = unsafe { libc::posix_memalign(&mut ptr, align, size) };

    if result != 0 {
        set_last_error("Aligned memory allocation failed");
        return std::ptr::null_mut();
    }

    ptr as *mut u8
}

/// Free aligned memory (same as regular free on POSIX)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_free_aligned(ptr: *mut u8) {
    arth_rt_free(ptr);
}

// -----------------------------------------------------------------------------
// Memory Operations
// -----------------------------------------------------------------------------

/// Set memory to a byte value
///
/// # Arguments
/// * `ptr` - Pointer to memory
/// * `value` - Byte value to set
/// * `count` - Number of bytes to set
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_memset(ptr: *mut u8, value: i32, count: usize) -> i32 {
    if ptr.is_null() && count > 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    if count > 0 {
        unsafe { libc::memset(ptr as *mut libc::c_void, value, count) };
    }
    0
}

/// Copy memory (non-overlapping)
///
/// # Arguments
/// * `dst` - Destination pointer
/// * `src` - Source pointer
/// * `count` - Number of bytes to copy
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_memcpy_raw(dst: *mut u8, src: *const u8, count: usize) -> i32 {
    if count == 0 {
        return 0;
    }

    if dst.is_null() || src.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    unsafe { libc::memcpy(dst as *mut libc::c_void, src as *const libc::c_void, count) };
    0
}

/// Move memory (handles overlapping regions)
///
/// # Arguments
/// * `dst` - Destination pointer
/// * `src` - Source pointer
/// * `count` - Number of bytes to move
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_memmove(dst: *mut u8, src: *const u8, count: usize) -> i32 {
    if count == 0 {
        return 0;
    }

    if dst.is_null() || src.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    unsafe { libc::memmove(dst as *mut libc::c_void, src as *const libc::c_void, count) };
    0
}

// -----------------------------------------------------------------------------
// Memory Info
// -----------------------------------------------------------------------------

/// Get the usable size of an allocation
///
/// Note: This is platform-specific and may return 0 if not supported.
///
/// # Arguments
/// * `ptr` - Pointer to allocated memory
///
/// # Returns
/// * Usable size in bytes, or 0 if unknown
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_alloc_size(ptr: *const u8) -> usize {
    if ptr.is_null() {
        return 0;
    }

    #[cfg(target_os = "macos")]
    {
        unsafe { libc::malloc_size(ptr as *const libc::c_void) }
    }

    #[cfg(target_os = "linux")]
    {
        // malloc_usable_size is available on glibc
        extern "C" {
            fn malloc_usable_size(ptr: *const libc::c_void) -> usize;
        }
        unsafe { malloc_usable_size(ptr as *const libc::c_void) }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        0 // Not supported on this platform
    }
}

// -----------------------------------------------------------------------------
// Region-Based Allocation
// -----------------------------------------------------------------------------
//
// Region-based allocation provides deterministic cleanup for loop-local values.
// When a loop exits, all allocations in that region can be freed at once.
// For now, these are no-ops as the VM handles region tracking internally.
// Future optimization: implement actual region allocators for native mode.

/// Enter a new allocation region
///
/// # Arguments
/// * `region_id` - Unique identifier for this region
///
/// This is called at the start of loops and other scopes that need
/// deterministic cleanup.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_region_enter(_region_id: u32) {
    // No-op for now - region tracking is handled by the compiler
    // Future: could implement arena/bump allocator for region-local allocations
}

/// Exit an allocation region and free all region-local allocations
///
/// # Arguments
/// * `region_id` - The region identifier passed to region_enter
///
/// This is called at the end of loops and other scopes.
/// All allocations made within this region are freed.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_region_exit(_region_id: u32) {
    // No-op for now - the compiler inserts explicit drop calls
    // Future: could bulk-free all region-local allocations
}

/// Allocate memory within the current region
///
/// # Arguments
/// * `size` - Number of bytes to allocate
///
/// # Returns
/// * Pointer to allocated memory
///
/// Memory allocated with this function is automatically freed when
/// the enclosing region exits.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_region_alloc(size: usize) -> *mut u8 {
    // For now, just use regular allocation
    // Future: allocate from region-specific arena
    arth_rt_alloc(size)
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alloc_free() {
        let ptr = arth_rt_alloc(1024);
        assert!(!ptr.is_null());

        // Write to verify it's usable
        unsafe {
            *ptr = 42;
            assert_eq!(*ptr, 42);
        }

        arth_rt_free(ptr);
    }

    #[test]
    fn test_calloc() {
        let ptr = arth_rt_calloc(10, 8);
        assert!(!ptr.is_null());

        // Verify zero-initialized
        for i in 0..80 {
            assert_eq!(unsafe { *ptr.add(i) }, 0);
        }

        arth_rt_free(ptr);
    }

    #[test]
    fn test_realloc() {
        let ptr = arth_rt_alloc(64);
        assert!(!ptr.is_null());

        // Write some data
        unsafe {
            *ptr = 123;
        }

        // Reallocate larger
        let new_ptr = arth_rt_realloc(ptr, 256);
        assert!(!new_ptr.is_null());

        // Data should be preserved
        assert_eq!(unsafe { *new_ptr }, 123);

        arth_rt_free(new_ptr);
    }

    #[test]
    fn test_aligned_alloc() {
        let ptr = arth_rt_alloc_aligned(64, 1024);
        assert!(!ptr.is_null());

        // Verify alignment
        assert_eq!((ptr as usize) % 64, 0);

        arth_rt_free_aligned(ptr);
    }

    #[test]
    fn test_memset() {
        let ptr = arth_rt_alloc(100);
        assert!(!ptr.is_null());

        arth_rt_memset(ptr, 0xAB, 100);

        for i in 0..100 {
            assert_eq!(unsafe { *ptr.add(i) }, 0xAB);
        }

        arth_rt_free(ptr);
    }

    #[test]
    fn test_memcpy() {
        let src = arth_rt_alloc(100);
        let dst = arth_rt_alloc(100);
        assert!(!src.is_null() && !dst.is_null());

        // Fill source with pattern
        for i in 0..100 {
            unsafe { *src.add(i) = i as u8 };
        }

        arth_rt_memcpy_raw(dst, src, 100);

        // Verify copy
        for i in 0..100 {
            assert_eq!(unsafe { *dst.add(i) }, i as u8);
        }

        arth_rt_free(src);
        arth_rt_free(dst);
    }

    #[test]
    fn test_zero_size() {
        let ptr = arth_rt_alloc(0);
        assert!(ptr.is_null());

        let ptr = arth_rt_calloc(0, 10);
        assert!(ptr.is_null());

        let ptr = arth_rt_calloc(10, 0);
        assert!(ptr.is_null());
    }

    #[test]
    fn test_free_null() {
        // Should not crash
        arth_rt_free(std::ptr::null_mut());
        arth_rt_free_aligned(std::ptr::null_mut());
    }
}
