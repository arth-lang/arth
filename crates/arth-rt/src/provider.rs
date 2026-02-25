//! Provider Runtime for Native Compilation
//!
//! Providers are Arth's mechanism for long-lived state. At runtime, providers
//! are represented as structs with two kinds of fields:
//!
//! - `final` fields: Immutable after construction, regular struct field access
//! - `shared` fields: Thread-safe mutable, wrapped in shared cells
//!
//! This module provides convenience wrappers that delegate to struct_rt and
//! shared modules, matching the IR's ProviderNew/ProviderFieldGet/ProviderFieldSet.

use crate::shared::{arth_rt_shared_load, arth_rt_shared_new, arth_rt_shared_store};
use crate::struct_rt::{arth_rt_struct_get, arth_rt_struct_new, arth_rt_struct_set};

// =============================================================================
// Provider Operations - C FFI
// =============================================================================

/// Create a new provider instance.
///
/// Providers are implemented as structs. This function creates the underlying
/// struct that will hold both `final` and `shared` field values.
///
/// # Arguments
/// * `type_name_ptr` - Pointer to the provider type name
/// * `type_name_len` - Length of the type name
/// * `field_count` - Number of fields in the provider
///
/// # Returns
/// Handle to the new provider, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_provider_new(
    type_name_ptr: *const u8,
    type_name_len: usize,
    field_count: i64,
) -> i64 {
    arth_rt_struct_new(type_name_ptr, type_name_len, field_count)
}

/// Get a field from a provider by index.
///
/// For `final` fields, returns the field value directly.
/// For `shared` fields, the value stored is a shared cell handle - the caller
/// must use arth_rt_shared_load to get the actual value.
///
/// # Arguments
/// * `handle` - Provider handle
/// * `field_idx` - Field index
///
/// # Returns
/// Field value (or shared cell handle for shared fields).
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_provider_field_get(handle: i64, field_idx: i64) -> i64 {
    arth_rt_struct_get(handle, field_idx)
}

/// Get a field from a provider by name.
///
/// For `final` fields, returns the field value directly.
/// For `shared` fields, the value stored is a shared cell handle - the caller
/// must use arth_rt_shared_load to get the actual value.
///
/// # Arguments
/// * `handle` - Provider handle
/// * `field_name_ptr` - Pointer to field name
/// * `field_name_len` - Length of field name
///
/// # Returns
/// Field value (or shared cell handle for shared fields).
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_provider_field_get_named(
    handle: i64,
    field_name_ptr: *const u8,
    field_name_len: usize,
) -> i64 {
    crate::struct_rt::arth_rt_struct_get_named(handle, field_name_ptr, field_name_len)
}

/// Set a field on a provider by index.
///
/// For `final` fields, sets the field value directly.
/// For `shared` fields, the value should be a shared cell handle.
///
/// # Arguments
/// * `handle` - Provider handle
/// * `field_idx` - Field index
/// * `value` - Field value (or shared cell handle for shared fields)
/// * `field_name_ptr` - Pointer to field name
/// * `field_name_len` - Length of field name
///
/// # Returns
/// Provider handle on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_provider_field_set(
    handle: i64,
    field_idx: i64,
    value: i64,
    field_name_ptr: *const u8,
    field_name_len: usize,
) -> i64 {
    arth_rt_struct_set(handle, field_idx, value, field_name_ptr, field_name_len)
}

/// Set a field on a provider by name.
///
/// For `final` fields, sets the field value directly.
/// For `shared` fields, the value should be a shared cell handle.
///
/// # Arguments
/// * `handle` - Provider handle
/// * `field_name_ptr` - Pointer to field name
/// * `field_name_len` - Length of field name
/// * `value` - Field value (or shared cell handle for shared fields)
///
/// # Returns
/// Provider handle on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_provider_field_set_named(
    handle: i64,
    field_name_ptr: *const u8,
    field_name_len: usize,
    value: i64,
) -> i64 {
    crate::struct_rt::arth_rt_struct_set_named(handle, field_name_ptr, field_name_len, value)
}

/// Initialize a shared field on a provider.
///
/// Creates a new shared cell and stores the initial value, then sets the
/// field to point to the shared cell handle.
///
/// # Arguments
/// * `handle` - Provider handle
/// * `field_idx` - Field index
/// * `initial_value` - Initial value for the shared field
/// * `field_name_ptr` - Pointer to field name
/// * `field_name_len` - Length of field name
///
/// # Returns
/// The shared cell handle, or -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_provider_shared_field_init(
    handle: i64,
    field_idx: i64,
    initial_value: i64,
    field_name_ptr: *const u8,
    field_name_len: usize,
) -> i64 {
    // Create shared cell with initial value
    let cell_handle = arth_rt_shared_new();
    arth_rt_shared_store(cell_handle, initial_value);

    // Store cell handle in provider field
    let result = arth_rt_struct_set(
        handle,
        field_idx,
        cell_handle,
        field_name_ptr,
        field_name_len,
    );
    if result < 0 {
        return -1;
    }

    cell_handle
}

/// Get a shared field value from a provider.
///
/// Convenience function that gets the shared cell handle from the field,
/// then loads the actual value from the cell.
///
/// # Arguments
/// * `handle` - Provider handle
/// * `field_idx` - Field index
///
/// # Returns
/// The actual value stored in the shared cell.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_provider_shared_get(handle: i64, field_idx: i64) -> i64 {
    let cell_handle = arth_rt_struct_get(handle, field_idx);
    if cell_handle <= 0 {
        return 0;
    }
    arth_rt_shared_load(cell_handle)
}

