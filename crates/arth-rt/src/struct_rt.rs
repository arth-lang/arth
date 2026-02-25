//! Struct Runtime for Native Compilation
//!
//! This module provides C FFI functions for struct operations in natively
//! compiled Arth code. Structs are represented as dynamically-typed objects
//! with named fields, similar to the VM implementation.
//!
//! # Design Notes
//!
//! For Phase 1, we use a dynamic struct representation (HashMap-based) that
//! matches the VM's behavior. This allows existing IR to work unchanged.
//!
//! Future optimization: With type information in the IR, we can switch to
//! static struct layouts with direct field access (GEP).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::new_handle;

// =============================================================================
// Struct Storage
// =============================================================================

/// A dynamically-typed struct value.
#[derive(Clone, Debug)]
struct StructValue {
    /// Type name of the struct (e.g., "Point", "User").
    type_name: String,
    /// Fields stored by name -> value.
    /// Values are stored as i64 (handles or primitives).
    fields: HashMap<String, i64>,
    /// Field order for JSON serialization.
    field_order: Vec<String>,
}

lazy_static::lazy_static! {
    /// Global struct storage. Maps handle -> StructValue.
    static ref STRUCTS: Mutex<HashMap<i64, StructValue>> = Mutex::new(HashMap::new());
}

// =============================================================================
// Struct Operations - C FFI
// =============================================================================

/// Create a new struct with the given type name and field count.
///
/// # Arguments
/// * `type_name_ptr` - Pointer to the type name string
/// * `type_name_len` - Length of the type name
/// * `field_count` - Number of fields (used for pre-allocation)
///
/// # Returns
/// Handle to the new struct, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_struct_new(
    type_name_ptr: *const u8,
    type_name_len: usize,
    field_count: i64,
) -> i64 {
    let type_name = unsafe {
        if type_name_ptr.is_null() {
            return -1;
        }
        let slice = std::slice::from_raw_parts(type_name_ptr, type_name_len);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    let handle = new_handle();
    let value = StructValue {
        type_name,
        fields: HashMap::with_capacity(field_count as usize),
        field_order: Vec::with_capacity(field_count as usize),
    };

    let mut structs = STRUCTS.lock().unwrap();
    structs.insert(handle, value);

    handle
}

/// Set a field on a struct by index.
///
/// # Arguments
/// * `handle` - Struct handle
/// * `field_idx` - Field index (for ordering)
/// * `value` - Field value (i64)
/// * `field_name_ptr` - Pointer to field name string
/// * `field_name_len` - Length of field name
///
/// # Returns
/// The struct handle on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_struct_set(
    handle: i64,
    _field_idx: i64,
    value: i64,
    field_name_ptr: *const u8,
    field_name_len: usize,
) -> i64 {
    let field_name = unsafe {
        if field_name_ptr.is_null() {
            return -1;
        }
        let slice = std::slice::from_raw_parts(field_name_ptr, field_name_len);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    let mut structs = STRUCTS.lock().unwrap();
    if let Some(sv) = structs.get_mut(&handle) {
        if !sv.fields.contains_key(&field_name) {
            sv.field_order.push(field_name.clone());
        }
        sv.fields.insert(field_name, value);
        handle
    } else {
        -1
    }
}

/// Get a field from a struct by index.
///
/// # Arguments
/// * `handle` - Struct handle
/// * `field_idx` - Field index
///
/// # Returns
/// Field value, or 0 if not found.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_struct_get(handle: i64, field_idx: i64) -> i64 {
    let structs = STRUCTS.lock().unwrap();
    if let Some(sv) = structs.get(&handle) {
        if let Some(name) = sv.field_order.get(field_idx as usize) {
            return sv.fields.get(name).copied().unwrap_or(0);
        }
    }
    0
}

/// Set a field on a struct by name.
///
/// # Arguments
/// * `handle` - Struct handle
/// * `field_name_ptr` - Pointer to field name string
/// * `field_name_len` - Length of field name
/// * `value` - Field value (i64)
///
/// # Returns
/// The struct handle on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_struct_set_named(
    handle: i64,
    field_name_ptr: *const u8,
    field_name_len: usize,
    value: i64,
) -> i64 {
    let field_name = unsafe {
        if field_name_ptr.is_null() {
            return -1;
        }
        let slice = std::slice::from_raw_parts(field_name_ptr, field_name_len);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    let mut structs = STRUCTS.lock().unwrap();
    if let Some(sv) = structs.get_mut(&handle) {
        if !sv.fields.contains_key(&field_name) {
            sv.field_order.push(field_name.clone());
        }
        sv.fields.insert(field_name, value);
        handle
    } else {
        -1
    }
}

/// Get a field from a struct by name.
///
/// # Arguments
/// * `handle` - Struct handle
/// * `field_name_ptr` - Pointer to field name string
/// * `field_name_len` - Length of field name
///
/// # Returns
/// Field value, or 0 if not found.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_struct_get_named(
    handle: i64,
    field_name_ptr: *const u8,
    field_name_len: usize,
) -> i64 {
    let field_name = unsafe {
        if field_name_ptr.is_null() {
            return 0;
        }
        let slice = std::slice::from_raw_parts(field_name_ptr, field_name_len);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    let structs = STRUCTS.lock().unwrap();
    if let Some(sv) = structs.get(&handle) {
        return sv.fields.get(field_name).copied().unwrap_or(0);
    }
    0
}

/// Copy all fields from source struct to destination struct.
///
/// # Arguments
/// * `dest_handle` - Destination struct handle
/// * `src_handle` - Source struct handle
///
/// # Returns
/// Destination handle on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_struct_copy(dest_handle: i64, src_handle: i64) -> i64 {
    let mut structs = STRUCTS.lock().unwrap();

    // Clone source fields first to avoid borrow conflicts
    let src_fields = structs
        .get(&src_handle)
        .map(|src| (src.fields.clone(), src.field_order.clone()));

    if let Some((fields, order)) = src_fields {
        if let Some(dest) = structs.get_mut(&dest_handle) {
            for name in order {
                if let Some(value) = fields.get(&name) {
                    if !dest.fields.contains_key(&name) {
                        dest.field_order.push(name.clone());
                    }
                    dest.fields.insert(name, *value);
                }
            }
            return dest_handle;
        }
    }
    -1
}

