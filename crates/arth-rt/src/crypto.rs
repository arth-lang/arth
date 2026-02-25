//! Cryptographic primitives for Arth - C FFI only
//!
//! This module provides:
//! - Secure memory allocation (mlock/munlock)
//! - Secure memory zeroing (explicit_bzero)
//! - Constant-time comparison
//! - Base64 and hex encoding/decoding
//!
//! All functions use the C ABI and can be called from native-compiled Arth code.
//!
//! # Design Principles
//!
//! 1. **C ABI Only**: No Rust library dependencies for crypto operations
//! 2. **Secure by Default**: Memory is locked and zeroed automatically
//! 3. **Platform Native**: Uses libc functions directly
//! 4. **Handle-based**: Secure memory regions use opaque handles

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use lazy_static::lazy_static;

use crate::new_handle;

// =============================================================================
// Secure Memory Handle Management
// =============================================================================

/// A secure memory region that is:
/// - Locked in RAM (mlock'd where available)
/// - Zeroed on deallocation
struct SecureRegion {
    ptr: *mut u8,
    len: usize,
    locked: bool,
}

// SAFETY: SecureRegion is only accessed through synchronized HashMap
unsafe impl Send for SecureRegion {}

lazy_static! {
    /// Global registry of secure memory regions
    static ref SECURE_REGIONS: Mutex<HashMap<i64, SecureRegion>> = Mutex::new(HashMap::new());
}

// =============================================================================
// Secure Memory Allocation
// =============================================================================

/// Allocate secure memory region
///
/// The memory is:
/// - Zeroed before use
/// - Locked in RAM (mlock) if supported by the platform
///
/// Returns a handle to the secure region, or -1 on failure.
///
/// # C ABI
/// ```c
/// int64_t arth_rt_secure_alloc(size_t size);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_secure_alloc(size: usize) -> i64 {
    if size == 0 {
        return -1;
    }

    // Allocate zeroed memory
    let ptr = unsafe { libc::calloc(1, size) as *mut u8 };
    if ptr.is_null() {
        return -1;
    }

    // Try to lock the memory (prevent swapping)
    let locked = unsafe { libc::mlock(ptr as *const libc::c_void, size) == 0 };

    let handle = new_handle();
    let region = SecureRegion {
        ptr,
        len: size,
        locked,
    };

    if let Ok(mut regions) = SECURE_REGIONS.lock() {
        regions.insert(handle, region);
        handle
    } else {
        // Cleanup on failure
        unsafe {
            if locked {
                libc::munlock(ptr as *const libc::c_void, size);
            }
            libc::free(ptr as *mut libc::c_void);
        }
        -1
    }
}

/// Free a secure memory region
///
/// The memory is:
/// - Securely zeroed before freeing
/// - Unlocked (munlock) if it was locked
///
/// Returns 0 on success, -1 on failure.
///
/// # C ABI
/// ```c
/// int32_t arth_rt_secure_free(int64_t handle);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_secure_free(handle: i64) -> i32 {
    let region = {
        let mut regions = match SECURE_REGIONS.lock() {
            Ok(r) => r,
            Err(_) => return -1,
        };
        regions.remove(&handle)
    };

    match region {
        Some(r) => {
            unsafe {
                // Secure zero the memory using platform-specific function
                secure_zero(r.ptr, r.len);

                // Unlock if locked
                if r.locked {
                    libc::munlock(r.ptr as *const libc::c_void, r.len);
                }

                // Free the memory
                libc::free(r.ptr as *mut libc::c_void);
            }
            0
        }
        None => -1,
    }
}

/// Get pointer to secure memory region
///
/// Returns the raw pointer to the secure memory, or null if handle is invalid.
///
/// # C ABI
/// ```c
/// void* arth_rt_secure_ptr(int64_t handle);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_secure_ptr(handle: i64) -> *mut u8 {
    let regions = match SECURE_REGIONS.lock() {
        Ok(r) => r,
        Err(_) => return std::ptr::null_mut(),
    };

    match regions.get(&handle) {
        Some(r) => r.ptr,
        None => std::ptr::null_mut(),
    }
}

/// Get size of secure memory region
///
/// Returns the size in bytes, or 0 if handle is invalid.
///
/// # C ABI
/// ```c
/// size_t arth_rt_secure_len(int64_t handle);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_secure_len(handle: i64) -> usize {
    let regions = match SECURE_REGIONS.lock() {
        Ok(r) => r,
        Err(_) => return 0,
    };

    match regions.get(&handle) {
        Some(r) => r.len,
        None => 0,
    }
}

/// Copy data into secure memory region
///
/// Returns the number of bytes copied, or -1 on error.
///
/// # C ABI
/// ```c
/// int64_t arth_rt_secure_write(int64_t handle, const void* src, size_t len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_secure_write(handle: i64, src: *const u8, len: usize) -> i64 {
    if src.is_null() {
        return -1;
    }

    let regions = match SECURE_REGIONS.lock() {
        Ok(r) => r,
        Err(_) => return -1,
    };

    match regions.get(&handle) {
        Some(r) => {
            if len > r.len {
                return -1;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(src, r.ptr, len);
            }
            len as i64
        }
        None => -1,
    }
}

/// Read data from secure memory region
///
/// Returns the number of bytes read, or -1 on error.
///
/// # C ABI
/// ```c
/// int64_t arth_rt_secure_read(int64_t handle, void* dst, size_t len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_secure_read(handle: i64, dst: *mut u8, len: usize) -> i64 {
    if dst.is_null() {
        return -1;
    }

    let regions = match SECURE_REGIONS.lock() {
        Ok(r) => r,
        Err(_) => return -1,
    };

    match regions.get(&handle) {
        Some(r) => {
            let to_read = std::cmp::min(len, r.len);
            unsafe {
                std::ptr::copy_nonoverlapping(r.ptr, dst, to_read);
            }
            to_read as i64
        }
        None => -1,
    }
}

// =============================================================================
// Secure Zeroing
// =============================================================================

/// Securely zero a memory region
///
/// Uses platform-specific functions that are guaranteed not to be
/// optimized away by the compiler.
///
/// # C ABI
/// ```c
/// void arth_rt_secure_zero(void* ptr, size_t len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_secure_zero(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    unsafe {
        secure_zero(ptr, len);
    }
}

// External C functions for secure zeroing
#[cfg(any(target_os = "freebsd", target_os = "openbsd", target_os = "linux"))]
unsafe extern "C" {
    fn explicit_bzero(s: *mut libc::c_void, n: libc::size_t);
}

#[cfg(target_os = "windows")]
unsafe extern "C" {
    fn RtlSecureZeroMemory(ptr: *mut libc::c_void, cnt: libc::size_t);
}

/// Internal secure zeroing function
///
/// Uses platform-specific functions to ensure memory is zeroed without
/// the compiler optimizing the operation away.
#[inline(never)]
unsafe fn secure_zero(ptr: *mut u8, len: usize) {
    // macOS: use memset_s which is guaranteed not to be optimized away
    #[cfg(target_os = "macos")]
    {
        // memset_s is available in C11 and macOS
        unsafe extern "C" {
            fn memset_s(
                s: *mut libc::c_void,
                smax: libc::size_t,
                c: libc::c_int,
                n: libc::size_t,
            ) -> libc::c_int;
        }
        unsafe {
            memset_s(ptr as *mut libc::c_void, len, 0, len);
        }
    }

    // Linux, FreeBSD, OpenBSD: use explicit_bzero
    #[cfg(any(target_os = "freebsd", target_os = "openbsd", target_os = "linux"))]
    {
        unsafe { explicit_bzero(ptr as *mut libc::c_void, len) };
    }

    #[cfg(target_os = "windows")]
    {
        // Windows uses SecureZeroMemory via RtlSecureZeroMemory
        unsafe { RtlSecureZeroMemory(ptr as *mut libc::c_void, len) };
    }

    // Fallback for other platforms: volatile writes
    #[cfg(not(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "linux",
        target_os = "windows"
    )))]
    {
        for i in 0..len {
            unsafe { std::ptr::write_volatile(ptr.add(i), 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

// =============================================================================
// Constant-Time Comparison
// =============================================================================

/// Compare two byte arrays in constant time
///
/// This comparison takes the same amount of time regardless of where
/// the arrays differ, preventing timing attacks.
///
/// Returns 1 if equal, 0 if not equal.
///
/// # C ABI
/// ```c
/// int32_t arth_rt_secure_compare(const void* a, const void* b, size_t len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_secure_compare(a: *const u8, b: *const u8, len: usize) -> i32 {
    if a.is_null() || b.is_null() {
        return if a == b { 1 } else { 0 };
    }

    let mut diff: u8 = 0;

    for i in 0..len {
        unsafe {
            diff |= *a.add(i) ^ *b.add(i);
        }
    }

    // Use volatile to prevent compiler from optimizing
    let result = if diff == 0 { 1 } else { 0 };
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    result
}

// Note: Base64 and hex encoding functions are in encoding.rs
// The crypto module re-exports them from there via intrinsics

// =============================================================================
// Cryptographic Hash Functions
// =============================================================================
//
// This section provides hash function implementations for:
// - SHA-256, SHA-384, SHA-512 (SHA-2 family)
// - SHA3-256, SHA3-512 (SHA-3 family)
// - BLAKE3
//
// All functions use the C ABI and are callable from Arth code via intrinsics.

use digest::Digest;

/// Hash algorithm identifiers matching Arth's `HashAlgorithm` enum.
/// The values correspond to the enum variant ordinals.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashAlgorithm {
    Sha256 = 0,
    Sha384 = 1,
    Sha512 = 2,
    Sha3_256 = 3,
    Sha3_512 = 4,
    Blake3 = 5,
}

impl HashAlgorithm {
    /// Returns the output size in bytes for this algorithm.
    pub fn output_size(self) -> usize {
        match self {
            HashAlgorithm::Sha256 => 32,
            HashAlgorithm::Sha384 => 48,
            HashAlgorithm::Sha512 => 64,
            HashAlgorithm::Sha3_256 => 32,
            HashAlgorithm::Sha3_512 => 64,
            HashAlgorithm::Blake3 => 32,
        }
    }

    /// Try to convert from an i32 value.
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(HashAlgorithm::Sha256),
            1 => Some(HashAlgorithm::Sha384),
            2 => Some(HashAlgorithm::Sha512),
            3 => Some(HashAlgorithm::Sha3_256),
            4 => Some(HashAlgorithm::Sha3_512),
            5 => Some(HashAlgorithm::Blake3),
            _ => None,
        }
    }
}

/// Get the output size for a hash algorithm.
///
/// # Arguments
/// * `algorithm` - Hash algorithm identifier (0-5)
///
/// # Returns
/// * Output size in bytes, or 0 if algorithm is invalid
///
/// # C ABI
/// ```c
/// size_t arth_rt_hash_output_size(int32_t algorithm);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hash_output_size(algorithm: i32) -> usize {
    match HashAlgorithm::from_i32(algorithm) {
        Some(algo) => algo.output_size(),
        None => 0,
    }
}

/// Compute a one-shot hash of the input data.
///
/// # Arguments
/// * `algorithm` - Hash algorithm identifier (0-5)
/// * `input` - Pointer to input data
/// * `input_len` - Length of input data in bytes
/// * `output` - Pointer to output buffer (must be at least `arth_rt_hash_output_size` bytes)
/// * `output_len` - Size of output buffer
///
/// # Returns
/// * Number of bytes written on success
/// * -1 if output buffer is too small
/// * -2 if input or output is null
/// * -3 if algorithm is invalid
///
/// # C ABI
/// ```c
/// int64_t arth_rt_hash(int32_t algorithm, const uint8_t* input, size_t input_len,
///                      uint8_t* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hash(
    algorithm: i32,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if output.is_null() {
        return -2;
    }

    let algo = match HashAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -3,
    };

    let required_len = algo.output_size();
    if output_len < required_len {
        return -1;
    }

    // Handle null or zero-length input
    let input_slice = if input.is_null() || input_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(input, input_len) }
    };

    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, required_len) };

    // Compute hash based on algorithm
    match algo {
        HashAlgorithm::Sha256 => {
            let result = sha2::Sha256::digest(input_slice);
            output_slice.copy_from_slice(&result);
        }
        HashAlgorithm::Sha384 => {
            let result = sha2::Sha384::digest(input_slice);
            output_slice.copy_from_slice(&result);
        }
        HashAlgorithm::Sha512 => {
            let result = sha2::Sha512::digest(input_slice);
            output_slice.copy_from_slice(&result);
        }
        HashAlgorithm::Sha3_256 => {
            let result = sha3::Sha3_256::digest(input_slice);
            output_slice.copy_from_slice(&result);
        }
        HashAlgorithm::Sha3_512 => {
            let result = sha3::Sha3_512::digest(input_slice);
            output_slice.copy_from_slice(&result);
        }
        HashAlgorithm::Blake3 => {
            let result = blake3::hash(input_slice);
            output_slice.copy_from_slice(result.as_bytes());
        }
    }

    required_len as i64
}

// =============================================================================
// Incremental Hasher
// =============================================================================

/// Enum to hold different hasher types for incremental hashing.
/// Large hashers (SHA-3, BLAKE3) are boxed to keep enum size reasonable.
enum HasherState {
    Sha256(sha2::Sha256),
    Sha384(sha2::Sha384),
    Sha512(sha2::Sha512),
    Sha3_256(Box<sha3::Sha3_256>),
    Sha3_512(Box<sha3::Sha3_512>),
    Blake3(Box<blake3::Hasher>),
}

/// Hasher wrapper with algorithm tag.
struct Hasher {
    algorithm: HashAlgorithm,
    state: HasherState,
}

lazy_static! {
    /// Global registry of active hashers
    static ref HASHERS: Mutex<HashMap<i64, Hasher>> = Mutex::new(HashMap::new());
}

/// Create a new incremental hasher.
///
/// # Arguments
/// * `algorithm` - Hash algorithm identifier (0-5)
///
/// # Returns
/// * Handle to the hasher on success (positive)
/// * -1 if failed to allocate
/// * -3 if algorithm is invalid
///
/// # C ABI
/// ```c
/// int64_t arth_rt_hasher_new(int32_t algorithm);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hasher_new(algorithm: i32) -> i64 {
    let algo = match HashAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -3,
    };

    let state = match algo {
        HashAlgorithm::Sha256 => HasherState::Sha256(sha2::Sha256::new()),
        HashAlgorithm::Sha384 => HasherState::Sha384(sha2::Sha384::new()),
        HashAlgorithm::Sha512 => HasherState::Sha512(sha2::Sha512::new()),
        HashAlgorithm::Sha3_256 => HasherState::Sha3_256(Box::new(sha3::Sha3_256::new())),
        HashAlgorithm::Sha3_512 => HasherState::Sha3_512(Box::new(sha3::Sha3_512::new())),
        HashAlgorithm::Blake3 => HasherState::Blake3(Box::new(blake3::Hasher::new())),
    };

    let hasher = Hasher {
        algorithm: algo,
        state,
    };

    let handle = new_handle();
    if let Ok(mut hashers) = HASHERS.lock() {
        hashers.insert(handle, hasher);
        handle
    } else {
        -1
    }
}

/// Update a hasher with more data.
///
/// # Arguments
/// * `handle` - Hasher handle from `arth_rt_hasher_new`
/// * `input` - Pointer to input data
/// * `input_len` - Length of input data in bytes
///
/// # Returns
/// * 0 on success
/// * -1 if handle is invalid
/// * -2 if input is null (and input_len > 0)
///
/// # C ABI
/// ```c
/// int32_t arth_rt_hasher_update(int64_t handle, const uint8_t* input, size_t input_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hasher_update(handle: i64, input: *const u8, input_len: usize) -> i32 {
    // Handle null or zero-length input
    let input_slice = if input.is_null() || input_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(input, input_len) }
    };

    let mut hashers = match HASHERS.lock() {
        Ok(h) => h,
        Err(_) => return -1,
    };

    let hasher = match hashers.get_mut(&handle) {
        Some(h) => h,
        None => return -1,
    };

    match &mut hasher.state {
        HasherState::Sha256(h) => h.update(input_slice),
        HasherState::Sha384(h) => h.update(input_slice),
        HasherState::Sha512(h) => h.update(input_slice),
        HasherState::Sha3_256(h) => h.update(input_slice),
        HasherState::Sha3_512(h) => h.update(input_slice),
        HasherState::Blake3(h) => {
            h.update(input_slice);
        }
    }

    0
}

/// Finalize a hasher and get the hash result.
///
/// This consumes the hasher - the handle is no longer valid after this call.
///
/// # Arguments
/// * `handle` - Hasher handle from `arth_rt_hasher_new`
/// * `output` - Pointer to output buffer
/// * `output_len` - Size of output buffer
///
/// # Returns
/// * Number of bytes written on success
/// * -1 if handle is invalid or output buffer too small
/// * -2 if output is null
///
/// # C ABI
/// ```c
/// int64_t arth_rt_hasher_finalize(int64_t handle, uint8_t* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hasher_finalize(handle: i64, output: *mut u8, output_len: usize) -> i64 {
    if output.is_null() {
        return -2;
    }

    let hasher = {
        let mut hashers = match HASHERS.lock() {
            Ok(h) => h,
            Err(_) => return -1,
        };
        match hashers.remove(&handle) {
            Some(h) => h,
            None => return -1,
        }
    };

    let required_len = hasher.algorithm.output_size();
    if output_len < required_len {
        return -1;
    }

    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, required_len) };

    match hasher.state {
        HasherState::Sha256(h) => {
            let result = h.finalize();
            output_slice.copy_from_slice(&result);
        }
        HasherState::Sha384(h) => {
            let result = h.finalize();
            output_slice.copy_from_slice(&result);
        }
        HasherState::Sha512(h) => {
            let result = h.finalize();
            output_slice.copy_from_slice(&result);
        }
        HasherState::Sha3_256(h) => {
            let result = h.finalize();
            output_slice.copy_from_slice(&result);
        }
        HasherState::Sha3_512(h) => {
            let result = h.finalize();
            output_slice.copy_from_slice(&result);
        }
        HasherState::Blake3(h) => {
            let result = h.finalize();
            output_slice.copy_from_slice(result.as_bytes());
        }
    }

    required_len as i64
}

/// Get the algorithm of a hasher.
///
/// # Arguments
/// * `handle` - Hasher handle from `arth_rt_hasher_new`
///
/// # Returns
/// * Algorithm identifier (0-5) on success
/// * -1 if handle is invalid
///
/// # C ABI
/// ```c
/// int32_t arth_rt_hasher_algorithm(int64_t handle);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hasher_algorithm(handle: i64) -> i32 {
    let hashers = match HASHERS.lock() {
        Ok(h) => h,
        Err(_) => return -1,
    };

    match hashers.get(&handle) {
        Some(h) => h.algorithm as i32,
        None => -1,
    }
}

/// Clone a hasher to allow getting intermediate results.
///
/// # Arguments
/// * `handle` - Hasher handle from `arth_rt_hasher_new`
///
/// # Returns
/// * New handle on success
/// * -1 if handle is invalid or clone failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_hasher_clone(int64_t handle);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hasher_clone(handle: i64) -> i64 {
    let mut hashers = match HASHERS.lock() {
        Ok(h) => h,
        Err(_) => return -1,
    };

    let original = match hashers.get(&handle) {
        Some(h) => h,
        None => return -1,
    };

    let cloned_state = match &original.state {
        HasherState::Sha256(h) => HasherState::Sha256(h.clone()),
        HasherState::Sha384(h) => HasherState::Sha384(h.clone()),
        HasherState::Sha512(h) => HasherState::Sha512(h.clone()),
        HasherState::Sha3_256(h) => HasherState::Sha3_256(Box::new((**h).clone())),
        HasherState::Sha3_512(h) => HasherState::Sha3_512(Box::new((**h).clone())),
        HasherState::Blake3(h) => HasherState::Blake3(Box::new((**h).clone())),
    };

    let new_hasher = Hasher {
        algorithm: original.algorithm,
        state: cloned_state,
    };

    let new_handle = new_handle();
    hashers.insert(new_handle, new_hasher);
    new_handle
}

/// Free a hasher without getting the result.
///
/// Use this if you need to abandon a hasher before finalization.
///
/// # Arguments
/// * `handle` - Hasher handle from `arth_rt_hasher_new`
///
/// # Returns
/// * 0 on success
/// * -1 if handle is invalid
///
/// # C ABI
/// ```c
/// int32_t arth_rt_hasher_free(int64_t handle);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hasher_free(handle: i64) -> i32 {
    let mut hashers = match HASHERS.lock() {
        Ok(h) => h,
        Err(_) => return -1,
    };

    match hashers.remove(&handle) {
        Some(_) => 0,
        None => -1,
    }
}

// =============================================================================
// Hash Verification
// =============================================================================

/// Verify that a hash matches the given data using constant-time comparison.
///
/// This computes the hash of the data and compares it to the expected hash
/// in constant time to prevent timing attacks.
///
/// # Arguments
/// * `algorithm` - Hash algorithm identifier (0-5)
/// * `data` - Pointer to data to hash
/// * `data_len` - Length of data in bytes
/// * `expected_hash` - Pointer to expected hash value
/// * `expected_len` - Length of expected hash (must match algorithm output size)
///
/// # Returns
/// * 1 if hashes match
/// * 0 if hashes don't match
/// * -1 if expected_len doesn't match algorithm output size
/// * -2 if pointers are null
/// * -3 if algorithm is invalid
///
/// # C ABI
/// ```c
/// int32_t arth_rt_hash_verify(int32_t algorithm, const uint8_t* data, size_t data_len,
///                              const uint8_t* expected_hash, size_t expected_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hash_verify(
    algorithm: i32,
    data: *const u8,
    data_len: usize,
    expected_hash: *const u8,
    expected_len: usize,
) -> i32 {
    if expected_hash.is_null() {
        return -2;
    }

    let algo = match HashAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -3,
    };

    let output_size = algo.output_size();
    if expected_len != output_size {
        return -1;
    }

    // Allocate buffer for computed hash
    let mut computed = vec![0u8; output_size];

    // Compute hash
    let result = arth_rt_hash(
        algorithm,
        data,
        data_len,
        computed.as_mut_ptr(),
        computed.len(),
    );
    if result < 0 {
        return result as i32;
    }

    // Constant-time comparison
    arth_rt_secure_compare(computed.as_ptr(), expected_hash, output_size)
}

/// Compare two hashes for equality using constant-time comparison.
///
/// Both hashes must have the same length. The comparison takes the same
/// amount of time regardless of where the hashes differ.
///
/// # Arguments
/// * `hash_a` - Pointer to first hash
/// * `hash_b` - Pointer to second hash
/// * `len` - Length of both hashes
///
/// # Returns
/// * 1 if hashes are equal
/// * 0 if hashes are not equal or inputs are null
///
/// # C ABI
/// ```c
/// int32_t arth_rt_hash_equals(const uint8_t* hash_a, const uint8_t* hash_b, size_t len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hash_equals(hash_a: *const u8, hash_b: *const u8, len: usize) -> i32 {
    arth_rt_secure_compare(hash_a, hash_b, len)
}

// =============================================================================
// HMAC (Keyed-Hash Message Authentication Code)
// =============================================================================
//
// HMAC provides message authentication using cryptographic hash functions.
// Supports: HMAC-SHA256, HMAC-SHA384, HMAC-SHA512, HMAC-SHA3-256, HMAC-SHA3-512
//
// Note: BLAKE3 has its own keyed hashing mode and is not typically used with HMAC.

use hmac::{Hmac, Mac};

// Type aliases for HMAC variants
type HmacSha256 = Hmac<sha2::Sha256>;
type HmacSha384 = Hmac<sha2::Sha384>;
type HmacSha512 = Hmac<sha2::Sha512>;
type HmacSha3_256 = Hmac<sha3::Sha3_256>;
type HmacSha3_512 = Hmac<sha3::Sha3_512>;

/// Compute a one-shot HMAC of the input data.
///
/// # Arguments
/// * `algorithm` - Hash algorithm identifier (0-4, BLAKE3 not supported for HMAC)
/// * `key` - Pointer to the secret key
/// * `key_len` - Length of the secret key in bytes
/// * `input` - Pointer to input data
/// * `input_len` - Length of input data in bytes
/// * `output` - Pointer to output buffer (must be at least `arth_rt_hash_output_size` bytes)
/// * `output_len` - Size of output buffer
///
/// # Returns
/// * Number of bytes written on success
/// * -1 if output buffer is too small
/// * -2 if pointers are null
/// * -3 if algorithm is invalid or unsupported for HMAC
/// * -4 if key is invalid
///
/// # C ABI
/// ```c
/// int64_t arth_rt_hmac(int32_t algorithm, const uint8_t* key, size_t key_len,
///                      const uint8_t* input, size_t input_len,
///                      uint8_t* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hmac(
    algorithm: i32,
    key: *const u8,
    key_len: usize,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if key.is_null() || output.is_null() {
        return -2;
    }

    let algo = match HashAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -3,
    };

    // BLAKE3 uses its own keyed mode, not HMAC
    if algo == HashAlgorithm::Blake3 {
        return -3;
    }

    let required_len = algo.output_size();
    if output_len < required_len {
        return -1;
    }

    let key_slice = if key_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(key, key_len) }
    };

    let input_slice = if input.is_null() || input_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(input, input_len) }
    };

    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, required_len) };

    // Compute HMAC based on algorithm (use explicit trait qualification to avoid ambiguity)
    match algo {
        HashAlgorithm::Sha256 => {
            let mut mac = <HmacSha256 as Mac>::new_from_slice(key_slice).unwrap();
            Mac::update(&mut mac, input_slice);
            let result = mac.finalize();
            output_slice.copy_from_slice(&result.into_bytes());
        }
        HashAlgorithm::Sha384 => {
            let mut mac = <HmacSha384 as Mac>::new_from_slice(key_slice).unwrap();
            Mac::update(&mut mac, input_slice);
            let result = mac.finalize();
            output_slice.copy_from_slice(&result.into_bytes());
        }
        HashAlgorithm::Sha512 => {
            let mut mac = <HmacSha512 as Mac>::new_from_slice(key_slice).unwrap();
            Mac::update(&mut mac, input_slice);
            let result = mac.finalize();
            output_slice.copy_from_slice(&result.into_bytes());
        }
        HashAlgorithm::Sha3_256 => {
            let mut mac = <HmacSha3_256 as Mac>::new_from_slice(key_slice).unwrap();
            Mac::update(&mut mac, input_slice);
            let result = mac.finalize();
            output_slice.copy_from_slice(&result.into_bytes());
        }
        HashAlgorithm::Sha3_512 => {
            let mut mac = <HmacSha3_512 as Mac>::new_from_slice(key_slice).unwrap();
            Mac::update(&mut mac, input_slice);
            let result = mac.finalize();
            output_slice.copy_from_slice(&result.into_bytes());
        }
        HashAlgorithm::Blake3 => {
            // Already checked above, but included for completeness
            return -3;
        }
    }

    required_len as i64
}

// =============================================================================
// Incremental HMAC
// =============================================================================

/// Enum to hold different HMAC state types.
/// Large states are boxed to keep enum size reasonable.
enum HmacStateInner {
    Sha256(HmacSha256),
    Sha384(HmacSha384),
    Sha512(HmacSha512),
    Sha3_256(Box<HmacSha3_256>),
    Sha3_512(Box<HmacSha3_512>),
}

/// HMAC state wrapper with algorithm tag.
struct HmacState {
    algorithm: HashAlgorithm,
    state: HmacStateInner,
}

lazy_static! {
    /// Global registry of active HMAC states
    static ref HMAC_STATES: Mutex<HashMap<i64, HmacState>> = Mutex::new(HashMap::new());
}

/// Create a new incremental HMAC state.
///
/// # Arguments
/// * `algorithm` - Hash algorithm identifier (0-4, BLAKE3 not supported)
/// * `key` - Pointer to the secret key
/// * `key_len` - Length of the secret key in bytes
///
/// # Returns
/// * Handle to the HMAC state on success (positive)
/// * -1 if failed to allocate
/// * -2 if key is null
/// * -3 if algorithm is invalid or unsupported
/// * -4 if key is invalid
///
/// # C ABI
/// ```c
/// int64_t arth_rt_hmac_new(int32_t algorithm, const uint8_t* key, size_t key_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hmac_new(algorithm: i32, key: *const u8, key_len: usize) -> i64 {
    if key.is_null() && key_len > 0 {
        return -2;
    }

    let algo = match HashAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -3,
    };

    // BLAKE3 uses its own keyed mode, not HMAC
    if algo == HashAlgorithm::Blake3 {
        return -3;
    }

    let key_slice = if key.is_null() || key_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(key, key_len) }
    };

    let state = match algo {
        HashAlgorithm::Sha256 => match <HmacSha256 as Mac>::new_from_slice(key_slice) {
            Ok(mac) => HmacStateInner::Sha256(mac),
            Err(_) => return -4,
        },
        HashAlgorithm::Sha384 => match <HmacSha384 as Mac>::new_from_slice(key_slice) {
            Ok(mac) => HmacStateInner::Sha384(mac),
            Err(_) => return -4,
        },
        HashAlgorithm::Sha512 => match <HmacSha512 as Mac>::new_from_slice(key_slice) {
            Ok(mac) => HmacStateInner::Sha512(mac),
            Err(_) => return -4,
        },
        HashAlgorithm::Sha3_256 => match <HmacSha3_256 as Mac>::new_from_slice(key_slice) {
            Ok(mac) => HmacStateInner::Sha3_256(Box::new(mac)),
            Err(_) => return -4,
        },
        HashAlgorithm::Sha3_512 => match <HmacSha3_512 as Mac>::new_from_slice(key_slice) {
            Ok(mac) => HmacStateInner::Sha3_512(Box::new(mac)),
            Err(_) => return -4,
        },
        HashAlgorithm::Blake3 => return -3,
    };

    let hmac_state = HmacState {
        algorithm: algo,
        state,
    };

    let handle = new_handle();
    if let Ok(mut states) = HMAC_STATES.lock() {
        states.insert(handle, hmac_state);
        handle
    } else {
        -1
    }
}

/// Update an HMAC state with more data.
///
/// # Arguments
/// * `handle` - HMAC state handle from `arth_rt_hmac_new`
/// * `input` - Pointer to input data
/// * `input_len` - Length of input data in bytes
///
/// # Returns
/// * 0 on success
/// * -1 if handle is invalid
///
/// # C ABI
/// ```c
/// int32_t arth_rt_hmac_update(int64_t handle, const uint8_t* input, size_t input_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hmac_update(handle: i64, input: *const u8, input_len: usize) -> i32 {
    let input_slice = if input.is_null() || input_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(input, input_len) }
    };

    let mut states = match HMAC_STATES.lock() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let hmac_state = match states.get_mut(&handle) {
        Some(s) => s,
        None => return -1,
    };

    match &mut hmac_state.state {
        HmacStateInner::Sha256(mac) => mac.update(input_slice),
        HmacStateInner::Sha384(mac) => mac.update(input_slice),
        HmacStateInner::Sha512(mac) => mac.update(input_slice),
        HmacStateInner::Sha3_256(mac) => mac.update(input_slice),
        HmacStateInner::Sha3_512(mac) => mac.update(input_slice),
    }

    0
}

/// Finalize an HMAC state and get the result.
///
/// This consumes the HMAC state - the handle is no longer valid after this call.
///
/// # Arguments
/// * `handle` - HMAC state handle from `arth_rt_hmac_new`
/// * `output` - Pointer to output buffer
/// * `output_len` - Size of output buffer
///
/// # Returns
/// * Number of bytes written on success
/// * -1 if handle is invalid or output buffer too small
/// * -2 if output is null
///
/// # C ABI
/// ```c
/// int64_t arth_rt_hmac_finalize(int64_t handle, uint8_t* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hmac_finalize(handle: i64, output: *mut u8, output_len: usize) -> i64 {
    if output.is_null() {
        return -2;
    }

    let hmac_state = {
        let mut states = match HMAC_STATES.lock() {
            Ok(s) => s,
            Err(_) => return -1,
        };
        match states.remove(&handle) {
            Some(s) => s,
            None => return -1,
        }
    };

    let required_len = hmac_state.algorithm.output_size();
    if output_len < required_len {
        return -1;
    }

    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, required_len) };

    match hmac_state.state {
        HmacStateInner::Sha256(mac) => {
            let result = mac.finalize();
            output_slice.copy_from_slice(&result.into_bytes());
        }
        HmacStateInner::Sha384(mac) => {
            let result = mac.finalize();
            output_slice.copy_from_slice(&result.into_bytes());
        }
        HmacStateInner::Sha512(mac) => {
            let result = mac.finalize();
            output_slice.copy_from_slice(&result.into_bytes());
        }
        HmacStateInner::Sha3_256(mac) => {
            let result = mac.finalize();
            output_slice.copy_from_slice(&result.into_bytes());
        }
        HmacStateInner::Sha3_512(mac) => {
            let result = mac.finalize();
            output_slice.copy_from_slice(&result.into_bytes());
        }
    }

    required_len as i64
}

/// Free an HMAC state without getting the result.
///
/// # Arguments
/// * `handle` - HMAC state handle from `arth_rt_hmac_new`
///
/// # Returns
/// * 0 on success
/// * -1 if handle is invalid
///
/// # C ABI
/// ```c
/// int32_t arth_rt_hmac_free(int64_t handle);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hmac_free(handle: i64) -> i32 {
    let mut states = match HMAC_STATES.lock() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    match states.remove(&handle) {
        Some(_) => 0,
        None => -1,
    }
}

// =============================================================================
// HMAC Verification
// =============================================================================

/// Verify that an HMAC matches the given data using constant-time comparison.
///
/// This computes the HMAC of the data and compares it to the expected HMAC
/// in constant time to prevent timing attacks.
///
/// # Arguments
/// * `algorithm` - Hash algorithm identifier (0-4, BLAKE3 not supported)
/// * `key` - Pointer to the secret key
/// * `key_len` - Length of the secret key in bytes
/// * `data` - Pointer to data
/// * `data_len` - Length of data in bytes
/// * `expected_hmac` - Pointer to expected HMAC value
/// * `expected_len` - Length of expected HMAC
///
/// # Returns
/// * 1 if HMACs match
/// * 0 if HMACs don't match
/// * -1 if expected_len doesn't match algorithm output size
/// * -2 if pointers are null
/// * -3 if algorithm is invalid
/// * -4 if key is invalid
///
/// # C ABI
/// ```c
/// int32_t arth_rt_hmac_verify(int32_t algorithm, const uint8_t* key, size_t key_len,
///                              const uint8_t* data, size_t data_len,
///                              const uint8_t* expected_hmac, size_t expected_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hmac_verify(
    algorithm: i32,
    key: *const u8,
    key_len: usize,
    data: *const u8,
    data_len: usize,
    expected_hmac: *const u8,
    expected_len: usize,
) -> i32 {
    if key.is_null() || expected_hmac.is_null() {
        return -2;
    }

    let algo = match HashAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -3,
    };

    if algo == HashAlgorithm::Blake3 {
        return -3;
    }

    let output_size = algo.output_size();
    if expected_len != output_size {
        return -1;
    }

    // Allocate buffer for computed HMAC
    let mut computed = vec![0u8; output_size];

    // Compute HMAC
    let result = arth_rt_hmac(
        algorithm,
        key,
        key_len,
        data,
        data_len,
        computed.as_mut_ptr(),
        computed.len(),
    );
    if result < 0 {
        return result as i32;
    }

    // Constant-time comparison
    arth_rt_secure_compare(computed.as_ptr(), expected_hmac, output_size)
}

/// Compare two HMACs for equality using constant-time comparison.
///
/// # Arguments
/// * `hmac_a` - Pointer to first HMAC
/// * `hmac_b` - Pointer to second HMAC
/// * `len` - Length of both HMACs
///
/// # Returns
/// * 1 if HMACs are equal
/// * 0 if HMACs are not equal or inputs are null
///
/// # C ABI
/// ```c
/// int32_t arth_rt_hmac_equals(const uint8_t* hmac_a, const uint8_t* hmac_b, size_t len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hmac_equals(hmac_a: *const u8, hmac_b: *const u8, len: usize) -> i32 {
    arth_rt_secure_compare(hmac_a, hmac_b, len)
}

// =============================================================================
// Cryptographically Secure Random Number Generation
// =============================================================================
//
// This section provides CSPRNG operations using platform-native sources:
// - Linux: getrandom() syscall
// - macOS: SecRandomCopyBytes
// - Windows: BCryptGenRandom
//
// All random generation is backed by the `getrandom` crate which handles
// platform-specific details and provides a consistent interface.

/// Fill a buffer with cryptographically secure random bytes.
///
/// # Arguments
/// * `output` - Pointer to output buffer
/// * `len` - Number of bytes to generate
///
/// # Returns
/// * Number of bytes written on success (equals `len`)
/// * -1 if output is null
/// * -2 if random generation failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_random_bytes(uint8_t* output, size_t len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_random_bytes(output: *mut u8, len: usize) -> i64 {
    if output.is_null() {
        return -1;
    }

    if len == 0 {
        return 0;
    }

    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, len) };

    match getrandom::getrandom(output_slice) {
        Ok(()) => len as i64,
        Err(_) => -2,
    }
}

/// Fill a buffer with cryptographically secure random bytes (in-place).
///
/// Same as `arth_rt_random_bytes` but returns 0 on success for convenience.
///
/// # Arguments
/// * `buffer` - Pointer to buffer to fill
/// * `len` - Length of buffer
///
/// # Returns
/// * 0 on success
/// * -1 if buffer is null
/// * -2 if random generation failed
///
/// # C ABI
/// ```c
/// int32_t arth_rt_random_fill(uint8_t* buffer, size_t len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_random_fill(buffer: *mut u8, len: usize) -> i32 {
    if buffer.is_null() && len > 0 {
        return -1;
    }

    if len == 0 {
        return 0;
    }

    let buffer_slice = unsafe { std::slice::from_raw_parts_mut(buffer, len) };

    match getrandom::getrandom(buffer_slice) {
        Ok(()) => 0,
        Err(_) => -2,
    }
}

/// Generate a random 64-bit unsigned integer in the range [0, max).
///
/// Uses rejection sampling to avoid modulo bias.
///
/// # Arguments
/// * `max` - Exclusive upper bound (must be > 0)
///
/// # Returns
/// * Random value in [0, max) on success
/// * -1 if max is 0
/// * -2 if random generation failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_random_u64_below(uint64_t max);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_random_u64_below(max: u64) -> i64 {
    if max == 0 {
        return -1;
    }

    // For powers of 2, we can just mask
    if max.is_power_of_two() {
        let mut buf = [0u8; 8];
        if getrandom::getrandom(&mut buf).is_err() {
            return -2;
        }
        let value = u64::from_le_bytes(buf);
        return (value & (max - 1)) as i64;
    }

    // Rejection sampling to avoid modulo bias
    // We need to find the largest multiple of max that fits in u64
    let threshold = u64::MAX - (u64::MAX % max);

    loop {
        let mut buf = [0u8; 8];
        if getrandom::getrandom(&mut buf).is_err() {
            return -2;
        }
        let value = u64::from_le_bytes(buf);

        // Reject values that would cause modulo bias
        if value < threshold {
            return (value % max) as i64;
        }
    }
}

/// Generate a random signed integer in the range [min, max).
///
/// Uses rejection sampling to avoid modulo bias.
///
/// # Arguments
/// * `min` - Inclusive lower bound
/// * `max` - Exclusive upper bound (must be > min)
///
/// # Returns
/// * Random value in [min, max) on success
/// * i64::MIN if min >= max (error)
/// * i64::MIN - 1 if random generation failed (in practice, just i64::MIN due to overflow)
///
/// # C ABI
/// ```c
/// int64_t arth_rt_random_int_range(int64_t min, int64_t max);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_random_int_range(min: i64, max: i64) -> i64 {
    if min >= max {
        return i64::MIN; // Error: invalid range
    }

    // Calculate the range size
    let range = (max as u64).wrapping_sub(min as u64);

    let random_offset = arth_rt_random_u64_below(range);
    if random_offset < 0 {
        return i64::MIN; // Random generation failed
    }

    min.wrapping_add(random_offset)
}

/// Generate a random UUID v4 (RFC 4122).
///
/// The UUID is written as 36 ASCII characters (8-4-4-4-12 format)
/// followed by a null terminator.
///
/// # Arguments
/// * `output` - Pointer to output buffer (must be at least 37 bytes)
/// * `output_len` - Size of output buffer
///
/// # Returns
/// * 36 on success (number of characters written, excluding null terminator)
/// * -1 if output is null or buffer too small
/// * -2 if random generation failed
///
/// # C ABI
/// ```c
/// int32_t arth_rt_random_uuid(char* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_random_uuid(output: *mut u8, output_len: usize) -> i32 {
    const UUID_STRING_LEN: usize = 36;

    if output.is_null() || output_len < UUID_STRING_LEN + 1 {
        return -1;
    }

    // Generate 16 random bytes
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        return -2;
    }

    // Set version to 4 (random UUID)
    bytes[6] = (bytes[6] & 0x0f) | 0x40;

    // Set variant to RFC 4122
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    // Format as UUID string: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, UUID_STRING_LEN + 1) };

    let mut pos = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        output_slice[pos] = hex_chars[(byte >> 4) as usize];
        pos += 1;
        output_slice[pos] = hex_chars[(byte & 0x0f) as usize];
        pos += 1;

        // Insert hyphens after bytes 4, 6, 8, 10
        if i == 3 || i == 5 || i == 7 || i == 9 {
            output_slice[pos] = b'-';
            pos += 1;
        }
    }

    output_slice[UUID_STRING_LEN] = 0; // Null terminator

    UUID_STRING_LEN as i32
}

/// Generate a random 32-bit unsigned integer.
///
/// # Returns
/// * Random u32 value on success (returned as i64 for error space)
/// * -1 if random generation failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_random_u32();
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_random_u32() -> i64 {
    let mut buf = [0u8; 4];
    if getrandom::getrandom(&mut buf).is_err() {
        return -1;
    }
    u32::from_le_bytes(buf) as i64
}

/// Generate a random 64-bit unsigned integer.
///
/// # Returns
/// * Random u64 value on success
/// * On error, returns a negative value (implementation limitation)
///
/// Note: Since the full u64 range overlaps with error codes, use
/// `arth_rt_random_bytes` for guaranteed error detection.
///
/// # C ABI
/// ```c
/// uint64_t arth_rt_random_u64();
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_random_u64() -> u64 {
    let mut buf = [0u8; 8];
    if getrandom::getrandom(&mut buf).is_err() {
        return 0; // Can't distinguish from valid 0, use random_bytes for error detection
    }
    u64::from_le_bytes(buf)
}

/// Check if the CSPRNG is available and working.
///
/// This attempts to generate a small amount of random data to verify
/// the system's random number generator is functioning.
///
/// # Returns
/// * 1 if CSPRNG is available and working
/// * 0 if CSPRNG is not available or failed
///
/// # C ABI
/// ```c
/// int32_t arth_rt_random_available();
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_random_available() -> i32 {
    let mut buf = [0u8; 1];
    match getrandom::getrandom(&mut buf) {
        Ok(()) => 1,
        Err(_) => 0,
    }
}

// =============================================================================
// AEAD (Authenticated Encryption with Associated Data)
// =============================================================================
//
// This section provides AEAD encryption using:
// - AES-128-GCM (128-bit key, 96-bit nonce, 128-bit tag)
// - AES-256-GCM (256-bit key, 96-bit nonce, 128-bit tag)
// - ChaCha20-Poly1305 (256-bit key, 96-bit nonce, 128-bit tag)
//
// All algorithms use the AEAD construction which provides:
// - Confidentiality: The plaintext is encrypted
// - Integrity: Any modification to the ciphertext is detected
// - Authenticity: The ciphertext was created by someone with the key

use aes_gcm::{
    Aes128Gcm, Aes256Gcm, Nonce as AesGcmNonce,
    aead::{Aead as AeadTrait, KeyInit as AeadKeyInit},
};
use chacha20poly1305::{
    ChaCha20Poly1305, Nonce as ChaChaPolyNonce, aead::KeyInit as ChaChaKeyInit,
};

/// AEAD algorithm identifiers matching Arth's `AeadAlgorithm` enum.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AeadAlgorithm {
    Aes128Gcm = 0,
    Aes256Gcm = 1,
    ChaCha20Poly1305 = 2,
}

impl AeadAlgorithm {
    /// Returns the key size in bytes for this algorithm.
    pub fn key_size(self) -> usize {
        match self {
            AeadAlgorithm::Aes128Gcm => 16,
            AeadAlgorithm::Aes256Gcm => 32,
            AeadAlgorithm::ChaCha20Poly1305 => 32,
        }
    }

    /// Returns the nonce size in bytes for this algorithm.
    pub fn nonce_size(self) -> usize {
        match self {
            AeadAlgorithm::Aes128Gcm => 12,
            AeadAlgorithm::Aes256Gcm => 12,
            AeadAlgorithm::ChaCha20Poly1305 => 12,
        }
    }

    /// Returns the authentication tag size in bytes.
    pub fn tag_size(self) -> usize {
        // All supported algorithms use 128-bit (16 byte) tags
        16
    }

    /// Try to convert from an i32 value.
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(AeadAlgorithm::Aes128Gcm),
            1 => Some(AeadAlgorithm::Aes256Gcm),
            2 => Some(AeadAlgorithm::ChaCha20Poly1305),
            _ => None,
        }
    }
}

/// Get the key size for an AEAD algorithm.
///
/// # Arguments
/// * `algorithm` - AEAD algorithm identifier (0-2)
///
/// # Returns
/// * Key size in bytes, or 0 if algorithm is invalid
///
/// # C ABI
/// ```c
/// size_t arth_rt_aead_key_size(int32_t algorithm);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_key_size(algorithm: i32) -> usize {
    match AeadAlgorithm::from_i32(algorithm) {
        Some(algo) => algo.key_size(),
        None => 0,
    }
}

/// Get the nonce size for an AEAD algorithm.
///
/// # Arguments
/// * `algorithm` - AEAD algorithm identifier (0-2)
///
/// # Returns
/// * Nonce size in bytes, or 0 if algorithm is invalid
///
/// # C ABI
/// ```c
/// size_t arth_rt_aead_nonce_size(int32_t algorithm);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_nonce_size(algorithm: i32) -> usize {
    match AeadAlgorithm::from_i32(algorithm) {
        Some(algo) => algo.nonce_size(),
        None => 0,
    }
}

/// Get the authentication tag size for an AEAD algorithm.
///
/// # Arguments
/// * `algorithm` - AEAD algorithm identifier (0-2)
///
/// # Returns
/// * Tag size in bytes, or 0 if algorithm is invalid
///
/// # C ABI
/// ```c
/// size_t arth_rt_aead_tag_size(int32_t algorithm);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_tag_size(algorithm: i32) -> usize {
    match AeadAlgorithm::from_i32(algorithm) {
        Some(algo) => algo.tag_size(),
        None => 0,
    }
}

/// Generate a random nonce for AEAD encryption.
///
/// The nonce is filled with cryptographically secure random bytes.
/// IMPORTANT: Never reuse a nonce with the same key!
///
/// # Arguments
/// * `algorithm` - AEAD algorithm identifier (0-2)
/// * `nonce` - Pointer to nonce buffer
/// * `nonce_len` - Length of nonce buffer (must match algorithm's nonce size)
///
/// # Returns
/// * 0 on success
/// * -1 if nonce buffer is wrong size
/// * -2 if nonce is null
/// * -3 if algorithm is invalid
/// * -4 if random generation failed
///
/// # C ABI
/// ```c
/// int32_t arth_rt_aead_generate_nonce(int32_t algorithm, uint8_t* nonce, size_t nonce_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_generate_nonce(
    algorithm: i32,
    nonce: *mut u8,
    nonce_len: usize,
) -> i32 {
    if nonce.is_null() {
        return -2;
    }

    let algo = match AeadAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -3,
    };

    let expected_size = algo.nonce_size();
    if nonce_len != expected_size {
        return -1;
    }

    let nonce_slice = unsafe { std::slice::from_raw_parts_mut(nonce, nonce_len) };

    match getrandom::getrandom(nonce_slice) {
        Ok(()) => 0,
        Err(_) => -4,
    }
}

/// Encrypt data using AEAD.
///
/// The output consists of ciphertext || tag (tag is appended).
///
/// # Arguments
/// * `algorithm` - AEAD algorithm identifier (0-2)
/// * `key` - Pointer to encryption key
/// * `key_len` - Length of key (must match algorithm's key size)
/// * `nonce` - Pointer to nonce (must be unique for each encryption with same key)
/// * `nonce_len` - Length of nonce (must match algorithm's nonce size)
/// * `plaintext` - Pointer to plaintext data
/// * `plaintext_len` - Length of plaintext
/// * `aad` - Pointer to additional authenticated data (can be null)
/// * `aad_len` - Length of AAD (can be 0)
/// * `output` - Pointer to output buffer (must be at least plaintext_len + tag_size)
/// * `output_len` - Length of output buffer
///
/// # Returns
/// * Number of bytes written (plaintext_len + tag_size) on success
/// * -1 if output buffer too small
/// * -2 if null pointer provided for required parameter
/// * -3 if algorithm is invalid
/// * -4 if key size is wrong
/// * -5 if nonce size is wrong
/// * -6 if encryption failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_aead_encrypt(int32_t algorithm,
///                               const uint8_t* key, size_t key_len,
///                               const uint8_t* nonce, size_t nonce_len,
///                               const uint8_t* plaintext, size_t plaintext_len,
///                               const uint8_t* aad, size_t aad_len,
///                               uint8_t* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_encrypt(
    algorithm: i32,
    key: *const u8,
    key_len: usize,
    nonce: *const u8,
    nonce_len: usize,
    plaintext: *const u8,
    plaintext_len: usize,
    aad: *const u8,
    aad_len: usize,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if key.is_null() || nonce.is_null() || output.is_null() {
        return -2;
    }

    let algo = match AeadAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -3,
    };

    if key_len != algo.key_size() {
        return -4;
    }

    if nonce_len != algo.nonce_size() {
        return -5;
    }

    let required_output_len = plaintext_len + algo.tag_size();
    if output_len < required_output_len {
        return -1;
    }

    let key_slice = unsafe { std::slice::from_raw_parts(key, key_len) };
    let nonce_slice = unsafe { std::slice::from_raw_parts(nonce, nonce_len) };

    let plaintext_slice = if plaintext.is_null() || plaintext_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(plaintext, plaintext_len) }
    };

    let aad_slice = if aad.is_null() || aad_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(aad, aad_len) }
    };

    // Encrypt based on algorithm
    let ciphertext = match algo {
        AeadAlgorithm::Aes128Gcm => {
            let cipher = match <Aes128Gcm as AeadKeyInit>::new_from_slice(key_slice) {
                Ok(c) => c,
                Err(_) => return -4,
            };
            let nonce = AesGcmNonce::from_slice(nonce_slice);

            if aad_slice.is_empty() {
                match AeadTrait::encrypt(&cipher, nonce, plaintext_slice) {
                    Ok(ct) => ct,
                    Err(_) => return -6,
                }
            } else {
                use aes_gcm::aead::Payload;
                let payload = Payload {
                    msg: plaintext_slice,
                    aad: aad_slice,
                };
                match AeadTrait::encrypt(&cipher, nonce, payload) {
                    Ok(ct) => ct,
                    Err(_) => return -6,
                }
            }
        }
        AeadAlgorithm::Aes256Gcm => {
            let cipher = match <Aes256Gcm as AeadKeyInit>::new_from_slice(key_slice) {
                Ok(c) => c,
                Err(_) => return -4,
            };
            let nonce = AesGcmNonce::from_slice(nonce_slice);

            if aad_slice.is_empty() {
                match AeadTrait::encrypt(&cipher, nonce, plaintext_slice) {
                    Ok(ct) => ct,
                    Err(_) => return -6,
                }
            } else {
                use aes_gcm::aead::Payload;
                let payload = Payload {
                    msg: plaintext_slice,
                    aad: aad_slice,
                };
                match AeadTrait::encrypt(&cipher, nonce, payload) {
                    Ok(ct) => ct,
                    Err(_) => return -6,
                }
            }
        }
        AeadAlgorithm::ChaCha20Poly1305 => {
            let cipher = match <ChaCha20Poly1305 as ChaChaKeyInit>::new_from_slice(key_slice) {
                Ok(c) => c,
                Err(_) => return -4,
            };
            let nonce = ChaChaPolyNonce::from_slice(nonce_slice);

            if aad_slice.is_empty() {
                match chacha20poly1305::aead::Aead::encrypt(&cipher, nonce, plaintext_slice) {
                    Ok(ct) => ct,
                    Err(_) => return -6,
                }
            } else {
                use chacha20poly1305::aead::Payload;
                let payload = Payload {
                    msg: plaintext_slice,
                    aad: aad_slice,
                };
                match chacha20poly1305::aead::Aead::encrypt(&cipher, nonce, payload) {
                    Ok(ct) => ct,
                    Err(_) => return -6,
                }
            }
        }
    };

    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, ciphertext.len()) };
    output_slice.copy_from_slice(&ciphertext);

    ciphertext.len() as i64
}

/// Decrypt data using AEAD.
///
/// The input is expected to be ciphertext || tag.
///
/// # Arguments
/// * `algorithm` - AEAD algorithm identifier (0-2)
/// * `key` - Pointer to decryption key
/// * `key_len` - Length of key
/// * `nonce` - Pointer to nonce (same as used for encryption)
/// * `nonce_len` - Length of nonce
/// * `ciphertext` - Pointer to ciphertext + tag
/// * `ciphertext_len` - Length of ciphertext including tag
/// * `aad` - Pointer to additional authenticated data (must match encryption)
/// * `aad_len` - Length of AAD
/// * `output` - Pointer to output buffer for plaintext
/// * `output_len` - Length of output buffer (must be at least ciphertext_len - tag_size)
///
/// # Returns
/// * Number of plaintext bytes written on success
/// * -1 if output buffer too small
/// * -2 if null pointer provided for required parameter
/// * -3 if algorithm is invalid
/// * -4 if key size is wrong
/// * -5 if nonce size is wrong
/// * -6 if ciphertext too short (no tag)
/// * -7 if decryption/authentication failed (wrong key, tampered, wrong AAD)
///
/// # C ABI
/// ```c
/// int64_t arth_rt_aead_decrypt(int32_t algorithm,
///                               const uint8_t* key, size_t key_len,
///                               const uint8_t* nonce, size_t nonce_len,
///                               const uint8_t* ciphertext, size_t ciphertext_len,
///                               const uint8_t* aad, size_t aad_len,
///                               uint8_t* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_decrypt(
    algorithm: i32,
    key: *const u8,
    key_len: usize,
    nonce: *const u8,
    nonce_len: usize,
    ciphertext: *const u8,
    ciphertext_len: usize,
    aad: *const u8,
    aad_len: usize,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if key.is_null() || nonce.is_null() || output.is_null() {
        return -2;
    }

    let algo = match AeadAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -3,
    };

    if key_len != algo.key_size() {
        return -4;
    }

    if nonce_len != algo.nonce_size() {
        return -5;
    }

    let tag_size = algo.tag_size();
    if ciphertext_len < tag_size {
        return -6;
    }

    let plaintext_len = ciphertext_len - tag_size;
    if output_len < plaintext_len {
        return -1;
    }

    let key_slice = unsafe { std::slice::from_raw_parts(key, key_len) };
    let nonce_slice = unsafe { std::slice::from_raw_parts(nonce, nonce_len) };

    let ciphertext_slice = if ciphertext.is_null() || ciphertext_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(ciphertext, ciphertext_len) }
    };

    let aad_slice = if aad.is_null() || aad_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(aad, aad_len) }
    };

    // Decrypt based on algorithm
    let plaintext = match algo {
        AeadAlgorithm::Aes128Gcm => {
            let cipher = match <Aes128Gcm as AeadKeyInit>::new_from_slice(key_slice) {
                Ok(c) => c,
                Err(_) => return -4,
            };
            let nonce = AesGcmNonce::from_slice(nonce_slice);

            if aad_slice.is_empty() {
                match AeadTrait::decrypt(&cipher, nonce, ciphertext_slice) {
                    Ok(pt) => pt,
                    Err(_) => return -7,
                }
            } else {
                use aes_gcm::aead::Payload;
                let payload = Payload {
                    msg: ciphertext_slice,
                    aad: aad_slice,
                };
                match AeadTrait::decrypt(&cipher, nonce, payload) {
                    Ok(pt) => pt,
                    Err(_) => return -7,
                }
            }
        }
        AeadAlgorithm::Aes256Gcm => {
            let cipher = match <Aes256Gcm as AeadKeyInit>::new_from_slice(key_slice) {
                Ok(c) => c,
                Err(_) => return -4,
            };
            let nonce = AesGcmNonce::from_slice(nonce_slice);

            if aad_slice.is_empty() {
                match AeadTrait::decrypt(&cipher, nonce, ciphertext_slice) {
                    Ok(pt) => pt,
                    Err(_) => return -7,
                }
            } else {
                use aes_gcm::aead::Payload;
                let payload = Payload {
                    msg: ciphertext_slice,
                    aad: aad_slice,
                };
                match AeadTrait::decrypt(&cipher, nonce, payload) {
                    Ok(pt) => pt,
                    Err(_) => return -7,
                }
            }
        }
        AeadAlgorithm::ChaCha20Poly1305 => {
            let cipher = match <ChaCha20Poly1305 as ChaChaKeyInit>::new_from_slice(key_slice) {
                Ok(c) => c,
                Err(_) => return -4,
            };
            let nonce = ChaChaPolyNonce::from_slice(nonce_slice);

            if aad_slice.is_empty() {
                match chacha20poly1305::aead::Aead::decrypt(&cipher, nonce, ciphertext_slice) {
                    Ok(pt) => pt,
                    Err(_) => return -7,
                }
            } else {
                use chacha20poly1305::aead::Payload;
                let payload = Payload {
                    msg: ciphertext_slice,
                    aad: aad_slice,
                };
                match chacha20poly1305::aead::Aead::decrypt(&cipher, nonce, payload) {
                    Ok(pt) => pt,
                    Err(_) => return -7,
                }
            }
        }
    };

    if !plaintext.is_empty() {
        let output_slice = unsafe { std::slice::from_raw_parts_mut(output, plaintext.len()) };
        output_slice.copy_from_slice(&plaintext);
    }

    plaintext.len() as i64
}

/// Generate a random key for an AEAD algorithm.
///
/// # Arguments
/// * `algorithm` - AEAD algorithm identifier (0-2)
/// * `key` - Pointer to key buffer
/// * `key_len` - Length of key buffer (must match algorithm's key size)
///
/// # Returns
/// * 0 on success
/// * -1 if key buffer is wrong size
/// * -2 if key is null
/// * -3 if algorithm is invalid
/// * -4 if random generation failed
///
/// # C ABI
/// ```c
/// int32_t arth_rt_aead_generate_key(int32_t algorithm, uint8_t* key, size_t key_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_generate_key(algorithm: i32, key: *mut u8, key_len: usize) -> i32 {
    if key.is_null() {
        return -2;
    }

    let algo = match AeadAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -3,
    };

    let expected_size = algo.key_size();
    if key_len != expected_size {
        return -1;
    }

    let key_slice = unsafe { std::slice::from_raw_parts_mut(key, key_len) };

    match getrandom::getrandom(key_slice) {
        Ok(()) => 0,
        Err(_) => -4,
    }
}

// =============================================================================
// Key Generation (Digital Signatures and Key Exchange)
// =============================================================================
//
// This section provides key pair generation for:
// - Ed25519 (digital signatures)
// - ECDSA P-256/P-384 (digital signatures)
// - X25519 (key exchange)
// - ECDH P-256/P-384 (key exchange)
//
// Key sizes:
// - Ed25519: 32-byte seed → 32-byte public key, 64-byte expanded secret key
// - X25519: 32-byte private key → 32-byte public key
// - P-256: 32-byte scalar → 33-byte compressed / 65-byte uncompressed public key
// - P-384: 48-byte scalar → 49-byte compressed / 97-byte uncompressed public key

use ed25519_dalek::SigningKey as Ed25519SigningKey;
use elliptic_curve::sec1::ToEncodedPoint;
use p256::ecdsa::SigningKey as P256SigningKey;
use p384::ecdsa::SigningKey as P384SigningKey;
use rand_core::OsRng;
use rsa::pkcs1::{
    DecodeRsaPrivateKey, DecodeRsaPublicKey, EncodeRsaPrivateKey, EncodeRsaPublicKey,
};
use rsa::pss::{
    Signature as RsaPssSignature, SigningKey as RsaPssSigningKey,
    VerifyingKey as RsaPssVerifyingKey,
};
use rsa::signature::{RandomizedSigner, SignatureEncoding, Verifier as RsaVerifier};
use rsa::{RsaPrivateKey, RsaPublicKey};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret as X25519StaticSecret};

/// RSA-2048 key sizes (PKCS#1 DER encoded)
/// Private key: ~1190-1220 bytes, we use 1300 as max buffer
/// Public key: ~270 bytes, we use 300 as max buffer
/// Signature: 256 bytes (2048 bits / 8)
const RSA_2048_PRIVATE_KEY_MAX: usize = 1300;
const RSA_2048_PUBLIC_KEY_MAX: usize = 300;
const RSA_2048_SIGNATURE_SIZE: usize = 256;

/// Signature algorithm identifiers matching Arth's `SignatureAlgorithm` enum.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    Ed25519 = 0,
    EcdsaP256Sha256 = 1,
    EcdsaP384Sha384 = 2,
    RsaPssSha256 = 3, // RSA-2048 PSS with SHA-256
}

impl SignatureAlgorithm {
    /// Returns the private key size in bytes for this algorithm.
    /// For RSA, returns maximum buffer size needed (actual size may be smaller).
    pub fn private_key_size(self) -> usize {
        match self {
            SignatureAlgorithm::Ed25519 => 32,         // 32-byte seed
            SignatureAlgorithm::EcdsaP256Sha256 => 32, // 32-byte scalar
            SignatureAlgorithm::EcdsaP384Sha384 => 48, // 48-byte scalar
            SignatureAlgorithm::RsaPssSha256 => RSA_2048_PRIVATE_KEY_MAX, // PKCS#1 DER
        }
    }

    /// Returns the public key size in bytes for this algorithm.
    /// For RSA, returns maximum buffer size needed (actual size may be smaller).
    pub fn public_key_size(self) -> usize {
        match self {
            SignatureAlgorithm::Ed25519 => 32,         // 32-byte public key
            SignatureAlgorithm::EcdsaP256Sha256 => 33, // 33-byte compressed
            SignatureAlgorithm::EcdsaP384Sha384 => 49, // 49-byte compressed
            SignatureAlgorithm::RsaPssSha256 => RSA_2048_PUBLIC_KEY_MAX, // PKCS#1 DER
        }
    }

    /// Returns the signature size in bytes.
    pub fn signature_size(self) -> usize {
        match self {
            SignatureAlgorithm::Ed25519 => 64,
            SignatureAlgorithm::EcdsaP256Sha256 => 64, // DER can be up to 72, but fixed-size is 64
            SignatureAlgorithm::EcdsaP384Sha384 => 96, // Fixed-size is 96
            SignatureAlgorithm::RsaPssSha256 => RSA_2048_SIGNATURE_SIZE, // 2048 bits / 8
        }
    }

    /// Returns true if this algorithm uses variable-size keys (like RSA).
    pub fn is_variable_size(self) -> bool {
        matches!(self, SignatureAlgorithm::RsaPssSha256)
    }

    /// Try to convert from an i32 value.
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(SignatureAlgorithm::Ed25519),
            1 => Some(SignatureAlgorithm::EcdsaP256Sha256),
            2 => Some(SignatureAlgorithm::EcdsaP384Sha384),
            3 => Some(SignatureAlgorithm::RsaPssSha256),
            _ => None,
        }
    }
}

/// Key exchange algorithm identifiers matching Arth's `KeyExchangeAlgorithm` enum.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyExchangeAlgorithm {
    X25519 = 0,
    EcdhP256 = 1,
    EcdhP384 = 2,
}

impl KeyExchangeAlgorithm {
    /// Returns the private key size in bytes for this algorithm.
    pub fn private_key_size(self) -> usize {
        match self {
            KeyExchangeAlgorithm::X25519 => 32,
            KeyExchangeAlgorithm::EcdhP256 => 32,
            KeyExchangeAlgorithm::EcdhP384 => 48,
        }
    }

    /// Returns the public key size in bytes for this algorithm.
    pub fn public_key_size(self) -> usize {
        match self {
            KeyExchangeAlgorithm::X25519 => 32,
            KeyExchangeAlgorithm::EcdhP256 => 33, // Compressed
            KeyExchangeAlgorithm::EcdhP384 => 49, // Compressed
        }
    }

    /// Returns the shared secret size in bytes.
    pub fn shared_secret_size(self) -> usize {
        match self {
            KeyExchangeAlgorithm::X25519 => 32,
            KeyExchangeAlgorithm::EcdhP256 => 32,
            KeyExchangeAlgorithm::EcdhP384 => 48,
        }
    }

    /// Try to convert from an i32 value.
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(KeyExchangeAlgorithm::X25519),
            1 => Some(KeyExchangeAlgorithm::EcdhP256),
            2 => Some(KeyExchangeAlgorithm::EcdhP384),
            _ => None,
        }
    }
}

// =========================================================================
// Signature Key Size Queries
// =========================================================================

/// Get the private key size for a signature algorithm.
///
/// # Arguments
/// * `algorithm` - Signature algorithm identifier (0-2)
///
/// # Returns
/// * Private key size in bytes, or 0 if algorithm is invalid
///
/// # C ABI
/// ```c
/// size_t arth_rt_signature_private_key_size(int32_t algorithm);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_signature_private_key_size(algorithm: i32) -> usize {
    match SignatureAlgorithm::from_i32(algorithm) {
        Some(algo) => algo.private_key_size(),
        None => 0,
    }
}

/// Get the public key size for a signature algorithm.
///
/// # Arguments
/// * `algorithm` - Signature algorithm identifier (0-2)
///
/// # Returns
/// * Public key size in bytes, or 0 if algorithm is invalid
///
/// # C ABI
/// ```c
/// size_t arth_rt_signature_public_key_size(int32_t algorithm);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_signature_public_key_size(algorithm: i32) -> usize {
    match SignatureAlgorithm::from_i32(algorithm) {
        Some(algo) => algo.public_key_size(),
        None => 0,
    }
}

/// Get the signature size for a signature algorithm.
///
/// # Arguments
/// * `algorithm` - Signature algorithm identifier (0-2)
///
/// # Returns
/// * Signature size in bytes, or 0 if algorithm is invalid
///
/// # C ABI
/// ```c
/// size_t arth_rt_signature_size(int32_t algorithm);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_signature_size(algorithm: i32) -> usize {
    match SignatureAlgorithm::from_i32(algorithm) {
        Some(algo) => algo.signature_size(),
        None => 0,
    }
}

// =========================================================================
// Key Exchange Key Size Queries
// =========================================================================

/// Get the private key size for a key exchange algorithm.
///
/// # Arguments
/// * `algorithm` - Key exchange algorithm identifier (0-2)
///
/// # Returns
/// * Private key size in bytes, or 0 if algorithm is invalid
///
/// # C ABI
/// ```c
/// size_t arth_rt_kex_private_key_size(int32_t algorithm);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_kex_private_key_size(algorithm: i32) -> usize {
    match KeyExchangeAlgorithm::from_i32(algorithm) {
        Some(algo) => algo.private_key_size(),
        None => 0,
    }
}

/// Get the public key size for a key exchange algorithm.
///
/// # Arguments
/// * `algorithm` - Key exchange algorithm identifier (0-2)
///
/// # Returns
/// * Public key size in bytes, or 0 if algorithm is invalid
///
/// # C ABI
/// ```c
/// size_t arth_rt_kex_public_key_size(int32_t algorithm);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_kex_public_key_size(algorithm: i32) -> usize {
    match KeyExchangeAlgorithm::from_i32(algorithm) {
        Some(algo) => algo.public_key_size(),
        None => 0,
    }
}

/// Get the shared secret size for a key exchange algorithm.
///
/// # Arguments
/// * `algorithm` - Key exchange algorithm identifier (0-2)
///
/// # Returns
/// * Shared secret size in bytes, or 0 if algorithm is invalid
///
/// # C ABI
/// ```c
/// size_t arth_rt_kex_shared_secret_size(int32_t algorithm);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_kex_shared_secret_size(algorithm: i32) -> usize {
    match KeyExchangeAlgorithm::from_i32(algorithm) {
        Some(algo) => algo.shared_secret_size(),
        None => 0,
    }
}

// =========================================================================
// Signature Key Pair Generation
// =========================================================================

/// Generate a signature key pair.
///
/// Generates a cryptographically secure random key pair for the specified
/// signature algorithm.
///
/// # Arguments
/// * `algorithm` - Signature algorithm identifier (0-3)
/// * `private_key` - Pointer to private key buffer
/// * `private_key_len` - Length of private key buffer
/// * `public_key` - Pointer to public key buffer
/// * `public_key_len` - Length of public key buffer
/// * `actual_priv_len` - Optional pointer to receive actual private key length (for RSA)
/// * `actual_pub_len` - Optional pointer to receive actual public key length (for RSA)
///
/// # Returns
/// * 0 on success
/// * -1 if private key buffer is too small
/// * -2 if public key buffer is too small
/// * -3 if private_key is null
/// * -4 if public_key is null
/// * -5 if algorithm is invalid
/// * -6 if key generation failed
///
/// # C ABI
/// ```c
/// int32_t arth_rt_signature_generate_keypair(
///     int32_t algorithm,
///     uint8_t* private_key, size_t private_key_len,
///     uint8_t* public_key, size_t public_key_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_signature_generate_keypair(
    algorithm: i32,
    private_key: *mut u8,
    private_key_len: usize,
    public_key: *mut u8,
    public_key_len: usize,
) -> i32 {
    if private_key.is_null() {
        return -3;
    }
    if public_key.is_null() {
        return -4;
    }

    let algo = match SignatureAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -5,
    };

    // For variable-size algorithms (RSA), check >= instead of ==
    if algo.is_variable_size() {
        if private_key_len < algo.private_key_size() {
            return -1;
        }
        if public_key_len < algo.public_key_size() {
            return -2;
        }
    } else {
        if private_key_len != algo.private_key_size() {
            return -1;
        }
        if public_key_len != algo.public_key_size() {
            return -2;
        }
    }

    let priv_slice = unsafe { std::slice::from_raw_parts_mut(private_key, private_key_len) };
    let pub_slice = unsafe { std::slice::from_raw_parts_mut(public_key, public_key_len) };

    match algo {
        SignatureAlgorithm::Ed25519 => {
            let signing_key = Ed25519SigningKey::generate(&mut OsRng);
            let verifying_key = signing_key.verifying_key();

            priv_slice.copy_from_slice(signing_key.as_bytes());
            pub_slice.copy_from_slice(verifying_key.as_bytes());
            0
        }
        SignatureAlgorithm::EcdsaP256Sha256 => {
            let signing_key = P256SigningKey::random(&mut OsRng);
            let verifying_key = signing_key.verifying_key();

            // Export private key as bytes
            let priv_bytes = signing_key.to_bytes();
            priv_slice.copy_from_slice(&priv_bytes);

            // Export public key as compressed SEC1
            let pub_bytes = verifying_key.to_encoded_point(true);
            pub_slice.copy_from_slice(pub_bytes.as_bytes());
            0
        }
        SignatureAlgorithm::EcdsaP384Sha384 => {
            let signing_key = P384SigningKey::random(&mut OsRng);
            let verifying_key = signing_key.verifying_key();

            // Export private key as bytes
            let priv_bytes = signing_key.to_bytes();
            priv_slice.copy_from_slice(&priv_bytes);

            // Export public key as compressed SEC1
            let pub_bytes = verifying_key.to_encoded_point(true);
            pub_slice.copy_from_slice(pub_bytes.as_bytes());
            0
        }
        SignatureAlgorithm::RsaPssSha256 => {
            // Generate RSA-2048 key pair
            let rsa_private = match RsaPrivateKey::new(&mut OsRng, 2048) {
                Ok(k) => k,
                Err(_) => return -6,
            };
            let rsa_public = rsa_private.to_public_key();

            // Encode private key as PKCS#1 DER
            let priv_der = match rsa_private.to_pkcs1_der() {
                Ok(der) => der,
                Err(_) => return -6,
            };
            let priv_bytes = priv_der.as_bytes();
            if priv_bytes.len() > private_key_len {
                return -1;
            }
            priv_slice[..priv_bytes.len()].copy_from_slice(priv_bytes);
            // Zero-pad the rest
            priv_slice[priv_bytes.len()..].fill(0);

            // Encode public key as PKCS#1 DER
            let pub_der = match rsa_public.to_pkcs1_der() {
                Ok(der) => der,
                Err(_) => return -6,
            };
            let pub_bytes = pub_der.as_bytes();
            if pub_bytes.len() > public_key_len {
                return -2;
            }
            pub_slice[..pub_bytes.len()].copy_from_slice(pub_bytes);
            // Zero-pad the rest
            pub_slice[pub_bytes.len()..].fill(0);

            0
        }
    }
}

/// Generate a signature key pair with actual size output.
///
/// Same as `arth_rt_signature_generate_keypair` but also returns the actual
/// key sizes (useful for variable-size keys like RSA).
///
/// # Arguments
/// * `algorithm` - Signature algorithm identifier (0-3)
/// * `private_key` - Pointer to private key buffer
/// * `private_key_len` - Length of private key buffer
/// * `public_key` - Pointer to public key buffer
/// * `public_key_len` - Length of public key buffer
/// * `actual_priv_len` - Pointer to receive actual private key length
/// * `actual_pub_len` - Pointer to receive actual public key length
///
/// # C ABI
/// ```c
/// int32_t arth_rt_signature_generate_keypair_ex(
///     int32_t algorithm,
///     uint8_t* private_key, size_t private_key_len,
///     uint8_t* public_key, size_t public_key_len,
///     size_t* actual_priv_len, size_t* actual_pub_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_signature_generate_keypair_ex(
    algorithm: i32,
    private_key: *mut u8,
    private_key_len: usize,
    public_key: *mut u8,
    public_key_len: usize,
    actual_priv_len: *mut usize,
    actual_pub_len: *mut usize,
) -> i32 {
    if private_key.is_null() {
        return -3;
    }
    if public_key.is_null() {
        return -4;
    }

    let algo = match SignatureAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -5,
    };

    // For variable-size algorithms (RSA), check >= instead of ==
    if algo.is_variable_size() {
        if private_key_len < algo.private_key_size() {
            return -1;
        }
        if public_key_len < algo.public_key_size() {
            return -2;
        }
    } else {
        if private_key_len != algo.private_key_size() {
            return -1;
        }
        if public_key_len != algo.public_key_size() {
            return -2;
        }
    }

    let priv_slice = unsafe { std::slice::from_raw_parts_mut(private_key, private_key_len) };
    let pub_slice = unsafe { std::slice::from_raw_parts_mut(public_key, public_key_len) };

    match algo {
        SignatureAlgorithm::Ed25519 => {
            let signing_key = Ed25519SigningKey::generate(&mut OsRng);
            let verifying_key = signing_key.verifying_key();

            priv_slice.copy_from_slice(signing_key.as_bytes());
            pub_slice.copy_from_slice(verifying_key.as_bytes());

            if !actual_priv_len.is_null() {
                unsafe {
                    *actual_priv_len = 32;
                }
            }
            if !actual_pub_len.is_null() {
                unsafe {
                    *actual_pub_len = 32;
                }
            }
            0
        }
        SignatureAlgorithm::EcdsaP256Sha256 => {
            let signing_key = P256SigningKey::random(&mut OsRng);
            let verifying_key = signing_key.verifying_key();

            let priv_bytes = signing_key.to_bytes();
            priv_slice.copy_from_slice(&priv_bytes);

            let pub_bytes = verifying_key.to_encoded_point(true);
            pub_slice.copy_from_slice(pub_bytes.as_bytes());

            if !actual_priv_len.is_null() {
                unsafe {
                    *actual_priv_len = 32;
                }
            }
            if !actual_pub_len.is_null() {
                unsafe {
                    *actual_pub_len = 33;
                }
            }
            0
        }
        SignatureAlgorithm::EcdsaP384Sha384 => {
            let signing_key = P384SigningKey::random(&mut OsRng);
            let verifying_key = signing_key.verifying_key();

            let priv_bytes = signing_key.to_bytes();
            priv_slice.copy_from_slice(&priv_bytes);

            let pub_bytes = verifying_key.to_encoded_point(true);
            pub_slice.copy_from_slice(pub_bytes.as_bytes());

            if !actual_priv_len.is_null() {
                unsafe {
                    *actual_priv_len = 48;
                }
            }
            if !actual_pub_len.is_null() {
                unsafe {
                    *actual_pub_len = 49;
                }
            }
            0
        }
        SignatureAlgorithm::RsaPssSha256 => {
            // Generate RSA-2048 key pair
            let rsa_private = match RsaPrivateKey::new(&mut OsRng, 2048) {
                Ok(k) => k,
                Err(_) => return -6,
            };
            let rsa_public = rsa_private.to_public_key();

            // Encode private key as PKCS#1 DER
            let priv_der = match rsa_private.to_pkcs1_der() {
                Ok(der) => der,
                Err(_) => return -6,
            };
            let priv_bytes = priv_der.as_bytes();
            if priv_bytes.len() > private_key_len {
                return -1;
            }
            priv_slice[..priv_bytes.len()].copy_from_slice(priv_bytes);
            priv_slice[priv_bytes.len()..].fill(0);

            // Encode public key as PKCS#1 DER
            let pub_der = match rsa_public.to_pkcs1_der() {
                Ok(der) => der,
                Err(_) => return -6,
            };
            let pub_bytes = pub_der.as_bytes();
            if pub_bytes.len() > public_key_len {
                return -2;
            }
            pub_slice[..pub_bytes.len()].copy_from_slice(pub_bytes);
            pub_slice[pub_bytes.len()..].fill(0);

            if !actual_priv_len.is_null() {
                unsafe {
                    *actual_priv_len = priv_bytes.len();
                }
            }
            if !actual_pub_len.is_null() {
                unsafe {
                    *actual_pub_len = pub_bytes.len();
                }
            }
            0
        }
    }
}

/// Derive the public key from a private key.
///
/// # Arguments
/// * `algorithm` - Signature algorithm identifier (0-3)
/// * `private_key` - Pointer to private key
/// * `private_key_len` - Length of private key
/// * `public_key` - Pointer to public key buffer
/// * `public_key_len` - Length of public key buffer
///
/// # Returns
/// * 0 on success (or actual public key size for variable-size algorithms)
/// * -1 if private key buffer is wrong size
/// * -2 if public key buffer is too small
/// * -3 if private_key is null
/// * -4 if public_key is null
/// * -5 if algorithm is invalid
/// * -6 if private key is invalid
///
/// # C ABI
/// ```c
/// int32_t arth_rt_signature_derive_public_key(
///     int32_t algorithm,
///     const uint8_t* private_key, size_t private_key_len,
///     uint8_t* public_key, size_t public_key_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_signature_derive_public_key(
    algorithm: i32,
    private_key: *const u8,
    private_key_len: usize,
    public_key: *mut u8,
    public_key_len: usize,
) -> i32 {
    if private_key.is_null() {
        return -3;
    }
    if public_key.is_null() {
        return -4;
    }

    let algo = match SignatureAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -5,
    };

    // For variable-size algorithms (RSA), we need to parse the actual key
    // For fixed-size algorithms, check exact sizes
    if !algo.is_variable_size() {
        if private_key_len != algo.private_key_size() {
            return -1;
        }
        if public_key_len != algo.public_key_size() {
            return -2;
        }
    } else if public_key_len < algo.public_key_size() {
        return -2;
    }

    let priv_slice = unsafe { std::slice::from_raw_parts(private_key, private_key_len) };
    let pub_slice = unsafe { std::slice::from_raw_parts_mut(public_key, public_key_len) };

    match algo {
        SignatureAlgorithm::Ed25519 => {
            let priv_bytes: [u8; 32] = match priv_slice.try_into() {
                Ok(b) => b,
                Err(_) => return -6,
            };
            let signing_key = Ed25519SigningKey::from_bytes(&priv_bytes);
            let verifying_key = signing_key.verifying_key();
            pub_slice.copy_from_slice(verifying_key.as_bytes());
            0
        }
        SignatureAlgorithm::EcdsaP256Sha256 => {
            let signing_key = match P256SigningKey::from_slice(priv_slice) {
                Ok(k) => k,
                Err(_) => return -6,
            };
            let verifying_key = signing_key.verifying_key();
            let pub_bytes = verifying_key.to_encoded_point(true);
            pub_slice.copy_from_slice(pub_bytes.as_bytes());
            0
        }
        SignatureAlgorithm::EcdsaP384Sha384 => {
            let signing_key = match P384SigningKey::from_slice(priv_slice) {
                Ok(k) => k,
                Err(_) => return -6,
            };
            let verifying_key = signing_key.verifying_key();
            let pub_bytes = verifying_key.to_encoded_point(true);
            pub_slice.copy_from_slice(pub_bytes.as_bytes());
            0
        }
        SignatureAlgorithm::RsaPssSha256 => {
            // For RSA, find actual key length (skip zero padding)
            let actual_priv_len = priv_slice
                .iter()
                .rposition(|&b| b != 0)
                .map(|pos| pos + 1)
                .unwrap_or(0);

            if actual_priv_len == 0 {
                return -6;
            }

            // Parse RSA private key from PKCS#1 DER
            let rsa_private = match RsaPrivateKey::from_pkcs1_der(&priv_slice[..actual_priv_len]) {
                Ok(k) => k,
                Err(_) => return -6,
            };
            let rsa_public = rsa_private.to_public_key();

            // Encode public key as PKCS#1 DER
            let pub_der = match rsa_public.to_pkcs1_der() {
                Ok(der) => der,
                Err(_) => return -6,
            };
            let pub_bytes = pub_der.as_bytes();
            if pub_bytes.len() > public_key_len {
                return -2;
            }
            pub_slice[..pub_bytes.len()].copy_from_slice(pub_bytes);
            pub_slice[pub_bytes.len()..].fill(0);
            0
        }
    }
}

// =========================================================================
// Key Exchange Key Pair Generation
// =========================================================================

/// Generate a key exchange key pair.
///
/// Generates a cryptographically secure random key pair for the specified
/// key exchange algorithm.
///
/// # Arguments
/// * `algorithm` - Key exchange algorithm identifier (0-2)
/// * `private_key` - Pointer to private key buffer
/// * `private_key_len` - Length of private key buffer
/// * `public_key` - Pointer to public key buffer
/// * `public_key_len` - Length of public key buffer
///
/// # Returns
/// * 0 on success
/// * -1 if private key buffer is wrong size
/// * -2 if public key buffer is wrong size
/// * -3 if private_key is null
/// * -4 if public_key is null
/// * -5 if algorithm is invalid
/// * -6 if key generation failed
///
/// # C ABI
/// ```c
/// int32_t arth_rt_kex_generate_keypair(
///     int32_t algorithm,
///     uint8_t* private_key, size_t private_key_len,
///     uint8_t* public_key, size_t public_key_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_kex_generate_keypair(
    algorithm: i32,
    private_key: *mut u8,
    private_key_len: usize,
    public_key: *mut u8,
    public_key_len: usize,
) -> i32 {
    if private_key.is_null() {
        return -3;
    }
    if public_key.is_null() {
        return -4;
    }

    let algo = match KeyExchangeAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -5,
    };

    if private_key_len != algo.private_key_size() {
        return -1;
    }

    if public_key_len != algo.public_key_size() {
        return -2;
    }

    let priv_slice = unsafe { std::slice::from_raw_parts_mut(private_key, private_key_len) };
    let pub_slice = unsafe { std::slice::from_raw_parts_mut(public_key, public_key_len) };

    match algo {
        KeyExchangeAlgorithm::X25519 => {
            let secret = X25519StaticSecret::random_from_rng(OsRng);
            let public = X25519PublicKey::from(&secret);

            priv_slice.copy_from_slice(secret.as_bytes());
            pub_slice.copy_from_slice(public.as_bytes());
            0
        }
        KeyExchangeAlgorithm::EcdhP256 => {
            // Use ecdsa SigningKey to generate a random scalar for ECDH
            let signing_key = P256SigningKey::random(&mut OsRng);
            let priv_bytes = signing_key.to_bytes();
            priv_slice.copy_from_slice(&priv_bytes);

            // Derive public key (compressed format)
            let public_key = p256::PublicKey::from_secret_scalar(signing_key.as_nonzero_scalar());
            let point = public_key.to_encoded_point(true);
            pub_slice.copy_from_slice(point.as_bytes());
            0
        }
        KeyExchangeAlgorithm::EcdhP384 => {
            // For P-384 ECDH, use ecdsa SigningKey to generate the scalar
            let signing_key = P384SigningKey::random(&mut OsRng);
            let priv_bytes = signing_key.to_bytes();
            priv_slice.copy_from_slice(&priv_bytes);

            // Derive public key
            let public_key = p384::PublicKey::from_secret_scalar(signing_key.as_nonzero_scalar());
            let point = public_key.to_encoded_point(true);
            pub_slice.copy_from_slice(point.as_bytes());
            0
        }
    }
}

/// Derive the public key from a private key for key exchange.
///
/// # Arguments
/// * `algorithm` - Key exchange algorithm identifier (0-2)
/// * `private_key` - Pointer to private key
/// * `private_key_len` - Length of private key
/// * `public_key` - Pointer to public key buffer
/// * `public_key_len` - Length of public key buffer
///
/// # Returns
/// * 0 on success
/// * -1 if private key buffer is wrong size
/// * -2 if public key buffer is wrong size
/// * -3 if private_key is null
/// * -4 if public_key is null
/// * -5 if algorithm is invalid
/// * -6 if private key is invalid
///
/// # C ABI
/// ```c
/// int32_t arth_rt_kex_derive_public_key(
///     int32_t algorithm,
///     const uint8_t* private_key, size_t private_key_len,
///     uint8_t* public_key, size_t public_key_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_kex_derive_public_key(
    algorithm: i32,
    private_key: *const u8,
    private_key_len: usize,
    public_key: *mut u8,
    public_key_len: usize,
) -> i32 {
    if private_key.is_null() {
        return -3;
    }
    if public_key.is_null() {
        return -4;
    }

    let algo = match KeyExchangeAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -5,
    };

    if private_key_len != algo.private_key_size() {
        return -1;
    }

    if public_key_len != algo.public_key_size() {
        return -2;
    }

    let priv_slice = unsafe { std::slice::from_raw_parts(private_key, private_key_len) };
    let pub_slice = unsafe { std::slice::from_raw_parts_mut(public_key, public_key_len) };

    match algo {
        KeyExchangeAlgorithm::X25519 => {
            let priv_bytes: [u8; 32] = match priv_slice.try_into() {
                Ok(b) => b,
                Err(_) => return -6,
            };
            let secret = X25519StaticSecret::from(priv_bytes);
            let public = X25519PublicKey::from(&secret);
            pub_slice.copy_from_slice(public.as_bytes());
            0
        }
        KeyExchangeAlgorithm::EcdhP256 => {
            let signing_key = match P256SigningKey::from_slice(priv_slice) {
                Ok(k) => k,
                Err(_) => return -6,
            };
            let public_key = p256::PublicKey::from_secret_scalar(signing_key.as_nonzero_scalar());
            let point = public_key.to_encoded_point(true);
            pub_slice.copy_from_slice(point.as_bytes());
            0
        }
        KeyExchangeAlgorithm::EcdhP384 => {
            let signing_key = match P384SigningKey::from_slice(priv_slice) {
                Ok(k) => k,
                Err(_) => return -6,
            };
            let public_key = p384::PublicKey::from_secret_scalar(signing_key.as_nonzero_scalar());
            let point = public_key.to_encoded_point(true);
            pub_slice.copy_from_slice(point.as_bytes());
            0
        }
    }
}

// =============================================================================
// Key Exchange Agreement (Diffie-Hellman)
// =============================================================================
//
// This section provides Diffie-Hellman key agreement:
// - X25519: Curve25519 key exchange (RFC 7748)
// - ECDH P-256: NIST P-256 curve ECDH
// - ECDH P-384: NIST P-384 curve ECDH
//
// Shared secret sizes:
// - X25519: 32 bytes
// - ECDH P-256: 32 bytes (x-coordinate of shared point)
// - ECDH P-384: 48 bytes (x-coordinate of shared point)

/// Perform Diffie-Hellman key agreement.
///
/// Computes the shared secret from a local private key and a remote public key.
/// Both parties compute the same shared secret when using their respective
/// private keys and each other's public keys.
///
/// # Arguments
/// * `algorithm` - Key exchange algorithm identifier (0-2)
/// * `private_key` - Pointer to local private key
/// * `private_key_len` - Length of private key
/// * `public_key` - Pointer to remote public key
/// * `public_key_len` - Length of public key
/// * `shared_secret` - Pointer to shared secret output buffer
/// * `shared_secret_len` - Length of shared secret buffer
///
/// # Returns
/// * Number of bytes written to shared_secret on success
/// * -1 if private key buffer is wrong size
/// * -2 if public key buffer is wrong size
/// * -3 if shared_secret buffer is too small
/// * -4 if private_key is null
/// * -5 if public_key is null
/// * -6 if shared_secret is null
/// * -7 if algorithm is invalid
/// * -8 if private key is invalid
/// * -9 if public key is invalid
/// * -10 if key agreement failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_kex_agree(
///     int32_t algorithm,
///     const uint8_t* private_key, size_t private_key_len,
///     const uint8_t* public_key, size_t public_key_len,
///     uint8_t* shared_secret, size_t shared_secret_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_kex_agree(
    algorithm: i32,
    private_key: *const u8,
    private_key_len: usize,
    public_key: *const u8,
    public_key_len: usize,
    shared_secret: *mut u8,
    shared_secret_len: usize,
) -> i64 {
    if private_key.is_null() {
        return -4;
    }
    if public_key.is_null() {
        return -5;
    }
    if shared_secret.is_null() {
        return -6;
    }

    let algo = match KeyExchangeAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -7,
    };

    if private_key_len != algo.private_key_size() {
        return -1;
    }

    if public_key_len != algo.public_key_size() {
        return -2;
    }

    let expected_secret_size = algo.shared_secret_size();
    if shared_secret_len < expected_secret_size {
        return -3;
    }

    let priv_slice = unsafe { std::slice::from_raw_parts(private_key, private_key_len) };
    let pub_slice = unsafe { std::slice::from_raw_parts(public_key, public_key_len) };
    let secret_slice =
        unsafe { std::slice::from_raw_parts_mut(shared_secret, expected_secret_size) };

    match algo {
        KeyExchangeAlgorithm::X25519 => {
            // Parse private key
            let priv_bytes: [u8; 32] = match priv_slice.try_into() {
                Ok(b) => b,
                Err(_) => return -8,
            };
            let secret = X25519StaticSecret::from(priv_bytes);

            // Parse public key
            let pub_bytes: [u8; 32] = match pub_slice.try_into() {
                Ok(b) => b,
                Err(_) => return -9,
            };
            let their_public = X25519PublicKey::from(pub_bytes);

            // Perform key agreement
            let shared = secret.diffie_hellman(&their_public);
            secret_slice.copy_from_slice(shared.as_bytes());
            32
        }
        KeyExchangeAlgorithm::EcdhP256 => {
            use elliptic_curve::sec1::FromEncodedPoint;

            // Parse private key as scalar
            let secret_key = match p256::SecretKey::from_slice(priv_slice) {
                Ok(k) => k,
                Err(_) => return -8,
            };

            // Parse public key from compressed SEC1 format
            let encoded_point = match p256::EncodedPoint::from_bytes(pub_slice) {
                Ok(p) => p,
                Err(_) => return -9,
            };
            let their_public =
                match p256::PublicKey::from_encoded_point(&encoded_point).into_option() {
                    Some(pk) => pk,
                    None => return -9,
                };

            // Perform ECDH
            let shared = p256::ecdh::diffie_hellman(
                secret_key.to_nonzero_scalar(),
                their_public.as_affine(),
            );
            secret_slice.copy_from_slice(shared.raw_secret_bytes());
            32
        }
        KeyExchangeAlgorithm::EcdhP384 => {
            use elliptic_curve::sec1::FromEncodedPoint;

            // Parse private key as scalar
            let secret_key = match p384::SecretKey::from_slice(priv_slice) {
                Ok(k) => k,
                Err(_) => return -8,
            };

            // Parse public key from compressed SEC1 format
            let encoded_point = match p384::EncodedPoint::from_bytes(pub_slice) {
                Ok(p) => p,
                Err(_) => return -9,
            };
            let their_public =
                match p384::PublicKey::from_encoded_point(&encoded_point).into_option() {
                    Some(pk) => pk,
                    None => return -9,
                };

            // Perform ECDH
            let shared = p384::ecdh::diffie_hellman(
                secret_key.to_nonzero_scalar(),
                their_public.as_affine(),
            );
            secret_slice.copy_from_slice(shared.raw_secret_bytes());
            48
        }
    }
}

/// Perform Diffie-Hellman key agreement with HKDF key derivation.
///
/// This function combines key exchange with HKDF to derive a key of the
/// desired length. This is the recommended way to use key exchange results
/// since raw shared secrets should not be used directly as keys.
///
/// # Arguments
/// * `kex_algorithm` - Key exchange algorithm identifier (0-2)
/// * `hash_algorithm` - Hash algorithm for HKDF (0=SHA-256, 2=SHA-512)
/// * `private_key` - Pointer to local private key
/// * `private_key_len` - Length of private key
/// * `public_key` - Pointer to remote public key
/// * `public_key_len` - Length of public key
/// * `salt` - Pointer to salt (can be null for unsalted)
/// * `salt_len` - Length of salt
/// * `info` - Pointer to context info (can be null)
/// * `info_len` - Length of info
/// * `output` - Pointer to output buffer for derived key
/// * `output_len` - Desired length of derived key
///
/// # Returns
/// * Number of bytes written to output on success
/// * -1 if private key buffer is wrong size
/// * -2 if public key buffer is wrong size
/// * -3 if output buffer is null
/// * -4 if private_key is null
/// * -5 if public_key is null
/// * -6 if kex_algorithm is invalid
/// * -7 if hash_algorithm is invalid
/// * -8 if private key is invalid
/// * -9 if public key is invalid
/// * -10 if key agreement failed
/// * -11 if HKDF derivation failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_kex_agree_with_kdf(
///     int32_t kex_algorithm,
///     int32_t hash_algorithm,
///     const uint8_t* private_key, size_t private_key_len,
///     const uint8_t* public_key, size_t public_key_len,
///     const uint8_t* salt, size_t salt_len,
///     const uint8_t* info, size_t info_len,
///     uint8_t* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_kex_agree_with_kdf(
    kex_algorithm: i32,
    hash_algorithm: i32,
    private_key: *const u8,
    private_key_len: usize,
    public_key: *const u8,
    public_key_len: usize,
    salt: *const u8,
    salt_len: usize,
    info: *const u8,
    info_len: usize,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if private_key.is_null() {
        return -4;
    }
    if public_key.is_null() {
        return -5;
    }
    if output.is_null() {
        return -3;
    }

    let kex_algo = match KeyExchangeAlgorithm::from_i32(kex_algorithm) {
        Some(a) => a,
        None => return -6,
    };

    // Validate hash algorithm (only SHA-256 and SHA-512 supported for HKDF)
    if hash_algorithm != 0 && hash_algorithm != 2 {
        return -7;
    }

    // First, perform key agreement to get shared secret
    let shared_secret_size = kex_algo.shared_secret_size();
    let mut shared_secret = vec![0u8; shared_secret_size];

    let kex_result = arth_rt_kex_agree(
        kex_algorithm,
        private_key,
        private_key_len,
        public_key,
        public_key_len,
        shared_secret.as_mut_ptr(),
        shared_secret_size,
    );

    if kex_result < 0 {
        // Map kex errors to our error codes
        return match kex_result {
            -1 => -1, // private key wrong size
            -2 => -2, // public key wrong size
            -8 => -8, // private key invalid
            -9 => -9, // public key invalid
            _ => -10, // key agreement failed
        };
    }

    // Get salt and info slices
    let salt_slice = if salt.is_null() || salt_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(salt, salt_len) }
    };

    let info_slice = if info.is_null() || info_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(info, info_len) }
    };

    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, output_len) };

    // Perform HKDF derivation
    let result = match hash_algorithm {
        0 => {
            // HKDF-SHA256
            let hk = hkdf::Hkdf::<sha2::Sha256>::new(
                if salt_slice.is_empty() {
                    None
                } else {
                    Some(salt_slice)
                },
                &shared_secret,
            );
            match hk.expand(info_slice, output_slice) {
                Ok(()) => output_len as i64,
                Err(_) => -11,
            }
        }
        2 => {
            // HKDF-SHA512
            let hk = hkdf::Hkdf::<sha2::Sha512>::new(
                if salt_slice.is_empty() {
                    None
                } else {
                    Some(salt_slice)
                },
                &shared_secret,
            );
            match hk.expand(info_slice, output_slice) {
                Ok(()) => output_len as i64,
                Err(_) => -11,
            }
        }
        _ => -7,
    };

    // Securely wipe shared secret
    shared_secret.fill(0);

    result
}

// =============================================================================
// Digital Signatures (Sign and Verify)
// =============================================================================
//
// This section provides digital signature operations:
// - Ed25519: Pure EdDSA signatures (RFC 8032)
// - ECDSA P-256: NIST P-256 curve with SHA-256
// - ECDSA P-384: NIST P-384 curve with SHA-384
//
// Signature sizes:
// - Ed25519: 64 bytes
// - ECDSA P-256: 64 bytes (fixed-size r||s format)
// - ECDSA P-384: 96 bytes (fixed-size r||s format)

use ed25519_dalek::{Signature as Ed25519Signature, Signer, VerifyingKey as Ed25519VerifyingKey};
use p256::ecdsa::{
    Signature as P256Signature, VerifyingKey as P256VerifyingKey, signature::Signer as P256Signer,
    signature::Verifier as P256Verifier,
};
use p384::ecdsa::{
    Signature as P384Signature, VerifyingKey as P384VerifyingKey, signature::Signer as P384Signer,
    signature::Verifier as P384Verifier,
};

/// Sign a message with a private key.
///
/// # Arguments
/// * `algorithm` - Signature algorithm identifier (0-3)
/// * `private_key` - Pointer to private key
/// * `private_key_len` - Length of private key
/// * `message` - Pointer to message to sign
/// * `message_len` - Length of message
/// * `signature` - Pointer to signature buffer
/// * `signature_len` - Length of signature buffer
///
/// # Returns
/// * Number of bytes written to signature on success
/// * -1 if private key buffer is wrong size
/// * -2 if signature buffer is wrong size
/// * -3 if private_key is null
/// * -4 if signature is null
/// * -5 if algorithm is invalid
/// * -6 if private key is invalid
/// * -7 if signing failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_signature_sign(
///     int32_t algorithm,
///     const uint8_t* private_key, size_t private_key_len,
///     const uint8_t* message, size_t message_len,
///     uint8_t* signature, size_t signature_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_signature_sign(
    algorithm: i32,
    private_key: *const u8,
    private_key_len: usize,
    message: *const u8,
    message_len: usize,
    signature: *mut u8,
    signature_len: usize,
) -> i64 {
    if private_key.is_null() {
        return -3;
    }
    if signature.is_null() {
        return -4;
    }

    let algo = match SignatureAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -5,
    };

    // For variable-size keys (RSA), don't check exact size
    if !algo.is_variable_size() && private_key_len != algo.private_key_size() {
        return -1;
    }

    let expected_sig_size = algo.signature_size();
    if signature_len < expected_sig_size {
        return -2;
    }

    let priv_slice = unsafe { std::slice::from_raw_parts(private_key, private_key_len) };
    let sig_slice = unsafe { std::slice::from_raw_parts_mut(signature, expected_sig_size) };

    let msg_slice = if message.is_null() || message_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(message, message_len) }
    };

    match algo {
        SignatureAlgorithm::Ed25519 => {
            let priv_bytes: [u8; 32] = match priv_slice.try_into() {
                Ok(b) => b,
                Err(_) => return -6,
            };
            let signing_key = Ed25519SigningKey::from_bytes(&priv_bytes);
            let sig = signing_key.sign(msg_slice);
            sig_slice.copy_from_slice(&sig.to_bytes());
            64
        }
        SignatureAlgorithm::EcdsaP256Sha256 => {
            let signing_key = match P256SigningKey::from_slice(priv_slice) {
                Ok(k) => k,
                Err(_) => return -6,
            };
            let sig: P256Signature = P256Signer::sign(&signing_key, msg_slice);
            sig_slice.copy_from_slice(&sig.to_bytes());
            64
        }
        SignatureAlgorithm::EcdsaP384Sha384 => {
            let signing_key = match P384SigningKey::from_slice(priv_slice) {
                Ok(k) => k,
                Err(_) => return -6,
            };
            let sig: P384Signature = P384Signer::sign(&signing_key, msg_slice);
            sig_slice.copy_from_slice(&sig.to_bytes());
            96
        }
        SignatureAlgorithm::RsaPssSha256 => {
            // For RSA, find actual key length (skip zero padding)
            let actual_priv_len = priv_slice
                .iter()
                .rposition(|&b| b != 0)
                .map(|pos| pos + 1)
                .unwrap_or(0);

            if actual_priv_len == 0 {
                return -6;
            }

            // Parse RSA private key from PKCS#1 DER
            let rsa_private = match RsaPrivateKey::from_pkcs1_der(&priv_slice[..actual_priv_len]) {
                Ok(k) => k,
                Err(_) => return -6,
            };

            // Create PSS signing key with SHA-256
            let signing_key = RsaPssSigningKey::<sha2::Sha256>::new(rsa_private);

            // Sign the message
            let sig = match signing_key.try_sign_with_rng(&mut OsRng, msg_slice) {
                Ok(s) => s,
                Err(_) => return -7,
            };
            let sig_bytes = sig.to_bytes();
            if sig_bytes.len() > signature_len {
                return -2;
            }
            sig_slice[..sig_bytes.len()].copy_from_slice(&sig_bytes);
            sig_bytes.len() as i64
        }
    }
}

/// Verify a signature against a message and public key.
///
/// # Arguments
/// * `algorithm` - Signature algorithm identifier (0-3)
/// * `public_key` - Pointer to public key
/// * `public_key_len` - Length of public key
/// * `message` - Pointer to message that was signed
/// * `message_len` - Length of message
/// * `signature` - Pointer to signature to verify
/// * `signature_len` - Length of signature
///
/// # Returns
/// * 1 if signature is valid
/// * 0 if signature is invalid
/// * -1 if public key buffer is wrong size
/// * -2 if signature buffer is wrong size
/// * -3 if public_key is null
/// * -4 if signature is null
/// * -5 if algorithm is invalid
/// * -6 if public key is malformed
/// * -7 if signature is malformed
///
/// # C ABI
/// ```c
/// int32_t arth_rt_signature_verify(
///     int32_t algorithm,
///     const uint8_t* public_key, size_t public_key_len,
///     const uint8_t* message, size_t message_len,
///     const uint8_t* signature, size_t signature_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_signature_verify(
    algorithm: i32,
    public_key: *const u8,
    public_key_len: usize,
    message: *const u8,
    message_len: usize,
    signature: *const u8,
    signature_len: usize,
) -> i32 {
    if public_key.is_null() {
        return -3;
    }
    if signature.is_null() {
        return -4;
    }

    let algo = match SignatureAlgorithm::from_i32(algorithm) {
        Some(a) => a,
        None => return -5,
    };

    // For variable-size algorithms (RSA), don't check exact size
    if !algo.is_variable_size() {
        if public_key_len != algo.public_key_size() {
            return -1;
        }
        if signature_len != algo.signature_size() {
            return -2;
        }
    }

    let pub_slice = unsafe { std::slice::from_raw_parts(public_key, public_key_len) };
    let sig_slice = unsafe { std::slice::from_raw_parts(signature, signature_len) };

    let msg_slice = if message.is_null() || message_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(message, message_len) }
    };

    match algo {
        SignatureAlgorithm::Ed25519 => {
            let pub_bytes: [u8; 32] = match pub_slice.try_into() {
                Ok(b) => b,
                Err(_) => return -6,
            };
            let verifying_key = match Ed25519VerifyingKey::from_bytes(&pub_bytes) {
                Ok(k) => k,
                Err(_) => return -6,
            };
            let sig_bytes: [u8; 64] = match sig_slice.try_into() {
                Ok(b) => b,
                Err(_) => return -7,
            };
            let sig = Ed25519Signature::from_bytes(&sig_bytes);
            match verifying_key.verify(msg_slice, &sig) {
                Ok(()) => 1,
                Err(_) => 0,
            }
        }
        SignatureAlgorithm::EcdsaP256Sha256 => {
            // Parse compressed SEC1 public key
            let verifying_key = match P256VerifyingKey::from_sec1_bytes(pub_slice) {
                Ok(k) => k,
                Err(_) => return -6,
            };
            let sig = match P256Signature::from_slice(sig_slice) {
                Ok(s) => s,
                Err(_) => return -7,
            };
            match P256Verifier::verify(&verifying_key, msg_slice, &sig) {
                Ok(()) => 1,
                Err(_) => 0,
            }
        }
        SignatureAlgorithm::EcdsaP384Sha384 => {
            // Parse compressed SEC1 public key
            let verifying_key = match P384VerifyingKey::from_sec1_bytes(pub_slice) {
                Ok(k) => k,
                Err(_) => return -6,
            };
            let sig = match P384Signature::from_slice(sig_slice) {
                Ok(s) => s,
                Err(_) => return -7,
            };
            match P384Verifier::verify(&verifying_key, msg_slice, &sig) {
                Ok(()) => 1,
                Err(_) => 0,
            }
        }
        SignatureAlgorithm::RsaPssSha256 => {
            // For RSA, find actual key length (skip zero padding)
            let actual_pub_len = pub_slice
                .iter()
                .rposition(|&b| b != 0)
                .map(|pos| pos + 1)
                .unwrap_or(0);

            if actual_pub_len == 0 {
                return -6;
            }

            // Parse RSA public key from PKCS#1 DER
            let rsa_public = match RsaPublicKey::from_pkcs1_der(&pub_slice[..actual_pub_len]) {
                Ok(k) => k,
                Err(_) => return -6,
            };

            // Create PSS verifying key with SHA-256
            let verifying_key = RsaPssVerifyingKey::<sha2::Sha256>::new(rsa_public);

            // Parse signature
            let sig = match RsaPssSignature::try_from(sig_slice) {
                Ok(s) => s,
                Err(_) => return -7,
            };

            // Verify the signature
            match RsaVerifier::verify(&verifying_key, msg_slice, &sig) {
                Ok(()) => 1,
                Err(_) => 0,
            }
        }
    }
}

// =============================================================================
// Key Derivation Functions (KDF)
// =============================================================================
//
// This section provides key derivation functions:
// - HKDF: HMAC-based Key Derivation Function (RFC 5869)
// - PBKDF2: Password-Based Key Derivation Function 2 (RFC 8018)
// - Argon2id: Memory-hard password hashing (RFC 9106)
//
// Use cases:
// - HKDF: Derive keys from shared secrets (key exchange), expand key material
// - PBKDF2: Derive keys from passwords (legacy, high iteration count required)
// - Argon2id: Derive keys from passwords (modern, memory-hard, recommended)

/// KDF algorithm identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum KdfAlgorithm {
    /// HKDF with SHA-256
    HkdfSha256 = 0,
    /// HKDF with SHA-384
    HkdfSha384 = 1,
    /// HKDF with SHA-512
    HkdfSha512 = 2,
    /// PBKDF2 with HMAC-SHA256
    Pbkdf2Sha256 = 3,
    /// PBKDF2 with HMAC-SHA512
    Pbkdf2Sha512 = 4,
    /// Argon2id (hybrid of Argon2i and Argon2d)
    Argon2id = 5,
}

impl KdfAlgorithm {
    /// Convert from i32 algorithm identifier.
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(KdfAlgorithm::HkdfSha256),
            1 => Some(KdfAlgorithm::HkdfSha384),
            2 => Some(KdfAlgorithm::HkdfSha512),
            3 => Some(KdfAlgorithm::Pbkdf2Sha256),
            4 => Some(KdfAlgorithm::Pbkdf2Sha512),
            5 => Some(KdfAlgorithm::Argon2id),
            _ => None,
        }
    }
}

// -----------------------------------------------------------------------------
// HKDF (HMAC-based Key Derivation Function)
// -----------------------------------------------------------------------------

/// Derive key material using HKDF (RFC 5869).
///
/// HKDF consists of two stages:
/// 1. Extract: Create a pseudorandom key (PRK) from input key material and salt
/// 2. Expand: Expand PRK into output key material using optional info
///
/// This function combines both stages.
///
/// # Arguments
/// * `algorithm` - HKDF algorithm (0=SHA256, 1=SHA384, 2=SHA512)
/// * `ikm` - Input key material (e.g., shared secret from key exchange)
/// * `ikm_len` - Length of input key material
/// * `salt` - Optional salt (can be null for no salt)
/// * `salt_len` - Length of salt (0 if salt is null)
/// * `info` - Optional context/application info (can be null)
/// * `info_len` - Length of info (0 if info is null)
/// * `okm` - Output key material buffer
/// * `okm_len` - Desired length of output key material
///
/// # Returns
/// * Number of bytes written to okm on success
/// * -1 if algorithm is invalid
/// * -2 if ikm is null
/// * -3 if okm is null
/// * -4 if okm_len is 0 or too large
/// * -5 if HKDF operation failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_hkdf_derive(
///     int32_t algorithm,
///     const uint8_t* ikm, size_t ikm_len,
///     const uint8_t* salt, size_t salt_len,
///     const uint8_t* info, size_t info_len,
///     uint8_t* okm, size_t okm_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hkdf_derive(
    algorithm: i32,
    ikm: *const u8,
    ikm_len: usize,
    salt: *const u8,
    salt_len: usize,
    info: *const u8,
    info_len: usize,
    okm: *mut u8,
    okm_len: usize,
) -> i64 {
    use hkdf::Hkdf;

    if ikm.is_null() {
        return -2;
    }
    if okm.is_null() {
        return -3;
    }
    if okm_len == 0 {
        return -4;
    }

    let ikm_slice = unsafe { std::slice::from_raw_parts(ikm, ikm_len) };
    let salt_slice = if salt.is_null() || salt_len == 0 {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(salt, salt_len) })
    };
    let info_slice = if info.is_null() || info_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(info, info_len) }
    };
    let okm_slice = unsafe { std::slice::from_raw_parts_mut(okm, okm_len) };

    match algorithm {
        0 => {
            // HKDF-SHA256
            let hk = Hkdf::<sha2::Sha256>::new(salt_slice, ikm_slice);
            if hk.expand(info_slice, okm_slice).is_err() {
                return -5;
            }
            okm_len as i64
        }
        1 => {
            // HKDF-SHA384
            let hk = Hkdf::<sha2::Sha384>::new(salt_slice, ikm_slice);
            if hk.expand(info_slice, okm_slice).is_err() {
                return -5;
            }
            okm_len as i64
        }
        2 => {
            // HKDF-SHA512
            let hk = Hkdf::<sha2::Sha512>::new(salt_slice, ikm_slice);
            if hk.expand(info_slice, okm_slice).is_err() {
                return -5;
            }
            okm_len as i64
        }
        _ => -1,
    }
}

/// HKDF Extract phase only.
///
/// Extracts a pseudorandom key (PRK) from input key material and salt.
/// Use this when you need to reuse the PRK for multiple expand operations.
///
/// # Arguments
/// * `algorithm` - HKDF algorithm (0=SHA256, 1=SHA384, 2=SHA512)
/// * `ikm` - Input key material
/// * `ikm_len` - Length of input key material
/// * `salt` - Optional salt (can be null)
/// * `salt_len` - Length of salt
/// * `prk` - Output pseudorandom key buffer
/// * `prk_len` - Length of PRK buffer (must match hash output size)
///
/// # Returns
/// * Number of bytes written to prk on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_hkdf_extract(
    algorithm: i32,
    ikm: *const u8,
    ikm_len: usize,
    salt: *const u8,
    salt_len: usize,
    prk: *mut u8,
    prk_len: usize,
) -> i64 {
    use hkdf::Hkdf;

    if ikm.is_null() {
        return -2;
    }
    if prk.is_null() {
        return -3;
    }

    let ikm_slice = unsafe { std::slice::from_raw_parts(ikm, ikm_len) };
    let salt_slice = if salt.is_null() || salt_len == 0 {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(salt, salt_len) })
    };
    let prk_slice = unsafe { std::slice::from_raw_parts_mut(prk, prk_len) };

    match algorithm {
        0 => {
            if prk_len < 32 {
                return -4;
            }
            let (extracted, _) = Hkdf::<sha2::Sha256>::extract(salt_slice, ikm_slice);
            prk_slice[..32].copy_from_slice(&extracted);
            32
        }
        1 => {
            if prk_len < 48 {
                return -4;
            }
            let (extracted, _) = Hkdf::<sha2::Sha384>::extract(salt_slice, ikm_slice);
            prk_slice[..48].copy_from_slice(&extracted);
            48
        }
        2 => {
            if prk_len < 64 {
                return -4;
            }
            let (extracted, _) = Hkdf::<sha2::Sha512>::extract(salt_slice, ikm_slice);
            prk_slice[..64].copy_from_slice(&extracted);
            64
        }
        _ => -1,
    }
}

// -----------------------------------------------------------------------------
// PBKDF2 (Password-Based Key Derivation Function 2)
// -----------------------------------------------------------------------------

/// Derive key from password using PBKDF2.
///
/// PBKDF2 applies a pseudorandom function (HMAC) to the password along with
/// a salt value, repeating the process many times to produce a derived key.
///
/// IMPORTANT: Use a high iteration count (at least 100,000 for SHA-256,
/// 210,000 for SHA-512) to slow down brute-force attacks.
///
/// # Arguments
/// * `algorithm` - PBKDF2 algorithm (3=HMAC-SHA256, 4=HMAC-SHA512)
/// * `password` - Password bytes
/// * `password_len` - Length of password
/// * `salt` - Salt bytes (should be at least 16 bytes, randomly generated)
/// * `salt_len` - Length of salt
/// * `iterations` - Number of iterations (higher = slower, more secure)
/// * `okm` - Output key material buffer
/// * `okm_len` - Desired length of output key material
///
/// # Returns
/// * Number of bytes written to okm on success
/// * -1 if algorithm is invalid
/// * -2 if password is null
/// * -3 if salt is null
/// * -4 if okm is null
/// * -5 if iterations is 0
/// * -6 if okm_len is 0
///
/// # C ABI
/// ```c
/// int64_t arth_rt_pbkdf2_derive(
///     int32_t algorithm,
///     const uint8_t* password, size_t password_len,
///     const uint8_t* salt, size_t salt_len,
///     uint32_t iterations,
///     uint8_t* okm, size_t okm_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pbkdf2_derive(
    algorithm: i32,
    password: *const u8,
    password_len: usize,
    salt: *const u8,
    salt_len: usize,
    iterations: u32,
    okm: *mut u8,
    okm_len: usize,
) -> i64 {
    use pbkdf2::pbkdf2_hmac;

    if password.is_null() {
        return -2;
    }
    if salt.is_null() {
        return -3;
    }
    if okm.is_null() {
        return -4;
    }
    if iterations == 0 {
        return -5;
    }
    if okm_len == 0 {
        return -6;
    }

    let password_slice = unsafe { std::slice::from_raw_parts(password, password_len) };
    let salt_slice = unsafe { std::slice::from_raw_parts(salt, salt_len) };
    let okm_slice = unsafe { std::slice::from_raw_parts_mut(okm, okm_len) };

    match algorithm {
        3 => {
            // PBKDF2-HMAC-SHA256
            pbkdf2_hmac::<sha2::Sha256>(password_slice, salt_slice, iterations, okm_slice);
            okm_len as i64
        }
        4 => {
            // PBKDF2-HMAC-SHA512
            pbkdf2_hmac::<sha2::Sha512>(password_slice, salt_slice, iterations, okm_slice);
            okm_len as i64
        }
        _ => -1,
    }
}

// -----------------------------------------------------------------------------
// Argon2id (Memory-Hard Password Hashing)
// -----------------------------------------------------------------------------

/// Derive key from password using Argon2id.
///
/// Argon2id is the recommended algorithm for password hashing (RFC 9106).
/// It is memory-hard, making it resistant to GPU and ASIC attacks.
///
/// Recommended parameters (OWASP):
/// - Memory: 64 MB (65536 KiB) minimum
/// - Iterations: 3 minimum
/// - Parallelism: 4 (or number of CPU cores)
///
/// # Arguments
/// * `password` - Password bytes
/// * `password_len` - Length of password
/// * `salt` - Salt bytes (must be at least 16 bytes)
/// * `salt_len` - Length of salt (minimum 16)
/// * `memory_kib` - Memory usage in KiB (e.g., 65536 for 64 MB)
/// * `iterations` - Number of iterations (time cost)
/// * `parallelism` - Degree of parallelism (lanes)
/// * `okm` - Output key material buffer
/// * `okm_len` - Desired length of output (4 to 2^32-1 bytes)
///
/// # Returns
/// * Number of bytes written to okm on success
/// * -1 if password is null
/// * -2 if salt is null or too short
/// * -3 if okm is null
/// * -4 if parameters are invalid
/// * -5 if Argon2 operation failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_argon2_derive(
///     const uint8_t* password, size_t password_len,
///     const uint8_t* salt, size_t salt_len,
///     uint32_t memory_kib, uint32_t iterations, uint32_t parallelism,
///     uint8_t* okm, size_t okm_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_argon2_derive(
    password: *const u8,
    password_len: usize,
    salt: *const u8,
    salt_len: usize,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    okm: *mut u8,
    okm_len: usize,
) -> i64 {
    use argon2::{Algorithm, Argon2, Params, Version};

    if password.is_null() {
        return -1;
    }
    if salt.is_null() || salt_len < 8 {
        return -2;
    }
    if okm.is_null() {
        return -3;
    }
    if okm_len < 4 {
        return -4;
    }

    let password_slice = unsafe { std::slice::from_raw_parts(password, password_len) };
    let salt_slice = unsafe { std::slice::from_raw_parts(salt, salt_len) };
    let okm_slice = unsafe { std::slice::from_raw_parts_mut(okm, okm_len) };

    // Build Argon2 params
    let params = match Params::new(memory_kib, iterations, parallelism, Some(okm_len)) {
        Ok(p) => p,
        Err(_) => return -4,
    };

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    match argon2.hash_password_into(password_slice, salt_slice, okm_slice) {
        Ok(()) => okm_len as i64,
        Err(_) => -5,
    }
}

/// Generate a random salt for KDF operations.
///
/// # Arguments
/// * `salt` - Output buffer for salt
/// * `salt_len` - Length of salt to generate (recommended: 16-32 bytes)
///
/// # Returns
/// * Number of bytes written on success
/// * -1 if salt is null
/// * -2 if random generation failed
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_kdf_generate_salt(salt: *mut u8, salt_len: usize) -> i64 {
    if salt.is_null() {
        return -1;
    }
    if salt_len == 0 {
        return 0;
    }

    let salt_slice = unsafe { std::slice::from_raw_parts_mut(salt, salt_len) };

    match getrandom::getrandom(salt_slice) {
        Ok(()) => salt_len as i64,
        Err(_) => -2,
    }
}

// =============================================================================
// Password Hashing
// =============================================================================
//
// This section provides password hashing for secure storage:
// - Argon2id: Memory-hard, recommended (RFC 9106)
// - bcrypt: Legacy but widely supported
//
// Password hashing produces self-describing hash strings (PHC format) that
// include the algorithm, parameters, salt, and hash. This is different from
// KDF which returns raw bytes.
//
// PHC format example:
// $argon2id$v=19$m=65536,t=3,p=4$c29tZXNhbHQ$RdescudvJCsgt3ub+b+dWRWJTmaaJObG

/// Password hashing algorithm identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum PasswordHashAlgorithm {
    /// Argon2id - recommended for new applications
    Argon2id = 0,
    /// bcrypt - legacy, widely supported
    Bcrypt = 1,
}

impl PasswordHashAlgorithm {
    /// Convert from i32 algorithm identifier.
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(PasswordHashAlgorithm::Argon2id),
            1 => Some(PasswordHashAlgorithm::Bcrypt),
            _ => None,
        }
    }
}

// -----------------------------------------------------------------------------
// Argon2id Password Hashing
// -----------------------------------------------------------------------------

/// Hash a password using Argon2id and return a PHC-format string.
///
/// The output is a self-describing string in PHC format that includes:
/// - Algorithm identifier ($argon2id$)
/// - Version (v=19)
/// - Parameters (m=memory, t=iterations, p=parallelism)
/// - Salt (base64)
/// - Hash (base64)
///
/// Example output:
/// $argon2id$v=19$m=65536,t=3,p=4$c29tZXNhbHQ$RdescudvJCsgt3ub+b+dWRWJTmaaJObG
///
/// # Arguments
/// * `password` - Password bytes
/// * `password_len` - Length of password
/// * `memory_kib` - Memory usage in KiB (e.g., 65536 for 64 MB)
/// * `iterations` - Number of iterations (time cost)
/// * `parallelism` - Degree of parallelism (lanes)
/// * `output` - Output buffer for PHC string (must be at least 128 bytes)
/// * `output_len` - Length of output buffer
///
/// # Returns
/// * Number of bytes written (excluding null terminator) on success
/// * -1 if password is null
/// * -2 if output is null
/// * -3 if output buffer is too small
/// * -4 if parameters are invalid
/// * -5 if hashing failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_password_hash_argon2id(
///     const uint8_t* password, size_t password_len,
///     uint32_t memory_kib, uint32_t iterations, uint32_t parallelism,
///     uint8_t* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_password_hash_argon2id(
    password: *const u8,
    password_len: usize,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    use argon2::password_hash::{PasswordHasher, SaltString};
    use argon2::{Algorithm, Argon2, Params, Version};

    if password.is_null() {
        return -1;
    }
    if output.is_null() {
        return -2;
    }
    if output_len < 97 {
        // Minimum PHC string length for Argon2id
        return -3;
    }

    let password_slice = unsafe { std::slice::from_raw_parts(password, password_len) };
    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, output_len) };

    // Generate random salt
    let salt = match SaltString::generate(&mut rand_core::OsRng) {
        salt => salt,
    };

    // Build Argon2 params
    let params = match Params::new(memory_kib, iterations, parallelism, None) {
        Ok(p) => p,
        Err(_) => return -4,
    };

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    // Hash the password
    let hash = match argon2.hash_password(password_slice, &salt) {
        Ok(h) => h,
        Err(_) => return -5,
    };

    let hash_string = hash.to_string();
    let hash_bytes = hash_string.as_bytes();

    if hash_bytes.len() >= output_len {
        return -3;
    }

    output_slice[..hash_bytes.len()].copy_from_slice(hash_bytes);
    output_slice[hash_bytes.len()] = 0; // Null terminate

    hash_bytes.len() as i64
}

/// Verify a password against an Argon2id hash.
///
/// # Arguments
/// * `password` - Password bytes to verify
/// * `password_len` - Length of password
/// * `hash` - PHC-format hash string (null-terminated)
///
/// # Returns
/// * 1 if password matches
/// * 0 if password does not match
/// * -1 if password is null
/// * -2 if hash is null
/// * -3 if hash is invalid format
///
/// # C ABI
/// ```c
/// int32_t arth_rt_password_verify_argon2id(
///     const uint8_t* password, size_t password_len,
///     const char* hash);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_password_verify_argon2id(
    password: *const u8,
    password_len: usize,
    hash: *const std::ffi::c_char,
) -> i32 {
    use argon2::Argon2;
    use argon2::password_hash::{PasswordHash, PasswordVerifier};

    if password.is_null() {
        return -1;
    }
    if hash.is_null() {
        return -2;
    }

    let password_slice = unsafe { std::slice::from_raw_parts(password, password_len) };
    let hash_cstr = unsafe { std::ffi::CStr::from_ptr(hash) };
    let hash_str = match hash_cstr.to_str() {
        Ok(s) => s,
        Err(_) => return -3,
    };

    let parsed_hash = match PasswordHash::new(hash_str) {
        Ok(h) => h,
        Err(_) => return -3,
    };

    let argon2 = Argon2::default();

    match argon2.verify_password(password_slice, &parsed_hash) {
        Ok(()) => 1,
        Err(_) => 0,
    }
}

// -----------------------------------------------------------------------------
// bcrypt Password Hashing
// -----------------------------------------------------------------------------

/// Hash a password using bcrypt.
///
/// The output is a bcrypt hash string in Modular Crypt Format:
/// $2b$<cost>$<22-char salt><31-char hash>
///
/// Example output:
/// $2b$12$LQv3c1yqBWVHxkd0LHAkCOYz6TtxMQJqhN8/X4.VTtYA7TYLD.VIa
///
/// # Arguments
/// * `password` - Password bytes (max 72 bytes, longer passwords are truncated)
/// * `password_len` - Length of password
/// * `cost` - Work factor (4-31, recommended: 12)
/// * `output` - Output buffer for hash string (must be at least 61 bytes)
/// * `output_len` - Length of output buffer
///
/// # Returns
/// * Number of bytes written (excluding null terminator) on success
/// * -1 if password is null
/// * -2 if output is null
/// * -3 if output buffer is too small
/// * -4 if cost is invalid (must be 4-31)
/// * -5 if hashing failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_password_hash_bcrypt(
///     const uint8_t* password, size_t password_len,
///     uint32_t cost,
///     uint8_t* output, size_t output_len);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_password_hash_bcrypt(
    password: *const u8,
    password_len: usize,
    cost: u32,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if password.is_null() {
        return -1;
    }
    if output.is_null() {
        return -2;
    }
    if output_len < 61 {
        // bcrypt hash is exactly 60 characters + null
        return -3;
    }
    if !(4..=31).contains(&cost) {
        return -4;
    }

    let password_slice = unsafe { std::slice::from_raw_parts(password, password_len) };
    let output_slice = unsafe { std::slice::from_raw_parts_mut(output, output_len) };

    // bcrypt only uses first 72 bytes
    let password_str = match std::str::from_utf8(password_slice) {
        Ok(s) => s,
        Err(_) => {
            // If not valid UTF-8, try to hash the raw bytes
            // bcrypt crate requires &str, so we need valid UTF-8
            // For non-UTF8 passwords, we'll base64 encode them
            return -5;
        }
    };

    let hash = match bcrypt::hash(password_str, cost) {
        Ok(h) => h,
        Err(_) => return -5,
    };

    let hash_bytes = hash.as_bytes();

    if hash_bytes.len() >= output_len {
        return -3;
    }

    output_slice[..hash_bytes.len()].copy_from_slice(hash_bytes);
    output_slice[hash_bytes.len()] = 0; // Null terminate

    hash_bytes.len() as i64
}

/// Verify a password against a bcrypt hash.
///
/// # Arguments
/// * `password` - Password bytes to verify
/// * `password_len` - Length of password
/// * `hash` - bcrypt hash string (null-terminated)
///
/// # Returns
/// * 1 if password matches
/// * 0 if password does not match
/// * -1 if password is null
/// * -2 if hash is null
/// * -3 if hash is invalid format
///
/// # C ABI
/// ```c
/// int32_t arth_rt_password_verify_bcrypt(
///     const uint8_t* password, size_t password_len,
///     const char* hash);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_password_verify_bcrypt(
    password: *const u8,
    password_len: usize,
    hash: *const std::ffi::c_char,
) -> i32 {
    if password.is_null() {
        return -1;
    }
    if hash.is_null() {
        return -2;
    }

    let password_slice = unsafe { std::slice::from_raw_parts(password, password_len) };
    let hash_cstr = unsafe { std::ffi::CStr::from_ptr(hash) };
    let hash_str = match hash_cstr.to_str() {
        Ok(s) => s,
        Err(_) => return -3,
    };

    let password_str = match std::str::from_utf8(password_slice) {
        Ok(s) => s,
        Err(_) => return -3,
    };

    match bcrypt::verify(password_str, hash_str) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(_) => -3,
    }
}

// -----------------------------------------------------------------------------
// Generic Password Hashing (Auto-detect algorithm)
// -----------------------------------------------------------------------------

/// Verify a password against a hash, auto-detecting the algorithm.
///
/// Supports:
/// - Argon2id ($argon2id$...)
/// - Argon2i ($argon2i$...)
/// - Argon2d ($argon2d$...)
/// - bcrypt ($2a$, $2b$, $2y$)
///
/// # Arguments
/// * `password` - Password bytes to verify
/// * `password_len` - Length of password
/// * `hash` - Hash string (null-terminated)
///
/// # Returns
/// * 1 if password matches
/// * 0 if password does not match
/// * -1 if password is null
/// * -2 if hash is null
/// * -3 if hash format is unrecognized
///
/// # C ABI
/// ```c
/// int32_t arth_rt_password_verify(
///     const uint8_t* password, size_t password_len,
///     const char* hash);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_password_verify(
    password: *const u8,
    password_len: usize,
    hash: *const std::ffi::c_char,
) -> i32 {
    if password.is_null() {
        return -1;
    }
    if hash.is_null() {
        return -2;
    }

    let hash_cstr = unsafe { std::ffi::CStr::from_ptr(hash) };
    let hash_str = match hash_cstr.to_str() {
        Ok(s) => s,
        Err(_) => return -3,
    };

    // Detect algorithm from hash prefix
    if hash_str.starts_with("$argon2") {
        arth_rt_password_verify_argon2id(password, password_len, hash)
    } else if hash_str.starts_with("$2a$")
        || hash_str.starts_with("$2b$")
        || hash_str.starts_with("$2y$")
    {
        arth_rt_password_verify_bcrypt(password, password_len, hash)
    } else {
        -3 // Unrecognized format
    }
}

/// Check if a password hash needs to be upgraded (rehashed with new parameters).
///
/// This is useful when you want to upgrade password hashes after:
/// - Increasing Argon2 memory/iterations
/// - Increasing bcrypt cost
/// - Migrating from bcrypt to Argon2
///
/// # Arguments
/// * `hash` - Hash string (null-terminated)
/// * `target_algorithm` - Desired algorithm (0=Argon2id, 1=bcrypt)
/// * `param1` - For Argon2: memory_kib; For bcrypt: cost
/// * `param2` - For Argon2: iterations; For bcrypt: unused (0)
/// * `param3` - For Argon2: parallelism; For bcrypt: unused (0)
///
/// # Returns
/// * 1 if hash needs upgrade
/// * 0 if hash is up to date
/// * -1 if hash is null
/// * -2 if hash format is unrecognized
/// * -3 if target algorithm is invalid
///
/// # C ABI
/// ```c
/// int32_t arth_rt_password_needs_rehash(
///     const char* hash,
///     int32_t target_algorithm,
///     uint32_t param1, uint32_t param2, uint32_t param3);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_password_needs_rehash(
    hash: *const std::ffi::c_char,
    target_algorithm: i32,
    param1: u32,
    param2: u32,
    param3: u32,
) -> i32 {
    use argon2::password_hash::PasswordHash;

    if hash.is_null() {
        return -1;
    }

    let hash_cstr = unsafe { std::ffi::CStr::from_ptr(hash) };
    let hash_str = match hash_cstr.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };

    let target_algo = match PasswordHashAlgorithm::from_i32(target_algorithm) {
        Some(a) => a,
        None => return -3,
    };

    // Check if algorithm matches target
    match target_algo {
        PasswordHashAlgorithm::Argon2id => {
            if !hash_str.starts_with("$argon2id$") {
                return 1; // Different algorithm, needs rehash
            }

            // Parse the hash to check parameters
            let parsed = match PasswordHash::new(hash_str) {
                Ok(h) => h,
                Err(_) => return -2,
            };

            // Extract parameters from the hash
            // Format: $argon2id$v=19$m=65536,t=3,p=4$...
            if parsed.params.iter().next().is_some() {
                // Parse m, t, p from the params
                let mut current_m = 0u32;
                let mut current_t = 0u32;
                let mut current_p = 0u32;

                for (key, value) in parsed.params.iter() {
                    match key.as_str() {
                        "m" => current_m = value.decimal().unwrap_or(0),
                        "t" => current_t = value.decimal().unwrap_or(0),
                        "p" => current_p = value.decimal().unwrap_or(0),
                        _ => {}
                    }
                }

                // Needs rehash if any parameter is below target
                if current_m < param1 || current_t < param2 || current_p < param3 {
                    return 1;
                }
            }

            0 // Up to date
        }
        PasswordHashAlgorithm::Bcrypt => {
            if !hash_str.starts_with("$2a$")
                && !hash_str.starts_with("$2b$")
                && !hash_str.starts_with("$2y$")
            {
                return 1; // Different algorithm, needs rehash
            }

            // Extract cost from bcrypt hash
            // Format: $2b$12$...
            let parts: Vec<&str> = hash_str.split('$').collect();
            if parts.len() < 4 {
                return -2;
            }

            let current_cost: u32 = match parts[2].parse() {
                Ok(c) => c,
                Err(_) => return -2,
            };

            if current_cost < param1 {
                return 1; // Needs upgrade
            }

            0 // Up to date
        }
    }
}

// =============================================================================
// Keyring - OS-Level Secure Secret Storage
// =============================================================================

/// Stores a password/secret in the OS keyring.
///
/// # Arguments
/// - `service`: Service name (null-terminated C string)
/// - `account`: Account/user name (null-terminated C string)
/// - `secret`: Secret data to store
/// - `secret_len`: Length of secret data
///
/// # Returns
/// - 0 on success
/// - -1 if service is null
/// - -2 if account is null
/// - -3 if secret is null
/// - -4 if keyring service is unavailable
/// - -5 if access denied
/// - -6 on other errors
#[cfg(feature = "keyring-store")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keyring_store(
    service: *const std::ffi::c_char,
    account: *const std::ffi::c_char,
    secret: *const u8,
    secret_len: usize,
) -> i32 {
    if service.is_null() {
        return -1;
    }
    if account.is_null() {
        return -2;
    }
    if secret.is_null() && secret_len > 0 {
        return -3;
    }

    let service_str = match unsafe { std::ffi::CStr::from_ptr(service) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let account_str = match unsafe { std::ffi::CStr::from_ptr(account) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };

    let secret_slice = if secret_len > 0 {
        unsafe { std::slice::from_raw_parts(secret, secret_len) }
    } else {
        &[]
    };

    // Convert secret to string for keyring (it stores passwords as strings)
    // For binary data, we use base64 encoding
    let secret_str = if secret_slice.iter().all(|&b| b >= 32 && b < 127) {
        // ASCII-safe, store as-is
        match std::str::from_utf8(secret_slice) {
            Ok(s) => s.to_string(),
            Err(_) => base64_encode_for_keyring(secret_slice),
        }
    } else {
        // Binary data, base64 encode with prefix
        format!("base64:{}", base64_encode_for_keyring(secret_slice))
    };

    let entry = match keyring::Entry::new(service_str, account_str) {
        Ok(e) => e,
        Err(keyring::Error::NoEntry) => return -4,
        Err(keyring::Error::NoStorageAccess(_)) => return -4,
        Err(keyring::Error::PlatformFailure(_)) => return -4,
        Err(_) => return -6,
    };

    match entry.set_password(&secret_str) {
        Ok(_) => 0,
        Err(keyring::Error::NoStorageAccess(_)) => -5,
        Err(keyring::Error::PlatformFailure(_)) => -4,
        Err(_) => -6,
    }
}

/// Helper function to base64 encode for keyring storage
#[cfg(feature = "keyring-store")]
fn base64_encode_for_keyring(data: &[u8]) -> String {
    const BASE64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    let chunks = data.chunks(3);

    for chunk in chunks {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;

        result.push(BASE64_CHARS[(b0 >> 2) & 0x3F] as char);
        result.push(BASE64_CHARS[((b0 << 4) | (b1 >> 4)) & 0x3F] as char);

        if chunk.len() > 1 {
            result.push(BASE64_CHARS[((b1 << 2) | (b2 >> 6)) & 0x3F] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(BASE64_CHARS[b2 & 0x3F] as char);
        } else {
            result.push('=');
        }
    }

    result
}

/// Helper function to base64 decode from keyring storage
#[cfg(feature = "keyring-store")]
fn base64_decode_for_keyring(data: &str) -> Option<Vec<u8>> {
    fn decode_char(c: char) -> Option<u8> {
        match c {
            'A'..='Z' => Some(c as u8 - b'A'),
            'a'..='z' => Some(c as u8 - b'a' + 26),
            '0'..='9' => Some(c as u8 - b'0' + 52),
            '+' => Some(62),
            '/' => Some(63),
            '=' => Some(0), // Padding
            _ => None,
        }
    }

    let chars: Vec<char> = data.chars().filter(|c| !c.is_whitespace()).collect();
    if chars.len() % 4 != 0 {
        return None;
    }

    let mut result = Vec::with_capacity(chars.len() * 3 / 4);

    for chunk in chars.chunks(4) {
        let b0 = decode_char(chunk[0])?;
        let b1 = decode_char(chunk[1])?;
        let b2 = decode_char(chunk[2])?;
        let b3 = decode_char(chunk[3])?;

        result.push((b0 << 2) | (b1 >> 4));

        if chunk[2] != '=' {
            result.push((b1 << 4) | (b2 >> 2));
        }

        if chunk[3] != '=' {
            result.push((b2 << 6) | b3);
        }
    }

    Some(result)
}

/// Loads a password/secret from the OS keyring.
///
/// # Arguments
/// - `service`: Service name (null-terminated C string)
/// - `account`: Account/user name (null-terminated C string)
/// - `output`: Output buffer for secret
/// - `output_len`: Size of output buffer
///
/// # Returns
/// - Positive length of secret on success
/// - 0 if entry not found
/// - -1 if service is null
/// - -2 if account is null
/// - -3 if buffer too small (returns needed size negated as -(size + 1000))
/// - -4 if keyring service is unavailable
/// - -5 if access denied
/// - -6 on other errors
#[cfg(feature = "keyring-store")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keyring_load(
    service: *const std::ffi::c_char,
    account: *const std::ffi::c_char,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if service.is_null() {
        return -1;
    }
    if account.is_null() {
        return -2;
    }

    let service_str = match unsafe { std::ffi::CStr::from_ptr(service) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let account_str = match unsafe { std::ffi::CStr::from_ptr(account) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };

    let entry = match keyring::Entry::new(service_str, account_str) {
        Ok(e) => e,
        Err(keyring::Error::NoEntry) => return 0,
        Err(keyring::Error::NoStorageAccess(_)) => return -4,
        Err(keyring::Error::PlatformFailure(_)) => return -4,
        Err(_) => return -6,
    };

    let password = match entry.get_password() {
        Ok(p) => p,
        Err(keyring::Error::NoEntry) => return 0,
        Err(keyring::Error::NoStorageAccess(_)) => return -5,
        Err(keyring::Error::PlatformFailure(_)) => return -4,
        Err(_) => return -6,
    };

    // Check if this is base64 encoded binary data
    let secret_bytes = if password.starts_with("base64:") {
        match base64_decode_for_keyring(&password[7..]) {
            Some(decoded) => decoded,
            None => password.as_bytes().to_vec(),
        }
    } else {
        password.as_bytes().to_vec()
    };

    // Check buffer size
    if output.is_null() {
        return secret_bytes.len() as i64;
    }

    if output_len < secret_bytes.len() {
        // Return needed size negated with offset to distinguish from other errors
        return -((secret_bytes.len() as i64) + 1000);
    }

    // Copy to output
    unsafe {
        std::ptr::copy_nonoverlapping(secret_bytes.as_ptr(), output, secret_bytes.len());
    }

    secret_bytes.len() as i64
}

/// Deletes an entry from the OS keyring.
///
/// # Arguments
/// - `service`: Service name (null-terminated C string)
/// - `account`: Account/user name (null-terminated C string)
///
/// # Returns
/// - 1 if entry was deleted
/// - 0 if entry not found
/// - -1 if service is null
/// - -2 if account is null
/// - -4 if keyring service is unavailable
/// - -5 if access denied
/// - -6 on other errors
#[cfg(feature = "keyring-store")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keyring_delete(
    service: *const std::ffi::c_char,
    account: *const std::ffi::c_char,
) -> i32 {
    if service.is_null() {
        return -1;
    }
    if account.is_null() {
        return -2;
    }

    let service_str = match unsafe { std::ffi::CStr::from_ptr(service) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let account_str = match unsafe { std::ffi::CStr::from_ptr(account) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };

    let entry = match keyring::Entry::new(service_str, account_str) {
        Ok(e) => e,
        Err(keyring::Error::NoEntry) => return 0,
        Err(keyring::Error::NoStorageAccess(_)) => return -4,
        Err(keyring::Error::PlatformFailure(_)) => return -4,
        Err(_) => return -6,
    };

    match entry.delete_credential() {
        Ok(_) => 1,
        Err(keyring::Error::NoEntry) => 0,
        Err(keyring::Error::NoStorageAccess(_)) => -5,
        Err(keyring::Error::PlatformFailure(_)) => -4,
        Err(_) => -6,
    }
}

/// Checks if an entry exists in the OS keyring.
///
/// # Arguments
/// - `service`: Service name (null-terminated C string)
/// - `account`: Account/user name (null-terminated C string)
///
/// # Returns
/// - 1 if entry exists
/// - 0 if entry does not exist
/// - -1 if service is null
/// - -2 if account is null
/// - -4 if keyring service is unavailable
/// - -5 if access denied
/// - -6 on other errors
#[cfg(feature = "keyring-store")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keyring_exists(
    service: *const std::ffi::c_char,
    account: *const std::ffi::c_char,
) -> i32 {
    if service.is_null() {
        return -1;
    }
    if account.is_null() {
        return -2;
    }

    let service_str = match unsafe { std::ffi::CStr::from_ptr(service) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let account_str = match unsafe { std::ffi::CStr::from_ptr(account) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };

    let entry = match keyring::Entry::new(service_str, account_str) {
        Ok(e) => e,
        Err(keyring::Error::NoEntry) => return 0,
        Err(keyring::Error::NoStorageAccess(_)) => return -4,
        Err(keyring::Error::PlatformFailure(_)) => return -4,
        Err(_) => return -6,
    };

    // Try to get the password to check if it exists
    match entry.get_password() {
        Ok(_) => 1,
        Err(keyring::Error::NoEntry) => 0,
        Err(keyring::Error::NoStorageAccess(_)) => -5,
        Err(keyring::Error::PlatformFailure(_)) => -4,
        Err(_) => -6,
    }
}

/// Stores a password string in the OS keyring (convenience for string passwords).
///
/// # Arguments
/// - `service`: Service name (null-terminated C string)
/// - `account`: Account/user name (null-terminated C string)
/// - `password`: Password string (null-terminated C string)
///
/// # Returns
/// - 0 on success
/// - -1 if service is null
/// - -2 if account is null
/// - -3 if password is null
/// - -4 if keyring service is unavailable
/// - -5 if access denied
/// - -6 on other errors
#[cfg(feature = "keyring-store")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keyring_store_password(
    service: *const std::ffi::c_char,
    account: *const std::ffi::c_char,
    password: *const std::ffi::c_char,
) -> i32 {
    if service.is_null() {
        return -1;
    }
    if account.is_null() {
        return -2;
    }
    if password.is_null() {
        return -3;
    }

    let service_str = match unsafe { std::ffi::CStr::from_ptr(service) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let account_str = match unsafe { std::ffi::CStr::from_ptr(account) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };

    let password_str = match unsafe { std::ffi::CStr::from_ptr(password) }.to_str() {
        Ok(s) => s,
        Err(_) => return -3,
    };

    let entry = match keyring::Entry::new(service_str, account_str) {
        Ok(e) => e,
        Err(keyring::Error::NoStorageAccess(_)) => return -4,
        Err(keyring::Error::PlatformFailure(_)) => return -4,
        Err(_) => return -6,
    };

    match entry.set_password(password_str) {
        Ok(_) => 0,
        Err(keyring::Error::NoStorageAccess(_)) => -5,
        Err(keyring::Error::PlatformFailure(_)) => -4,
        Err(_) => -6,
    }
}

/// Loads a password string from the OS keyring.
///
/// # Arguments
/// - `service`: Service name (null-terminated C string)
/// - `account`: Account/user name (null-terminated C string)
/// - `output`: Output buffer for password (null-terminated)
/// - `output_len`: Size of output buffer
///
/// # Returns
/// - Positive length of password (not including null terminator) on success
/// - 0 if entry not found
/// - -1 if service is null
/// - -2 if account is null
/// - -3 if buffer too small
/// - -4 if keyring service is unavailable
/// - -5 if access denied
/// - -6 on other errors
#[cfg(feature = "keyring-store")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keyring_load_password(
    service: *const std::ffi::c_char,
    account: *const std::ffi::c_char,
    output: *mut std::ffi::c_char,
    output_len: usize,
) -> i64 {
    if service.is_null() {
        return -1;
    }
    if account.is_null() {
        return -2;
    }

    let service_str = match unsafe { std::ffi::CStr::from_ptr(service) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let account_str = match unsafe { std::ffi::CStr::from_ptr(account) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };

    let entry = match keyring::Entry::new(service_str, account_str) {
        Ok(e) => e,
        Err(keyring::Error::NoEntry) => return 0,
        Err(keyring::Error::NoStorageAccess(_)) => return -4,
        Err(keyring::Error::PlatformFailure(_)) => return -4,
        Err(_) => return -6,
    };

    let password = match entry.get_password() {
        Ok(p) => p,
        Err(keyring::Error::NoEntry) => return 0,
        Err(keyring::Error::NoStorageAccess(_)) => return -5,
        Err(keyring::Error::PlatformFailure(_)) => return -4,
        Err(_) => return -6,
    };

    // Need space for password + null terminator
    let needed = password.len() + 1;

    if output.is_null() {
        return password.len() as i64;
    }

    if output_len < needed {
        return -3;
    }

    // Copy password and add null terminator
    unsafe {
        std::ptr::copy_nonoverlapping(password.as_ptr(), output as *mut u8, password.len());
        *output.add(password.len()) = 0;
    }

    password.len() as i64
}

/// Returns the platform name for the keyring backend.
///
/// # Arguments
/// - `output`: Output buffer
/// - `output_len`: Size of output buffer
///
/// # Returns
/// - Length of platform name on success
/// - -1 if buffer too small
#[cfg(feature = "keyring-store")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keyring_platform(output: *mut u8, output_len: usize) -> i64 {
    #[cfg(target_os = "macos")]
    let platform = "macos-keychain";

    #[cfg(target_os = "windows")]
    let platform = "windows-credential-manager";

    #[cfg(target_os = "linux")]
    let platform = "linux-secret-service";

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let platform = "unknown";

    if output.is_null() {
        return platform.len() as i64;
    }

    if output_len < platform.len() {
        return -1;
    }

    unsafe {
        std::ptr::copy_nonoverlapping(platform.as_ptr(), output, platform.len());
    }

    platform.len() as i64
}

/// Checks if the keyring service is available on this platform.
///
/// # Returns
/// - 1 if available
/// - 0 if not available
#[cfg(feature = "keyring-store")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keyring_is_available() -> i32 {
    // Try to create a test entry to check availability
    // Use a unique test service/account that won't conflict
    match keyring::Entry::new("__arth_keyring_test__", "__availability_check__") {
        Ok(_) => 1,
        Err(_) => 0,
    }
}

// =============================================================================
// Key Store - In-Memory Secure Key Storage
// =============================================================================

use std::sync::RwLock;

/// A key entry stored in the key store.
/// The key data is stored in secure memory and zeroed on drop.
struct KeyStoreEntry {
    /// Algorithm identifier (0=AES-128-GCM, 1=AES-256-GCM, 2=ChaCha20-Poly1305)
    algorithm: i32,
    /// The key bytes (copied from secure memory)
    data: Vec<u8>,
}

impl Drop for KeyStoreEntry {
    fn drop(&mut self) {
        // Securely zero the key data
        for byte in self.data.iter_mut() {
            unsafe {
                std::ptr::write_volatile(byte, 0);
            }
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

/// In-memory key store with thread-safe access.
struct KeyStore {
    /// Keys stored by ID
    keys: HashMap<String, KeyStoreEntry>,
}

impl KeyStore {
    fn new() -> Self {
        KeyStore {
            keys: HashMap::new(),
        }
    }

    fn store(&mut self, key_id: &str, algorithm: i32, data: &[u8]) -> Result<(), &'static str> {
        if key_id.is_empty() {
            return Err("Key ID cannot be empty");
        }
        let entry = KeyStoreEntry {
            algorithm,
            data: data.to_vec(),
        };
        self.keys.insert(key_id.to_string(), entry);
        Ok(())
    }

    fn load(&self, key_id: &str) -> Option<&KeyStoreEntry> {
        self.keys.get(key_id)
    }

    fn delete(&mut self, key_id: &str) -> bool {
        self.keys.remove(key_id).is_some()
    }

    fn exists(&self, key_id: &str) -> bool {
        self.keys.contains_key(key_id)
    }

    fn list(&self) -> Vec<String> {
        self.keys.keys().cloned().collect()
    }

    fn clear(&mut self) {
        self.keys.clear();
    }

    fn count(&self) -> usize {
        self.keys.len()
    }
}

lazy_static! {
    /// Global registry of key stores
    static ref KEY_STORES: RwLock<HashMap<i64, RwLock<KeyStore>>> = RwLock::new(HashMap::new());
}

/// Creates a new in-memory key store.
///
/// # Returns
/// - Positive handle on success
/// - -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keystore_create() -> i64 {
    let handle = new_handle();
    let store = KeyStore::new();

    match KEY_STORES.write() {
        Ok(mut stores) => {
            stores.insert(handle, RwLock::new(store));
            handle
        }
        Err(_) => -1,
    }
}

/// Destroys a key store, securely wiping all keys.
///
/// # Arguments
/// - `handle`: The key store handle
///
/// # Returns
/// - 0 on success
/// - -1 if handle is invalid
/// - -2 if lock acquisition fails
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keystore_destroy(handle: i64) -> i32 {
    match KEY_STORES.write() {
        Ok(mut stores) => {
            if stores.remove(&handle).is_some() {
                0
            } else {
                -1 // Invalid handle
            }
        }
        Err(_) => -2,
    }
}

/// Stores a key in the key store.
///
/// # Arguments
/// - `handle`: The key store handle
/// - `key_id`: The key identifier (null-terminated C string)
/// - `algorithm`: The algorithm identifier
/// - `key_data`: Pointer to key bytes
/// - `key_len`: Length of key bytes
///
/// # Returns
/// - 0 on success
/// - -1 if handle is invalid
/// - -2 if key_id is null or empty
/// - -3 if key_data is null
/// - -4 if lock acquisition fails
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keystore_store(
    handle: i64,
    key_id: *const std::ffi::c_char,
    algorithm: i32,
    key_data: *const u8,
    key_len: usize,
) -> i32 {
    if key_id.is_null() {
        return -2;
    }
    if key_data.is_null() && key_len > 0 {
        return -3;
    }

    let key_id_str = match unsafe { std::ffi::CStr::from_ptr(key_id) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };

    if key_id_str.is_empty() {
        return -2;
    }

    let key_slice = if key_len > 0 {
        unsafe { std::slice::from_raw_parts(key_data, key_len) }
    } else {
        &[]
    };

    let stores = match KEY_STORES.read() {
        Ok(s) => s,
        Err(_) => return -4,
    };

    let store = match stores.get(&handle) {
        Some(s) => s,
        None => return -1,
    };

    match store.write() {
        Ok(mut s) => match s.store(key_id_str, algorithm, key_slice) {
            Ok(_) => 0,
            Err(_) => -2,
        },
        Err(_) => -4,
    }
}

/// Loads a key from the key store.
///
/// # Arguments
/// - `handle`: The key store handle
/// - `key_id`: The key identifier (null-terminated C string)
/// - `algorithm_out`: Output pointer for algorithm (can be null)
/// - `key_data_out`: Output buffer for key bytes
/// - `key_data_out_len`: Size of output buffer
///
/// # Returns
/// - Positive key length on success
/// - 0 if key not found
/// - -1 if handle is invalid
/// - -2 if key_id is null
/// - -3 if buffer too small (needed size in algorithm_out if not null)
/// - -4 if lock acquisition fails
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keystore_load(
    handle: i64,
    key_id: *const std::ffi::c_char,
    algorithm_out: *mut i32,
    key_data_out: *mut u8,
    key_data_out_len: usize,
) -> i64 {
    if key_id.is_null() {
        return -2;
    }

    let key_id_str = match unsafe { std::ffi::CStr::from_ptr(key_id) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };

    let stores = match KEY_STORES.read() {
        Ok(s) => s,
        Err(_) => return -4,
    };

    let store = match stores.get(&handle) {
        Some(s) => s,
        None => return -1,
    };

    match store.read() {
        Ok(s) => match s.load(key_id_str) {
            Some(entry) => {
                // Check buffer size
                if !key_data_out.is_null() && key_data_out_len < entry.data.len() {
                    // Buffer too small, return needed size
                    if !algorithm_out.is_null() {
                        unsafe { *algorithm_out = entry.data.len() as i32 };
                    }
                    return -3;
                }

                // Write algorithm if requested
                if !algorithm_out.is_null() {
                    unsafe { *algorithm_out = entry.algorithm };
                }

                // Copy key data if buffer provided
                if !key_data_out.is_null() {
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            entry.data.as_ptr(),
                            key_data_out,
                            entry.data.len(),
                        );
                    }
                }

                entry.data.len() as i64
            }
            None => 0, // Key not found
        },
        Err(_) => -4,
    }
}

/// Deletes a key from the key store.
///
/// # Arguments
/// - `handle`: The key store handle
/// - `key_id`: The key identifier (null-terminated C string)
///
/// # Returns
/// - 1 if key was deleted
/// - 0 if key not found
/// - -1 if handle is invalid
/// - -2 if key_id is null
/// - -4 if lock acquisition fails
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keystore_delete(handle: i64, key_id: *const std::ffi::c_char) -> i32 {
    if key_id.is_null() {
        return -2;
    }

    let key_id_str = match unsafe { std::ffi::CStr::from_ptr(key_id) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };

    let stores = match KEY_STORES.read() {
        Ok(s) => s,
        Err(_) => return -4,
    };

    let store = match stores.get(&handle) {
        Some(s) => s,
        None => return -1,
    };

    match store.write() {
        Ok(mut s) => {
            if s.delete(key_id_str) {
                1
            } else {
                0
            }
        }
        Err(_) => -4,
    }
}

/// Checks if a key exists in the key store.
///
/// # Arguments
/// - `handle`: The key store handle
/// - `key_id`: The key identifier (null-terminated C string)
///
/// # Returns
/// - 1 if key exists
/// - 0 if key does not exist
/// - -1 if handle is invalid
/// - -2 if key_id is null
/// - -4 if lock acquisition fails
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keystore_exists(handle: i64, key_id: *const std::ffi::c_char) -> i32 {
    if key_id.is_null() {
        return -2;
    }

    let key_id_str = match unsafe { std::ffi::CStr::from_ptr(key_id) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };

    let stores = match KEY_STORES.read() {
        Ok(s) => s,
        Err(_) => return -4,
    };

    let store = match stores.get(&handle) {
        Some(s) => s,
        None => return -1,
    };

    match store.read() {
        Ok(s) => {
            if s.exists(key_id_str) {
                1
            } else {
                0
            }
        }
        Err(_) => -4,
    }
}

/// Returns the number of keys in the key store.
///
/// # Arguments
/// - `handle`: The key store handle
///
/// # Returns
/// - Non-negative count on success
/// - -1 if handle is invalid
/// - -4 if lock acquisition fails
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keystore_count(handle: i64) -> i64 {
    let stores = match KEY_STORES.read() {
        Ok(s) => s,
        Err(_) => return -4,
    };

    let store = match stores.get(&handle) {
        Some(s) => s,
        None => return -1,
    };

    match store.read() {
        Ok(s) => s.count() as i64,
        Err(_) => -4,
    }
}

/// Lists all key IDs in the key store.
///
/// This function writes key IDs as null-terminated strings separated by null bytes.
/// The output buffer should be at least the size returned by arth_rt_keystore_list_size().
///
/// # Arguments
/// - `handle`: The key store handle
/// - `output`: Output buffer for key IDs
/// - `output_len`: Size of output buffer
///
/// # Returns
/// - Number of keys on success
/// - -1 if handle is invalid
/// - -3 if buffer too small
/// - -4 if lock acquisition fails
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keystore_list(handle: i64, output: *mut u8, output_len: usize) -> i64 {
    let stores = match KEY_STORES.read() {
        Ok(s) => s,
        Err(_) => return -4,
    };

    let store = match stores.get(&handle) {
        Some(s) => s,
        None => return -1,
    };

    match store.read() {
        Ok(s) => {
            let keys = s.list();

            // Calculate needed size: sum of (key_len + 1) for each key
            let needed_size: usize = keys.iter().map(|k| k.len() + 1).sum();

            if output.is_null() {
                return needed_size as i64;
            }

            if output_len < needed_size {
                return -3;
            }

            // Write keys as null-terminated strings
            let mut offset = 0;
            for key in &keys {
                unsafe {
                    std::ptr::copy_nonoverlapping(key.as_ptr(), output.add(offset), key.len());
                    *output.add(offset + key.len()) = 0; // Null terminator
                }
                offset += key.len() + 1;
            }

            keys.len() as i64
        }
        Err(_) => -4,
    }
}

/// Returns the total size needed for list output buffer.
///
/// # Arguments
/// - `handle`: The key store handle
///
/// # Returns
/// - Non-negative size on success
/// - -1 if handle is invalid
/// - -4 if lock acquisition fails
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keystore_list_size(handle: i64) -> i64 {
    let stores = match KEY_STORES.read() {
        Ok(s) => s,
        Err(_) => return -4,
    };

    let store = match stores.get(&handle) {
        Some(s) => s,
        None => return -1,
    };

    match store.read() {
        Ok(s) => {
            let keys = s.list();
            let size: usize = keys.iter().map(|k| k.len() + 1).sum();
            size as i64
        }
        Err(_) => -4,
    }
}

/// Clears all keys from the key store.
///
/// # Arguments
/// - `handle`: The key store handle
///
/// # Returns
/// - Number of keys cleared on success
/// - -1 if handle is invalid
/// - -4 if lock acquisition fails
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_keystore_clear(handle: i64) -> i64 {
    let stores = match KEY_STORES.read() {
        Ok(s) => s,
        Err(_) => return -4,
    };

    let store = match stores.get(&handle) {
        Some(s) => s,
        None => return -1,
    };

    match store.write() {
        Ok(mut s) => {
            let count = s.count();
            s.clear();
            count as i64
        }
        Err(_) => -4,
    }
}

// =============================================================================
// Async Crypto Operations
// =============================================================================
//
// This section provides async versions of CPU-intensive crypto operations.
// These run in a background thread pool and return immediately with a task ID.
// The caller can then poll for completion and retrieve results.

use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::thread;

/// Status of an async crypto task
#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
enum AsyncTaskStatus {
    /// Task is running
    Running = 0,
    /// Task completed successfully
    Completed = 1,
    /// Task failed with error
    Failed = 2,
}

/// Result type for async crypto operations
enum AsyncTaskResult {
    /// Password hash result (PHC string)
    PasswordHash(String),
    /// KDF result (derived key bytes)
    DerivedKey(Vec<u8>),
    /// Error message
    Error(String),
}

/// An async crypto task
struct AsyncCryptoTask {
    /// Task status
    status: AtomicU8,
    /// Result (only valid when status is Completed or Failed)
    result: Mutex<Option<AsyncTaskResult>>,
}

impl AsyncCryptoTask {
    fn new() -> Self {
        AsyncCryptoTask {
            status: AtomicU8::new(AsyncTaskStatus::Running as u8),
            result: Mutex::new(None),
        }
    }

    fn set_result(&self, result: AsyncTaskResult) {
        let status = match &result {
            AsyncTaskResult::Error(_) => AsyncTaskStatus::Failed,
            _ => AsyncTaskStatus::Completed,
        };
        if let Ok(mut guard) = self.result.lock() {
            *guard = Some(result);
        }
        self.status.store(status as u8, Ordering::Release);
    }

    fn get_status(&self) -> AsyncTaskStatus {
        match self.status.load(Ordering::Acquire) {
            0 => AsyncTaskStatus::Running,
            1 => AsyncTaskStatus::Completed,
            _ => AsyncTaskStatus::Failed,
        }
    }
}

lazy_static! {
    /// Global registry of async crypto tasks
    static ref ASYNC_TASKS: RwLock<HashMap<i64, std::sync::Arc<AsyncCryptoTask>>> = RwLock::new(HashMap::new());

    /// Counter for generating async task IDs
    static ref ASYNC_TASK_COUNTER: AtomicI64 = AtomicI64::new(1);
}

fn new_async_task_id() -> i64 {
    ASYNC_TASK_COUNTER.fetch_add(1, Ordering::SeqCst)
}

// -----------------------------------------------------------------------------
// Async Password Hashing - Argon2id
// -----------------------------------------------------------------------------

/// Starts an async Argon2id password hash operation.
///
/// The operation runs in a background thread and returns immediately.
/// Use `arth_rt_async_task_poll` to check completion status.
/// Use `arth_rt_async_task_result_string` to get the result.
///
/// # Arguments
/// * `password` - Password bytes
/// * `password_len` - Length of password
/// * `memory_kib` - Memory in KiB (e.g., 65536 for 64MB)
/// * `iterations` - Number of iterations
/// * `parallelism` - Degree of parallelism
///
/// # Returns
/// * Task ID (positive) on success
/// * -1 if password is null
/// * -2 if spawning thread fails
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_password_hash_argon2id_async(
    password: *const u8,
    password_len: usize,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
) -> i64 {
    if password.is_null() {
        return -1;
    }

    // Copy password to owned data for thread safety
    let password_vec = unsafe { std::slice::from_raw_parts(password, password_len).to_vec() };

    let task = std::sync::Arc::new(AsyncCryptoTask::new());
    let task_id = new_async_task_id();

    // Register task
    if let Ok(mut tasks) = ASYNC_TASKS.write() {
        tasks.insert(task_id, task.clone());
    } else {
        return -2;
    }

    // Spawn background thread
    let task_clone = task.clone();
    if thread::Builder::new()
        .name(format!("crypto-argon2-{}", task_id))
        .spawn(move || {
            use argon2::password_hash::{PasswordHasher, SaltString};
            use argon2::{Algorithm, Argon2, Params, Version};

            // Generate salt
            let salt = SaltString::generate(&mut rand_core::OsRng);

            // Build params
            let params = match Params::new(memory_kib, iterations, parallelism, None) {
                Ok(p) => p,
                Err(e) => {
                    task_clone.set_result(AsyncTaskResult::Error(format!(
                        "Invalid Argon2 params: {}",
                        e
                    )));
                    return;
                }
            };

            let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

            // Hash password
            match argon2.hash_password(&password_vec, &salt) {
                Ok(hash) => {
                    task_clone.set_result(AsyncTaskResult::PasswordHash(hash.to_string()));
                }
                Err(e) => {
                    task_clone.set_result(AsyncTaskResult::Error(format!(
                        "Argon2id hash failed: {}",
                        e
                    )));
                }
            }
        })
        .is_err()
    {
        // Failed to spawn thread - cleanup
        if let Ok(mut tasks) = ASYNC_TASKS.write() {
            tasks.remove(&task_id);
        }
        return -2;
    }

    task_id
}

/// Starts an async bcrypt password hash operation.
///
/// # Arguments
/// * `password` - Password bytes (max 72 bytes)
/// * `password_len` - Length of password
/// * `cost` - Work factor (4-31)
///
/// # Returns
/// * Task ID (positive) on success
/// * -1 if password is null
/// * -2 if spawning thread fails
/// * -3 if cost is invalid
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_password_hash_bcrypt_async(
    password: *const u8,
    password_len: usize,
    cost: u32,
) -> i64 {
    if password.is_null() {
        return -1;
    }
    if !(4..=31).contains(&cost) {
        return -3;
    }

    // Copy password (bcrypt truncates at 72 bytes)
    let len = std::cmp::min(password_len, 72);
    let password_vec = unsafe { std::slice::from_raw_parts(password, len).to_vec() };

    let task = std::sync::Arc::new(AsyncCryptoTask::new());
    let task_id = new_async_task_id();

    // Register task
    if let Ok(mut tasks) = ASYNC_TASKS.write() {
        tasks.insert(task_id, task.clone());
    } else {
        return -2;
    }

    // Spawn background thread
    let task_clone = task.clone();
    if thread::Builder::new()
        .name(format!("crypto-bcrypt-{}", task_id))
        .spawn(move || match bcrypt::hash(&password_vec, cost) {
            Ok(hash) => {
                task_clone.set_result(AsyncTaskResult::PasswordHash(hash));
            }
            Err(e) => {
                task_clone.set_result(AsyncTaskResult::Error(format!("bcrypt hash failed: {}", e)));
            }
        })
        .is_err()
    {
        if let Ok(mut tasks) = ASYNC_TASKS.write() {
            tasks.remove(&task_id);
        }
        return -2;
    }

    task_id
}

// -----------------------------------------------------------------------------
// Async Key Derivation
// -----------------------------------------------------------------------------

/// Starts an async PBKDF2 key derivation operation.
///
/// # Arguments
/// * `algorithm` - 0 for SHA-256, 1 for SHA-512
/// * `password` - Password bytes
/// * `password_len` - Length of password
/// * `salt` - Salt bytes
/// * `salt_len` - Length of salt
/// * `iterations` - Number of iterations
/// * `output_len` - Desired output length
///
/// # Returns
/// * Task ID (positive) on success
/// * -1 if password is null
/// * -2 if spawning thread fails
/// * -3 if salt is null
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pbkdf2_derive_async(
    algorithm: i32,
    password: *const u8,
    password_len: usize,
    salt: *const u8,
    salt_len: usize,
    iterations: u32,
    output_len: usize,
) -> i64 {
    if password.is_null() {
        return -1;
    }
    if salt.is_null() {
        return -3;
    }

    let password_vec = unsafe { std::slice::from_raw_parts(password, password_len).to_vec() };
    let salt_vec = unsafe { std::slice::from_raw_parts(salt, salt_len).to_vec() };

    let task = std::sync::Arc::new(AsyncCryptoTask::new());
    let task_id = new_async_task_id();

    if let Ok(mut tasks) = ASYNC_TASKS.write() {
        tasks.insert(task_id, task.clone());
    } else {
        return -2;
    }

    let task_clone = task.clone();
    if thread::Builder::new()
        .name(format!("crypto-pbkdf2-{}", task_id))
        .spawn(move || {
            use pbkdf2::pbkdf2_hmac;
            use sha2::{Sha256, Sha512};

            let mut output = vec![0u8; output_len];

            match algorithm {
                3 => {
                    // PBKDF2-HMAC-SHA256 (matches sync function)
                    pbkdf2_hmac::<Sha256>(&password_vec, &salt_vec, iterations, &mut output);
                    task_clone.set_result(AsyncTaskResult::DerivedKey(output));
                }
                4 => {
                    // PBKDF2-HMAC-SHA512 (matches sync function)
                    pbkdf2_hmac::<Sha512>(&password_vec, &salt_vec, iterations, &mut output);
                    task_clone.set_result(AsyncTaskResult::DerivedKey(output));
                }
                _ => {
                    task_clone.set_result(AsyncTaskResult::Error(
                        "Unknown PBKDF2 algorithm".to_string(),
                    ));
                }
            }
        })
        .is_err()
    {
        if let Ok(mut tasks) = ASYNC_TASKS.write() {
            tasks.remove(&task_id);
        }
        return -2;
    }

    task_id
}

/// Starts an async Argon2id key derivation operation.
///
/// # Arguments
/// * `password` - Password bytes
/// * `password_len` - Length of password
/// * `salt` - Salt bytes
/// * `salt_len` - Length of salt
/// * `memory_kib` - Memory in KiB
/// * `iterations` - Number of iterations
/// * `parallelism` - Degree of parallelism
/// * `output_len` - Desired output length
///
/// # Returns
/// * Task ID (positive) on success
/// * -1 if password is null
/// * -2 if spawning thread fails
/// * -3 if salt is null
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_argon2_derive_async(
    password: *const u8,
    password_len: usize,
    salt: *const u8,
    salt_len: usize,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    output_len: usize,
) -> i64 {
    if password.is_null() {
        return -1;
    }
    if salt.is_null() {
        return -3;
    }

    let password_vec = unsafe { std::slice::from_raw_parts(password, password_len).to_vec() };
    let salt_vec = unsafe { std::slice::from_raw_parts(salt, salt_len).to_vec() };

    let task = std::sync::Arc::new(AsyncCryptoTask::new());
    let task_id = new_async_task_id();

    if let Ok(mut tasks) = ASYNC_TASKS.write() {
        tasks.insert(task_id, task.clone());
    } else {
        return -2;
    }

    let task_clone = task.clone();
    if thread::Builder::new()
        .name(format!("crypto-argon2kdf-{}", task_id))
        .spawn(move || {
            use argon2::{Algorithm, Argon2, Params, Version};

            let params = match Params::new(memory_kib, iterations, parallelism, Some(output_len)) {
                Ok(p) => p,
                Err(e) => {
                    task_clone.set_result(AsyncTaskResult::Error(format!(
                        "Invalid Argon2 params: {}",
                        e
                    )));
                    return;
                }
            };

            let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
            let mut output = vec![0u8; output_len];

            match argon2.hash_password_into(&password_vec, &salt_vec, &mut output) {
                Ok(()) => {
                    task_clone.set_result(AsyncTaskResult::DerivedKey(output));
                }
                Err(e) => {
                    task_clone.set_result(AsyncTaskResult::Error(format!(
                        "Argon2id derivation failed: {}",
                        e
                    )));
                }
            }
        })
        .is_err()
    {
        if let Ok(mut tasks) = ASYNC_TASKS.write() {
            tasks.remove(&task_id);
        }
        return -2;
    }

    task_id
}

// -----------------------------------------------------------------------------
// Async Task Management
// -----------------------------------------------------------------------------

/// Polls the status of an async crypto task.
///
/// # Arguments
/// * `task_id` - The task ID returned from an async operation
///
/// # Returns
/// * 0 if task is still running
/// * 1 if task completed successfully
/// * 2 if task failed
/// * -1 if task ID is invalid
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_async_task_poll(task_id: i64) -> i32 {
    let tasks = match ASYNC_TASKS.read() {
        Ok(t) => t,
        Err(_) => return -1,
    };

    match tasks.get(&task_id) {
        Some(task) => task.get_status() as i32,
        None => -1,
    }
}

/// Gets the string result of a completed async task (for password hashing).
///
/// # Arguments
/// * `task_id` - The task ID
/// * `output` - Buffer to write the result string
/// * `output_len` - Length of output buffer
///
/// # Returns
/// * Length of result string on success
/// * -1 if task ID is invalid
/// * -2 if task is still running
/// * -3 if output buffer is too small
/// * -4 if task failed (use `arth_rt_async_task_error` to get error message)
/// * -5 if result is not a string
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_async_task_result_string(
    task_id: i64,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if output.is_null() {
        return -3;
    }

    let tasks = match ASYNC_TASKS.read() {
        Ok(t) => t,
        Err(_) => return -1,
    };

    let task = match tasks.get(&task_id) {
        Some(t) => t,
        None => return -1,
    };

    if task.get_status() == AsyncTaskStatus::Running {
        return -2;
    }

    let result = match task.result.lock() {
        Ok(r) => r,
        Err(_) => return -1,
    };

    match result.as_ref() {
        Some(AsyncTaskResult::PasswordHash(hash)) => {
            let bytes = hash.as_bytes();
            if bytes.len() >= output_len {
                return -3;
            }
            let output_slice = unsafe { std::slice::from_raw_parts_mut(output, output_len) };
            output_slice[..bytes.len()].copy_from_slice(bytes);
            output_slice[bytes.len()] = 0; // Null terminate
            bytes.len() as i64
        }
        Some(AsyncTaskResult::Error(_)) => -4,
        Some(AsyncTaskResult::DerivedKey(_)) => -5,
        None => -1,
    }
}

/// Gets the bytes result of a completed async task (for key derivation).
///
/// # Arguments
/// * `task_id` - The task ID
/// * `output` - Buffer to write the result bytes
/// * `output_len` - Length of output buffer
///
/// # Returns
/// * Length of result on success
/// * -1 if task ID is invalid
/// * -2 if task is still running
/// * -3 if output buffer is too small
/// * -4 if task failed
/// * -5 if result is not bytes
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_async_task_result_bytes(
    task_id: i64,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if output.is_null() {
        return -3;
    }

    let tasks = match ASYNC_TASKS.read() {
        Ok(t) => t,
        Err(_) => return -1,
    };

    let task = match tasks.get(&task_id) {
        Some(t) => t,
        None => return -1,
    };

    if task.get_status() == AsyncTaskStatus::Running {
        return -2;
    }

    let result = match task.result.lock() {
        Ok(r) => r,
        Err(_) => return -1,
    };

    match result.as_ref() {
        Some(AsyncTaskResult::DerivedKey(bytes)) => {
            if bytes.len() > output_len {
                return -3;
            }
            let output_slice = unsafe { std::slice::from_raw_parts_mut(output, output_len) };
            output_slice[..bytes.len()].copy_from_slice(bytes);
            bytes.len() as i64
        }
        Some(AsyncTaskResult::Error(_)) => -4,
        Some(AsyncTaskResult::PasswordHash(_)) => -5,
        None => -1,
    }
}

/// Gets the error message from a failed async task.
///
/// # Arguments
/// * `task_id` - The task ID
/// * `output` - Buffer to write the error message
/// * `output_len` - Length of output buffer
///
/// # Returns
/// * Length of error message on success
/// * -1 if task ID is invalid
/// * -2 if task is still running
/// * -3 if output buffer is too small
/// * -5 if task did not fail
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_async_task_error(
    task_id: i64,
    output: *mut u8,
    output_len: usize,
) -> i64 {
    if output.is_null() {
        return -3;
    }

    let tasks = match ASYNC_TASKS.read() {
        Ok(t) => t,
        Err(_) => return -1,
    };

    let task = match tasks.get(&task_id) {
        Some(t) => t,
        None => return -1,
    };

    if task.get_status() == AsyncTaskStatus::Running {
        return -2;
    }

    let result = match task.result.lock() {
        Ok(r) => r,
        Err(_) => return -1,
    };

    match result.as_ref() {
        Some(AsyncTaskResult::Error(msg)) => {
            let bytes = msg.as_bytes();
            if bytes.len() >= output_len {
                return -3;
            }
            let output_slice = unsafe { std::slice::from_raw_parts_mut(output, output_len) };
            output_slice[..bytes.len()].copy_from_slice(bytes);
            output_slice[bytes.len()] = 0;
            bytes.len() as i64
        }
        _ => -5,
    }
}

/// Removes a completed async task from the registry.
///
/// Call this after retrieving the result to free resources.
///
/// # Arguments
/// * `task_id` - The task ID
///
/// # Returns
/// * 0 on success
/// * -1 if task ID is invalid
/// * -2 if task is still running
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_async_task_cleanup(task_id: i64) -> i32 {
    // Check if still running first
    {
        let tasks = match ASYNC_TASKS.read() {
            Ok(t) => t,
            Err(_) => return -1,
        };

        match tasks.get(&task_id) {
            Some(task) => {
                if task.get_status() == AsyncTaskStatus::Running {
                    return -2;
                }
            }
            None => return -1,
        }
    }

    // Remove the task
    let mut tasks = match ASYNC_TASKS.write() {
        Ok(t) => t,
        Err(_) => return -1,
    };

    match tasks.remove(&task_id) {
        Some(_) => 0,
        None => -1,
    }
}

/// Waits for an async task to complete (blocking).
///
/// This spins with yields until the task completes, with a timeout.
///
/// # Arguments
/// * `task_id` - The task ID
/// * `timeout_ms` - Maximum time to wait in milliseconds (0 for no timeout)
///
/// # Returns
/// * 1 if task completed successfully
/// * 2 if task failed
/// * -1 if task ID is invalid
/// * -3 if timeout expired
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_async_task_wait(task_id: i64, timeout_ms: u64) -> i32 {
    let start = std::time::Instant::now();
    let timeout = if timeout_ms > 0 {
        Some(std::time::Duration::from_millis(timeout_ms))
    } else {
        None
    };

    loop {
        let status = arth_rt_async_task_poll(task_id);

        match status {
            0 => {
                // Still running - check timeout
                if let Some(t) = timeout {
                    if start.elapsed() >= t {
                        return -3;
                    }
                }
                // Yield to other threads
                thread::yield_now();
                // Small sleep to avoid busy spinning
                thread::sleep(std::time::Duration::from_micros(100));
            }
            1 | 2 => return status,
            _ => return status, // Error (-1)
        }
    }
}

// =============================================================================
// Nonce-Misuse Detection
// =============================================================================
//
// Nonce tracking state for detecting nonce reuse.
//
// For AES-GCM and ChaCha20-Poly1305, reusing a nonce with the same key is
// catastrophic - it can lead to plaintext recovery and forgery attacks.
// This module provides optional nonce tracking to detect and warn about
// potential nonce reuse.
//
// Design:
// - Uses SHA-256 to compute a fingerprint of the key (avoids storing raw keys)
// - Stores (key_fingerprint, nonce) pairs in a thread-safe hash set
// - Tracking is opt-in and disabled by default
// - Warnings can be emitted to stderr when reuse is detected
//
// Usage:
// 1. Call `arth_rt_aead_nonce_tracking_enable(1)` to enable tracking
// 2. Before encrypting, call `arth_rt_aead_nonce_check(...)` to check for reuse
// 3. After encrypting, call `arth_rt_aead_nonce_mark_used(...)` to record usage
// 4. Optionally call `arth_rt_aead_nonce_tracking_clear()` to reset

use std::sync::atomic::AtomicBool;

lazy_static! {
    /// Set of (key_fingerprint, nonce) pairs that have been used.
    /// The key fingerprint is a 32-byte SHA-256 hash of the key.
    /// The nonce is stored as a Vec<u8> since different algorithms have different nonce sizes.
    static ref NONCE_REGISTRY: Mutex<HashSet<([u8; 32], Vec<u8>)>> = Mutex::new(HashSet::new());
}

/// Whether nonce tracking is currently enabled
static NONCE_TRACKING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether to emit warnings to stderr when nonce reuse is detected
static NONCE_WARN_STDERR: AtomicBool = AtomicBool::new(true);

/// Compute a fingerprint of the key using SHA-256.
/// This avoids storing the raw key material in the registry.
#[cfg(feature = "crypto")]
fn compute_key_fingerprint(key: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.finalize().into()
}

/// Enable or disable nonce tracking.
///
/// # Arguments
/// * `enable` - 1 to enable, 0 to disable
///
/// # Returns
/// * Previous state (0 = was disabled, 1 = was enabled)
///
/// # C ABI
/// ```c
/// int32_t arth_rt_aead_nonce_tracking_enable(int32_t enable);
/// ```
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_nonce_tracking_enable(enable: i32) -> i32 {
    let was_enabled = NONCE_TRACKING_ENABLED.swap(enable != 0, Ordering::SeqCst);
    if was_enabled { 1 } else { 0 }
}

/// Enable or disable stderr warnings when nonce reuse is detected.
///
/// # Arguments
/// * `enable` - 1 to enable warnings (default), 0 to disable
///
/// # Returns
/// * Previous state (0 = warnings disabled, 1 = warnings enabled)
///
/// # C ABI
/// ```c
/// int32_t arth_rt_aead_nonce_warn_enable(int32_t enable);
/// ```
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_nonce_warn_enable(enable: i32) -> i32 {
    let was_enabled = NONCE_WARN_STDERR.swap(enable != 0, Ordering::SeqCst);
    if was_enabled { 1 } else { 0 }
}

/// Check if a key+nonce combination has been used before.
///
/// This function checks if the given key+nonce combination exists in the
/// tracking registry. It does not modify the registry.
///
/// # Arguments
/// * `key` - Pointer to the encryption key
/// * `key_len` - Length of the key in bytes
/// * `nonce` - Pointer to the nonce
/// * `nonce_len` - Length of the nonce in bytes
///
/// # Returns
/// * 0 - Nonce has not been used with this key
/// * 1 - Nonce has been used with this key (potential reuse!)
/// * -1 - Tracking is not enabled
/// * -2 - Null pointer provided
/// * -3 - Registry lock failed
///
/// # C ABI
/// ```c
/// int32_t arth_rt_aead_nonce_check(const uint8_t* key, size_t key_len,
///                                   const uint8_t* nonce, size_t nonce_len);
/// ```
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_nonce_check(
    key: *const u8,
    key_len: usize,
    nonce: *const u8,
    nonce_len: usize,
) -> i32 {
    if !NONCE_TRACKING_ENABLED.load(Ordering::SeqCst) {
        return -1;
    }

    if key.is_null() || nonce.is_null() {
        return -2;
    }

    let key_slice = unsafe { std::slice::from_raw_parts(key, key_len) };
    let nonce_slice = unsafe { std::slice::from_raw_parts(nonce, nonce_len) };

    let key_fingerprint = compute_key_fingerprint(key_slice);
    let nonce_vec = nonce_slice.to_vec();

    match NONCE_REGISTRY.lock() {
        Ok(registry) => {
            if registry.contains(&(key_fingerprint, nonce_vec)) {
                1
            } else {
                0
            }
        }
        Err(_) => -3,
    }
}

/// Mark a key+nonce combination as used.
///
/// This function adds the given key+nonce combination to the tracking registry.
/// If the combination already exists, it emits a warning (if warnings are enabled).
///
/// # Arguments
/// * `key` - Pointer to the encryption key
/// * `key_len` - Length of the key in bytes
/// * `nonce` - Pointer to the nonce
/// * `nonce_len` - Length of the nonce in bytes
///
/// # Returns
/// * 0 - Successfully marked as used (first use)
/// * 1 - Already existed (nonce reuse detected!)
/// * -1 - Tracking is not enabled
/// * -2 - Null pointer provided
/// * -3 - Registry lock failed
///
/// # C ABI
/// ```c
/// int32_t arth_rt_aead_nonce_mark_used(const uint8_t* key, size_t key_len,
///                                       const uint8_t* nonce, size_t nonce_len);
/// ```
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_nonce_mark_used(
    key: *const u8,
    key_len: usize,
    nonce: *const u8,
    nonce_len: usize,
) -> i32 {
    if !NONCE_TRACKING_ENABLED.load(Ordering::SeqCst) {
        return -1;
    }

    if key.is_null() || nonce.is_null() {
        return -2;
    }

    let key_slice = unsafe { std::slice::from_raw_parts(key, key_len) };
    let nonce_slice = unsafe { std::slice::from_raw_parts(nonce, nonce_len) };

    let key_fingerprint = compute_key_fingerprint(key_slice);
    let nonce_vec = nonce_slice.to_vec();

    match NONCE_REGISTRY.lock() {
        Ok(mut registry) => {
            if registry.insert((key_fingerprint, nonce_vec.clone())) {
                // New entry, first use
                0
            } else {
                // Already existed - nonce reuse detected!
                if NONCE_WARN_STDERR.load(Ordering::SeqCst) {
                    eprintln!(
                        "[CRYPTO WARNING] Nonce reuse detected! \
                         Key fingerprint: {:02x}{:02x}..{:02x}{:02x}, \
                         Nonce: {} bytes",
                        key_fingerprint[0],
                        key_fingerprint[1],
                        key_fingerprint[30],
                        key_fingerprint[31],
                        nonce_slice.len()
                    );
                }
                1
            }
        }
        Err(_) => -3,
    }
}

/// Clear all tracked nonce entries.
///
/// This function removes all key+nonce combinations from the tracking registry.
/// Useful when rotating keys or resetting state.
///
/// # Returns
/// * Number of entries cleared (>= 0)
/// * -1 - Tracking is not enabled
/// * -3 - Registry lock failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_aead_nonce_tracking_clear(void);
/// ```
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_nonce_tracking_clear() -> i64 {
    if !NONCE_TRACKING_ENABLED.load(Ordering::SeqCst) {
        return -1;
    }

    match NONCE_REGISTRY.lock() {
        Ok(mut registry) => {
            let count = registry.len() as i64;
            registry.clear();
            count
        }
        Err(_) => -3,
    }
}

/// Get the number of tracked nonce entries.
///
/// # Returns
/// * Number of entries (>= 0)
/// * -1 - Tracking is not enabled
/// * -3 - Registry lock failed
///
/// # C ABI
/// ```c
/// int64_t arth_rt_aead_nonce_tracking_count(void);
/// ```
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_nonce_tracking_count() -> i64 {
    if !NONCE_TRACKING_ENABLED.load(Ordering::SeqCst) {
        return -1;
    }

    match NONCE_REGISTRY.lock() {
        Ok(registry) => registry.len() as i64,
        Err(_) => -3,
    }
}

/// Check and mark a nonce as used in a single atomic operation.
///
/// This is a convenience function that combines `arth_rt_aead_nonce_check`
/// and `arth_rt_aead_nonce_mark_used` into a single atomic operation.
/// This is the recommended way to use nonce tracking.
///
/// # Arguments
/// * `key` - Pointer to the encryption key
/// * `key_len` - Length of the key in bytes
/// * `nonce` - Pointer to the nonce
/// * `nonce_len` - Length of the nonce in bytes
///
/// # Returns
/// * 0 - Nonce was not previously used (now marked as used)
/// * 1 - Nonce was already used (warning emitted if enabled)
/// * -1 - Tracking is not enabled
/// * -2 - Null pointer provided
/// * -3 - Registry lock failed
///
/// # C ABI
/// ```c
/// int32_t arth_rt_aead_nonce_check_and_mark(const uint8_t* key, size_t key_len,
///                                            const uint8_t* nonce, size_t nonce_len);
/// ```
#[cfg(feature = "crypto")]
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_aead_nonce_check_and_mark(
    key: *const u8,
    key_len: usize,
    nonce: *const u8,
    nonce_len: usize,
) -> i32 {
    if !NONCE_TRACKING_ENABLED.load(Ordering::SeqCst) {
        return -1;
    }

    if key.is_null() || nonce.is_null() {
        return -2;
    }

    let key_slice = unsafe { std::slice::from_raw_parts(key, key_len) };
    let nonce_slice = unsafe { std::slice::from_raw_parts(nonce, nonce_len) };

    let key_fingerprint = compute_key_fingerprint(key_slice);
    let nonce_vec = nonce_slice.to_vec();

    match NONCE_REGISTRY.lock() {
        Ok(mut registry) => {
            if registry.insert((key_fingerprint, nonce_vec.clone())) {
                // New entry, first use
                0
            } else {
                // Already existed - nonce reuse detected!
                if NONCE_WARN_STDERR.load(Ordering::SeqCst) {
                    eprintln!(
                        "[CRYPTO WARNING] Nonce reuse detected! \
                         This is a serious security vulnerability that can compromise \
                         the confidentiality and authenticity of encrypted data. \
                         Key fingerprint: {:02x}{:02x}..{:02x}{:02x}, \
                         Nonce size: {} bytes",
                        key_fingerprint[0],
                        key_fingerprint[1],
                        key_fingerprint[30],
                        key_fingerprint[31],
                        nonce_slice.len()
                    );
                }
                1
            }
        }
        Err(_) => -3,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper function to convert hex string to bytes for test vectors
    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        let hex = hex.replace(" ", "").replace("\n", "");
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn test_secure_alloc_free() {
        let handle = arth_rt_secure_alloc(32);
        assert!(handle > 0);

        let ptr = arth_rt_secure_ptr(handle);
        assert!(!ptr.is_null());

        let len = arth_rt_secure_len(handle);
        assert_eq!(len, 32);

        let result = arth_rt_secure_free(handle);
        assert_eq!(result, 0);

        // After free, ptr should be invalid
        let ptr = arth_rt_secure_ptr(handle);
        assert!(ptr.is_null());
    }

    #[test]
    fn test_secure_write_read() {
        let handle = arth_rt_secure_alloc(16);
        assert!(handle > 0);

        let data = b"Hello, World!!!!"; // Exactly 16 bytes
        let written = arth_rt_secure_write(handle, data.as_ptr(), data.len());
        assert_eq!(written, 16);

        let mut buf = [0u8; 16];
        let read = arth_rt_secure_read(handle, buf.as_mut_ptr(), buf.len());
        assert_eq!(read, 16);
        assert_eq!(&buf, data);

        arth_rt_secure_free(handle);
    }

    #[test]
    fn test_secure_compare() {
        let a = b"hello";
        let b = b"hello";
        let c = b"world";

        assert_eq!(arth_rt_secure_compare(a.as_ptr(), b.as_ptr(), 5), 1);
        assert_eq!(arth_rt_secure_compare(a.as_ptr(), c.as_ptr(), 5), 0);
    }

    #[test]
    fn test_secure_zero() {
        let mut data = [0xffu8; 32];
        arth_rt_secure_zero(data.as_mut_ptr(), data.len());
        assert!(data.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_secure_alloc_zero() {
        // Verify that allocated memory is zeroed
        let handle = arth_rt_secure_alloc(64);
        assert!(handle > 0);

        let mut buf = [0xffu8; 64];
        let read = arth_rt_secure_read(handle, buf.as_mut_ptr(), buf.len());
        assert_eq!(read, 64);
        assert!(
            buf.iter().all(|&b| b == 0),
            "Secure memory should be zeroed on allocation"
        );

        arth_rt_secure_free(handle);
    }

    #[test]
    fn test_secure_compare_different_lengths() {
        // Same content but comparing different lengths should still work
        let a = b"hello world";
        let b = b"hello planet";

        // First 5 characters are the same
        assert_eq!(arth_rt_secure_compare(a.as_ptr(), b.as_ptr(), 5), 1);
        // But full comparison differs
        assert_eq!(arth_rt_secure_compare(a.as_ptr(), b.as_ptr(), 11), 0);
    }

    #[test]
    fn test_secure_memory_isolation() {
        // Allocate two regions and verify they don't interfere
        let h1 = arth_rt_secure_alloc(16);
        let h2 = arth_rt_secure_alloc(16);
        assert!(h1 > 0);
        assert!(h2 > 0);
        assert_ne!(h1, h2);

        let data1 = b"AAAAAAAAAAAAAAAA";
        let data2 = b"BBBBBBBBBBBBBBBB";

        arth_rt_secure_write(h1, data1.as_ptr(), 16);
        arth_rt_secure_write(h2, data2.as_ptr(), 16);

        let mut buf1 = [0u8; 16];
        let mut buf2 = [0u8; 16];
        arth_rt_secure_read(h1, buf1.as_mut_ptr(), 16);
        arth_rt_secure_read(h2, buf2.as_mut_ptr(), 16);

        assert_eq!(&buf1, data1);
        assert_eq!(&buf2, data2);

        arth_rt_secure_free(h1);
        arth_rt_secure_free(h2);
    }

    // =========================================================================
    // Hash Function Tests - NIST Test Vectors
    // =========================================================================

    #[test]
    fn test_hash_output_size() {
        assert_eq!(arth_rt_hash_output_size(0), 32); // SHA-256
        assert_eq!(arth_rt_hash_output_size(1), 48); // SHA-384
        assert_eq!(arth_rt_hash_output_size(2), 64); // SHA-512
        assert_eq!(arth_rt_hash_output_size(3), 32); // SHA3-256
        assert_eq!(arth_rt_hash_output_size(4), 64); // SHA3-512
        assert_eq!(arth_rt_hash_output_size(5), 32); // BLAKE3
        assert_eq!(arth_rt_hash_output_size(99), 0); // Invalid
    }

    #[test]
    fn test_sha256_empty() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let expected =
            hex_to_bytes("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        let input: &[u8] = &[];
        let mut output = [0u8; 32];

        let len = arth_rt_hash(0, input.as_ptr(), 0, output.as_mut_ptr(), 32);
        assert_eq!(len, 32);
        assert_eq!(&output[..], &expected[..]);
    }

    #[test]
    fn test_sha256_abc() {
        // NIST FIPS 180-4 Example: SHA-256("abc")
        let expected =
            hex_to_bytes("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let input = b"abc";
        let mut output = [0u8; 32];

        let len = arth_rt_hash(0, input.as_ptr(), input.len(), output.as_mut_ptr(), 32);
        assert_eq!(len, 32);
        assert_eq!(&output[..], &expected[..]);
    }

    #[test]
    fn test_sha256_long() {
        // SHA-256("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")
        let expected =
            hex_to_bytes("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        let input = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        let mut output = [0u8; 32];

        let len = arth_rt_hash(0, input.as_ptr(), input.len(), output.as_mut_ptr(), 32);
        assert_eq!(len, 32);
        assert_eq!(&output[..], &expected[..]);
    }

    #[test]
    fn test_sha512_empty() {
        // SHA-512("")
        let expected = hex_to_bytes(
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
             47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
        );
        let input: &[u8] = &[];
        let mut output = [0u8; 64];

        let len = arth_rt_hash(2, input.as_ptr(), 0, output.as_mut_ptr(), 64);
        assert_eq!(len, 64);
        assert_eq!(&output[..], &expected[..]);
    }

    #[test]
    fn test_sha512_abc() {
        // NIST FIPS 180-4 Example: SHA-512("abc")
        let expected = hex_to_bytes(
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        );
        let input = b"abc";
        let mut output = [0u8; 64];

        let len = arth_rt_hash(2, input.as_ptr(), input.len(), output.as_mut_ptr(), 64);
        assert_eq!(len, 64);
        assert_eq!(&output[..], &expected[..]);
    }

    #[test]
    fn test_sha384_abc() {
        // NIST FIPS 180-4 Example: SHA-384("abc")
        let expected = hex_to_bytes(
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed\
             8086072ba1e7cc2358baeca134c825a7",
        );
        let input = b"abc";
        let mut output = [0u8; 48];

        let len = arth_rt_hash(1, input.as_ptr(), input.len(), output.as_mut_ptr(), 48);
        assert_eq!(len, 48);
        assert_eq!(&output[..], &expected[..]);
    }

    #[test]
    fn test_sha3_256_empty() {
        // SHA3-256("")
        let expected =
            hex_to_bytes("a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a");
        let input: &[u8] = &[];
        let mut output = [0u8; 32];

        let len = arth_rt_hash(3, input.as_ptr(), 0, output.as_mut_ptr(), 32);
        assert_eq!(len, 32);
        assert_eq!(&output[..], &expected[..]);
    }

    #[test]
    fn test_sha3_256_abc() {
        // NIST FIPS 202 Example: SHA3-256("abc")
        let expected =
            hex_to_bytes("3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532");
        let input = b"abc";
        let mut output = [0u8; 32];

        let len = arth_rt_hash(3, input.as_ptr(), input.len(), output.as_mut_ptr(), 32);
        assert_eq!(len, 32);
        assert_eq!(&output[..], &expected[..]);
    }

    #[test]
    fn test_sha3_512_abc() {
        // NIST FIPS 202 Example: SHA3-512("abc")
        let expected = hex_to_bytes(
            "b751850b1a57168a5693cd924b6b096e08f621827444f70d884f5d0240d2712e\
             10e116e9192af3c91a7ec57647e3934057340b4cf408d5a56592f8274eec53f0",
        );
        let input = b"abc";
        let mut output = [0u8; 64];

        let len = arth_rt_hash(4, input.as_ptr(), input.len(), output.as_mut_ptr(), 64);
        assert_eq!(len, 64);
        assert_eq!(&output[..], &expected[..]);
    }

    #[test]
    fn test_blake3_empty() {
        // BLAKE3("") from official test vectors
        let expected =
            hex_to_bytes("af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262");
        let input: &[u8] = &[];
        let mut output = [0u8; 32];

        let len = arth_rt_hash(5, input.as_ptr(), 0, output.as_mut_ptr(), 32);
        assert_eq!(len, 32);
        assert_eq!(&output[..], &expected[..]);
    }

    #[test]
    fn test_blake3_hello() {
        // BLAKE3("Hello, World!")
        let input = b"Hello, World!";
        let mut output = [0u8; 32];

        let len = arth_rt_hash(5, input.as_ptr(), input.len(), output.as_mut_ptr(), 32);
        assert_eq!(len, 32);

        // Verify against known hash (computed independently)
        let expected =
            hex_to_bytes("288a86a79f20a3d6dccdca7713beaed178798296bdfa7913fa2a62d9727bf8f8");
        assert_eq!(&output[..], &expected[..]);
    }

    #[test]
    fn test_hash_invalid_algorithm() {
        let input = b"test";
        let mut output = [0u8; 32];

        let len = arth_rt_hash(99, input.as_ptr(), input.len(), output.as_mut_ptr(), 32);
        assert_eq!(len, -3); // Invalid algorithm error
    }

    #[test]
    fn test_hash_buffer_too_small() {
        let input = b"test";
        let mut output = [0u8; 16]; // Too small for SHA-256

        let len = arth_rt_hash(0, input.as_ptr(), input.len(), output.as_mut_ptr(), 16);
        assert_eq!(len, -1); // Buffer too small error
    }

    // =========================================================================
    // Incremental Hasher Tests
    // =========================================================================

    #[test]
    fn test_hasher_sha256_one_shot_equivalent() {
        let input = b"The quick brown fox jumps over the lazy dog";

        // One-shot hash
        let mut one_shot = [0u8; 32];
        arth_rt_hash(0, input.as_ptr(), input.len(), one_shot.as_mut_ptr(), 32);

        // Incremental hash
        let handle = arth_rt_hasher_new(0);
        assert!(handle > 0);

        arth_rt_hasher_update(handle, input.as_ptr(), input.len());

        let mut incremental = [0u8; 32];
        let len = arth_rt_hasher_finalize(handle, incremental.as_mut_ptr(), 32);
        assert_eq!(len, 32);

        // Both should be equal
        assert_eq!(&one_shot[..], &incremental[..]);
    }

    #[test]
    fn test_hasher_incremental_chunks() {
        // Split input into chunks and verify result matches one-shot
        let full_input = b"Hello, World! This is a test message for incremental hashing.";

        // One-shot hash
        let mut one_shot = [0u8; 32];
        arth_rt_hash(
            0,
            full_input.as_ptr(),
            full_input.len(),
            one_shot.as_mut_ptr(),
            32,
        );

        // Incremental hash in chunks
        let handle = arth_rt_hasher_new(0);
        assert!(handle > 0);

        // Feed data in multiple chunks
        arth_rt_hasher_update(handle, full_input[..7].as_ptr(), 7); // "Hello, "
        arth_rt_hasher_update(handle, full_input[7..14].as_ptr(), 7); // "World! "
        arth_rt_hasher_update(handle, full_input[14..].as_ptr(), full_input.len() - 14);

        let mut incremental = [0u8; 32];
        let len = arth_rt_hasher_finalize(handle, incremental.as_mut_ptr(), 32);
        assert_eq!(len, 32);

        // Both should be equal
        assert_eq!(&one_shot[..], &incremental[..]);
    }

    #[test]
    fn test_hasher_all_algorithms() {
        let input = b"test input";

        for algo in 0..=5 {
            let output_size = arth_rt_hash_output_size(algo);
            assert!(
                output_size > 0,
                "Algorithm {} should have valid output size",
                algo
            );

            // One-shot
            let mut one_shot = vec![0u8; output_size];
            let len1 = arth_rt_hash(
                algo,
                input.as_ptr(),
                input.len(),
                one_shot.as_mut_ptr(),
                output_size,
            );
            assert_eq!(len1 as usize, output_size);

            // Incremental
            let handle = arth_rt_hasher_new(algo);
            assert!(handle > 0, "Failed to create hasher for algorithm {}", algo);

            arth_rt_hasher_update(handle, input.as_ptr(), input.len());

            let mut incremental = vec![0u8; output_size];
            let len2 = arth_rt_hasher_finalize(handle, incremental.as_mut_ptr(), output_size);
            assert_eq!(len2 as usize, output_size);

            // Should match
            assert_eq!(
                one_shot, incremental,
                "One-shot and incremental should match for algorithm {}",
                algo
            );
        }
    }

    #[test]
    fn test_hasher_clone() {
        let handle = arth_rt_hasher_new(0);
        assert!(handle > 0);

        arth_rt_hasher_update(handle, b"Hello, ".as_ptr(), 7);

        // Clone the hasher
        let cloned = arth_rt_hasher_clone(handle);
        assert!(cloned > 0);
        assert_ne!(handle, cloned);

        // Update both differently
        arth_rt_hasher_update(handle, b"World!".as_ptr(), 6);
        arth_rt_hasher_update(cloned, b"Arth!".as_ptr(), 5);

        // Finalize both
        let mut result1 = [0u8; 32];
        let mut result2 = [0u8; 32];

        arth_rt_hasher_finalize(handle, result1.as_mut_ptr(), 32);
        arth_rt_hasher_finalize(cloned, result2.as_mut_ptr(), 32);

        // They should be different
        assert_ne!(&result1[..], &result2[..]);

        // Verify result1 is hash of "Hello, World!"
        let mut expected = [0u8; 32];
        arth_rt_hash(0, b"Hello, World!".as_ptr(), 13, expected.as_mut_ptr(), 32);
        assert_eq!(&result1[..], &expected[..]);

        // Verify result2 is hash of "Hello, Arth!"
        arth_rt_hash(0, b"Hello, Arth!".as_ptr(), 12, expected.as_mut_ptr(), 32);
        assert_eq!(&result2[..], &expected[..]);
    }

    #[test]
    fn test_hasher_algorithm() {
        for algo in 0..=5 {
            let handle = arth_rt_hasher_new(algo);
            assert!(handle > 0);

            let reported_algo = arth_rt_hasher_algorithm(handle);
            assert_eq!(reported_algo, algo);

            arth_rt_hasher_free(handle);
        }
    }

    #[test]
    fn test_hasher_free() {
        let handle = arth_rt_hasher_new(0);
        assert!(handle > 0);

        let result = arth_rt_hasher_free(handle);
        assert_eq!(result, 0);

        // Trying to free again should fail
        let result = arth_rt_hasher_free(handle);
        assert_eq!(result, -1);
    }

    #[test]
    fn test_hasher_invalid_handle() {
        let result = arth_rt_hasher_update(999999, b"test".as_ptr(), 4);
        assert_eq!(result, -1);

        let mut output = [0u8; 32];
        let result = arth_rt_hasher_finalize(999999, output.as_mut_ptr(), 32);
        assert_eq!(result, -1);

        let result = arth_rt_hasher_algorithm(999999);
        assert_eq!(result, -1);

        let result = arth_rt_hasher_clone(999999);
        assert_eq!(result, -1);
    }

    // =========================================================================
    // Hash Verification Tests
    // =========================================================================

    #[test]
    fn test_hash_verify_valid() {
        let data = b"Hello, World!";

        // Compute hash
        let mut hash = [0u8; 32];
        arth_rt_hash(0, data.as_ptr(), data.len(), hash.as_mut_ptr(), 32);

        // Verify should succeed
        let result = arth_rt_hash_verify(0, data.as_ptr(), data.len(), hash.as_ptr(), 32);
        assert_eq!(result, 1);
    }

    #[test]
    fn test_hash_verify_invalid() {
        let data = b"Hello, World!";

        // Compute hash
        let mut hash = [0u8; 32];
        arth_rt_hash(0, data.as_ptr(), data.len(), hash.as_mut_ptr(), 32);

        // Modify the data
        let modified_data = b"Hello, Arth!!";

        // Verify should fail
        let result = arth_rt_hash_verify(
            0,
            modified_data.as_ptr(),
            modified_data.len(),
            hash.as_ptr(),
            32,
        );
        assert_eq!(result, 0);
    }

    #[test]
    fn test_hash_verify_wrong_length() {
        let data = b"test";
        let hash = [0u8; 16]; // Wrong size for SHA-256

        let result = arth_rt_hash_verify(0, data.as_ptr(), data.len(), hash.as_ptr(), 16);
        assert_eq!(result, -1); // Wrong length error
    }

    #[test]
    fn test_hash_equals() {
        let a = [1u8, 2, 3, 4, 5];
        let b = [1u8, 2, 3, 4, 5];
        let c = [1u8, 2, 3, 4, 6];

        assert_eq!(arth_rt_hash_equals(a.as_ptr(), b.as_ptr(), 5), 1);
        assert_eq!(arth_rt_hash_equals(a.as_ptr(), c.as_ptr(), 5), 0);
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn test_hash_empty_input() {
        for algo in 0..=5 {
            let output_size = arth_rt_hash_output_size(algo);
            let mut output = vec![0u8; output_size];

            // Empty input with null pointer
            let len = arth_rt_hash(algo, std::ptr::null(), 0, output.as_mut_ptr(), output_size);
            assert_eq!(
                len as usize, output_size,
                "Empty hash should succeed for algo {}",
                algo
            );

            // Verify it matches the known empty hash for the algorithm
            let mut output2 = vec![0u8; output_size];
            let empty: &[u8] = &[];
            arth_rt_hash(algo, empty.as_ptr(), 0, output2.as_mut_ptr(), output_size);
            assert_eq!(output, output2);
        }
    }

    #[test]
    fn test_hasher_empty_updates() {
        let handle = arth_rt_hasher_new(0);
        assert!(handle > 0);

        // Multiple empty updates
        arth_rt_hasher_update(handle, std::ptr::null(), 0);
        arth_rt_hasher_update(handle, b"".as_ptr(), 0);
        arth_rt_hasher_update(handle, b"data".as_ptr(), 4);
        arth_rt_hasher_update(handle, std::ptr::null(), 0);

        let mut result = [0u8; 32];
        arth_rt_hasher_finalize(handle, result.as_mut_ptr(), 32);

        // Should equal hash of just "data"
        let mut expected = [0u8; 32];
        arth_rt_hash(0, b"data".as_ptr(), 4, expected.as_mut_ptr(), 32);

        assert_eq!(&result[..], &expected[..]);
    }

    #[test]
    fn test_large_input() {
        // Test with 1MB of data
        let large_input: Vec<u8> = (0..1024 * 1024).map(|i| (i % 256) as u8).collect();
        let mut output = [0u8; 32];

        let len = arth_rt_hash(
            0,
            large_input.as_ptr(),
            large_input.len(),
            output.as_mut_ptr(),
            32,
        );
        assert_eq!(len, 32);

        // Verify with incremental
        let handle = arth_rt_hasher_new(0);

        // Feed in 64KB chunks
        for chunk in large_input.chunks(64 * 1024) {
            arth_rt_hasher_update(handle, chunk.as_ptr(), chunk.len());
        }

        let mut incremental = [0u8; 32];
        arth_rt_hasher_finalize(handle, incremental.as_mut_ptr(), 32);

        assert_eq!(&output[..], &incremental[..]);
    }

    // =========================================================================
    // HMAC Tests - RFC 4231 Test Vectors
    // =========================================================================

    // RFC 4231 Test Case 1: HMAC-SHA-256
    #[test]
    fn test_hmac_sha256_rfc4231_case1() {
        // Key = 0x0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b (20 bytes)
        let key = [0x0bu8; 20];
        // Data = "Hi There"
        let data = b"Hi There";
        // Expected HMAC-SHA-256
        let expected =
            hex_to_bytes("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");

        let mut output = [0u8; 32];
        let len = arth_rt_hmac(
            0, // SHA-256
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            output.as_mut_ptr(),
            32,
        );

        assert_eq!(len, 32);
        assert_eq!(&output[..], &expected[..]);
    }

    // RFC 4231 Test Case 2: HMAC-SHA-256
    #[test]
    fn test_hmac_sha256_rfc4231_case2() {
        // Key = "Jefe"
        let key = b"Jefe";
        // Data = "what do ya want for nothing?"
        let data = b"what do ya want for nothing?";
        // Expected HMAC-SHA-256
        let expected =
            hex_to_bytes("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");

        let mut output = [0u8; 32];
        let len = arth_rt_hmac(
            0,
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            output.as_mut_ptr(),
            32,
        );

        assert_eq!(len, 32);
        assert_eq!(&output[..], &expected[..]);
    }

    // RFC 4231 Test Case 3: HMAC-SHA-256 with 0xaa key
    #[test]
    fn test_hmac_sha256_rfc4231_case3() {
        // Key = 0xaaaa... (20 bytes)
        let key = [0xaau8; 20];
        // Data = 0xdddd... (50 bytes)
        let data = [0xddu8; 50];
        // Expected HMAC-SHA-256
        let expected =
            hex_to_bytes("773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe");

        let mut output = [0u8; 32];
        let len = arth_rt_hmac(
            0,
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            output.as_mut_ptr(),
            32,
        );

        assert_eq!(len, 32);
        assert_eq!(&output[..], &expected[..]);
    }

    // RFC 4231 Test Case 1: HMAC-SHA-512
    #[test]
    fn test_hmac_sha512_rfc4231_case1() {
        // Key = 0x0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b (20 bytes)
        let key = [0x0bu8; 20];
        // Data = "Hi There"
        let data = b"Hi There";
        // Expected HMAC-SHA-512
        let expected = hex_to_bytes(
            "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cde\
             daa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854",
        );

        let mut output = [0u8; 64];
        let len = arth_rt_hmac(
            2, // SHA-512
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            output.as_mut_ptr(),
            64,
        );

        assert_eq!(len, 64);
        assert_eq!(&output[..], &expected[..]);
    }

    // RFC 4231 Test Case 2: HMAC-SHA-512
    #[test]
    fn test_hmac_sha512_rfc4231_case2() {
        // Key = "Jefe"
        let key = b"Jefe";
        // Data = "what do ya want for nothing?"
        let data = b"what do ya want for nothing?";
        // Expected HMAC-SHA-512
        let expected = hex_to_bytes(
            "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea250554\
             9758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737",
        );

        let mut output = [0u8; 64];
        let len = arth_rt_hmac(
            2,
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            output.as_mut_ptr(),
            64,
        );

        assert_eq!(len, 64);
        assert_eq!(&output[..], &expected[..]);
    }

    // RFC 4231 Test Case 4: Long key (131 bytes) - HMAC-SHA-256
    #[test]
    fn test_hmac_sha256_rfc4231_case6_long_key() {
        // Key = 0xaaaa... (131 bytes, longer than block size)
        let key = [0xaau8; 131];
        // Data = "Test Using Larger Than Block-Size Key - Hash Key First"
        let data = b"Test Using Larger Than Block-Size Key - Hash Key First";
        // Expected HMAC-SHA-256
        let expected =
            hex_to_bytes("60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");

        let mut output = [0u8; 32];
        let len = arth_rt_hmac(
            0,
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            output.as_mut_ptr(),
            32,
        );

        assert_eq!(len, 32);
        assert_eq!(&output[..], &expected[..]);
    }

    // HMAC-SHA384 test
    #[test]
    fn test_hmac_sha384() {
        let key = b"secret key";
        let data = b"test message";

        let mut output = [0u8; 48];
        let len = arth_rt_hmac(
            1, // SHA-384
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            output.as_mut_ptr(),
            48,
        );

        assert_eq!(len, 48);
        // Just verify it produces output - actual value verified by cross-checking
    }

    // =========================================================================
    // Incremental HMAC Tests
    // =========================================================================

    #[test]
    fn test_hmac_incremental_equals_oneshot() {
        let key = b"my secret key";
        let data = b"The quick brown fox jumps over the lazy dog";

        // One-shot HMAC
        let mut oneshot = [0u8; 32];
        arth_rt_hmac(
            0,
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            oneshot.as_mut_ptr(),
            32,
        );

        // Incremental HMAC
        let handle = arth_rt_hmac_new(0, key.as_ptr(), key.len());
        assert!(handle > 0);

        arth_rt_hmac_update(handle, data.as_ptr(), data.len());

        let mut incremental = [0u8; 32];
        let len = arth_rt_hmac_finalize(handle, incremental.as_mut_ptr(), 32);
        assert_eq!(len, 32);

        assert_eq!(&oneshot[..], &incremental[..]);
    }

    #[test]
    fn test_hmac_incremental_chunks() {
        let key = b"secret";
        let data1 = b"Hello, ";
        let data2 = b"World!";
        let full_data = b"Hello, World!";

        // One-shot with full data
        let mut oneshot = [0u8; 32];
        arth_rt_hmac(
            0,
            key.as_ptr(),
            key.len(),
            full_data.as_ptr(),
            full_data.len(),
            oneshot.as_mut_ptr(),
            32,
        );

        // Incremental in chunks
        let handle = arth_rt_hmac_new(0, key.as_ptr(), key.len());
        assert!(handle > 0);

        arth_rt_hmac_update(handle, data1.as_ptr(), data1.len());
        arth_rt_hmac_update(handle, data2.as_ptr(), data2.len());

        let mut incremental = [0u8; 32];
        arth_rt_hmac_finalize(handle, incremental.as_mut_ptr(), 32);

        assert_eq!(&oneshot[..], &incremental[..]);
    }

    #[test]
    fn test_hmac_incremental_all_algorithms() {
        let key = b"test key";
        let data = b"test data";

        // Test algorithms 0-4 (SHA-256, SHA-384, SHA-512, SHA3-256, SHA3-512)
        for algo in 0..=4 {
            let output_size = arth_rt_hash_output_size(algo);

            // One-shot
            let mut oneshot = vec![0u8; output_size];
            let len1 = arth_rt_hmac(
                algo,
                key.as_ptr(),
                key.len(),
                data.as_ptr(),
                data.len(),
                oneshot.as_mut_ptr(),
                output_size,
            );
            assert_eq!(
                len1 as usize, output_size,
                "One-shot failed for algo {}",
                algo
            );

            // Incremental
            let handle = arth_rt_hmac_new(algo, key.as_ptr(), key.len());
            assert!(handle > 0, "Failed to create HMAC for algo {}", algo);

            arth_rt_hmac_update(handle, data.as_ptr(), data.len());

            let mut incremental = vec![0u8; output_size];
            let len2 = arth_rt_hmac_finalize(handle, incremental.as_mut_ptr(), output_size);
            assert_eq!(len2 as usize, output_size);

            assert_eq!(
                oneshot, incremental,
                "One-shot and incremental should match for algo {}",
                algo
            );
        }
    }

    #[test]
    fn test_hmac_free() {
        let key = b"key";
        let handle = arth_rt_hmac_new(0, key.as_ptr(), key.len());
        assert!(handle > 0);

        let result = arth_rt_hmac_free(handle);
        assert_eq!(result, 0);

        // Try to free again - should fail
        let result = arth_rt_hmac_free(handle);
        assert_eq!(result, -1);
    }

    #[test]
    fn test_hmac_blake3_unsupported() {
        let key = b"key";
        let data = b"data";
        let mut output = [0u8; 32];

        // BLAKE3 (algo 5) should be rejected for HMAC
        let len = arth_rt_hmac(
            5,
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            output.as_mut_ptr(),
            32,
        );
        assert_eq!(len, -3);

        // Incremental too
        let handle = arth_rt_hmac_new(5, key.as_ptr(), key.len());
        assert_eq!(handle, -3);
    }

    // =========================================================================
    // HMAC Verification Tests
    // =========================================================================

    #[test]
    fn test_hmac_verify_valid() {
        let key = b"secret key";
        let data = b"message to authenticate";

        // Compute HMAC
        let mut hmac = [0u8; 32];
        arth_rt_hmac(
            0,
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            hmac.as_mut_ptr(),
            32,
        );

        // Verify should succeed
        let result = arth_rt_hmac_verify(
            0,
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            hmac.as_ptr(),
            32,
        );
        assert_eq!(result, 1);
    }

    #[test]
    fn test_hmac_verify_invalid_data() {
        let key = b"secret key";
        let data = b"message to authenticate";

        // Compute HMAC
        let mut hmac = [0u8; 32];
        arth_rt_hmac(
            0,
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            hmac.as_mut_ptr(),
            32,
        );

        // Verify with different data should fail
        let bad_data = b"tampered message";
        let result = arth_rt_hmac_verify(
            0,
            key.as_ptr(),
            key.len(),
            bad_data.as_ptr(),
            bad_data.len(),
            hmac.as_ptr(),
            32,
        );
        assert_eq!(result, 0);
    }

    #[test]
    fn test_hmac_verify_wrong_key() {
        let key = b"secret key";
        let data = b"message";

        // Compute HMAC with correct key
        let mut hmac = [0u8; 32];
        arth_rt_hmac(
            0,
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            hmac.as_mut_ptr(),
            32,
        );

        // Verify with wrong key should fail
        let wrong_key = b"wrong key";
        let result = arth_rt_hmac_verify(
            0,
            wrong_key.as_ptr(),
            wrong_key.len(),
            data.as_ptr(),
            data.len(),
            hmac.as_ptr(),
            32,
        );
        assert_eq!(result, 0);
    }

    #[test]
    fn test_hmac_verify_wrong_length() {
        let key = b"key";
        let data = b"data";
        let hmac = [0u8; 16]; // Wrong size for SHA-256

        let result = arth_rt_hmac_verify(
            0,
            key.as_ptr(),
            key.len(),
            data.as_ptr(),
            data.len(),
            hmac.as_ptr(),
            16,
        );
        assert_eq!(result, -1);
    }

    #[test]
    fn test_hmac_equals() {
        let a = [1u8, 2, 3, 4, 5];
        let b = [1u8, 2, 3, 4, 5];
        let c = [1u8, 2, 3, 4, 6];

        assert_eq!(arth_rt_hmac_equals(a.as_ptr(), b.as_ptr(), 5), 1);
        assert_eq!(arth_rt_hmac_equals(a.as_ptr(), c.as_ptr(), 5), 0);
    }

    #[test]
    fn test_hmac_empty_key() {
        // Empty key is valid for HMAC
        let key: &[u8] = &[];
        let data = b"data";
        let mut output = [0u8; 32];

        let len = arth_rt_hmac(
            0,
            key.as_ptr(),
            0,
            data.as_ptr(),
            data.len(),
            output.as_mut_ptr(),
            32,
        );
        assert_eq!(len, 32);
    }

    #[test]
    fn test_hmac_empty_data() {
        let key = b"key";
        let mut output = [0u8; 32];

        let len = arth_rt_hmac(
            0,
            key.as_ptr(),
            key.len(),
            std::ptr::null(),
            0,
            output.as_mut_ptr(),
            32,
        );
        assert_eq!(len, 32);
    }

    // =========================================================================
    // Secure Random Tests
    // =========================================================================

    #[test]
    fn test_random_available() {
        // CSPRNG should be available on all supported platforms
        let result = arth_rt_random_available();
        assert_eq!(result, 1, "CSPRNG should be available");
    }

    #[test]
    fn test_random_bytes_basic() {
        let mut buf = [0u8; 32];
        let result = arth_rt_random_bytes(buf.as_mut_ptr(), buf.len());
        assert_eq!(result, 32);

        // Very unlikely to be all zeros if random
        assert!(
            !buf.iter().all(|&b| b == 0),
            "Random bytes should not be all zeros"
        );
    }

    #[test]
    fn test_random_bytes_empty() {
        let mut buf = [0u8; 0];
        let result = arth_rt_random_bytes(buf.as_mut_ptr(), 0);
        assert_eq!(result, 0); // Empty request should succeed
    }

    #[test]
    fn test_random_bytes_null() {
        let result = arth_rt_random_bytes(std::ptr::null_mut(), 32);
        assert_eq!(result, -1); // Null pointer error
    }

    #[test]
    fn test_random_fill() {
        let mut buf = [0u8; 64];
        let result = arth_rt_random_fill(buf.as_mut_ptr(), buf.len());
        assert_eq!(result, 0);

        // Very unlikely to be all zeros if random
        assert!(
            !buf.iter().all(|&b| b == 0),
            "Random fill should not produce all zeros"
        );
    }

    #[test]
    fn test_random_fill_empty() {
        let result = arth_rt_random_fill(std::ptr::null_mut(), 0);
        assert_eq!(result, 0); // Empty fill should succeed even with null pointer
    }

    #[test]
    fn test_random_bytes_uniqueness() {
        // Generate multiple random buffers and verify they're different
        let mut buffers: Vec<[u8; 32]> = Vec::new();

        for _ in 0..10 {
            let mut buf = [0u8; 32];
            let result = arth_rt_random_bytes(buf.as_mut_ptr(), buf.len());
            assert_eq!(result, 32);
            buffers.push(buf);
        }

        // All buffers should be unique
        for i in 0..buffers.len() {
            for j in (i + 1)..buffers.len() {
                assert_ne!(
                    buffers[i], buffers[j],
                    "Random buffers {} and {} should be different",
                    i, j
                );
            }
        }
    }

    #[test]
    fn test_random_u64_below() {
        // Test with small max
        for _ in 0..100 {
            let result = arth_rt_random_u64_below(10);
            assert!(
                result >= 0 && result < 10,
                "Result {} should be in [0, 10)",
                result
            );
        }

        // Test with power of 2
        for _ in 0..100 {
            let result = arth_rt_random_u64_below(256);
            assert!(
                result >= 0 && result < 256,
                "Result {} should be in [0, 256)",
                result
            );
        }

        // Test with large max
        for _ in 0..100 {
            let result = arth_rt_random_u64_below(1_000_000);
            assert!(result >= 0 && result < 1_000_000);
        }
    }

    #[test]
    fn test_random_u64_below_edge_cases() {
        // Max of 1 should always return 0
        for _ in 0..10 {
            let result = arth_rt_random_u64_below(1);
            assert_eq!(result, 0, "Max of 1 should always return 0");
        }

        // Max of 0 should return error
        let result = arth_rt_random_u64_below(0);
        assert_eq!(result, -1, "Max of 0 should return error");
    }

    #[test]
    fn test_random_int_range() {
        // Test positive range
        for _ in 0..100 {
            let result = arth_rt_random_int_range(5, 15);
            assert!(
                result >= 5 && result < 15,
                "Result {} should be in [5, 15)",
                result
            );
        }

        // Test negative range
        for _ in 0..100 {
            let result = arth_rt_random_int_range(-10, -5);
            assert!(
                result >= -10 && result < -5,
                "Result {} should be in [-10, -5)",
                result
            );
        }

        // Test range crossing zero
        for _ in 0..100 {
            let result = arth_rt_random_int_range(-5, 5);
            assert!(
                result >= -5 && result < 5,
                "Result {} should be in [-5, 5)",
                result
            );
        }
    }

    #[test]
    fn test_random_int_range_errors() {
        // min >= max should return error
        let result = arth_rt_random_int_range(10, 5);
        assert_eq!(result, i64::MIN, "Invalid range should return error");

        let result = arth_rt_random_int_range(5, 5);
        assert_eq!(result, i64::MIN, "Empty range should return error");
    }

    #[test]
    fn test_random_uuid_format() {
        let mut buf = [0u8; 37];
        let result = arth_rt_random_uuid(buf.as_mut_ptr(), buf.len());
        assert_eq!(result, 36, "UUID should be 36 characters");

        let uuid = std::str::from_utf8(&buf[..36]).expect("UUID should be valid UTF-8");

        // Validate format: 8-4-4-4-12
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid.chars().nth(8), Some('-'));
        assert_eq!(uuid.chars().nth(13), Some('-'));
        assert_eq!(uuid.chars().nth(18), Some('-'));
        assert_eq!(uuid.chars().nth(23), Some('-'));

        // Validate version 4 (character at position 14 should be '4')
        assert_eq!(uuid.chars().nth(14), Some('4'), "UUID version should be 4");

        // Validate variant (character at position 19 should be 8, 9, a, or b)
        let variant_char = uuid.chars().nth(19).unwrap();
        assert!(
            variant_char == '8'
                || variant_char == '9'
                || variant_char == 'a'
                || variant_char == 'b',
            "UUID variant should be RFC 4122 (8, 9, a, or b), got {}",
            variant_char
        );

        // All other characters should be hex digits or hyphens
        for (i, c) in uuid.chars().enumerate() {
            if i == 8 || i == 13 || i == 18 || i == 23 {
                assert_eq!(c, '-');
            } else {
                assert!(
                    c.is_ascii_hexdigit(),
                    "Character at {} should be hex, got {}",
                    i,
                    c
                );
            }
        }
    }

    #[test]
    fn test_random_uuid_uniqueness() {
        let mut uuids: Vec<String> = Vec::new();

        for _ in 0..100 {
            let mut buf = [0u8; 37];
            let result = arth_rt_random_uuid(buf.as_mut_ptr(), buf.len());
            assert_eq!(result, 36);

            let uuid = std::str::from_utf8(&buf[..36]).unwrap().to_string();
            uuids.push(uuid);
        }

        // All UUIDs should be unique
        for i in 0..uuids.len() {
            for j in (i + 1)..uuids.len() {
                assert_ne!(
                    uuids[i], uuids[j],
                    "UUIDs {} and {} should be different",
                    i, j
                );
            }
        }
    }

    #[test]
    fn test_random_uuid_buffer_too_small() {
        let mut buf = [0u8; 30]; // Too small
        let result = arth_rt_random_uuid(buf.as_mut_ptr(), buf.len());
        assert_eq!(result, -1, "Buffer too small should return error");
    }

    #[test]
    fn test_random_uuid_null() {
        let result = arth_rt_random_uuid(std::ptr::null_mut(), 37);
        assert_eq!(result, -1, "Null pointer should return error");
    }

    #[test]
    fn test_random_u32() {
        let mut values: Vec<u32> = Vec::new();

        for _ in 0..100 {
            let result = arth_rt_random_u32();
            assert!(result >= 0, "Random u32 should succeed");
            values.push(result as u32);
        }

        // Should have some variety (extremely unlikely to be all same)
        let first = values[0];
        assert!(
            !values.iter().all(|&v| v == first),
            "Random u32 values should vary"
        );
    }

    #[test]
    fn test_random_u64() {
        let mut values: Vec<u64> = Vec::new();

        for _ in 0..100 {
            let value = arth_rt_random_u64();
            values.push(value);
        }

        // Should have some variety
        let first = values[0];
        assert!(
            !values.iter().all(|&v| v == first),
            "Random u64 values should vary"
        );
    }

    #[test]
    fn test_random_distribution_basic() {
        // Generate many random bytes and check distribution isn't obviously biased
        let mut counts = [0u32; 256];
        let sample_size = 10000;

        let mut buf = vec![0u8; sample_size];
        let result = arth_rt_random_bytes(buf.as_mut_ptr(), buf.len());
        assert_eq!(result as usize, sample_size);

        for &byte in &buf {
            counts[byte as usize] += 1;
        }

        // Expected count per bucket is sample_size / 256 ≈ 39
        // Allow significant variance (chi-squared would be better, but this is a basic check)
        let expected = sample_size as f64 / 256.0;
        let min_expected = (expected * 0.3) as u32; // At least 30% of expected
        let max_expected = (expected * 3.0) as u32; // At most 300% of expected

        for (byte, &count) in counts.iter().enumerate() {
            assert!(
                count >= min_expected && count <= max_expected,
                "Byte {} appeared {} times, expected ~{:.1} (range {}-{})",
                byte,
                count,
                expected,
                min_expected,
                max_expected
            );
        }
    }

    #[test]
    fn test_random_large_buffer() {
        // Test with a larger buffer (1MB)
        let size = 1024 * 1024;
        let mut buf = vec![0u8; size];

        let result = arth_rt_random_bytes(buf.as_mut_ptr(), buf.len());
        assert_eq!(result as usize, size);

        // Shouldn't be all zeros
        assert!(!buf.iter().all(|&b| b == 0));
    }

    // =========================================================================
    // AEAD Tests
    // =========================================================================

    #[test]
    fn test_aead_sizes() {
        // AES-128-GCM
        assert_eq!(arth_rt_aead_key_size(0), 16);
        assert_eq!(arth_rt_aead_nonce_size(0), 12);
        assert_eq!(arth_rt_aead_tag_size(0), 16);

        // AES-256-GCM
        assert_eq!(arth_rt_aead_key_size(1), 32);
        assert_eq!(arth_rt_aead_nonce_size(1), 12);
        assert_eq!(arth_rt_aead_tag_size(1), 16);

        // ChaCha20-Poly1305
        assert_eq!(arth_rt_aead_key_size(2), 32);
        assert_eq!(arth_rt_aead_nonce_size(2), 12);
        assert_eq!(arth_rt_aead_tag_size(2), 16);

        // Invalid algorithm
        assert_eq!(arth_rt_aead_key_size(99), 0);
        assert_eq!(arth_rt_aead_nonce_size(99), 0);
        assert_eq!(arth_rt_aead_tag_size(99), 0);
    }

    #[test]
    fn test_aead_generate_nonce() {
        for algo in 0..=2 {
            let nonce_size = arth_rt_aead_nonce_size(algo);
            let mut nonce = vec![0u8; nonce_size];

            let result = arth_rt_aead_generate_nonce(algo, nonce.as_mut_ptr(), nonce_size);
            assert_eq!(
                result, 0,
                "Nonce generation should succeed for algo {}",
                algo
            );

            // Should not be all zeros
            assert!(
                !nonce.iter().all(|&b| b == 0),
                "Nonce should not be all zeros"
            );
        }
    }

    #[test]
    fn test_aead_generate_key() {
        for algo in 0..=2 {
            let key_size = arth_rt_aead_key_size(algo);
            let mut key = vec![0u8; key_size];

            let result = arth_rt_aead_generate_key(algo, key.as_mut_ptr(), key_size);
            assert_eq!(result, 0, "Key generation should succeed for algo {}", algo);

            // Should not be all zeros
            assert!(!key.iter().all(|&b| b == 0), "Key should not be all zeros");
        }
    }

    #[test]
    fn test_aead_roundtrip_all_algorithms() {
        let plaintext = b"Hello, World! This is a test message for AEAD.";

        for algo in 0..=2 {
            let key_size = arth_rt_aead_key_size(algo);
            let nonce_size = arth_rt_aead_nonce_size(algo);
            let tag_size = arth_rt_aead_tag_size(algo);

            // Generate key and nonce
            let mut key = vec![0u8; key_size];
            let mut nonce = vec![0u8; nonce_size];
            arth_rt_aead_generate_key(algo, key.as_mut_ptr(), key_size);
            arth_rt_aead_generate_nonce(algo, nonce.as_mut_ptr(), nonce_size);

            // Encrypt
            let ciphertext_len = plaintext.len() + tag_size;
            let mut ciphertext = vec![0u8; ciphertext_len];

            let result = arth_rt_aead_encrypt(
                algo,
                key.as_ptr(),
                key_size,
                nonce.as_ptr(),
                nonce_size,
                plaintext.as_ptr(),
                plaintext.len(),
                std::ptr::null(),
                0,
                ciphertext.as_mut_ptr(),
                ciphertext_len,
            );
            assert_eq!(
                result as usize, ciphertext_len,
                "Encrypt should succeed for algo {}",
                algo
            );

            // Decrypt
            let mut decrypted = vec![0u8; plaintext.len()];
            let result = arth_rt_aead_decrypt(
                algo,
                key.as_ptr(),
                key_size,
                nonce.as_ptr(),
                nonce_size,
                ciphertext.as_ptr(),
                ciphertext_len,
                std::ptr::null(),
                0,
                decrypted.as_mut_ptr(),
                plaintext.len(),
            );
            assert_eq!(
                result as usize,
                plaintext.len(),
                "Decrypt should succeed for algo {}",
                algo
            );
            assert_eq!(
                &decrypted[..],
                &plaintext[..],
                "Decrypted should match plaintext for algo {}",
                algo
            );
        }
    }

    #[test]
    fn test_aead_with_aad() {
        let plaintext = b"Secret message";
        let aad = b"Additional authenticated data";

        for algo in 0..=2 {
            let key_size = arth_rt_aead_key_size(algo);
            let nonce_size = arth_rt_aead_nonce_size(algo);
            let tag_size = arth_rt_aead_tag_size(algo);

            let mut key = vec![0u8; key_size];
            let mut nonce = vec![0u8; nonce_size];
            arth_rt_aead_generate_key(algo, key.as_mut_ptr(), key_size);
            arth_rt_aead_generate_nonce(algo, nonce.as_mut_ptr(), nonce_size);

            // Encrypt with AAD
            let ciphertext_len = plaintext.len() + tag_size;
            let mut ciphertext = vec![0u8; ciphertext_len];

            let result = arth_rt_aead_encrypt(
                algo,
                key.as_ptr(),
                key_size,
                nonce.as_ptr(),
                nonce_size,
                plaintext.as_ptr(),
                plaintext.len(),
                aad.as_ptr(),
                aad.len(),
                ciphertext.as_mut_ptr(),
                ciphertext_len,
            );
            assert_eq!(result as usize, ciphertext_len);

            // Decrypt with same AAD
            let mut decrypted = vec![0u8; plaintext.len()];
            let result = arth_rt_aead_decrypt(
                algo,
                key.as_ptr(),
                key_size,
                nonce.as_ptr(),
                nonce_size,
                ciphertext.as_ptr(),
                ciphertext_len,
                aad.as_ptr(),
                aad.len(),
                decrypted.as_mut_ptr(),
                plaintext.len(),
            );
            assert_eq!(result as usize, plaintext.len());
            assert_eq!(&decrypted[..], &plaintext[..]);
        }
    }

    #[test]
    fn test_aead_wrong_aad_fails() {
        let plaintext = b"Secret message";
        let aad = b"Correct AAD";
        let wrong_aad = b"Wrong AAD!!";

        let algo = 0; // AES-128-GCM
        let key_size = arth_rt_aead_key_size(algo);
        let nonce_size = arth_rt_aead_nonce_size(algo);
        let tag_size = arth_rt_aead_tag_size(algo);

        let mut key = vec![0u8; key_size];
        let mut nonce = vec![0u8; nonce_size];
        arth_rt_aead_generate_key(algo, key.as_mut_ptr(), key_size);
        arth_rt_aead_generate_nonce(algo, nonce.as_mut_ptr(), nonce_size);

        // Encrypt with correct AAD
        let ciphertext_len = plaintext.len() + tag_size;
        let mut ciphertext = vec![0u8; ciphertext_len];
        arth_rt_aead_encrypt(
            algo,
            key.as_ptr(),
            key_size,
            nonce.as_ptr(),
            nonce_size,
            plaintext.as_ptr(),
            plaintext.len(),
            aad.as_ptr(),
            aad.len(),
            ciphertext.as_mut_ptr(),
            ciphertext_len,
        );

        // Decrypt with wrong AAD should fail
        let mut decrypted = vec![0u8; plaintext.len()];
        let result = arth_rt_aead_decrypt(
            algo,
            key.as_ptr(),
            key_size,
            nonce.as_ptr(),
            nonce_size,
            ciphertext.as_ptr(),
            ciphertext_len,
            wrong_aad.as_ptr(),
            wrong_aad.len(),
            decrypted.as_mut_ptr(),
            plaintext.len(),
        );
        assert_eq!(result, -7, "Decryption with wrong AAD should fail");
    }

    #[test]
    fn test_aead_tampered_ciphertext_fails() {
        let plaintext = b"Secret message";

        for algo in 0..=2 {
            let key_size = arth_rt_aead_key_size(algo);
            let nonce_size = arth_rt_aead_nonce_size(algo);
            let tag_size = arth_rt_aead_tag_size(algo);

            let mut key = vec![0u8; key_size];
            let mut nonce = vec![0u8; nonce_size];
            arth_rt_aead_generate_key(algo, key.as_mut_ptr(), key_size);
            arth_rt_aead_generate_nonce(algo, nonce.as_mut_ptr(), nonce_size);

            // Encrypt
            let ciphertext_len = plaintext.len() + tag_size;
            let mut ciphertext = vec![0u8; ciphertext_len];
            arth_rt_aead_encrypt(
                algo,
                key.as_ptr(),
                key_size,
                nonce.as_ptr(),
                nonce_size,
                plaintext.as_ptr(),
                plaintext.len(),
                std::ptr::null(),
                0,
                ciphertext.as_mut_ptr(),
                ciphertext_len,
            );

            // Tamper with ciphertext
            ciphertext[0] ^= 0xff;

            // Decrypt should fail
            let mut decrypted = vec![0u8; plaintext.len()];
            let result = arth_rt_aead_decrypt(
                algo,
                key.as_ptr(),
                key_size,
                nonce.as_ptr(),
                nonce_size,
                ciphertext.as_ptr(),
                ciphertext_len,
                std::ptr::null(),
                0,
                decrypted.as_mut_ptr(),
                plaintext.len(),
            );
            assert_eq!(
                result, -7,
                "Decryption of tampered ciphertext should fail for algo {}",
                algo
            );
        }
    }

    #[test]
    fn test_aead_wrong_key_fails() {
        let plaintext = b"Secret message";

        let algo = 1; // AES-256-GCM
        let key_size = arth_rt_aead_key_size(algo);
        let nonce_size = arth_rt_aead_nonce_size(algo);
        let tag_size = arth_rt_aead_tag_size(algo);

        let mut key = vec![0u8; key_size];
        let mut wrong_key = vec![0u8; key_size];
        let mut nonce = vec![0u8; nonce_size];
        arth_rt_aead_generate_key(algo, key.as_mut_ptr(), key_size);
        arth_rt_aead_generate_key(algo, wrong_key.as_mut_ptr(), key_size);
        arth_rt_aead_generate_nonce(algo, nonce.as_mut_ptr(), nonce_size);

        // Encrypt with correct key
        let ciphertext_len = plaintext.len() + tag_size;
        let mut ciphertext = vec![0u8; ciphertext_len];
        arth_rt_aead_encrypt(
            algo,
            key.as_ptr(),
            key_size,
            nonce.as_ptr(),
            nonce_size,
            plaintext.as_ptr(),
            plaintext.len(),
            std::ptr::null(),
            0,
            ciphertext.as_mut_ptr(),
            ciphertext_len,
        );

        // Decrypt with wrong key should fail
        let mut decrypted = vec![0u8; plaintext.len()];
        let result = arth_rt_aead_decrypt(
            algo,
            wrong_key.as_ptr(),
            key_size,
            nonce.as_ptr(),
            nonce_size,
            ciphertext.as_ptr(),
            ciphertext_len,
            std::ptr::null(),
            0,
            decrypted.as_mut_ptr(),
            plaintext.len(),
        );
        assert_eq!(result, -7, "Decryption with wrong key should fail");
    }

    #[test]
    fn test_aead_empty_plaintext() {
        // Encrypting empty plaintext should produce just the tag
        for algo in 0..=2 {
            let key_size = arth_rt_aead_key_size(algo);
            let nonce_size = arth_rt_aead_nonce_size(algo);
            let tag_size = arth_rt_aead_tag_size(algo);

            let mut key = vec![0u8; key_size];
            let mut nonce = vec![0u8; nonce_size];
            arth_rt_aead_generate_key(algo, key.as_mut_ptr(), key_size);
            arth_rt_aead_generate_nonce(algo, nonce.as_mut_ptr(), nonce_size);

            // Encrypt empty plaintext
            let mut ciphertext = vec![0u8; tag_size];
            let result = arth_rt_aead_encrypt(
                algo,
                key.as_ptr(),
                key_size,
                nonce.as_ptr(),
                nonce_size,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                ciphertext.as_mut_ptr(),
                tag_size,
            );
            assert_eq!(
                result as usize, tag_size,
                "Empty encrypt should produce tag for algo {}",
                algo
            );

            // Decrypt should produce empty output
            let mut decrypted = vec![0u8; 0];
            let result = arth_rt_aead_decrypt(
                algo,
                key.as_ptr(),
                key_size,
                nonce.as_ptr(),
                nonce_size,
                ciphertext.as_ptr(),
                tag_size,
                std::ptr::null(),
                0,
                decrypted.as_mut_ptr(),
                0,
            );
            assert_eq!(
                result, 0,
                "Decrypt of empty ciphertext should succeed for algo {}",
                algo
            );
        }
    }

    // NIST AES-GCM test vector (Test Case 3 from GCM Specification)
    #[test]
    fn test_aes_128_gcm_nist_vector() {
        // NIST AES-GCM Test Case 3
        // Key: 00000000000000000000000000000000
        // IV:  000000000000000000000000
        // PT:  (empty)
        // AAD: (empty)
        // CT:  (empty)
        // Tag: 58e2fccefa7e3061367f1d57a4e7455a
        let key = [0u8; 16];
        let nonce = [0u8; 12];
        let expected_tag = hex_to_bytes("58e2fccefa7e3061367f1d57a4e7455a");

        let mut ciphertext = vec![0u8; 16]; // Just the tag
        let result = arth_rt_aead_encrypt(
            0, // AES-128-GCM
            key.as_ptr(),
            16,
            nonce.as_ptr(),
            12,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            ciphertext.as_mut_ptr(),
            16,
        );
        assert_eq!(result, 16);
        assert_eq!(
            &ciphertext[..],
            &expected_tag[..],
            "AES-128-GCM NIST test vector failed"
        );
    }

    // NIST AES-GCM test vector (Test Case 4)
    #[test]
    fn test_aes_128_gcm_nist_vector_with_plaintext() {
        // NIST AES-GCM Test Case 4
        // Key: 00000000000000000000000000000000
        // IV:  000000000000000000000000
        // PT:  00000000000000000000000000000000
        // AAD: (empty)
        // CT:  0388dace60b6a392f328c2b971b2fe78
        // Tag: ab6e47d42cec13bdf53a67b21257bddf
        let key = [0u8; 16];
        let nonce = [0u8; 12];
        let plaintext = [0u8; 16];
        let expected_ct = hex_to_bytes("0388dace60b6a392f328c2b971b2fe78");
        let expected_tag = hex_to_bytes("ab6e47d42cec13bdf53a67b21257bddf");

        let mut ciphertext = vec![0u8; 32]; // 16 CT + 16 tag
        let result = arth_rt_aead_encrypt(
            0,
            key.as_ptr(),
            16,
            nonce.as_ptr(),
            12,
            plaintext.as_ptr(),
            16,
            std::ptr::null(),
            0,
            ciphertext.as_mut_ptr(),
            32,
        );
        assert_eq!(result, 32);
        assert_eq!(
            &ciphertext[..16],
            &expected_ct[..],
            "AES-128-GCM ciphertext mismatch"
        );
        assert_eq!(
            &ciphertext[16..],
            &expected_tag[..],
            "AES-128-GCM tag mismatch"
        );

        // Verify decryption
        let mut decrypted = vec![0u8; 16];
        let result = arth_rt_aead_decrypt(
            0,
            key.as_ptr(),
            16,
            nonce.as_ptr(),
            12,
            ciphertext.as_ptr(),
            32,
            std::ptr::null(),
            0,
            decrypted.as_mut_ptr(),
            16,
        );
        assert_eq!(result, 16);
        assert_eq!(&decrypted[..], &plaintext[..]);
    }

    // RFC 8439 ChaCha20-Poly1305 test vector
    #[test]
    fn test_chacha20_poly1305_rfc8439() {
        // RFC 8439 Section 2.8.2 Example
        let key = hex_to_bytes("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
        let nonce = hex_to_bytes("070000004041424344454647");
        let aad = hex_to_bytes("50515253c0c1c2c3c4c5c6c7");
        let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";

        let expected_ct = hex_to_bytes(
            "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d6\
             3dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b36\
             92ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc\
             3ff4def08e4b7a9de576d26586cec64b6116",
        );
        let expected_tag = hex_to_bytes("1ae10b594f09e26a7e902ecbd0600691");

        let ciphertext_len = plaintext.len() + 16;
        let mut ciphertext = vec![0u8; ciphertext_len];

        let result = arth_rt_aead_encrypt(
            2, // ChaCha20-Poly1305
            key.as_ptr(),
            32,
            nonce.as_ptr(),
            12,
            plaintext.as_ptr(),
            plaintext.len(),
            aad.as_ptr(),
            aad.len(),
            ciphertext.as_mut_ptr(),
            ciphertext_len,
        );
        assert_eq!(result as usize, ciphertext_len);

        let ct_len = plaintext.len();
        assert_eq!(
            &ciphertext[..ct_len],
            &expected_ct[..],
            "ChaCha20-Poly1305 ciphertext mismatch"
        );
        assert_eq!(
            &ciphertext[ct_len..],
            &expected_tag[..],
            "ChaCha20-Poly1305 tag mismatch"
        );

        // Verify decryption
        let mut decrypted = vec![0u8; plaintext.len()];
        let result = arth_rt_aead_decrypt(
            2,
            key.as_ptr(),
            32,
            nonce.as_ptr(),
            12,
            ciphertext.as_ptr(),
            ciphertext_len,
            aad.as_ptr(),
            aad.len(),
            decrypted.as_mut_ptr(),
            plaintext.len(),
        );
        assert_eq!(result as usize, plaintext.len());
        assert_eq!(&decrypted[..], &plaintext[..]);
    }

    #[test]
    fn test_aead_invalid_key_size() {
        let plaintext = b"test";
        let key = [0u8; 8]; // Wrong size
        let nonce = [0u8; 12];
        let mut output = [0u8; 32];

        let result = arth_rt_aead_encrypt(
            0, // AES-128-GCM expects 16-byte key
            key.as_ptr(),
            8,
            nonce.as_ptr(),
            12,
            plaintext.as_ptr(),
            4,
            std::ptr::null(),
            0,
            output.as_mut_ptr(),
            32,
        );
        assert_eq!(result, -4, "Wrong key size should fail");
    }

    #[test]
    fn test_aead_invalid_nonce_size() {
        let plaintext = b"test";
        let key = [0u8; 16];
        let nonce = [0u8; 8]; // Wrong size
        let mut output = [0u8; 32];

        let result = arth_rt_aead_encrypt(
            0, // AES-128-GCM expects 12-byte nonce
            key.as_ptr(),
            16,
            nonce.as_ptr(),
            8,
            plaintext.as_ptr(),
            4,
            std::ptr::null(),
            0,
            output.as_mut_ptr(),
            32,
        );
        assert_eq!(result, -5, "Wrong nonce size should fail");
    }

    #[test]
    fn test_aead_buffer_too_small() {
        let plaintext = b"test message";
        let key = [0u8; 16];
        let nonce = [0u8; 12];
        let mut output = [0u8; 10]; // Too small for plaintext + tag

        let result = arth_rt_aead_encrypt(
            0,
            key.as_ptr(),
            16,
            nonce.as_ptr(),
            12,
            plaintext.as_ptr(),
            plaintext.len(),
            std::ptr::null(),
            0,
            output.as_mut_ptr(),
            output.len(),
        );
        assert_eq!(result, -1, "Buffer too small should fail");
    }

    #[test]
    fn test_aead_large_message() {
        // Test with a larger message (64KB)
        let size = 64 * 1024;
        let plaintext: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();

        let algo = 2; // ChaCha20-Poly1305
        let key_size = arth_rt_aead_key_size(algo);
        let nonce_size = arth_rt_aead_nonce_size(algo);
        let tag_size = arth_rt_aead_tag_size(algo);

        let mut key = vec![0u8; key_size];
        let mut nonce = vec![0u8; nonce_size];
        arth_rt_aead_generate_key(algo, key.as_mut_ptr(), key_size);
        arth_rt_aead_generate_nonce(algo, nonce.as_mut_ptr(), nonce_size);

        let ciphertext_len = size + tag_size;
        let mut ciphertext = vec![0u8; ciphertext_len];

        let result = arth_rt_aead_encrypt(
            algo,
            key.as_ptr(),
            key_size,
            nonce.as_ptr(),
            nonce_size,
            plaintext.as_ptr(),
            size,
            std::ptr::null(),
            0,
            ciphertext.as_mut_ptr(),
            ciphertext_len,
        );
        assert_eq!(result as usize, ciphertext_len);

        let mut decrypted = vec![0u8; size];
        let result = arth_rt_aead_decrypt(
            algo,
            key.as_ptr(),
            key_size,
            nonce.as_ptr(),
            nonce_size,
            ciphertext.as_ptr(),
            ciphertext_len,
            std::ptr::null(),
            0,
            decrypted.as_mut_ptr(),
            size,
        );
        assert_eq!(result as usize, size);
        assert_eq!(&decrypted[..], &plaintext[..]);
    }

    // =========================================================================
    // Key Generation Tests - Signature Algorithms
    // =========================================================================

    #[test]
    fn test_signature_key_sizes() {
        // Ed25519
        assert_eq!(arth_rt_signature_private_key_size(0), 32);
        assert_eq!(arth_rt_signature_public_key_size(0), 32);
        assert_eq!(arth_rt_signature_size(0), 64);

        // ECDSA P-256
        assert_eq!(arth_rt_signature_private_key_size(1), 32);
        assert_eq!(arth_rt_signature_public_key_size(1), 33);
        assert_eq!(arth_rt_signature_size(1), 64);

        // ECDSA P-384
        assert_eq!(arth_rt_signature_private_key_size(2), 48);
        assert_eq!(arth_rt_signature_public_key_size(2), 49);
        assert_eq!(arth_rt_signature_size(2), 96);

        // Invalid algorithm
        assert_eq!(arth_rt_signature_private_key_size(99), 0);
        assert_eq!(arth_rt_signature_public_key_size(99), 0);
        assert_eq!(arth_rt_signature_size(99), 0);
    }

    #[test]
    fn test_ed25519_keygen() {
        let mut private_key = [0u8; 32];
        let mut public_key = [0u8; 32];

        let result = arth_rt_signature_generate_keypair(
            0, // Ed25519
            private_key.as_mut_ptr(),
            32,
            public_key.as_mut_ptr(),
            32,
        );
        assert_eq!(result, 0);

        // Keys should not be all zeros
        assert!(!private_key.iter().all(|&b| b == 0));
        assert!(!public_key.iter().all(|&b| b == 0));

        // Derive public key from private key
        let mut derived_public = [0u8; 32];
        let result = arth_rt_signature_derive_public_key(
            0,
            private_key.as_ptr(),
            32,
            derived_public.as_mut_ptr(),
            32,
        );
        assert_eq!(result, 0);

        // Derived public key should match the generated one
        assert_eq!(&public_key[..], &derived_public[..]);
    }

    #[test]
    fn test_ecdsa_p256_keygen() {
        let mut private_key = [0u8; 32];
        let mut public_key = [0u8; 33];

        let result = arth_rt_signature_generate_keypair(
            1, // ECDSA P-256
            private_key.as_mut_ptr(),
            32,
            public_key.as_mut_ptr(),
            33,
        );
        assert_eq!(result, 0);

        // Keys should not be all zeros
        assert!(!private_key.iter().all(|&b| b == 0));
        assert!(!public_key.iter().all(|&b| b == 0));

        // First byte of compressed public key should be 0x02 or 0x03
        assert!(public_key[0] == 0x02 || public_key[0] == 0x03);

        // Derive public key from private key
        let mut derived_public = [0u8; 33];
        let result = arth_rt_signature_derive_public_key(
            1,
            private_key.as_ptr(),
            32,
            derived_public.as_mut_ptr(),
            33,
        );
        assert_eq!(result, 0);

        // Derived public key should match the generated one
        assert_eq!(&public_key[..], &derived_public[..]);
    }

    #[test]
    fn test_ecdsa_p384_keygen() {
        let mut private_key = [0u8; 48];
        let mut public_key = [0u8; 49];

        let result = arth_rt_signature_generate_keypair(
            2, // ECDSA P-384
            private_key.as_mut_ptr(),
            48,
            public_key.as_mut_ptr(),
            49,
        );
        assert_eq!(result, 0);

        // Keys should not be all zeros
        assert!(!private_key.iter().all(|&b| b == 0));
        assert!(!public_key.iter().all(|&b| b == 0));

        // First byte of compressed public key should be 0x02 or 0x03
        assert!(public_key[0] == 0x02 || public_key[0] == 0x03);

        // Derive public key from private key
        let mut derived_public = [0u8; 49];
        let result = arth_rt_signature_derive_public_key(
            2,
            private_key.as_ptr(),
            48,
            derived_public.as_mut_ptr(),
            49,
        );
        assert_eq!(result, 0);

        // Derived public key should match the generated one
        assert_eq!(&public_key[..], &derived_public[..]);
    }

    #[test]
    fn test_rsa_pss_keygen() {
        let mut private_key = [0u8; RSA_2048_PRIVATE_KEY_MAX];
        let mut public_key = [0u8; RSA_2048_PUBLIC_KEY_MAX];
        let mut actual_priv_len: usize = 0;
        let mut actual_pub_len: usize = 0;

        let result = arth_rt_signature_generate_keypair_ex(
            3, // RSA-PSS-SHA256
            private_key.as_mut_ptr(),
            RSA_2048_PRIVATE_KEY_MAX,
            public_key.as_mut_ptr(),
            RSA_2048_PUBLIC_KEY_MAX,
            &mut actual_priv_len as *mut usize,
            &mut actual_pub_len as *mut usize,
        );
        assert_eq!(result, 0);

        // Keys should not be all zeros
        assert!(!private_key[..actual_priv_len].iter().all(|&b| b == 0));
        assert!(!public_key[..actual_pub_len].iter().all(|&b| b == 0));

        // RSA-2048 private key should be ~1190-1220 bytes
        assert!(actual_priv_len > 1100 && actual_priv_len < 1300);

        // RSA-2048 public key should be ~270 bytes
        assert!(actual_pub_len > 250 && actual_pub_len < 300);
    }

    #[test]
    fn test_rsa_pss_sign_verify() {
        // Generate RSA key pair
        let mut private_key = [0u8; RSA_2048_PRIVATE_KEY_MAX];
        let mut public_key = [0u8; RSA_2048_PUBLIC_KEY_MAX];
        let mut actual_priv_len: usize = 0;
        let mut actual_pub_len: usize = 0;

        let result = arth_rt_signature_generate_keypair_ex(
            3, // RSA-PSS-SHA256
            private_key.as_mut_ptr(),
            RSA_2048_PRIVATE_KEY_MAX,
            public_key.as_mut_ptr(),
            RSA_2048_PUBLIC_KEY_MAX,
            &mut actual_priv_len as *mut usize,
            &mut actual_pub_len as *mut usize,
        );
        assert_eq!(result, 0);

        // Test message
        let message = b"Hello, RSA-PSS!";

        // Sign the message
        let mut signature = [0u8; RSA_2048_SIGNATURE_SIZE];
        let sig_result = arth_rt_signature_sign(
            3, // RSA-PSS-SHA256
            private_key.as_ptr(),
            RSA_2048_PRIVATE_KEY_MAX,
            message.as_ptr(),
            message.len(),
            signature.as_mut_ptr(),
            RSA_2048_SIGNATURE_SIZE,
        );
        assert_eq!(sig_result, RSA_2048_SIGNATURE_SIZE as i64);

        // Verify the signature
        let verify_result = arth_rt_signature_verify(
            3, // RSA-PSS-SHA256
            public_key.as_ptr(),
            RSA_2048_PUBLIC_KEY_MAX,
            message.as_ptr(),
            message.len(),
            signature.as_ptr(),
            RSA_2048_SIGNATURE_SIZE,
        );
        assert_eq!(verify_result, 1);

        // Tamper with message and verify should fail
        let tampered_message = b"Hello, RSA-PSS?";
        let verify_result = arth_rt_signature_verify(
            3,
            public_key.as_ptr(),
            RSA_2048_PUBLIC_KEY_MAX,
            tampered_message.as_ptr(),
            tampered_message.len(),
            signature.as_ptr(),
            RSA_2048_SIGNATURE_SIZE,
        );
        assert_eq!(verify_result, 0);
    }

    #[test]
    fn test_rsa_pss_derive_public_key() {
        // Generate RSA key pair
        let mut private_key = [0u8; RSA_2048_PRIVATE_KEY_MAX];
        let mut public_key = [0u8; RSA_2048_PUBLIC_KEY_MAX];
        let mut actual_priv_len: usize = 0;
        let mut actual_pub_len: usize = 0;

        let result = arth_rt_signature_generate_keypair_ex(
            3, // RSA-PSS-SHA256
            private_key.as_mut_ptr(),
            RSA_2048_PRIVATE_KEY_MAX,
            public_key.as_mut_ptr(),
            RSA_2048_PUBLIC_KEY_MAX,
            &mut actual_priv_len as *mut usize,
            &mut actual_pub_len as *mut usize,
        );
        assert_eq!(result, 0);

        // Derive public key from private key
        let mut derived_public = [0u8; RSA_2048_PUBLIC_KEY_MAX];
        let result = arth_rt_signature_derive_public_key(
            3,
            private_key.as_ptr(),
            RSA_2048_PRIVATE_KEY_MAX,
            derived_public.as_mut_ptr(),
            RSA_2048_PUBLIC_KEY_MAX,
        );
        assert_eq!(result, 0);

        // Derived public key should match the generated one
        assert_eq!(
            &public_key[..actual_pub_len],
            &derived_public[..actual_pub_len]
        );
    }

    #[test]
    fn test_signature_keygen_wrong_sizes() {
        // Wrong private key size
        let mut private_key = [0u8; 16]; // Wrong size
        let mut public_key = [0u8; 32];
        let result = arth_rt_signature_generate_keypair(
            0,
            private_key.as_mut_ptr(),
            16,
            public_key.as_mut_ptr(),
            32,
        );
        assert_eq!(result, -1);

        // Wrong public key size
        let mut private_key = [0u8; 32];
        let mut public_key = [0u8; 16]; // Wrong size
        let result = arth_rt_signature_generate_keypair(
            0,
            private_key.as_mut_ptr(),
            32,
            public_key.as_mut_ptr(),
            16,
        );
        assert_eq!(result, -2);
    }

    #[test]
    fn test_signature_keygen_null_pointers() {
        let mut public_key = [0u8; 32];
        let result = arth_rt_signature_generate_keypair(
            0,
            std::ptr::null_mut(),
            32,
            public_key.as_mut_ptr(),
            32,
        );
        assert_eq!(result, -3);

        let mut private_key = [0u8; 32];
        let result = arth_rt_signature_generate_keypair(
            0,
            private_key.as_mut_ptr(),
            32,
            std::ptr::null_mut(),
            32,
        );
        assert_eq!(result, -4);
    }

    #[test]
    fn test_signature_keygen_invalid_algorithm() {
        let mut private_key = [0u8; 32];
        let mut public_key = [0u8; 32];
        let result = arth_rt_signature_generate_keypair(
            99,
            private_key.as_mut_ptr(),
            32,
            public_key.as_mut_ptr(),
            32,
        );
        assert_eq!(result, -5);
    }

    #[test]
    fn test_ed25519_derive_invalid_key() {
        // Invalid private key (all zeros) should still work for Ed25519
        // (it's just a weak key, not invalid)
        let private_key = [0u8; 32];
        let mut public_key = [0u8; 32];
        let result = arth_rt_signature_derive_public_key(
            0,
            private_key.as_ptr(),
            32,
            public_key.as_mut_ptr(),
            32,
        );
        // Ed25519 accepts any 32-byte seed
        assert_eq!(result, 0);
    }

    // =========================================================================
    // Key Generation Tests - Key Exchange Algorithms
    // =========================================================================

    #[test]
    fn test_kex_key_sizes() {
        // X25519
        assert_eq!(arth_rt_kex_private_key_size(0), 32);
        assert_eq!(arth_rt_kex_public_key_size(0), 32);
        assert_eq!(arth_rt_kex_shared_secret_size(0), 32);

        // ECDH P-256
        assert_eq!(arth_rt_kex_private_key_size(1), 32);
        assert_eq!(arth_rt_kex_public_key_size(1), 33);
        assert_eq!(arth_rt_kex_shared_secret_size(1), 32);

        // ECDH P-384
        assert_eq!(arth_rt_kex_private_key_size(2), 48);
        assert_eq!(arth_rt_kex_public_key_size(2), 49);
        assert_eq!(arth_rt_kex_shared_secret_size(2), 48);

        // Invalid algorithm
        assert_eq!(arth_rt_kex_private_key_size(99), 0);
        assert_eq!(arth_rt_kex_public_key_size(99), 0);
        assert_eq!(arth_rt_kex_shared_secret_size(99), 0);
    }

    #[test]
    fn test_x25519_keygen() {
        let mut private_key = [0u8; 32];
        let mut public_key = [0u8; 32];

        let result = arth_rt_kex_generate_keypair(
            0, // X25519
            private_key.as_mut_ptr(),
            32,
            public_key.as_mut_ptr(),
            32,
        );
        assert_eq!(result, 0);

        // Keys should not be all zeros
        assert!(!private_key.iter().all(|&b| b == 0));
        assert!(!public_key.iter().all(|&b| b == 0));

        // Derive public key from private key
        let mut derived_public = [0u8; 32];
        let result = arth_rt_kex_derive_public_key(
            0,
            private_key.as_ptr(),
            32,
            derived_public.as_mut_ptr(),
            32,
        );
        assert_eq!(result, 0);

        // Derived public key should match the generated one
        assert_eq!(&public_key[..], &derived_public[..]);
    }

    #[test]
    fn test_ecdh_p256_keygen() {
        let mut private_key = [0u8; 32];
        let mut public_key = [0u8; 33];

        let result = arth_rt_kex_generate_keypair(
            1, // ECDH P-256
            private_key.as_mut_ptr(),
            32,
            public_key.as_mut_ptr(),
            33,
        );
        assert_eq!(result, 0);

        // Keys should not be all zeros
        assert!(!private_key.iter().all(|&b| b == 0));
        assert!(!public_key.iter().all(|&b| b == 0));

        // First byte of compressed public key should be 0x02 or 0x03
        assert!(public_key[0] == 0x02 || public_key[0] == 0x03);

        // Derive public key from private key
        let mut derived_public = [0u8; 33];
        let result = arth_rt_kex_derive_public_key(
            1,
            private_key.as_ptr(),
            32,
            derived_public.as_mut_ptr(),
            33,
        );
        assert_eq!(result, 0);

        // Derived public key should match the generated one
        assert_eq!(&public_key[..], &derived_public[..]);
    }

    #[test]
    fn test_ecdh_p384_keygen() {
        let mut private_key = [0u8; 48];
        let mut public_key = [0u8; 49];

        let result = arth_rt_kex_generate_keypair(
            2, // ECDH P-384
            private_key.as_mut_ptr(),
            48,
            public_key.as_mut_ptr(),
            49,
        );
        assert_eq!(result, 0);

        // Keys should not be all zeros
        assert!(!private_key.iter().all(|&b| b == 0));
        assert!(!public_key.iter().all(|&b| b == 0));

        // First byte of compressed public key should be 0x02 or 0x03
        assert!(public_key[0] == 0x02 || public_key[0] == 0x03);

        // Derive public key from private key
        let mut derived_public = [0u8; 49];
        let result = arth_rt_kex_derive_public_key(
            2,
            private_key.as_ptr(),
            48,
            derived_public.as_mut_ptr(),
            49,
        );
        assert_eq!(result, 0);

        // Derived public key should match the generated one
        assert_eq!(&public_key[..], &derived_public[..]);
    }

    #[test]
    fn test_kex_keygen_wrong_sizes() {
        // Wrong private key size
        let mut private_key = [0u8; 16]; // Wrong size
        let mut public_key = [0u8; 32];
        let result = arth_rt_kex_generate_keypair(
            0,
            private_key.as_mut_ptr(),
            16,
            public_key.as_mut_ptr(),
            32,
        );
        assert_eq!(result, -1);

        // Wrong public key size
        let mut private_key = [0u8; 32];
        let mut public_key = [0u8; 16]; // Wrong size
        let result = arth_rt_kex_generate_keypair(
            0,
            private_key.as_mut_ptr(),
            32,
            public_key.as_mut_ptr(),
            16,
        );
        assert_eq!(result, -2);
    }

    #[test]
    fn test_kex_keygen_null_pointers() {
        let mut public_key = [0u8; 32];
        let result =
            arth_rt_kex_generate_keypair(0, std::ptr::null_mut(), 32, public_key.as_mut_ptr(), 32);
        assert_eq!(result, -3);

        let mut private_key = [0u8; 32];
        let result =
            arth_rt_kex_generate_keypair(0, private_key.as_mut_ptr(), 32, std::ptr::null_mut(), 32);
        assert_eq!(result, -4);
    }

    #[test]
    fn test_kex_keygen_invalid_algorithm() {
        let mut private_key = [0u8; 32];
        let mut public_key = [0u8; 32];
        let result = arth_rt_kex_generate_keypair(
            99,
            private_key.as_mut_ptr(),
            32,
            public_key.as_mut_ptr(),
            32,
        );
        assert_eq!(result, -5);
    }

    // =========================================================================
    // Key Exchange Agreement Tests
    // =========================================================================

    #[test]
    fn test_x25519_agree_roundtrip() {
        // Generate Alice's key pair
        let mut alice_priv = [0u8; 32];
        let mut alice_pub = [0u8; 32];
        assert_eq!(
            arth_rt_kex_generate_keypair(
                0,
                alice_priv.as_mut_ptr(),
                32,
                alice_pub.as_mut_ptr(),
                32
            ),
            0
        );

        // Generate Bob's key pair
        let mut bob_priv = [0u8; 32];
        let mut bob_pub = [0u8; 32];
        assert_eq!(
            arth_rt_kex_generate_keypair(0, bob_priv.as_mut_ptr(), 32, bob_pub.as_mut_ptr(), 32),
            0
        );

        // Alice computes shared secret using her private key and Bob's public key
        let mut alice_shared = [0u8; 32];
        let result = arth_rt_kex_agree(
            0,
            alice_priv.as_ptr(),
            32,
            bob_pub.as_ptr(),
            32,
            alice_shared.as_mut_ptr(),
            32,
        );
        assert_eq!(result, 32);

        // Bob computes shared secret using his private key and Alice's public key
        let mut bob_shared = [0u8; 32];
        let result = arth_rt_kex_agree(
            0,
            bob_priv.as_ptr(),
            32,
            alice_pub.as_ptr(),
            32,
            bob_shared.as_mut_ptr(),
            32,
        );
        assert_eq!(result, 32);

        // Both should have computed the same shared secret
        assert_eq!(&alice_shared[..], &bob_shared[..]);
    }

    #[test]
    fn test_x25519_rfc7748_test_vector() {
        // RFC 7748 Section 6.1 Test Vector 1
        // Alice's private key (clamped)
        let alice_priv: [u8; 32] = [
            0x77, 0x07, 0x6d, 0x0a, 0x73, 0x18, 0xa5, 0x7d, 0x3c, 0x16, 0xc1, 0x72, 0x51, 0xb2,
            0x66, 0x45, 0xdf, 0x4c, 0x2f, 0x87, 0xeb, 0xc0, 0x99, 0x2a, 0xb1, 0x77, 0xfb, 0xa5,
            0x1d, 0xb9, 0x2c, 0x2a,
        ];
        // Bob's public key
        let bob_pub: [u8; 32] = [
            0xde, 0x9e, 0xdb, 0x7d, 0x7b, 0x7d, 0xc1, 0xb4, 0xd3, 0x5b, 0x61, 0xc2, 0xec, 0xe4,
            0x35, 0x37, 0x3f, 0x83, 0x43, 0xc8, 0x5b, 0x78, 0x67, 0x4d, 0xad, 0xfc, 0x7e, 0x14,
            0x6f, 0x88, 0x2b, 0x4f,
        ];
        // Expected shared secret
        let expected: [u8; 32] = [
            0x4a, 0x5d, 0x9d, 0x5b, 0xa4, 0xce, 0x2d, 0xe1, 0x72, 0x8e, 0x3b, 0xf4, 0x80, 0x35,
            0x0f, 0x25, 0xe0, 0x7e, 0x21, 0xc9, 0x47, 0xd1, 0x9e, 0x33, 0x76, 0xf0, 0x9b, 0x3c,
            0x1e, 0x16, 0x17, 0x42,
        ];

        let mut shared = [0u8; 32];
        let result = arth_rt_kex_agree(
            0,
            alice_priv.as_ptr(),
            32,
            bob_pub.as_ptr(),
            32,
            shared.as_mut_ptr(),
            32,
        );
        assert_eq!(result, 32);
        assert_eq!(&shared[..], &expected[..]);
    }

    #[test]
    fn test_ecdh_p256_agree_roundtrip() {
        // Generate Alice's key pair
        let mut alice_priv = [0u8; 32];
        let mut alice_pub = [0u8; 33]; // Compressed SEC1
        assert_eq!(
            arth_rt_kex_generate_keypair(
                1,
                alice_priv.as_mut_ptr(),
                32,
                alice_pub.as_mut_ptr(),
                33
            ),
            0
        );

        // Generate Bob's key pair
        let mut bob_priv = [0u8; 32];
        let mut bob_pub = [0u8; 33];
        assert_eq!(
            arth_rt_kex_generate_keypair(1, bob_priv.as_mut_ptr(), 32, bob_pub.as_mut_ptr(), 33),
            0
        );

        // Alice computes shared secret
        let mut alice_shared = [0u8; 32];
        let result = arth_rt_kex_agree(
            1,
            alice_priv.as_ptr(),
            32,
            bob_pub.as_ptr(),
            33,
            alice_shared.as_mut_ptr(),
            32,
        );
        assert_eq!(result, 32);

        // Bob computes shared secret
        let mut bob_shared = [0u8; 32];
        let result = arth_rt_kex_agree(
            1,
            bob_priv.as_ptr(),
            32,
            alice_pub.as_ptr(),
            33,
            bob_shared.as_mut_ptr(),
            32,
        );
        assert_eq!(result, 32);

        // Both should match
        assert_eq!(&alice_shared[..], &bob_shared[..]);
    }

    #[test]
    fn test_ecdh_p384_agree_roundtrip() {
        // Generate Alice's key pair
        let mut alice_priv = [0u8; 48];
        let mut alice_pub = [0u8; 49]; // Compressed SEC1
        assert_eq!(
            arth_rt_kex_generate_keypair(
                2,
                alice_priv.as_mut_ptr(),
                48,
                alice_pub.as_mut_ptr(),
                49
            ),
            0
        );

        // Generate Bob's key pair
        let mut bob_priv = [0u8; 48];
        let mut bob_pub = [0u8; 49];
        assert_eq!(
            arth_rt_kex_generate_keypair(2, bob_priv.as_mut_ptr(), 48, bob_pub.as_mut_ptr(), 49),
            0
        );

        // Alice computes shared secret
        let mut alice_shared = [0u8; 48];
        let result = arth_rt_kex_agree(
            2,
            alice_priv.as_ptr(),
            48,
            bob_pub.as_ptr(),
            49,
            alice_shared.as_mut_ptr(),
            48,
        );
        assert_eq!(result, 48);

        // Bob computes shared secret
        let mut bob_shared = [0u8; 48];
        let result = arth_rt_kex_agree(
            2,
            bob_priv.as_ptr(),
            48,
            alice_pub.as_ptr(),
            49,
            bob_shared.as_mut_ptr(),
            48,
        );
        assert_eq!(result, 48);

        // Both should match
        assert_eq!(&alice_shared[..], &bob_shared[..]);
    }

    #[test]
    fn test_kex_agree_wrong_private_key() {
        // Generate Alice's key pair
        let mut alice_priv = [0u8; 32];
        let mut alice_pub = [0u8; 32];
        arth_rt_kex_generate_keypair(0, alice_priv.as_mut_ptr(), 32, alice_pub.as_mut_ptr(), 32);

        // Generate Bob's key pair
        let mut bob_priv = [0u8; 32];
        let mut bob_pub = [0u8; 32];
        arth_rt_kex_generate_keypair(0, bob_priv.as_mut_ptr(), 32, bob_pub.as_mut_ptr(), 32);

        // Generate Charlie's key pair (intruder)
        let mut charlie_priv = [0u8; 32];
        let mut charlie_pub = [0u8; 32];
        arth_rt_kex_generate_keypair(
            0,
            charlie_priv.as_mut_ptr(),
            32,
            charlie_pub.as_mut_ptr(),
            32,
        );

        // Alice computes shared secret with Bob
        let mut alice_shared = [0u8; 32];
        arth_rt_kex_agree(
            0,
            alice_priv.as_ptr(),
            32,
            bob_pub.as_ptr(),
            32,
            alice_shared.as_mut_ptr(),
            32,
        );

        // Charlie tries to compute shared secret with Bob's public key
        let mut charlie_shared = [0u8; 32];
        arth_rt_kex_agree(
            0,
            charlie_priv.as_ptr(),
            32,
            bob_pub.as_ptr(),
            32,
            charlie_shared.as_mut_ptr(),
            32,
        );

        // Charlie's shared secret should be different from Alice's
        assert_ne!(&alice_shared[..], &charlie_shared[..]);
    }

    #[test]
    fn test_kex_agree_null_pointers() {
        let private_key = [0u8; 32];
        let public_key = [0u8; 32];
        let mut shared = [0u8; 32];

        // Null private key
        assert_eq!(
            arth_rt_kex_agree(
                0,
                std::ptr::null(),
                32,
                public_key.as_ptr(),
                32,
                shared.as_mut_ptr(),
                32
            ),
            -4
        );

        // Null public key
        assert_eq!(
            arth_rt_kex_agree(
                0,
                private_key.as_ptr(),
                32,
                std::ptr::null(),
                32,
                shared.as_mut_ptr(),
                32
            ),
            -5
        );

        // Null shared secret
        assert_eq!(
            arth_rt_kex_agree(
                0,
                private_key.as_ptr(),
                32,
                public_key.as_ptr(),
                32,
                std::ptr::null_mut(),
                32
            ),
            -6
        );
    }

    #[test]
    fn test_kex_agree_wrong_sizes() {
        let mut alice_priv = [0u8; 32];
        let mut alice_pub = [0u8; 32];
        arth_rt_kex_generate_keypair(0, alice_priv.as_mut_ptr(), 32, alice_pub.as_mut_ptr(), 32);
        let mut shared = [0u8; 32];

        // Wrong private key size
        assert_eq!(
            arth_rt_kex_agree(
                0,
                alice_priv.as_ptr(),
                16, // Wrong size
                alice_pub.as_ptr(),
                32,
                shared.as_mut_ptr(),
                32
            ),
            -1
        );

        // Wrong public key size
        assert_eq!(
            arth_rt_kex_agree(
                0,
                alice_priv.as_ptr(),
                32,
                alice_pub.as_ptr(),
                16, // Wrong size
                shared.as_mut_ptr(),
                32
            ),
            -2
        );

        // Shared secret buffer too small
        assert_eq!(
            arth_rt_kex_agree(
                0,
                alice_priv.as_ptr(),
                32,
                alice_pub.as_ptr(),
                32,
                shared.as_mut_ptr(),
                16 // Too small
            ),
            -3
        );
    }

    #[test]
    fn test_kex_agree_invalid_algorithm() {
        let private_key = [0u8; 32];
        let public_key = [0u8; 32];
        let mut shared = [0u8; 32];

        assert_eq!(
            arth_rt_kex_agree(
                99, // Invalid algorithm
                private_key.as_ptr(),
                32,
                public_key.as_ptr(),
                32,
                shared.as_mut_ptr(),
                32
            ),
            -7
        );
    }

    #[test]
    fn test_kex_agree_consistency() {
        // Same key agreement should produce the same result
        let mut alice_priv = [0u8; 32];
        let mut alice_pub = [0u8; 32];
        let mut bob_priv = [0u8; 32];
        let mut bob_pub = [0u8; 32];

        arth_rt_kex_generate_keypair(0, alice_priv.as_mut_ptr(), 32, alice_pub.as_mut_ptr(), 32);
        arth_rt_kex_generate_keypair(0, bob_priv.as_mut_ptr(), 32, bob_pub.as_mut_ptr(), 32);

        let mut shared1 = [0u8; 32];
        let mut shared2 = [0u8; 32];

        for _ in 0..5 {
            arth_rt_kex_agree(
                0,
                alice_priv.as_ptr(),
                32,
                bob_pub.as_ptr(),
                32,
                shared1.as_mut_ptr(),
                32,
            );
            arth_rt_kex_agree(
                0,
                alice_priv.as_ptr(),
                32,
                bob_pub.as_ptr(),
                32,
                shared2.as_mut_ptr(),
                32,
            );
            assert_eq!(&shared1[..], &shared2[..]);
        }
    }

    #[test]
    fn test_kex_agree_with_kdf_x25519() {
        // Generate key pairs
        let mut alice_priv = [0u8; 32];
        let mut alice_pub = [0u8; 32];
        let mut bob_priv = [0u8; 32];
        let mut bob_pub = [0u8; 32];

        arth_rt_kex_generate_keypair(0, alice_priv.as_mut_ptr(), 32, alice_pub.as_mut_ptr(), 32);
        arth_rt_kex_generate_keypair(0, bob_priv.as_mut_ptr(), 32, bob_pub.as_mut_ptr(), 32);

        let salt = b"test salt";
        let info = b"encryption key v1";

        // Derive 32-byte key using HKDF-SHA256
        let mut alice_key = [0u8; 32];
        let mut bob_key = [0u8; 32];

        let result = arth_rt_kex_agree_with_kdf(
            0, // X25519
            0, // SHA-256
            alice_priv.as_ptr(),
            32,
            bob_pub.as_ptr(),
            32,
            salt.as_ptr(),
            salt.len(),
            info.as_ptr(),
            info.len(),
            alice_key.as_mut_ptr(),
            32,
        );
        assert_eq!(result, 32);

        let result = arth_rt_kex_agree_with_kdf(
            0,
            0,
            bob_priv.as_ptr(),
            32,
            alice_pub.as_ptr(),
            32,
            salt.as_ptr(),
            salt.len(),
            info.as_ptr(),
            info.len(),
            bob_key.as_mut_ptr(),
            32,
        );
        assert_eq!(result, 32);

        // Both should derive the same key
        assert_eq!(&alice_key[..], &bob_key[..]);
    }

    #[test]
    fn test_kex_agree_with_kdf_different_lengths() {
        let mut priv_key = [0u8; 32];
        let mut pub_key = [0u8; 32];
        arth_rt_kex_generate_keypair(0, priv_key.as_mut_ptr(), 32, pub_key.as_mut_ptr(), 32);

        // Derive 16-byte key
        let mut key16 = [0u8; 16];
        let result = arth_rt_kex_agree_with_kdf(
            0,
            0,
            priv_key.as_ptr(),
            32,
            pub_key.as_ptr(),
            32,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            key16.as_mut_ptr(),
            16,
        );
        assert_eq!(result, 16);

        // Derive 64-byte key
        let mut key64 = [0u8; 64];
        let result = arth_rt_kex_agree_with_kdf(
            0,
            0,
            priv_key.as_ptr(),
            32,
            pub_key.as_ptr(),
            32,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            key64.as_mut_ptr(),
            64,
        );
        assert_eq!(result, 64);
    }

    #[test]
    fn test_kex_agree_with_kdf_sha512() {
        let mut alice_priv = [0u8; 32];
        let mut alice_pub = [0u8; 32];
        let mut bob_priv = [0u8; 32];
        let mut bob_pub = [0u8; 32];

        arth_rt_kex_generate_keypair(0, alice_priv.as_mut_ptr(), 32, alice_pub.as_mut_ptr(), 32);
        arth_rt_kex_generate_keypair(0, bob_priv.as_mut_ptr(), 32, bob_pub.as_mut_ptr(), 32);

        // Derive using HKDF-SHA512
        let mut alice_key = [0u8; 64];
        let mut bob_key = [0u8; 64];

        let result = arth_rt_kex_agree_with_kdf(
            0,
            2, // SHA-512
            alice_priv.as_ptr(),
            32,
            bob_pub.as_ptr(),
            32,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            alice_key.as_mut_ptr(),
            64,
        );
        assert_eq!(result, 64);

        let result = arth_rt_kex_agree_with_kdf(
            0,
            2,
            bob_priv.as_ptr(),
            32,
            alice_pub.as_ptr(),
            32,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            bob_key.as_mut_ptr(),
            64,
        );
        assert_eq!(result, 64);

        assert_eq!(&alice_key[..], &bob_key[..]);
    }

    #[test]
    fn test_kex_agree_with_kdf_invalid_hash() {
        let mut priv_key = [0u8; 32];
        let mut pub_key = [0u8; 32];
        arth_rt_kex_generate_keypair(0, priv_key.as_mut_ptr(), 32, pub_key.as_mut_ptr(), 32);

        let mut key = [0u8; 32];
        let result = arth_rt_kex_agree_with_kdf(
            0,
            99, // Invalid hash algorithm
            priv_key.as_ptr(),
            32,
            pub_key.as_ptr(),
            32,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            key.as_mut_ptr(),
            32,
        );
        assert_eq!(result, -7);
    }

    // =========================================================================
    // Key Derivation Function (KDF) Tests
    // =========================================================================

    #[test]
    fn test_hkdf_sha256_rfc5869_case1() {
        // RFC 5869 Test Case 1
        let ikm = hex_to_bytes("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");
        let salt = hex_to_bytes("000102030405060708090a0b0c");
        let info = hex_to_bytes("f0f1f2f3f4f5f6f7f8f9");
        let expected_okm = hex_to_bytes(
            "3cb25f25faacd57a90434f64d0362f2a\
             2d2d0a90cf1a5a4c5db02d56ecc4c5bf\
             34007208d5b887185865",
        );

        let mut okm = vec![0u8; 42];
        let result = arth_rt_hkdf_derive(
            0, // HKDF-SHA256
            ikm.as_ptr(),
            ikm.len(),
            salt.as_ptr(),
            salt.len(),
            info.as_ptr(),
            info.len(),
            okm.as_mut_ptr(),
            okm.len(),
        );

        assert_eq!(result, 42);
        assert_eq!(&okm[..], &expected_okm[..]);
    }

    #[test]
    fn test_hkdf_sha256_rfc5869_case2() {
        // RFC 5869 Test Case 2 - longer inputs/outputs
        let ikm = hex_to_bytes(
            "000102030405060708090a0b0c0d0e0f\
             101112131415161718191a1b1c1d1e1f\
             202122232425262728292a2b2c2d2e2f\
             303132333435363738393a3b3c3d3e3f\
             404142434445464748494a4b4c4d4e4f",
        );
        let salt = hex_to_bytes(
            "606162636465666768696a6b6c6d6e6f\
             707172737475767778797a7b7c7d7e7f\
             808182838485868788898a8b8c8d8e8f\
             909192939495969798999a9b9c9d9e9f\
             a0a1a2a3a4a5a6a7a8a9aaabacadaeaf",
        );
        let info = hex_to_bytes(
            "b0b1b2b3b4b5b6b7b8b9babbbcbdbebf\
             c0c1c2c3c4c5c6c7c8c9cacbcccdcecf\
             d0d1d2d3d4d5d6d7d8d9dadbdcdddedf\
             e0e1e2e3e4e5e6e7e8e9eaebecedeeef\
             f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff",
        );
        let expected_okm = hex_to_bytes(
            "b11e398dc80327a1c8e7f78c596a4934\
             4f012eda2d4efad8a050cc4c19afa97c\
             59045a99cac7827271cb41c65e590e09\
             da3275600c2f09b8367793a9aca3db71\
             cc30c58179ec3e87c14c01d5c1f3434f\
             1d87",
        );

        let mut okm = vec![0u8; 82];
        let result = arth_rt_hkdf_derive(
            0,
            ikm.as_ptr(),
            ikm.len(),
            salt.as_ptr(),
            salt.len(),
            info.as_ptr(),
            info.len(),
            okm.as_mut_ptr(),
            okm.len(),
        );

        assert_eq!(result, 82);
        assert_eq!(&okm[..], &expected_okm[..]);
    }

    #[test]
    fn test_hkdf_sha256_rfc5869_case3() {
        // RFC 5869 Test Case 3 - zero-length salt/info
        let ikm = hex_to_bytes("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");
        let expected_okm = hex_to_bytes(
            "8da4e775a563c18f715f802a063c5a31\
             b8a11f5c5ee1879ec3454e5f3c738d2d\
             9d201395faa4b61a96c8",
        );

        let mut okm = vec![0u8; 42];
        let result = arth_rt_hkdf_derive(
            0,
            ikm.as_ptr(),
            ikm.len(),
            std::ptr::null(), // No salt
            0,
            std::ptr::null(), // No info
            0,
            okm.as_mut_ptr(),
            okm.len(),
        );

        assert_eq!(result, 42);
        assert_eq!(&okm[..], &expected_okm[..]);
    }

    #[test]
    fn test_hkdf_sha512() {
        // Basic SHA-512 test
        let ikm = b"input key material";
        let salt = b"random salt";
        let info = b"context info";

        let mut okm = [0u8; 64];
        let result = arth_rt_hkdf_derive(
            2, // HKDF-SHA512
            ikm.as_ptr(),
            ikm.len(),
            salt.as_ptr(),
            salt.len(),
            info.as_ptr(),
            info.len(),
            okm.as_mut_ptr(),
            okm.len(),
        );

        assert_eq!(result, 64);
        // Verify output is not all zeros
        assert!(okm.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_hkdf_extract() {
        let ikm = hex_to_bytes("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");
        let salt = hex_to_bytes("000102030405060708090a0b0c");
        let expected_prk =
            hex_to_bytes("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5");

        let mut prk = [0u8; 32];
        let result = arth_rt_hkdf_extract(
            0,
            ikm.as_ptr(),
            ikm.len(),
            salt.as_ptr(),
            salt.len(),
            prk.as_mut_ptr(),
            prk.len(),
        );

        assert_eq!(result, 32);
        assert_eq!(&prk[..], &expected_prk[..]);
    }

    #[test]
    fn test_hkdf_errors() {
        let ikm = b"test";
        let mut okm = [0u8; 32];

        // Null IKM
        assert_eq!(
            arth_rt_hkdf_derive(
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                okm.as_mut_ptr(),
                32
            ),
            -2
        );

        // Null OKM
        assert_eq!(
            arth_rt_hkdf_derive(
                0,
                ikm.as_ptr(),
                ikm.len(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                32
            ),
            -3
        );

        // Zero OKM length
        assert_eq!(
            arth_rt_hkdf_derive(
                0,
                ikm.as_ptr(),
                ikm.len(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                okm.as_mut_ptr(),
                0
            ),
            -4
        );

        // Invalid algorithm
        assert_eq!(
            arth_rt_hkdf_derive(
                99,
                ikm.as_ptr(),
                ikm.len(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                okm.as_mut_ptr(),
                32
            ),
            -1
        );
    }

    #[test]
    fn test_pbkdf2_sha256_rfc6070_case1() {
        // RFC 6070 Test Vector 1
        let password = b"password";
        let salt = b"salt";
        let expected =
            hex_to_bytes("120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b");

        let mut okm = [0u8; 32];
        let result = arth_rt_pbkdf2_derive(
            3, // PBKDF2-SHA256
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            1, // 1 iteration
            okm.as_mut_ptr(),
            okm.len(),
        );

        assert_eq!(result, 32);
        assert_eq!(&okm[..], &expected[..]);
    }

    #[test]
    fn test_pbkdf2_sha256_rfc6070_case2() {
        // RFC 6070 Test Vector 2
        let password = b"password";
        let salt = b"salt";
        let expected =
            hex_to_bytes("ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43");

        let mut okm = [0u8; 32];
        let result = arth_rt_pbkdf2_derive(
            3, // PBKDF2-SHA256
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            2, // 2 iterations
            okm.as_mut_ptr(),
            okm.len(),
        );

        assert_eq!(result, 32);
        assert_eq!(&okm[..], &expected[..]);
    }

    #[test]
    fn test_pbkdf2_sha256_rfc6070_case3() {
        // RFC 6070 Test Vector 3 (4096 iterations)
        let password = b"password";
        let salt = b"salt";
        let expected =
            hex_to_bytes("c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a");

        let mut okm = [0u8; 32];
        let result = arth_rt_pbkdf2_derive(
            3,
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            4096,
            okm.as_mut_ptr(),
            okm.len(),
        );

        assert_eq!(result, 32);
        assert_eq!(&okm[..], &expected[..]);
    }

    #[test]
    fn test_pbkdf2_sha512() {
        // Basic SHA-512 test
        let password = b"password";
        let salt = b"salt";

        let mut okm = [0u8; 64];
        let result = arth_rt_pbkdf2_derive(
            4, // PBKDF2-SHA512
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            1000,
            okm.as_mut_ptr(),
            okm.len(),
        );

        assert_eq!(result, 64);
        assert!(okm.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_pbkdf2_errors() {
        let password = b"password";
        let salt = b"salt";
        let mut okm = [0u8; 32];

        // Null password
        assert_eq!(
            arth_rt_pbkdf2_derive(
                3,
                std::ptr::null(),
                0,
                salt.as_ptr(),
                salt.len(),
                1000,
                okm.as_mut_ptr(),
                32
            ),
            -2
        );

        // Null salt
        assert_eq!(
            arth_rt_pbkdf2_derive(
                3,
                password.as_ptr(),
                password.len(),
                std::ptr::null(),
                0,
                1000,
                okm.as_mut_ptr(),
                32
            ),
            -3
        );

        // Null output
        assert_eq!(
            arth_rt_pbkdf2_derive(
                3,
                password.as_ptr(),
                password.len(),
                salt.as_ptr(),
                salt.len(),
                1000,
                std::ptr::null_mut(),
                32
            ),
            -4
        );

        // Zero iterations
        assert_eq!(
            arth_rt_pbkdf2_derive(
                3,
                password.as_ptr(),
                password.len(),
                salt.as_ptr(),
                salt.len(),
                0,
                okm.as_mut_ptr(),
                32
            ),
            -5
        );

        // Invalid algorithm
        assert_eq!(
            arth_rt_pbkdf2_derive(
                99,
                password.as_ptr(),
                password.len(),
                salt.as_ptr(),
                salt.len(),
                1000,
                okm.as_mut_ptr(),
                32
            ),
            -1
        );
    }

    #[test]
    fn test_argon2id_basic() {
        let password = b"password";
        let salt = b"somesalt12345678"; // 16 bytes minimum

        let mut okm = [0u8; 32];
        let result = arth_rt_argon2_derive(
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            4096, // 4 MB memory (small for testing)
            1,    // 1 iteration
            1,    // 1 parallel lane
            okm.as_mut_ptr(),
            okm.len(),
        );

        assert_eq!(result, 32);
        assert!(okm.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_argon2id_consistency() {
        let password = b"password";
        let salt = b"consistentsalt16"; // 16 bytes

        let mut okm1 = [0u8; 32];
        let mut okm2 = [0u8; 32];

        arth_rt_argon2_derive(
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            4096,
            1,
            1,
            okm1.as_mut_ptr(),
            okm1.len(),
        );

        arth_rt_argon2_derive(
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            4096,
            1,
            1,
            okm2.as_mut_ptr(),
            okm2.len(),
        );

        // Same inputs should produce same output
        assert_eq!(&okm1[..], &okm2[..]);
    }

    #[test]
    fn test_argon2id_different_passwords() {
        let password1 = b"password1";
        let password2 = b"password2";
        let salt = b"samesalt12345678";

        let mut okm1 = [0u8; 32];
        let mut okm2 = [0u8; 32];

        arth_rt_argon2_derive(
            password1.as_ptr(),
            password1.len(),
            salt.as_ptr(),
            salt.len(),
            4096,
            1,
            1,
            okm1.as_mut_ptr(),
            okm1.len(),
        );

        arth_rt_argon2_derive(
            password2.as_ptr(),
            password2.len(),
            salt.as_ptr(),
            salt.len(),
            4096,
            1,
            1,
            okm2.as_mut_ptr(),
            okm2.len(),
        );

        // Different passwords should produce different outputs
        assert_ne!(&okm1[..], &okm2[..]);
    }

    #[test]
    fn test_argon2id_different_salts() {
        let password = b"samepassword";
        let salt1 = b"salt1234567890ab";
        let salt2 = b"salt0987654321cd";

        let mut okm1 = [0u8; 32];
        let mut okm2 = [0u8; 32];

        arth_rt_argon2_derive(
            password.as_ptr(),
            password.len(),
            salt1.as_ptr(),
            salt1.len(),
            4096,
            1,
            1,
            okm1.as_mut_ptr(),
            okm1.len(),
        );

        arth_rt_argon2_derive(
            password.as_ptr(),
            password.len(),
            salt2.as_ptr(),
            salt2.len(),
            4096,
            1,
            1,
            okm2.as_mut_ptr(),
            okm2.len(),
        );

        // Different salts should produce different outputs
        assert_ne!(&okm1[..], &okm2[..]);
    }

    #[test]
    fn test_argon2id_errors() {
        let password = b"password";
        let salt = b"somesalt12345678";
        let mut okm = [0u8; 32];

        // Null password
        assert_eq!(
            arth_rt_argon2_derive(
                std::ptr::null(),
                0,
                salt.as_ptr(),
                salt.len(),
                4096,
                1,
                1,
                okm.as_mut_ptr(),
                okm.len(),
            ),
            -1
        );

        // Null salt
        assert_eq!(
            arth_rt_argon2_derive(
                password.as_ptr(),
                password.len(),
                std::ptr::null(),
                0,
                4096,
                1,
                1,
                okm.as_mut_ptr(),
                okm.len(),
            ),
            -2
        );

        // Salt too short
        assert_eq!(
            arth_rt_argon2_derive(
                password.as_ptr(),
                password.len(),
                salt.as_ptr(),
                4, // Too short
                4096,
                1,
                1,
                okm.as_mut_ptr(),
                okm.len(),
            ),
            -2
        );

        // Null output
        assert_eq!(
            arth_rt_argon2_derive(
                password.as_ptr(),
                password.len(),
                salt.as_ptr(),
                salt.len(),
                4096,
                1,
                1,
                std::ptr::null_mut(),
                32,
            ),
            -3
        );

        // Output too short
        assert_eq!(
            arth_rt_argon2_derive(
                password.as_ptr(),
                password.len(),
                salt.as_ptr(),
                salt.len(),
                4096,
                1,
                1,
                okm.as_mut_ptr(),
                2, // Too short (min 4)
            ),
            -4
        );
    }

    #[test]
    fn test_kdf_generate_salt() {
        let mut salt = [0u8; 32];
        let result = arth_rt_kdf_generate_salt(salt.as_mut_ptr(), salt.len());

        assert_eq!(result, 32);
        // Should not be all zeros
        assert!(salt.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_kdf_generate_salt_uniqueness() {
        let mut salt1 = [0u8; 16];
        let mut salt2 = [0u8; 16];

        arth_rt_kdf_generate_salt(salt1.as_mut_ptr(), salt1.len());
        arth_rt_kdf_generate_salt(salt2.as_mut_ptr(), salt2.len());

        // Two random salts should be different
        assert_ne!(&salt1[..], &salt2[..]);
    }

    #[test]
    fn test_kdf_generate_salt_errors() {
        // Null pointer
        assert_eq!(arth_rt_kdf_generate_salt(std::ptr::null_mut(), 16), -1);

        // Zero length should return 0 (no error, just no bytes)
        let mut salt = [0u8; 16];
        assert_eq!(arth_rt_kdf_generate_salt(salt.as_mut_ptr(), 0), 0);
    }

    // =========================================================================
    // Password Hashing Tests
    // =========================================================================

    #[test]
    fn test_argon2id_hash_verify_roundtrip() {
        let password = b"correct horse battery staple";
        let mut hash = [0u8; 256];

        // Hash the password
        let result = arth_rt_password_hash_argon2id(
            password.as_ptr(),
            password.len(),
            4096, // 4 MB (small for testing)
            1,
            1,
            hash.as_mut_ptr(),
            hash.len(),
        );
        assert!(result > 0, "Hash should succeed, got {}", result);

        // Verify the password
        let verify_result = arth_rt_password_verify_argon2id(
            password.as_ptr(),
            password.len(),
            hash.as_ptr() as *const std::ffi::c_char,
        );
        assert_eq!(verify_result, 1, "Correct password should verify");

        // Verify wrong password fails
        let wrong_password = b"wrong password";
        let verify_wrong = arth_rt_password_verify_argon2id(
            wrong_password.as_ptr(),
            wrong_password.len(),
            hash.as_ptr() as *const std::ffi::c_char,
        );
        assert_eq!(verify_wrong, 0, "Wrong password should not verify");
    }

    #[test]
    fn test_argon2id_phc_format() {
        let password = b"password123";
        let mut hash = [0u8; 256];

        let result = arth_rt_password_hash_argon2id(
            password.as_ptr(),
            password.len(),
            4096,
            1,
            1,
            hash.as_mut_ptr(),
            hash.len(),
        );
        assert!(result > 0);

        // Check that the hash is in PHC format
        let hash_str = std::str::from_utf8(&hash[..result as usize]).unwrap();
        assert!(
            hash_str.starts_with("$argon2id$"),
            "Hash should start with $argon2id$"
        );
        assert!(hash_str.contains("$v="), "Hash should contain version");
        assert!(hash_str.contains("$m="), "Hash should contain memory param");
        assert!(hash_str.contains(",t="), "Hash should contain time param");
        assert!(
            hash_str.contains(",p="),
            "Hash should contain parallelism param"
        );
    }

    #[test]
    fn test_argon2id_unique_salts() {
        let password = b"same password";
        let mut hash1 = [0u8; 256];
        let mut hash2 = [0u8; 256];

        arth_rt_password_hash_argon2id(
            password.as_ptr(),
            password.len(),
            4096,
            1,
            1,
            hash1.as_mut_ptr(),
            hash1.len(),
        );

        arth_rt_password_hash_argon2id(
            password.as_ptr(),
            password.len(),
            4096,
            1,
            1,
            hash2.as_mut_ptr(),
            hash2.len(),
        );

        // Same password should produce different hashes (due to random salt)
        assert_ne!(&hash1[..], &hash2[..]);
    }

    #[test]
    fn test_argon2id_hash_errors() {
        let password = b"password";
        let mut hash = [0u8; 256];

        // Null password
        assert_eq!(
            arth_rt_password_hash_argon2id(
                std::ptr::null(),
                0,
                4096,
                1,
                1,
                hash.as_mut_ptr(),
                hash.len(),
            ),
            -1
        );

        // Null output
        assert_eq!(
            arth_rt_password_hash_argon2id(
                password.as_ptr(),
                password.len(),
                4096,
                1,
                1,
                std::ptr::null_mut(),
                256,
            ),
            -2
        );

        // Output buffer too small
        assert_eq!(
            arth_rt_password_hash_argon2id(
                password.as_ptr(),
                password.len(),
                4096,
                1,
                1,
                hash.as_mut_ptr(),
                50, // Too small
            ),
            -3
        );
    }

    #[test]
    fn test_argon2id_verify_errors() {
        let password = b"password";

        // Null password
        assert_eq!(
            arth_rt_password_verify_argon2id(
                std::ptr::null(),
                0,
                b"$argon2id$v=19$m=4096,t=1,p=1$...\0".as_ptr() as *const std::ffi::c_char,
            ),
            -1
        );

        // Null hash
        assert_eq!(
            arth_rt_password_verify_argon2id(password.as_ptr(), password.len(), std::ptr::null(),),
            -2
        );

        // Invalid hash format
        assert_eq!(
            arth_rt_password_verify_argon2id(
                password.as_ptr(),
                password.len(),
                b"invalid hash format\0".as_ptr() as *const std::ffi::c_char,
            ),
            -3
        );
    }

    #[test]
    fn test_bcrypt_hash_verify_roundtrip() {
        let password = b"correcthorsebatterystaple";
        let mut hash = [0u8; 128];

        // Hash the password with cost 4 (minimum, fast for testing)
        let result = arth_rt_password_hash_bcrypt(
            password.as_ptr(),
            password.len(),
            4,
            hash.as_mut_ptr(),
            hash.len(),
        );
        assert_eq!(result, 60, "bcrypt hash should be 60 bytes");

        // Verify the password
        let verify_result = arth_rt_password_verify_bcrypt(
            password.as_ptr(),
            password.len(),
            hash.as_ptr() as *const std::ffi::c_char,
        );
        assert_eq!(verify_result, 1, "Correct password should verify");

        // Verify wrong password fails
        let wrong_password = b"wrongpassword";
        let verify_wrong = arth_rt_password_verify_bcrypt(
            wrong_password.as_ptr(),
            wrong_password.len(),
            hash.as_ptr() as *const std::ffi::c_char,
        );
        assert_eq!(verify_wrong, 0, "Wrong password should not verify");
    }

    #[test]
    fn test_bcrypt_format() {
        let password = b"password";
        let mut hash = [0u8; 128];

        let result = arth_rt_password_hash_bcrypt(
            password.as_ptr(),
            password.len(),
            4,
            hash.as_mut_ptr(),
            hash.len(),
        );
        assert_eq!(result, 60);

        let hash_str = std::str::from_utf8(&hash[..60]).unwrap();
        assert!(
            hash_str.starts_with("$2a$")
                || hash_str.starts_with("$2b$")
                || hash_str.starts_with("$2y$"),
            "bcrypt hash should start with $2a$, $2b$, or $2y$"
        );
        assert!(hash_str.contains("$04$"), "bcrypt hash should contain cost");
    }

    #[test]
    fn test_bcrypt_unique_salts() {
        let password = b"samepassword";
        let mut hash1 = [0u8; 128];
        let mut hash2 = [0u8; 128];

        arth_rt_password_hash_bcrypt(
            password.as_ptr(),
            password.len(),
            4,
            hash1.as_mut_ptr(),
            hash1.len(),
        );

        arth_rt_password_hash_bcrypt(
            password.as_ptr(),
            password.len(),
            4,
            hash2.as_mut_ptr(),
            hash2.len(),
        );

        // Same password should produce different hashes
        assert_ne!(&hash1[..60], &hash2[..60]);
    }

    #[test]
    fn test_bcrypt_hash_errors() {
        let password = b"password";
        let mut hash = [0u8; 128];

        // Null password
        assert_eq!(
            arth_rt_password_hash_bcrypt(std::ptr::null(), 0, 4, hash.as_mut_ptr(), hash.len()),
            -1
        );

        // Null output
        assert_eq!(
            arth_rt_password_hash_bcrypt(
                password.as_ptr(),
                password.len(),
                4,
                std::ptr::null_mut(),
                128
            ),
            -2
        );

        // Output too small
        assert_eq!(
            arth_rt_password_hash_bcrypt(
                password.as_ptr(),
                password.len(),
                4,
                hash.as_mut_ptr(),
                50
            ),
            -3
        );

        // Invalid cost (too low)
        assert_eq!(
            arth_rt_password_hash_bcrypt(
                password.as_ptr(),
                password.len(),
                2,
                hash.as_mut_ptr(),
                hash.len()
            ),
            -4
        );

        // Invalid cost (too high)
        assert_eq!(
            arth_rt_password_hash_bcrypt(
                password.as_ptr(),
                password.len(),
                32,
                hash.as_mut_ptr(),
                hash.len()
            ),
            -4
        );
    }

    #[test]
    fn test_password_verify_auto_detect() {
        let password = b"mypassword";

        // Test with Argon2id hash
        let mut argon2_hash = [0u8; 256];
        arth_rt_password_hash_argon2id(
            password.as_ptr(),
            password.len(),
            4096,
            1,
            1,
            argon2_hash.as_mut_ptr(),
            argon2_hash.len(),
        );

        let result = arth_rt_password_verify(
            password.as_ptr(),
            password.len(),
            argon2_hash.as_ptr() as *const std::ffi::c_char,
        );
        assert_eq!(result, 1, "Should auto-detect and verify Argon2id");

        // Test with bcrypt hash
        let mut bcrypt_hash = [0u8; 128];
        arth_rt_password_hash_bcrypt(
            password.as_ptr(),
            password.len(),
            4,
            bcrypt_hash.as_mut_ptr(),
            bcrypt_hash.len(),
        );

        let result = arth_rt_password_verify(
            password.as_ptr(),
            password.len(),
            bcrypt_hash.as_ptr() as *const std::ffi::c_char,
        );
        assert_eq!(result, 1, "Should auto-detect and verify bcrypt");
    }

    #[test]
    fn test_password_verify_unrecognized() {
        let password = b"password";

        // Unknown hash format
        assert_eq!(
            arth_rt_password_verify(
                password.as_ptr(),
                password.len(),
                b"$unknown$format\0".as_ptr() as *const std::ffi::c_char,
            ),
            -3
        );
    }

    #[test]
    fn test_needs_rehash_argon2_upgrade_memory() {
        let password = b"password";
        let mut hash = [0u8; 256];

        // Hash with low memory (4096 KiB = 4 MB)
        arth_rt_password_hash_argon2id(
            password.as_ptr(),
            password.len(),
            4096,
            1,
            1,
            hash.as_mut_ptr(),
            hash.len(),
        );

        // Should need rehash when target memory is higher
        let result = arth_rt_password_needs_rehash(
            hash.as_ptr() as *const std::ffi::c_char,
            0,     // Argon2id
            65536, // 64 MB (higher than 4 MB)
            1,
            1,
        );
        assert_eq!(result, 1, "Should need rehash for higher memory");

        // Should not need rehash when target memory is lower or equal
        let result = arth_rt_password_needs_rehash(
            hash.as_ptr() as *const std::ffi::c_char,
            0,
            4096, // Same as current
            1,
            1,
        );
        assert_eq!(result, 0, "Should not need rehash for same params");
    }

    #[test]
    fn test_needs_rehash_bcrypt_upgrade_cost() {
        let password = b"password";
        let mut hash = [0u8; 128];

        // Hash with low cost (4)
        arth_rt_password_hash_bcrypt(
            password.as_ptr(),
            password.len(),
            4,
            hash.as_mut_ptr(),
            hash.len(),
        );

        // Should need rehash when target cost is higher
        let result = arth_rt_password_needs_rehash(
            hash.as_ptr() as *const std::ffi::c_char,
            1,  // bcrypt
            12, // Higher cost
            0,
            0,
        );
        assert_eq!(result, 1, "Should need rehash for higher cost");

        // Should not need rehash when target cost is lower or equal
        let result = arth_rt_password_needs_rehash(
            hash.as_ptr() as *const std::ffi::c_char,
            1,
            4, // Same as current
            0,
            0,
        );
        assert_eq!(result, 0, "Should not need rehash for same cost");
    }

    #[test]
    fn test_needs_rehash_algorithm_migration() {
        let password = b"password";
        let mut bcrypt_hash = [0u8; 128];

        // Hash with bcrypt
        arth_rt_password_hash_bcrypt(
            password.as_ptr(),
            password.len(),
            4,
            bcrypt_hash.as_mut_ptr(),
            bcrypt_hash.len(),
        );

        // Should need rehash when migrating to Argon2id
        let result = arth_rt_password_needs_rehash(
            bcrypt_hash.as_ptr() as *const std::ffi::c_char,
            0, // Target: Argon2id
            65536,
            3,
            4,
        );
        assert_eq!(
            result, 1,
            "Should need rehash when migrating from bcrypt to Argon2id"
        );
    }

    #[test]
    fn test_needs_rehash_errors() {
        // Null hash
        assert_eq!(
            arth_rt_password_needs_rehash(std::ptr::null(), 0, 65536, 3, 4),
            -1
        );

        // Invalid algorithm
        assert_eq!(
            arth_rt_password_needs_rehash(
                b"$argon2id$v=19$m=4096,t=1,p=1$...\0".as_ptr() as *const std::ffi::c_char,
                99, // Invalid
                65536,
                3,
                4,
            ),
            -3
        );
    }

    #[test]
    fn test_keygen_uniqueness() {
        // Generate multiple key pairs and verify they're unique
        let mut keys: Vec<[u8; 32]> = Vec::new();

        for _ in 0..10 {
            let mut private_key = [0u8; 32];
            let mut public_key = [0u8; 32];
            let result = arth_rt_signature_generate_keypair(
                0,
                private_key.as_mut_ptr(),
                32,
                public_key.as_mut_ptr(),
                32,
            );
            assert_eq!(result, 0);

            // Check that this key is unique
            for existing in &keys {
                assert_ne!(existing, &private_key);
            }
            keys.push(private_key);
        }
    }

    #[test]
    fn test_derive_consistency() {
        // Deriving public key multiple times should give the same result
        let mut private_key = [0u8; 32];
        let mut public_key1 = [0u8; 32];

        arth_rt_signature_generate_keypair(
            0,
            private_key.as_mut_ptr(),
            32,
            public_key1.as_mut_ptr(),
            32,
        );

        for _ in 0..5 {
            let mut public_key2 = [0u8; 32];
            let result = arth_rt_signature_derive_public_key(
                0,
                private_key.as_ptr(),
                32,
                public_key2.as_mut_ptr(),
                32,
            );
            assert_eq!(result, 0);
            assert_eq!(&public_key1[..], &public_key2[..]);
        }
    }

    // =========================================================================
    // Digital Signature Tests
    // =========================================================================

    #[test]
    fn test_ed25519_sign_verify_roundtrip() {
        let mut private_key = [0u8; 32];
        let mut public_key = [0u8; 32];

        // Generate key pair
        arth_rt_signature_generate_keypair(
            0,
            private_key.as_mut_ptr(),
            32,
            public_key.as_mut_ptr(),
            32,
        );

        // Sign a message
        let message = b"Hello, World!";
        let mut signature = [0u8; 64];
        let result = arth_rt_signature_sign(
            0,
            private_key.as_ptr(),
            32,
            message.as_ptr(),
            message.len(),
            signature.as_mut_ptr(),
            64,
        );
        assert_eq!(result, 64);

        // Verify the signature
        let result = arth_rt_signature_verify(
            0,
            public_key.as_ptr(),
            32,
            message.as_ptr(),
            message.len(),
            signature.as_ptr(),
            64,
        );
        assert_eq!(result, 1);
    }

    #[test]
    fn test_ed25519_rfc8032_test_vector_1() {
        // RFC 8032 Test Vector 1 (empty message)
        // SECRET KEY: 9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60
        // PUBLIC KEY: d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a
        // MESSAGE: (empty)
        // SIGNATURE: e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b

        let private_key =
            hex_to_bytes("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60");
        let expected_public =
            hex_to_bytes("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let expected_sig = hex_to_bytes(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        );

        // Derive public key
        let mut public_key = [0u8; 32];
        arth_rt_signature_derive_public_key(
            0,
            private_key.as_ptr(),
            32,
            public_key.as_mut_ptr(),
            32,
        );
        assert_eq!(&public_key[..], &expected_public[..]);

        // Sign empty message
        let mut signature = [0u8; 64];
        let result = arth_rt_signature_sign(
            0,
            private_key.as_ptr(),
            32,
            std::ptr::null(),
            0,
            signature.as_mut_ptr(),
            64,
        );
        assert_eq!(result, 64);
        assert_eq!(&signature[..], &expected_sig[..]);

        // Verify
        let result = arth_rt_signature_verify(
            0,
            public_key.as_ptr(),
            32,
            std::ptr::null(),
            0,
            signature.as_ptr(),
            64,
        );
        assert_eq!(result, 1);
    }

    #[test]
    fn test_ed25519_rfc8032_test_vector_2() {
        // RFC 8032 Test Vector 2 (single byte message)
        // SECRET KEY: 4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb
        // PUBLIC KEY: 3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c
        // MESSAGE: 72 (0x72)
        // SIGNATURE: 92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00

        let private_key =
            hex_to_bytes("4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb");
        let expected_public =
            hex_to_bytes("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c");
        let message = [0x72u8];
        let expected_sig = hex_to_bytes(
            "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
        );

        // Derive public key
        let mut public_key = [0u8; 32];
        arth_rt_signature_derive_public_key(
            0,
            private_key.as_ptr(),
            32,
            public_key.as_mut_ptr(),
            32,
        );
        assert_eq!(&public_key[..], &expected_public[..]);

        // Sign message
        let mut signature = [0u8; 64];
        let result = arth_rt_signature_sign(
            0,
            private_key.as_ptr(),
            32,
            message.as_ptr(),
            1,
            signature.as_mut_ptr(),
            64,
        );
        assert_eq!(result, 64);
        assert_eq!(&signature[..], &expected_sig[..]);

        // Verify
        let result = arth_rt_signature_verify(
            0,
            public_key.as_ptr(),
            32,
            message.as_ptr(),
            1,
            signature.as_ptr(),
            64,
        );
        assert_eq!(result, 1);
    }

    #[test]
    fn test_ecdsa_p256_sign_verify_roundtrip() {
        let mut private_key = [0u8; 32];
        let mut public_key = [0u8; 33];

        // Generate key pair
        arth_rt_signature_generate_keypair(
            1,
            private_key.as_mut_ptr(),
            32,
            public_key.as_mut_ptr(),
            33,
        );

        // Sign a message
        let message = b"Hello, World!";
        let mut signature = [0u8; 64];
        let result = arth_rt_signature_sign(
            1,
            private_key.as_ptr(),
            32,
            message.as_ptr(),
            message.len(),
            signature.as_mut_ptr(),
            64,
        );
        assert_eq!(result, 64);

        // Verify the signature
        let result = arth_rt_signature_verify(
            1,
            public_key.as_ptr(),
            33,
            message.as_ptr(),
            message.len(),
            signature.as_ptr(),
            64,
        );
        assert_eq!(result, 1);
    }

    #[test]
    fn test_ecdsa_p384_sign_verify_roundtrip() {
        let mut private_key = [0u8; 48];
        let mut public_key = [0u8; 49];

        // Generate key pair
        arth_rt_signature_generate_keypair(
            2,
            private_key.as_mut_ptr(),
            48,
            public_key.as_mut_ptr(),
            49,
        );

        // Sign a message
        let message = b"Hello, World!";
        let mut signature = [0u8; 96];
        let result = arth_rt_signature_sign(
            2,
            private_key.as_ptr(),
            48,
            message.as_ptr(),
            message.len(),
            signature.as_mut_ptr(),
            96,
        );
        assert_eq!(result, 96);

        // Verify the signature
        let result = arth_rt_signature_verify(
            2,
            public_key.as_ptr(),
            49,
            message.as_ptr(),
            message.len(),
            signature.as_ptr(),
            96,
        );
        assert_eq!(result, 1);
    }

    #[test]
    fn test_signature_verify_wrong_key() {
        // Generate two different key pairs
        let mut private_key1 = [0u8; 32];
        let mut public_key1 = [0u8; 32];
        let mut private_key2 = [0u8; 32];
        let mut public_key2 = [0u8; 32];

        arth_rt_signature_generate_keypair(
            0,
            private_key1.as_mut_ptr(),
            32,
            public_key1.as_mut_ptr(),
            32,
        );
        arth_rt_signature_generate_keypair(
            0,
            private_key2.as_mut_ptr(),
            32,
            public_key2.as_mut_ptr(),
            32,
        );

        // Sign with first key
        let message = b"Test message";
        let mut signature = [0u8; 64];
        arth_rt_signature_sign(
            0,
            private_key1.as_ptr(),
            32,
            message.as_ptr(),
            message.len(),
            signature.as_mut_ptr(),
            64,
        );

        // Verify with wrong key should fail
        let result = arth_rt_signature_verify(
            0,
            public_key2.as_ptr(),
            32,
            message.as_ptr(),
            message.len(),
            signature.as_ptr(),
            64,
        );
        assert_eq!(result, 0);
    }

    #[test]
    fn test_signature_verify_tampered_message() {
        let mut private_key = [0u8; 32];
        let mut public_key = [0u8; 32];

        arth_rt_signature_generate_keypair(
            0,
            private_key.as_mut_ptr(),
            32,
            public_key.as_mut_ptr(),
            32,
        );

        // Sign a message
        let message = b"Original message";
        let mut signature = [0u8; 64];
        arth_rt_signature_sign(
            0,
            private_key.as_ptr(),
            32,
            message.as_ptr(),
            message.len(),
            signature.as_mut_ptr(),
            64,
        );

        // Verify with tampered message should fail
        let tampered = b"Tampered message";
        let result = arth_rt_signature_verify(
            0,
            public_key.as_ptr(),
            32,
            tampered.as_ptr(),
            tampered.len(),
            signature.as_ptr(),
            64,
        );
        assert_eq!(result, 0);
    }

    #[test]
    fn test_signature_verify_tampered_signature() {
        let mut private_key = [0u8; 32];
        let mut public_key = [0u8; 32];

        arth_rt_signature_generate_keypair(
            0,
            private_key.as_mut_ptr(),
            32,
            public_key.as_mut_ptr(),
            32,
        );

        // Sign a message
        let message = b"Test message";
        let mut signature = [0u8; 64];
        arth_rt_signature_sign(
            0,
            private_key.as_ptr(),
            32,
            message.as_ptr(),
            message.len(),
            signature.as_mut_ptr(),
            64,
        );

        // Tamper with signature
        signature[0] ^= 0xFF;
        signature[32] ^= 0xFF;

        // Verify should fail
        let result = arth_rt_signature_verify(
            0,
            public_key.as_ptr(),
            32,
            message.as_ptr(),
            message.len(),
            signature.as_ptr(),
            64,
        );
        assert_eq!(result, 0);
    }

    #[test]
    fn test_signature_sign_null_pointers() {
        let mut signature = [0u8; 64];

        // Null private key
        let result = arth_rt_signature_sign(
            0,
            std::ptr::null(),
            32,
            b"test".as_ptr(),
            4,
            signature.as_mut_ptr(),
            64,
        );
        assert_eq!(result, -3);

        let private_key = [0u8; 32];

        // Null signature buffer
        let result = arth_rt_signature_sign(
            0,
            private_key.as_ptr(),
            32,
            b"test".as_ptr(),
            4,
            std::ptr::null_mut(),
            64,
        );
        assert_eq!(result, -4);
    }

    #[test]
    fn test_signature_verify_null_pointers() {
        let signature = [0u8; 64];

        // Null public key
        let result = arth_rt_signature_verify(
            0,
            std::ptr::null(),
            32,
            b"test".as_ptr(),
            4,
            signature.as_ptr(),
            64,
        );
        assert_eq!(result, -3);

        let public_key = [0u8; 32];

        // Null signature
        let result = arth_rt_signature_verify(
            0,
            public_key.as_ptr(),
            32,
            b"test".as_ptr(),
            4,
            std::ptr::null(),
            64,
        );
        assert_eq!(result, -4);
    }

    #[test]
    fn test_signature_sign_wrong_sizes() {
        let private_key = [0u8; 16]; // Wrong size
        let mut signature = [0u8; 64];

        let result = arth_rt_signature_sign(
            0,
            private_key.as_ptr(),
            16,
            b"test".as_ptr(),
            4,
            signature.as_mut_ptr(),
            64,
        );
        assert_eq!(result, -1);

        let private_key = [0u8; 32];
        let mut signature = [0u8; 32]; // Too small

        let result = arth_rt_signature_sign(
            0,
            private_key.as_ptr(),
            32,
            b"test".as_ptr(),
            4,
            signature.as_mut_ptr(),
            32,
        );
        assert_eq!(result, -2);
    }

    #[test]
    fn test_signature_invalid_algorithm() {
        let private_key = [0u8; 32];
        let mut signature = [0u8; 64];

        let result = arth_rt_signature_sign(
            99,
            private_key.as_ptr(),
            32,
            b"test".as_ptr(),
            4,
            signature.as_mut_ptr(),
            64,
        );
        assert_eq!(result, -5);

        let public_key = [0u8; 32];
        let result = arth_rt_signature_verify(
            99,
            public_key.as_ptr(),
            32,
            b"test".as_ptr(),
            4,
            signature.as_ptr(),
            64,
        );
        assert_eq!(result, -5);
    }

    #[test]
    fn test_signature_large_message() {
        let mut private_key = [0u8; 32];
        let mut public_key = [0u8; 32];

        arth_rt_signature_generate_keypair(
            0,
            private_key.as_mut_ptr(),
            32,
            public_key.as_mut_ptr(),
            32,
        );

        // Sign a large message (64KB)
        let message: Vec<u8> = (0..65536).map(|i| (i % 256) as u8).collect();
        let mut signature = [0u8; 64];
        let result = arth_rt_signature_sign(
            0,
            private_key.as_ptr(),
            32,
            message.as_ptr(),
            message.len(),
            signature.as_mut_ptr(),
            64,
        );
        assert_eq!(result, 64);

        // Verify
        let result = arth_rt_signature_verify(
            0,
            public_key.as_ptr(),
            32,
            message.as_ptr(),
            message.len(),
            signature.as_ptr(),
            64,
        );
        assert_eq!(result, 1);
    }

    #[test]
    fn test_ecdsa_p256_verify_wrong_key() {
        let mut private_key1 = [0u8; 32];
        let mut public_key1 = [0u8; 33];
        let mut private_key2 = [0u8; 32];
        let mut public_key2 = [0u8; 33];

        arth_rt_signature_generate_keypair(
            1,
            private_key1.as_mut_ptr(),
            32,
            public_key1.as_mut_ptr(),
            33,
        );
        arth_rt_signature_generate_keypair(
            1,
            private_key2.as_mut_ptr(),
            32,
            public_key2.as_mut_ptr(),
            33,
        );

        let message = b"Test message";
        let mut signature = [0u8; 64];
        arth_rt_signature_sign(
            1,
            private_key1.as_ptr(),
            32,
            message.as_ptr(),
            message.len(),
            signature.as_mut_ptr(),
            64,
        );

        // Verify with wrong key
        let result = arth_rt_signature_verify(
            1,
            public_key2.as_ptr(),
            33,
            message.as_ptr(),
            message.len(),
            signature.as_ptr(),
            64,
        );
        assert_eq!(result, 0);
    }

    #[test]
    fn test_ecdsa_p384_verify_wrong_key() {
        let mut private_key1 = [0u8; 48];
        let mut public_key1 = [0u8; 49];
        let mut private_key2 = [0u8; 48];
        let mut public_key2 = [0u8; 49];

        arth_rt_signature_generate_keypair(
            2,
            private_key1.as_mut_ptr(),
            48,
            public_key1.as_mut_ptr(),
            49,
        );
        arth_rt_signature_generate_keypair(
            2,
            private_key2.as_mut_ptr(),
            48,
            public_key2.as_mut_ptr(),
            49,
        );

        let message = b"Test message";
        let mut signature = [0u8; 96];
        arth_rt_signature_sign(
            2,
            private_key1.as_ptr(),
            48,
            message.as_ptr(),
            message.len(),
            signature.as_mut_ptr(),
            96,
        );

        // Verify with wrong key
        let result = arth_rt_signature_verify(
            2,
            public_key2.as_ptr(),
            49,
            message.as_ptr(),
            message.len(),
            signature.as_ptr(),
            96,
        );
        assert_eq!(result, 0);
    }

    // =========================================================================
    // KeyStore Tests
    // =========================================================================

    #[test]
    fn test_keystore_create_destroy() {
        let handle = arth_rt_keystore_create();
        assert!(handle > 0, "Failed to create key store");

        let result = arth_rt_keystore_destroy(handle);
        assert_eq!(result, 0, "Failed to destroy key store");

        // Double destroy should fail
        let result = arth_rt_keystore_destroy(handle);
        assert_eq!(result, -1, "Double destroy should fail");
    }

    #[test]
    fn test_keystore_store_load() {
        let handle = arth_rt_keystore_create();
        assert!(handle > 0);

        let key_id = std::ffi::CString::new("test-key-1").unwrap();
        let key_data: [u8; 32] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
            0x1c, 0x1d, 0x1e, 0x1f,
        ];
        let algorithm = 1; // AES-256-GCM

        // Store the key
        let result = arth_rt_keystore_store(
            handle,
            key_id.as_ptr(),
            algorithm,
            key_data.as_ptr(),
            key_data.len(),
        );
        assert_eq!(result, 0, "Failed to store key");

        // Load the key
        let mut loaded_algorithm: i32 = 0;
        let mut loaded_data = [0u8; 32];
        let result = arth_rt_keystore_load(
            handle,
            key_id.as_ptr(),
            &mut loaded_algorithm,
            loaded_data.as_mut_ptr(),
            loaded_data.len(),
        );
        assert_eq!(result, 32, "Failed to load key");
        assert_eq!(loaded_algorithm, algorithm, "Algorithm mismatch");
        assert_eq!(loaded_data, key_data, "Key data mismatch");

        arth_rt_keystore_destroy(handle);
    }

    #[test]
    fn test_keystore_exists_delete() {
        let handle = arth_rt_keystore_create();
        assert!(handle > 0);

        let key_id = std::ffi::CString::new("delete-test").unwrap();
        let key_data = [0u8; 16];

        // Initially should not exist
        let result = arth_rt_keystore_exists(handle, key_id.as_ptr());
        assert_eq!(result, 0, "Key should not exist initially");

        // Store the key
        arth_rt_keystore_store(
            handle,
            key_id.as_ptr(),
            0,
            key_data.as_ptr(),
            key_data.len(),
        );

        // Now it should exist
        let result = arth_rt_keystore_exists(handle, key_id.as_ptr());
        assert_eq!(result, 1, "Key should exist after store");

        // Delete the key
        let result = arth_rt_keystore_delete(handle, key_id.as_ptr());
        assert_eq!(result, 1, "Delete should return 1 for existing key");

        // Should not exist anymore
        let result = arth_rt_keystore_exists(handle, key_id.as_ptr());
        assert_eq!(result, 0, "Key should not exist after delete");

        // Delete again should return 0
        let result = arth_rt_keystore_delete(handle, key_id.as_ptr());
        assert_eq!(result, 0, "Delete of non-existent key should return 0");

        arth_rt_keystore_destroy(handle);
    }

    #[test]
    fn test_keystore_count() {
        let handle = arth_rt_keystore_create();
        assert!(handle > 0);

        // Initial count should be 0
        let count = arth_rt_keystore_count(handle);
        assert_eq!(count, 0);

        // Add some keys
        let key_data = [0u8; 16];
        for i in 0..5 {
            let key_id = std::ffi::CString::new(format!("key-{}", i)).unwrap();
            arth_rt_keystore_store(
                handle,
                key_id.as_ptr(),
                0,
                key_data.as_ptr(),
                key_data.len(),
            );
        }

        let count = arth_rt_keystore_count(handle);
        assert_eq!(count, 5, "Count should be 5 after adding 5 keys");

        arth_rt_keystore_destroy(handle);
    }

    #[test]
    fn test_keystore_clear() {
        let handle = arth_rt_keystore_create();
        assert!(handle > 0);

        // Add some keys
        let key_data = [0u8; 16];
        for i in 0..3 {
            let key_id = std::ffi::CString::new(format!("clear-key-{}", i)).unwrap();
            arth_rt_keystore_store(
                handle,
                key_id.as_ptr(),
                0,
                key_data.as_ptr(),
                key_data.len(),
            );
        }

        let count = arth_rt_keystore_count(handle);
        assert_eq!(count, 3);

        // Clear the store
        let cleared = arth_rt_keystore_clear(handle);
        assert_eq!(cleared, 3, "Clear should return number of keys cleared");

        // Count should now be 0
        let count = arth_rt_keystore_count(handle);
        assert_eq!(count, 0, "Count should be 0 after clear");

        arth_rt_keystore_destroy(handle);
    }

    #[test]
    fn test_keystore_list() {
        let handle = arth_rt_keystore_create();
        assert!(handle > 0);

        // Add keys
        let key_data = [0u8; 16];
        let key_ids = ["alpha", "beta", "gamma"];
        for key_id in &key_ids {
            let cstr = std::ffi::CString::new(*key_id).unwrap();
            arth_rt_keystore_store(handle, cstr.as_ptr(), 0, key_data.as_ptr(), key_data.len());
        }

        // Get list size
        let size = arth_rt_keystore_list_size(handle);
        assert!(size > 0, "List size should be positive");

        // Get list
        let mut buffer = vec![0u8; size as usize];
        let count = arth_rt_keystore_list(handle, buffer.as_mut_ptr(), buffer.len());
        assert_eq!(count, 3, "Should have 3 keys");

        // Parse the list (null-separated strings)
        let mut found_keys: Vec<String> = Vec::new();
        let mut start = 0;
        for (i, &byte) in buffer.iter().enumerate() {
            if byte == 0 {
                if i > start {
                    let key = String::from_utf8_lossy(&buffer[start..i]).to_string();
                    found_keys.push(key);
                }
                start = i + 1;
            }
        }

        assert_eq!(found_keys.len(), 3, "Should have found 3 keys");
        for key_id in &key_ids {
            assert!(
                found_keys.contains(&key_id.to_string()),
                "Should contain {}",
                key_id
            );
        }

        arth_rt_keystore_destroy(handle);
    }

    #[test]
    fn test_keystore_load_not_found() {
        let handle = arth_rt_keystore_create();
        assert!(handle > 0);

        let key_id = std::ffi::CString::new("nonexistent").unwrap();
        let mut algorithm: i32 = 0;
        let mut buffer = [0u8; 32];

        let result = arth_rt_keystore_load(
            handle,
            key_id.as_ptr(),
            &mut algorithm,
            buffer.as_mut_ptr(),
            buffer.len(),
        );
        assert_eq!(result, 0, "Load of nonexistent key should return 0");

        arth_rt_keystore_destroy(handle);
    }

    #[test]
    fn test_keystore_overwrite() {
        let handle = arth_rt_keystore_create();
        assert!(handle > 0);

        let key_id = std::ffi::CString::new("overwrite-test").unwrap();
        let key_data1 = [0xAA; 16];
        let key_data2 = [0xBB; 16];

        // Store first version
        arth_rt_keystore_store(
            handle,
            key_id.as_ptr(),
            0,
            key_data1.as_ptr(),
            key_data1.len(),
        );

        // Store second version (overwrite)
        arth_rt_keystore_store(
            handle,
            key_id.as_ptr(),
            1,
            key_data2.as_ptr(),
            key_data2.len(),
        );

        // Load should return second version
        let mut algorithm: i32 = 0;
        let mut buffer = [0u8; 16];
        let result = arth_rt_keystore_load(
            handle,
            key_id.as_ptr(),
            &mut algorithm,
            buffer.as_mut_ptr(),
            buffer.len(),
        );
        assert_eq!(result, 16);
        assert_eq!(algorithm, 1, "Algorithm should be updated");
        assert_eq!(buffer, key_data2, "Data should be updated");

        // Count should still be 1
        let count = arth_rt_keystore_count(handle);
        assert_eq!(count, 1);

        arth_rt_keystore_destroy(handle);
    }

    #[test]
    fn test_keystore_invalid_handle() {
        let invalid_handle = 999999999i64;

        let key_id = std::ffi::CString::new("test").unwrap();
        let key_data = [0u8; 16];

        // All operations should fail with invalid handle
        assert_eq!(
            arth_rt_keystore_store(
                invalid_handle,
                key_id.as_ptr(),
                0,
                key_data.as_ptr(),
                key_data.len()
            ),
            -1
        );
        assert_eq!(arth_rt_keystore_exists(invalid_handle, key_id.as_ptr()), -1);
        assert_eq!(arth_rt_keystore_delete(invalid_handle, key_id.as_ptr()), -1);
        assert_eq!(arth_rt_keystore_count(invalid_handle), -1);
        assert_eq!(arth_rt_keystore_clear(invalid_handle), -1);
        assert_eq!(arth_rt_keystore_list_size(invalid_handle), -1);
        assert_eq!(arth_rt_keystore_destroy(invalid_handle), -1);
    }

    #[test]
    fn test_keystore_empty_key_id() {
        let handle = arth_rt_keystore_create();
        assert!(handle > 0);

        let empty_key_id = std::ffi::CString::new("").unwrap();
        let key_data = [0u8; 16];

        // Store with empty key ID should fail
        let result = arth_rt_keystore_store(
            handle,
            empty_key_id.as_ptr(),
            0,
            key_data.as_ptr(),
            key_data.len(),
        );
        assert_eq!(result, -2, "Store with empty key ID should fail");

        arth_rt_keystore_destroy(handle);
    }

    #[test]
    fn test_keystore_null_key_id() {
        let handle = arth_rt_keystore_create();
        assert!(handle > 0);

        let key_data = [0u8; 16];

        // Store with null key ID should fail
        let result = arth_rt_keystore_store(
            handle,
            std::ptr::null(),
            0,
            key_data.as_ptr(),
            key_data.len(),
        );
        assert_eq!(result, -2, "Store with null key ID should fail");

        // Exists with null key ID should fail
        let result = arth_rt_keystore_exists(handle, std::ptr::null());
        assert_eq!(result, -2);

        // Delete with null key ID should fail
        let result = arth_rt_keystore_delete(handle, std::ptr::null());
        assert_eq!(result, -2);

        arth_rt_keystore_destroy(handle);
    }

    #[test]
    fn test_keystore_concurrent_access() {
        use std::thread;

        let handle = arth_rt_keystore_create();
        assert!(handle > 0);

        let mut handles = Vec::new();

        // Spawn multiple threads that store keys concurrently
        for i in 0..10 {
            let h = handle;
            let thread_handle = thread::spawn(move || {
                let key_id = std::ffi::CString::new(format!("concurrent-key-{}", i)).unwrap();
                let key_data = [i as u8; 16];

                let result = arth_rt_keystore_store(
                    h,
                    key_id.as_ptr(),
                    0,
                    key_data.as_ptr(),
                    key_data.len(),
                );
                assert_eq!(result, 0, "Concurrent store should succeed");

                // Read back
                let mut buffer = [0u8; 16];
                let mut algorithm: i32 = 0;
                let result = arth_rt_keystore_load(
                    h,
                    key_id.as_ptr(),
                    &mut algorithm,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                );
                assert_eq!(result, 16, "Concurrent load should succeed");
                assert_eq!(buffer, key_data, "Data should match");
            });
            handles.push(thread_handle);
        }

        for h in handles {
            h.join().expect("Thread should complete");
        }

        // All 10 keys should be stored
        let count = arth_rt_keystore_count(handle);
        assert_eq!(count, 10, "All concurrent keys should be stored");

        arth_rt_keystore_destroy(handle);
    }

    // =========================================================================
    // Keyring Tests
    // =========================================================================

    #[cfg(feature = "keyring-store")]
    #[test]
    fn test_keyring_is_available() {
        let result = arth_rt_keyring_is_available();
        // Just verify it returns a valid result (0 or 1)
        assert!(
            result == 0 || result == 1,
            "is_available should return 0 or 1"
        );
    }

    #[cfg(feature = "keyring-store")]
    #[test]
    fn test_keyring_platform() {
        let mut buffer = [0u8; 64];
        let len = arth_rt_keyring_platform(buffer.as_mut_ptr(), buffer.len());
        assert!(len > 0, "Platform name should have positive length");

        let platform = std::str::from_utf8(&buffer[..len as usize]).unwrap();
        assert!(
            platform == "macos-keychain"
                || platform == "windows-credential-manager"
                || platform == "linux-secret-service"
                || platform == "unknown",
            "Platform should be recognized: {}",
            platform
        );
    }

    #[cfg(feature = "keyring-store")]
    #[test]
    fn test_keyring_store_load_delete_roundtrip() {
        // Skip if keyring is not available
        if arth_rt_keyring_is_available() != 1 {
            eprintln!("Skipping keyring test: keyring not available");
            return;
        }

        let service = std::ffi::CString::new("arth-rt-test").unwrap();
        let account = std::ffi::CString::new("test-account-roundtrip").unwrap();
        let secret = b"test-secret-123";

        // Clean up any previous test data
        arth_rt_keyring_delete(service.as_ptr(), account.as_ptr());

        // Store the secret
        let result = arth_rt_keyring_store(
            service.as_ptr(),
            account.as_ptr(),
            secret.as_ptr(),
            secret.len(),
        );
        if result != 0 {
            eprintln!("Skipping keyring test: store failed with code {}", result);
            return;
        }

        // Check it exists - if this fails, the keyring might not be working properly
        let exists = arth_rt_keyring_exists(service.as_ptr(), account.as_ptr());
        if exists != 1 {
            eprintln!(
                "Skipping keyring test: keyring store succeeded but entry not persisted (exists={})",
                exists
            );
            return;
        }

        // Load the secret
        let mut buffer = [0u8; 64];
        let len = arth_rt_keyring_load(
            service.as_ptr(),
            account.as_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
        );
        assert_eq!(len, secret.len() as i64, "Loaded length should match");
        assert_eq!(
            &buffer[..len as usize],
            secret,
            "Loaded secret should match"
        );

        // Delete the entry
        let deleted = arth_rt_keyring_delete(service.as_ptr(), account.as_ptr());
        assert_eq!(deleted, 1, "Delete should return 1 for existing entry");

        // Verify it's gone
        let exists = arth_rt_keyring_exists(service.as_ptr(), account.as_ptr());
        assert_eq!(exists, 0, "Entry should not exist after delete");

        // Delete again should return 0
        let deleted = arth_rt_keyring_delete(service.as_ptr(), account.as_ptr());
        assert_eq!(deleted, 0, "Delete of non-existent entry should return 0");
    }

    #[cfg(feature = "keyring-store")]
    #[test]
    fn test_keyring_password_store_load() {
        // Skip if keyring is not available
        if arth_rt_keyring_is_available() != 1 {
            eprintln!("Skipping keyring test: keyring not available");
            return;
        }

        let service = std::ffi::CString::new("arth-rt-test").unwrap();
        let account = std::ffi::CString::new("test-account-password").unwrap();
        let password = std::ffi::CString::new("my-secret-password-456").unwrap();

        // Clean up any previous test data
        arth_rt_keyring_delete(service.as_ptr(), account.as_ptr());

        // Store the password
        let result =
            arth_rt_keyring_store_password(service.as_ptr(), account.as_ptr(), password.as_ptr());
        if result != 0 {
            eprintln!(
                "Skipping keyring test: store password failed with code {}",
                result
            );
            return;
        }

        // Verify it exists before loading
        let exists = arth_rt_keyring_exists(service.as_ptr(), account.as_ptr());
        if exists != 1 {
            eprintln!(
                "Skipping keyring test: password not persisted (exists={})",
                exists
            );
            return;
        }

        // Load the password
        let mut buffer = [0i8; 64];
        let len = arth_rt_keyring_load_password(
            service.as_ptr(),
            account.as_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
        );
        assert!(len > 0, "Load password should succeed");

        let loaded_password = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
        assert_eq!(
            loaded_password,
            password.as_c_str(),
            "Password should match"
        );

        // Clean up
        arth_rt_keyring_delete(service.as_ptr(), account.as_ptr());
    }

    #[cfg(feature = "keyring-store")]
    #[test]
    fn test_keyring_binary_data() {
        // Skip if keyring is not available
        if arth_rt_keyring_is_available() != 1 {
            eprintln!("Skipping keyring test: keyring not available");
            return;
        }

        let service = std::ffi::CString::new("arth-rt-test").unwrap();
        let account = std::ffi::CString::new("test-account-binary").unwrap();
        // Binary data with non-ASCII bytes
        let secret: [u8; 16] = [
            0x00, 0x01, 0xFF, 0xFE, 0x80, 0x7F, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE,
            0xBA, 0xBE,
        ];

        // Clean up any previous test data
        arth_rt_keyring_delete(service.as_ptr(), account.as_ptr());

        // Store the binary data
        let result = arth_rt_keyring_store(
            service.as_ptr(),
            account.as_ptr(),
            secret.as_ptr(),
            secret.len(),
        );
        if result != 0 {
            eprintln!(
                "Skipping keyring test: store binary failed with code {}",
                result
            );
            return;
        }

        // Verify it exists before loading
        let exists = arth_rt_keyring_exists(service.as_ptr(), account.as_ptr());
        if exists != 1 {
            eprintln!(
                "Skipping keyring test: binary data not persisted (exists={})",
                exists
            );
            return;
        }

        // Load the binary data
        let mut buffer = [0u8; 64];
        let len = arth_rt_keyring_load(
            service.as_ptr(),
            account.as_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
        );
        assert_eq!(len, secret.len() as i64, "Binary length should match");
        assert_eq!(&buffer[..len as usize], &secret, "Binary data should match");

        // Clean up
        arth_rt_keyring_delete(service.as_ptr(), account.as_ptr());
    }

    #[cfg(feature = "keyring-store")]
    #[test]
    fn test_keyring_not_found() {
        // Skip if keyring is not available
        if arth_rt_keyring_is_available() != 1 {
            eprintln!("Skipping keyring test: keyring not available");
            return;
        }

        let service = std::ffi::CString::new("arth-rt-test").unwrap();
        let account = std::ffi::CString::new("nonexistent-account-xyz").unwrap();

        // Make sure it doesn't exist
        arth_rt_keyring_delete(service.as_ptr(), account.as_ptr());

        // Load should return 0 for not found
        let mut buffer = [0u8; 64];
        let len = arth_rt_keyring_load(
            service.as_ptr(),
            account.as_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
        );
        assert_eq!(len, 0, "Load of non-existent entry should return 0");

        // Exists should return 0
        let exists = arth_rt_keyring_exists(service.as_ptr(), account.as_ptr());
        assert_eq!(exists, 0, "Non-existent entry should not exist");
    }

    #[cfg(feature = "keyring-store")]
    #[test]
    fn test_keyring_null_args() {
        // Null service
        let result = arth_rt_keyring_store(std::ptr::null(), std::ptr::null(), std::ptr::null(), 0);
        assert_eq!(result, -1, "Null service should return -1");

        let service = std::ffi::CString::new("test").unwrap();

        // Null account
        let result = arth_rt_keyring_store(service.as_ptr(), std::ptr::null(), std::ptr::null(), 0);
        assert_eq!(result, -2, "Null account should return -2");

        // Null exists checks
        assert_eq!(
            arth_rt_keyring_exists(std::ptr::null(), std::ptr::null()),
            -1
        );
        assert_eq!(
            arth_rt_keyring_delete(std::ptr::null(), std::ptr::null()),
            -1
        );
    }

    #[cfg(feature = "keyring-store")]
    #[test]
    fn test_keyring_overwrite() {
        // Skip if keyring is not available
        if arth_rt_keyring_is_available() != 1 {
            eprintln!("Skipping keyring test: keyring not available");
            return;
        }

        let service = std::ffi::CString::new("arth-rt-test").unwrap();
        let account = std::ffi::CString::new("test-account-overwrite").unwrap();
        let secret1 = b"first-secret";
        let secret2 = b"second-secret-longer";

        // Clean up
        arth_rt_keyring_delete(service.as_ptr(), account.as_ptr());

        // Store first secret
        let result = arth_rt_keyring_store(
            service.as_ptr(),
            account.as_ptr(),
            secret1.as_ptr(),
            secret1.len(),
        );
        if result != 0 {
            eprintln!(
                "Skipping keyring test: store first failed with code {}",
                result
            );
            return;
        }

        // Verify first secret was stored
        let exists = arth_rt_keyring_exists(service.as_ptr(), account.as_ptr());
        if exists != 1 {
            eprintln!(
                "Skipping keyring test: first secret not persisted (exists={})",
                exists
            );
            return;
        }

        // Overwrite with second secret
        let result = arth_rt_keyring_store(
            service.as_ptr(),
            account.as_ptr(),
            secret2.as_ptr(),
            secret2.len(),
        );
        assert_eq!(result, 0, "Overwrite should succeed");

        // Load should return second secret
        let mut buffer = [0u8; 64];
        let len = arth_rt_keyring_load(
            service.as_ptr(),
            account.as_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
        );
        assert_eq!(
            len,
            secret2.len() as i64,
            "Should return second secret length"
        );
        assert_eq!(
            &buffer[..len as usize],
            secret2,
            "Should return second secret"
        );

        // Clean up
        arth_rt_keyring_delete(service.as_ptr(), account.as_ptr());
    }

    // =========================================================================
    // Async Crypto Tests
    // =========================================================================

    #[cfg(feature = "crypto")]
    #[test]
    fn test_async_argon2id_hash() {
        let password = b"test-password-123";

        // Start async hash operation with fast params for testing
        let task_id = arth_rt_password_hash_argon2id_async(
            password.as_ptr(),
            password.len(),
            8192, // 8 MB (minimum for tests)
            1,    // 1 iteration
            1,    // 1 lane
        );
        assert!(task_id > 0, "Should return positive task ID");

        // Wait for completion (10 second timeout)
        let status = arth_rt_async_task_wait(task_id, 10000);
        assert_eq!(status, 1, "Task should complete successfully");

        // Get result
        let mut buffer = [0u8; 256];
        let len = arth_rt_async_task_result_string(task_id, buffer.as_mut_ptr(), buffer.len());
        assert!(len > 0, "Should return hash string length");

        let hash = std::str::from_utf8(&buffer[..len as usize]).unwrap();
        assert!(
            hash.starts_with("$argon2id$"),
            "Hash should be in PHC format"
        );

        // Verify the hash works with sync verify
        let hash_cstr = std::ffi::CString::new(hash).unwrap();
        let valid =
            arth_rt_password_verify_argon2id(password.as_ptr(), password.len(), hash_cstr.as_ptr());
        assert_eq!(
            valid, 1,
            "Password should verify against async-generated hash"
        );

        // Cleanup
        let result = arth_rt_async_task_cleanup(task_id);
        assert_eq!(result, 0, "Cleanup should succeed");
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn test_async_bcrypt_hash() {
        let password = b"test-password-bcrypt";

        // Start async hash operation with low cost for testing
        let task_id = arth_rt_password_hash_bcrypt_async(
            password.as_ptr(),
            password.len(),
            4, // Minimum cost
        );
        assert!(task_id > 0, "Should return positive task ID");

        // Wait for completion
        let status = arth_rt_async_task_wait(task_id, 10000);
        assert_eq!(status, 1, "Task should complete successfully");

        // Get result
        let mut buffer = [0u8; 128];
        let len = arth_rt_async_task_result_string(task_id, buffer.as_mut_ptr(), buffer.len());
        assert!(len > 0, "Should return hash string length");

        let hash = std::str::from_utf8(&buffer[..len as usize]).unwrap();
        assert!(hash.starts_with("$2b$"), "Hash should be in MCF format");

        // Verify the hash works
        let hash_cstr = std::ffi::CString::new(hash).unwrap();
        let valid =
            arth_rt_password_verify_bcrypt(password.as_ptr(), password.len(), hash_cstr.as_ptr());
        assert_eq!(
            valid, 1,
            "Password should verify against async-generated hash"
        );

        // Cleanup
        arth_rt_async_task_cleanup(task_id);
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn test_async_pbkdf2_derive() {
        let password = b"test-password";
        let salt = b"random-salt-bytes";

        // Start async PBKDF2 derivation with SHA-256 (algorithm 3)
        let task_id = arth_rt_pbkdf2_derive_async(
            3, // PBKDF2-HMAC-SHA256
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            1000, // Reduced iterations for test
            32,   // 32-byte output
        );
        assert!(task_id > 0, "Should return positive task ID");

        // Wait for completion
        let status = arth_rt_async_task_wait(task_id, 10000);
        assert_eq!(status, 1, "Task should complete successfully");

        // Get result
        let mut output = [0u8; 32];
        let len = arth_rt_async_task_result_bytes(task_id, output.as_mut_ptr(), output.len());
        assert_eq!(len, 32, "Should return 32 bytes");

        // Verify against sync version
        let mut sync_output = [0u8; 32];
        let sync_len = arth_rt_pbkdf2_derive(
            3, // PBKDF2-HMAC-SHA256
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            1000,
            sync_output.as_mut_ptr(),
            sync_output.len(),
        );
        assert_eq!(sync_len, 32);
        assert_eq!(output, sync_output, "Async and sync results should match");

        // Cleanup
        arth_rt_async_task_cleanup(task_id);
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn test_async_argon2_derive() {
        let password = b"derive-key-password";
        let salt = b"derive-key-salt1234567890"; // Needs to be at least 8 bytes

        // Start async Argon2id key derivation with minimal params
        let task_id = arth_rt_argon2_derive_async(
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            8192, // 8 MB
            1,    // 1 iteration
            1,    // 1 lane
            32,   // 32-byte output
        );
        assert!(task_id > 0, "Should return positive task ID");

        // Wait for completion
        let status = arth_rt_async_task_wait(task_id, 10000);
        assert_eq!(status, 1, "Task should complete successfully");

        // Get result
        let mut output = [0u8; 32];
        let len = arth_rt_async_task_result_bytes(task_id, output.as_mut_ptr(), output.len());
        assert_eq!(len, 32, "Should return 32 bytes");

        // Verify against sync version
        let mut sync_output = [0u8; 32];
        let sync_len = arth_rt_argon2_derive(
            password.as_ptr(),
            password.len(),
            salt.as_ptr(),
            salt.len(),
            8192,
            1,
            1,
            sync_output.as_mut_ptr(),
            sync_output.len(),
        );
        assert_eq!(sync_len, 32);
        assert_eq!(output, sync_output, "Async and sync results should match");

        // Cleanup
        arth_rt_async_task_cleanup(task_id);
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn test_async_task_poll() {
        let password = b"poll-test";

        // Start a longer-running operation
        let task_id =
            arth_rt_password_hash_argon2id_async(password.as_ptr(), password.len(), 8192, 1, 1);
        assert!(task_id > 0);

        // Poll until complete
        let mut polls = 0;
        loop {
            let status = arth_rt_async_task_poll(task_id);
            if status != 0 {
                assert!(status == 1 || status == 2, "Should be completed or failed");
                break;
            }
            polls += 1;
            if polls > 100000 {
                panic!("Task took too long");
            }
            std::thread::sleep(std::time::Duration::from_micros(100));
        }

        // Cleanup
        arth_rt_async_task_cleanup(task_id);
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn test_async_task_invalid_id() {
        // Poll invalid task
        let status = arth_rt_async_task_poll(999999);
        assert_eq!(status, -1, "Invalid task ID should return -1");

        // Try to get result from invalid task
        let mut buffer = [0u8; 64];
        let len = arth_rt_async_task_result_string(999999, buffer.as_mut_ptr(), buffer.len());
        assert_eq!(len, -1, "Invalid task ID should return -1");

        // Try to cleanup invalid task
        let result = arth_rt_async_task_cleanup(999999);
        assert_eq!(result, -1, "Invalid task ID should return -1");
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn test_async_parallel_tasks() {
        // Start multiple async tasks in parallel
        let passwords: Vec<&[u8]> =
            vec![b"password-1", b"password-2", b"password-3", b"password-4"];

        let task_ids: Vec<i64> = passwords
            .iter()
            .map(|p| arth_rt_password_hash_bcrypt_async(p.as_ptr(), p.len(), 4))
            .collect();

        // All task IDs should be positive and unique
        for &id in &task_ids {
            assert!(id > 0, "All task IDs should be positive");
        }
        let unique_count = task_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert_eq!(
            unique_count,
            task_ids.len(),
            "All task IDs should be unique"
        );

        // Wait for all tasks to complete
        for &task_id in &task_ids {
            let status = arth_rt_async_task_wait(task_id, 30000);
            assert_eq!(status, 1, "All tasks should complete successfully");
        }

        // Cleanup all tasks
        for &task_id in &task_ids {
            arth_rt_async_task_cleanup(task_id);
        }
    }

    // =========================================================================
    // Nonce-Misuse Detection Tests
    // =========================================================================

    lazy_static! {
        /// Mutex to serialize nonce tracking tests since they use global state.
        static ref NONCE_TEST_MUTEX: Mutex<()> = Mutex::new(());
    }

    /// Helper to ensure nonce tracking tests don't interfere with each other.
    /// Returns a MutexGuard that must be held for the duration of the test.
    fn acquire_nonce_test_lock() -> std::sync::MutexGuard<'static, ()> {
        let guard = NONCE_TEST_MUTEX.lock().unwrap();
        // Reset state while holding the lock
        NONCE_TRACKING_ENABLED.store(false, Ordering::SeqCst);
        NONCE_WARN_STDERR.store(true, Ordering::SeqCst);
        // Enable temporarily to clear the registry
        NONCE_TRACKING_ENABLED.store(true, Ordering::SeqCst);
        if let Ok(mut registry) = NONCE_REGISTRY.lock() {
            registry.clear();
        }
        NONCE_TRACKING_ENABLED.store(false, Ordering::SeqCst);
        guard
    }

    #[test]
    fn test_nonce_tracking_disabled_by_default() {
        let _guard = acquire_nonce_test_lock();

        // Without enabling tracking, all functions should return -1
        let key = [0u8; 32];
        let nonce = [0u8; 12];

        let check = arth_rt_aead_nonce_check(key.as_ptr(), key.len(), nonce.as_ptr(), nonce.len());
        assert_eq!(check, -1, "Should return -1 when tracking disabled");

        let mark =
            arth_rt_aead_nonce_mark_used(key.as_ptr(), key.len(), nonce.as_ptr(), nonce.len());
        assert_eq!(mark, -1, "Should return -1 when tracking disabled");

        let count = arth_rt_aead_nonce_tracking_count();
        assert_eq!(count, -1, "Should return -1 when tracking disabled");
    }

    #[test]
    fn test_nonce_tracking_enable_disable() {
        let _guard = acquire_nonce_test_lock();

        // Initially disabled
        let was_enabled = arth_rt_aead_nonce_tracking_enable(1);
        assert_eq!(was_enabled, 0, "Should have been disabled");

        // Should now be enabled, disable it
        let was_enabled = arth_rt_aead_nonce_tracking_enable(0);
        assert_eq!(was_enabled, 1, "Should have been enabled");

        // Should now be disabled, enable again
        let was_enabled = arth_rt_aead_nonce_tracking_enable(1);
        assert_eq!(was_enabled, 0, "Should have been disabled");
    }

    #[test]
    fn test_nonce_tracking_basic() {
        let _guard = acquire_nonce_test_lock();
        arth_rt_aead_nonce_tracking_enable(1);

        let key = [1u8; 32]; // AES-256 key
        let nonce = [2u8; 12]; // AES-GCM nonce

        // Check should return 0 (not used)
        let check = arth_rt_aead_nonce_check(key.as_ptr(), key.len(), nonce.as_ptr(), nonce.len());
        assert_eq!(check, 0, "Nonce should not be marked as used yet");

        // Mark as used
        let mark =
            arth_rt_aead_nonce_mark_used(key.as_ptr(), key.len(), nonce.as_ptr(), nonce.len());
        assert_eq!(mark, 0, "First mark should return 0 (new entry)");

        // Check should now return 1 (used)
        let check = arth_rt_aead_nonce_check(key.as_ptr(), key.len(), nonce.as_ptr(), nonce.len());
        assert_eq!(check, 1, "Nonce should be marked as used");

        // Count should be 1
        let count = arth_rt_aead_nonce_tracking_count();
        assert_eq!(count, 1, "Should have 1 tracked entry");
    }

    #[test]
    fn test_nonce_tracking_reuse_detection() {
        let _guard = acquire_nonce_test_lock();
        arth_rt_aead_nonce_tracking_enable(1);
        arth_rt_aead_nonce_warn_enable(0);

        let key = [3u8; 32];
        let nonce = [4u8; 12];

        // First use
        let mark1 =
            arth_rt_aead_nonce_mark_used(key.as_ptr(), key.len(), nonce.as_ptr(), nonce.len());
        assert_eq!(mark1, 0, "First use should return 0");

        // Second use of same key+nonce (reuse!)
        let mark2 =
            arth_rt_aead_nonce_mark_used(key.as_ptr(), key.len(), nonce.as_ptr(), nonce.len());
        assert_eq!(mark2, 1, "Second use should detect reuse and return 1");
    }

    #[test]
    fn test_nonce_tracking_different_keys() {
        let _guard = acquire_nonce_test_lock();
        arth_rt_aead_nonce_tracking_enable(1);
        arth_rt_aead_nonce_warn_enable(0);

        let key1 = [5u8; 32];
        let key2 = [6u8; 32];
        let nonce = [7u8; 12]; // Same nonce for both keys

        // Mark nonce as used with key1
        let mark1 =
            arth_rt_aead_nonce_mark_used(key1.as_ptr(), key1.len(), nonce.as_ptr(), nonce.len());
        assert_eq!(mark1, 0, "First use with key1");

        // Using same nonce with different key should be fine
        let mark2 =
            arth_rt_aead_nonce_mark_used(key2.as_ptr(), key2.len(), nonce.as_ptr(), nonce.len());
        assert_eq!(
            mark2, 0,
            "First use with key2 (different key, same nonce is OK)"
        );

        // Count should be 2 (different key+nonce pairs)
        let count = arth_rt_aead_nonce_tracking_count();
        assert_eq!(count, 2, "Should have 2 tracked entries");
    }

    #[test]
    fn test_nonce_tracking_different_nonces() {
        let _guard = acquire_nonce_test_lock();
        arth_rt_aead_nonce_tracking_enable(1);
        arth_rt_aead_nonce_warn_enable(0);

        let key = [8u8; 32];
        let nonce1 = [9u8; 12];
        let nonce2 = [10u8; 12];

        // Mark nonce1 as used
        let mark1 =
            arth_rt_aead_nonce_mark_used(key.as_ptr(), key.len(), nonce1.as_ptr(), nonce1.len());
        assert_eq!(mark1, 0, "First nonce");

        // Using different nonce with same key should be fine
        let mark2 =
            arth_rt_aead_nonce_mark_used(key.as_ptr(), key.len(), nonce2.as_ptr(), nonce2.len());
        assert_eq!(mark2, 0, "Second nonce (different nonce is OK)");

        // Using nonce1 again should detect reuse
        let mark3 =
            arth_rt_aead_nonce_mark_used(key.as_ptr(), key.len(), nonce1.as_ptr(), nonce1.len());
        assert_eq!(mark3, 1, "Reusing first nonce should be detected");
    }

    #[test]
    fn test_nonce_check_and_mark_atomic() {
        let _guard = acquire_nonce_test_lock();
        arth_rt_aead_nonce_tracking_enable(1);
        arth_rt_aead_nonce_warn_enable(0);

        let key = [11u8; 32];
        let nonce = [12u8; 12];

        // First check_and_mark should return 0 (new)
        let result1 =
            arth_rt_aead_nonce_check_and_mark(key.as_ptr(), key.len(), nonce.as_ptr(), nonce.len());
        assert_eq!(result1, 0, "First use should return 0");

        // Second check_and_mark should return 1 (reuse)
        let result2 =
            arth_rt_aead_nonce_check_and_mark(key.as_ptr(), key.len(), nonce.as_ptr(), nonce.len());
        assert_eq!(result2, 1, "Second use should detect reuse");
    }

    #[test]
    fn test_nonce_tracking_clear() {
        let _guard = acquire_nonce_test_lock();
        arth_rt_aead_nonce_tracking_enable(1);
        arth_rt_aead_nonce_warn_enable(0);

        let key = [13u8; 32];
        let nonce = [14u8; 12];

        // Add an entry
        arth_rt_aead_nonce_mark_used(key.as_ptr(), key.len(), nonce.as_ptr(), nonce.len());

        // Count should be 1
        assert_eq!(arth_rt_aead_nonce_tracking_count(), 1);

        // Clear
        let cleared = arth_rt_aead_nonce_tracking_clear();
        assert_eq!(cleared, 1, "Should have cleared 1 entry");

        // Count should be 0
        assert_eq!(arth_rt_aead_nonce_tracking_count(), 0);

        // Now the same key+nonce should be "new" again
        let mark =
            arth_rt_aead_nonce_mark_used(key.as_ptr(), key.len(), nonce.as_ptr(), nonce.len());
        assert_eq!(mark, 0, "After clear, should be treated as new");
    }

    #[test]
    fn test_nonce_tracking_null_pointers() {
        let _guard = acquire_nonce_test_lock();
        arth_rt_aead_nonce_tracking_enable(1);

        let key = [15u8; 32];
        let nonce = [16u8; 12];

        // Null key
        let result = arth_rt_aead_nonce_check(std::ptr::null(), 32, nonce.as_ptr(), nonce.len());
        assert_eq!(result, -2, "Null key should return -2");

        // Null nonce
        let result = arth_rt_aead_nonce_check(key.as_ptr(), key.len(), std::ptr::null(), 12);
        assert_eq!(result, -2, "Null nonce should return -2");
    }

    #[test]
    fn test_nonce_tracking_chacha_nonce_size() {
        let _guard = acquire_nonce_test_lock();
        arth_rt_aead_nonce_tracking_enable(1);
        arth_rt_aead_nonce_warn_enable(0);

        let key = [17u8; 32]; // ChaCha key
        let nonce_chacha = [18u8; 12]; // ChaCha nonce (12 bytes)

        let mark = arth_rt_aead_nonce_mark_used(
            key.as_ptr(),
            key.len(),
            nonce_chacha.as_ptr(),
            nonce_chacha.len(),
        );
        assert_eq!(mark, 0, "First use of ChaCha nonce");

        // Check with same nonce
        let check = arth_rt_aead_nonce_check(
            key.as_ptr(),
            key.len(),
            nonce_chacha.as_ptr(),
            nonce_chacha.len(),
        );
        assert_eq!(check, 1, "Should detect reuse");
    }
}