/// Set a shared field value on a provider.
///
/// Convenience function that gets the shared cell handle from the field,
/// then stores the value to the cell.
///
/// # Arguments
/// * `handle` - Provider handle
/// * `field_idx` - Field index
/// * `value` - Value to store
///
/// # Returns
/// 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_provider_shared_set(handle: i64, field_idx: i64, value: i64) -> i64 {
    let cell_handle = arth_rt_struct_get(handle, field_idx);
    if cell_handle <= 0 {
        return -1;
    }
    arth_rt_shared_store(cell_handle, value)
}

// =============================================================================
// Provider Cleanup / Deallocation
// =============================================================================

/// Free a provider's underlying struct storage.
///
/// This frees the struct that holds the provider's field values.
/// Note: This does NOT automatically free shared cells - those must be
/// freed separately by calling `arth_rt_provider_shared_field_free` for
/// each shared field before calling this function.
///
/// The typical cleanup sequence for a provider with shared fields:
/// 1. Call `arth_rt_provider_shared_field_free` for each shared field
/// 2. Call `arth_rt_provider_free` to free the struct storage
///
/// # Arguments
/// * `handle` - Provider handle
///
/// # Returns
/// 0 on success, -1 if handle not found.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_provider_free(handle: i64) -> i32 {
    crate::struct_rt::arth_rt_struct_free(handle)
}

/// Free a shared cell at a specific field index.
///
/// This retrieves the shared cell handle stored at the given field index
/// and frees that shared cell. Call this for each `shared` field before
/// freeing the provider.
///
/// # Arguments
/// * `handle` - Provider handle
/// * `field_idx` - Field index of the shared field
///
/// # Returns
/// 0 on success, -1 if handle or field not found.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_provider_shared_field_free(handle: i64, field_idx: i64) -> i32 {
    let cell_handle = arth_rt_struct_get(handle, field_idx);
    if cell_handle <= 0 {
        return -1;
    }
    crate::shared::arth_rt_shared_free(cell_handle)
}

/// Free a shared cell at a named field.
///
/// This retrieves the shared cell handle stored at the given field name
/// and frees that shared cell. Call this for each `shared` field before
/// freeing the provider.
///
/// # Arguments
/// * `handle` - Provider handle
/// * `field_name_ptr` - Pointer to field name
/// * `field_name_len` - Length of field name
///
/// # Returns
/// 0 on success, -1 if handle or field not found.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_provider_shared_field_free_named(
    handle: i64,
    field_name_ptr: *const u8,
    field_name_len: usize,
) -> i32 {
    let cell_handle =
        crate::struct_rt::arth_rt_struct_get_named(handle, field_name_ptr, field_name_len);
    if cell_handle <= 0 {
        return -1;
    }
    crate::shared::arth_rt_shared_free(cell_handle)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_create_and_access() {
        let type_name = "ConfigProvider";
        let handle = arth_rt_provider_new(type_name.as_ptr(), type_name.len(), 2);
        assert!(handle > 0);

        // Set final field
        let field1 = "capacity";
        arth_rt_provider_field_set(handle, 0, 100, field1.as_ptr(), field1.len());

        // Get final field
        assert_eq!(arth_rt_provider_field_get(handle, 0), 100);
    }

    #[test]
    fn test_provider_shared_field() {
        let type_name = "CounterProvider";
        let handle = arth_rt_provider_new(type_name.as_ptr(), type_name.len(), 1);
        assert!(handle > 0);

        // Initialize shared field
        let field_name = "count";
        let cell = arth_rt_provider_shared_field_init(
            handle,
            0,
            42,
            field_name.as_ptr(),
            field_name.len(),
        );
        assert!(cell > 0);

        // Read via convenience function
        assert_eq!(arth_rt_provider_shared_get(handle, 0), 42);

        // Write via convenience function
        arth_rt_provider_shared_set(handle, 0, 100);
        assert_eq!(arth_rt_provider_shared_get(handle, 0), 100);

        // Can also use raw shared cell handle
        let cell_handle = arth_rt_provider_field_get(handle, 0);
        assert_eq!(arth_rt_shared_load(cell_handle), 100);
    }

    #[test]
    fn test_provider_free() {
        let type_name = "TempProvider";
        let handle = arth_rt_provider_new(type_name.as_ptr(), type_name.len(), 1);
        assert!(handle > 0);

        // Set a field
        let field_name = "value";
        arth_rt_provider_field_set(handle, 0, 42, field_name.as_ptr(), field_name.len());

        // Free the provider
        assert_eq!(arth_rt_provider_free(handle), 0);

        // Trying to free again should fail
        assert_eq!(arth_rt_provider_free(handle), -1);
    }

    #[test]
    fn test_provider_shared_field_free() {
        let type_name = "SharedProvider";
        let handle = arth_rt_provider_new(type_name.as_ptr(), type_name.len(), 2);
        assert!(handle > 0);

        // Initialize two shared fields
        let field1 = "count1";
        let cell1 =
            arth_rt_provider_shared_field_init(handle, 0, 10, field1.as_ptr(), field1.len());
        assert!(cell1 > 0);

        let field2 = "count2";
        let cell2 =
            arth_rt_provider_shared_field_init(handle, 1, 20, field2.as_ptr(), field2.len());
        assert!(cell2 > 0);

        // Values should be accessible
        assert_eq!(arth_rt_provider_shared_get(handle, 0), 10);
        assert_eq!(arth_rt_provider_shared_get(handle, 1), 20);

        // Free shared field 0
        assert_eq!(arth_rt_provider_shared_field_free(handle, 0), 0);

        // Free shared field 1 by name
        assert_eq!(
            arth_rt_provider_shared_field_free_named(handle, field2.as_ptr(), field2.len()),
            0
        );

        // Free the provider struct
        assert_eq!(arth_rt_provider_free(handle), 0);
    }
}