/// Get the type name of a struct.
///
/// # Arguments
/// * `handle` - Struct handle
/// * `buf` - Output buffer for type name
/// * `buf_len` - Buffer length
///
/// # Returns
/// Length of type name written, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_struct_type_name(handle: i64, buf: *mut u8, buf_len: usize) -> i64 {
    if buf.is_null() {
        return -1;
    }

    let structs = STRUCTS.lock().unwrap();
    if let Some(sv) = structs.get(&handle) {
        let name_bytes = sv.type_name.as_bytes();
        if name_bytes.len() > buf_len {
            return -1;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(name_bytes.as_ptr(), buf, name_bytes.len());
        }
        return name_bytes.len() as i64;
    }
    -1
}

/// Get the type name pointer (for exception dispatch).
/// Returns a handle that can be compared for equality.
///
/// # Arguments
/// * `handle` - Struct handle
///
/// # Returns
/// A hash of the type name for comparison, or 0 if not found.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_struct_type_id(handle: i64) -> i64 {
    let structs = STRUCTS.lock().unwrap();
    if let Some(sv) = structs.get(&handle) {
        // Use a simple hash for type comparison
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        sv.type_name.hash(&mut hasher);
        return hasher.finish() as i64;
    }
    0
}

/// Free a struct.
///
/// # Arguments
/// * `handle` - Struct handle
///
/// # Returns
/// 0 on success, -1 if handle not found.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_struct_free(handle: i64) -> i32 {
    let mut structs = STRUCTS.lock().unwrap();
    if structs.remove(&handle).is_some() {
        0
    } else {
        -1
    }
}

/// Get the number of fields in a struct.
///
/// # Arguments
/// * `handle` - Struct handle
///
/// # Returns
/// Number of fields, or -1 if handle not found.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_struct_field_count(handle: i64) -> i64 {
    let structs = STRUCTS.lock().unwrap();
    if let Some(sv) = structs.get(&handle) {
        sv.fields.len() as i64
    } else {
        -1
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_struct_create_and_access() {
        let type_name = "Point";
        let handle = arth_rt_struct_new(type_name.as_ptr(), type_name.len(), 2);
        assert!(handle > 0);

        // Set fields
        let x_name = "x";
        let result = arth_rt_struct_set(handle, 0, 42, x_name.as_ptr(), x_name.len());
        assert_eq!(result, handle);

        let y_name = "y";
        let result = arth_rt_struct_set(handle, 1, 100, y_name.as_ptr(), y_name.len());
        assert_eq!(result, handle);

        // Get by index
        assert_eq!(arth_rt_struct_get(handle, 0), 42);
        assert_eq!(arth_rt_struct_get(handle, 1), 100);

        // Get by name
        assert_eq!(
            arth_rt_struct_get_named(handle, x_name.as_ptr(), x_name.len()),
            42
        );

        // Field count
        assert_eq!(arth_rt_struct_field_count(handle), 2);

        // Free
        assert_eq!(arth_rt_struct_free(handle), 0);
    }

    #[test]
    fn test_struct_copy() {
        let type_name = "Point";
        let src = arth_rt_struct_new(type_name.as_ptr(), type_name.len(), 2);

        let x_name = "x";
        let y_name = "y";
        arth_rt_struct_set(src, 0, 10, x_name.as_ptr(), x_name.len());
        arth_rt_struct_set(src, 1, 20, y_name.as_ptr(), y_name.len());

        let dest = arth_rt_struct_new(type_name.as_ptr(), type_name.len(), 2);
        arth_rt_struct_copy(dest, src);

        assert_eq!(
            arth_rt_struct_get_named(dest, x_name.as_ptr(), x_name.len()),
            10
        );
        assert_eq!(
            arth_rt_struct_get_named(dest, y_name.as_ptr(), y_name.len()),
            20
        );

        arth_rt_struct_free(src);
        arth_rt_struct_free(dest);
    }
}
