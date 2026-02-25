//! Shared Memory Cells for Provider Fields
//!
//! This module provides thread-safe shared memory cells for provider `shared` fields.
//! Each cell is an atomic storage location identified by a handle.
//!
//! # Design Notes
//!
//! - Uses atomic i64 operations for thread safety
//! - Handle-based API matches the VM's SharedNew/SharedLoad/SharedStore operations
//! - Named cells support global/provider-level shared state

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

use crate::new_handle;

// =============================================================================
// Shared Cell Storage
// =============================================================================

/// A thread-safe shared cell containing an i64 value.
struct SharedCell {
    value: AtomicI64,
}

impl SharedCell {
    fn new() -> Self {
        Self {
            value: AtomicI64::new(0),
        }
    }

    fn load(&self) -> i64 {
        self.value.load(Ordering::SeqCst)
    }

    fn store(&self, val: i64) {
        self.value.store(val, Ordering::SeqCst);
    }
}

lazy_static::lazy_static! {
    /// Global shared cell storage. Maps handle -> SharedCell.
    static ref SHARED_CELLS: Mutex<HashMap<i64, SharedCell>> = Mutex::new(HashMap::new());

    /// Named shared cells. Maps name -> handle for global/provider-level shared state.
    static ref NAMED_CELLS: Mutex<HashMap<String, i64>> = Mutex::new(HashMap::new());
}

// =============================================================================
// Shared Cell Operations - C FFI
// =============================================================================

/// Create a new shared cell.
///
/// # Returns
/// Handle to the new shared cell.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_shared_new() -> i64 {
    let handle = new_handle();
    let mut cells = SHARED_CELLS.lock().unwrap();
    cells.insert(handle, SharedCell::new());
    handle
}

/// Load value from a shared cell.
///
/// # Arguments
/// * `handle` - Shared cell handle
///
/// # Returns
/// The current value in the cell (0 if not found).
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_shared_load(handle: i64) -> i64 {
    let cells = SHARED_CELLS.lock().unwrap();
    cells.get(&handle).map(|c| c.load()).unwrap_or(0)
}

/// Store value to a shared cell.
///
/// # Arguments
/// * `handle` - Shared cell handle
/// * `value` - Value to store
///
/// # Returns
/// 0 on success, -1 if handle not found.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_shared_store(handle: i64, value: i64) -> i64 {
    let cells = SHARED_CELLS.lock().unwrap();
    if let Some(cell) = cells.get(&handle) {
        cell.store(value);
        0
    } else {
        -1
    }
}

/// Get or create a named shared cell.
///
/// # Arguments
/// * `name_ptr` - Pointer to the name string
/// * `name_len` - Length of the name string
///
/// # Returns
/// Handle to the shared cell (creates if doesn't exist).
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_shared_get_named(name_ptr: *const u8, name_len: usize) -> i64 {
    let name = unsafe {
        if name_ptr.is_null() {
            return -1;
        }
        let slice = std::slice::from_raw_parts(name_ptr, name_len);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    let mut named = NAMED_CELLS.lock().unwrap();
    if let Some(&handle) = named.get(&name) {
        return handle;
    }

    // Create new cell
    let handle = arth_rt_shared_new();
    named.insert(name, handle);
    handle
}

/// Free a shared cell.
///
/// # Arguments
/// * `handle` - Shared cell handle
///
/// # Returns
/// 0 on success, -1 if handle not found.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_shared_free(handle: i64) -> i32 {
    let mut cells = SHARED_CELLS.lock().unwrap();
    if cells.remove(&handle).is_some() {
        0
    } else {
        -1
    }
}

/// Compare and swap a shared cell value.
///
/// # Arguments
/// * `handle` - Shared cell handle
/// * `expected` - Expected current value
/// * `new_value` - New value to set if current matches expected
///
/// # Returns
/// 1 if swap succeeded, 0 if it failed (value didn't match), -1 if handle not found.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_shared_cas(handle: i64, expected: i64, new_value: i64) -> i64 {
    let cells = SHARED_CELLS.lock().unwrap();
    if let Some(cell) = cells.get(&handle) {
        match cell
            .value
            .compare_exchange(expected, new_value, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => 1,
            Err(_) => 0,
        }
    } else {
        -1
    }
}

/// Atomically add to a shared cell value.
///
/// # Arguments
/// * `handle` - Shared cell handle
/// * `delta` - Value to add
///
/// # Returns
/// The previous value, or 0 if handle not found.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_shared_add(handle: i64, delta: i64) -> i64 {
    let cells = SHARED_CELLS.lock().unwrap();
    if let Some(cell) = cells.get(&handle) {
        cell.value.fetch_add(delta, Ordering::SeqCst)
    } else {
        0
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_new_and_access() {
        let handle = arth_rt_shared_new();
        assert!(handle > 0);

        // Initial value is 0
        assert_eq!(arth_rt_shared_load(handle), 0);

        // Store and load
        assert_eq!(arth_rt_shared_store(handle, 42), 0);
        assert_eq!(arth_rt_shared_load(handle), 42);

        // Free
        assert_eq!(arth_rt_shared_free(handle), 0);
    }

    #[test]
    fn test_shared_named() {
        let name = "test.counter";
        let h1 = arth_rt_shared_get_named(name.as_ptr(), name.len());
        let h2 = arth_rt_shared_get_named(name.as_ptr(), name.len());

        // Same name returns same handle
        assert_eq!(h1, h2);

        // Can store and load via handle
        arth_rt_shared_store(h1, 100);
        assert_eq!(arth_rt_shared_load(h2), 100);
    }

    #[test]
    fn test_shared_cas() {
        let handle = arth_rt_shared_new();
        arth_rt_shared_store(handle, 10);

        // CAS success
        assert_eq!(arth_rt_shared_cas(handle, 10, 20), 1);
        assert_eq!(arth_rt_shared_load(handle), 20);

        // CAS failure (expected doesn't match)
        assert_eq!(arth_rt_shared_cas(handle, 10, 30), 0);
        assert_eq!(arth_rt_shared_load(handle), 20); // Unchanged

        arth_rt_shared_free(handle);
    }

    #[test]
    fn test_shared_add() {
        let handle = arth_rt_shared_new();
        arth_rt_shared_store(handle, 100);

        // Add returns previous value
        assert_eq!(arth_rt_shared_add(handle, 5), 100);
        assert_eq!(arth_rt_shared_load(handle), 105);

        assert_eq!(arth_rt_shared_add(handle, -10), 105);
        assert_eq!(arth_rt_shared_load(handle), 95);

        arth_rt_shared_free(handle);
    }

    #[test]
    fn test_shared_invalid_handle() {
        assert_eq!(arth_rt_shared_load(999999), 0);
        assert_eq!(arth_rt_shared_store(999999, 42), -1);
        assert_eq!(arth_rt_shared_free(999999), -1);
        assert_eq!(arth_rt_shared_cas(999999, 0, 1), -1);
    }
}
