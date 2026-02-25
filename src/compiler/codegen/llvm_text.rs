use std::fmt::Write as _;

use crate::compiler::ir::{BinOp, BlockData, CmpPred, Func, Inst, InstKind, Module, Ty};

use std::collections::{HashMap, HashSet};

use super::llvm_debug::DebugInfoBuilder;
use super::llvm_types::{ArthType, FieldDef, TypeRegistry};
use super::llvm_types::{EnumDef as LlvmEnumDef, StructDef as LlvmStructDef, VariantDef};
use super::mangle::mangle_function;

/// Maps VM intrinsic call names (e.g., "__arth_file_open") to native symbols (e.g., "arth_rt_file_open").
/// Returns the native symbol if one exists for native compilation, None otherwise.
pub fn native_symbol_for_call(name: &str) -> Option<&'static str> {
    match name {
        // VM-specific print functions - use helper functions that take null-terminated strings
        "__arth_vm_print_raw" | "__arth_vm_print_str" => Some("arth_rt_console_write_str"),
        "__arth_vm_print_ln" => Some("arth_rt_console_write_ln"),
        "__arth_vm_print_val" => Some("arth_rt_console_write_i64"),
        // These take (prefix_str, value) - for now just print the value
        "__arth_vm_print_raw_str_val" | "__arth_vm_print_str_val" => {
            Some("arth_rt_console_write_i64")
        }

        // Console I/O
        "__arth_print" | "__arth_console_write" => Some("arth_rt_console_write"),
        "__arth_println" => Some("arth_rt_console_write"), // newline handled by caller
        "__arth_console_write_err" => Some("arth_rt_console_write_err"),
        "__arth_console_read_line" => Some("arth_rt_console_read_line"),

        // File operations
        "__arth_file_open" => Some("arth_rt_file_open"),
        "__arth_file_close" => Some("arth_rt_file_close"),
        "__arth_file_read" => Some("arth_rt_file_read"),
        "__arth_file_write" => Some("arth_rt_file_write"),
        "__arth_file_flush" => Some("arth_rt_file_flush"),
        "__arth_file_seek" => Some("arth_rt_file_seek"),
        "__arth_file_size" => Some("arth_rt_file_size"),
        "__arth_file_exists" => Some("arth_rt_file_exists"),
        "__arth_file_delete" => Some("arth_rt_file_delete"),
        "__arth_file_copy" => Some("arth_rt_file_copy"),
        "__arth_file_move" => Some("arth_rt_file_move"),

        // Directory operations
        "__arth_dir_create" => Some("arth_rt_dir_create"),
        "__arth_dir_create_all" => Some("arth_rt_dir_create_all"),
        "__arth_dir_delete" => Some("arth_rt_dir_delete"),
        "__arth_dir_list" => Some("arth_rt_dir_list"),
        "__arth_dir_exists" => Some("arth_rt_dir_exists"),
        "__arth_is_dir" => Some("arth_rt_is_dir"),
        "__arth_is_file" => Some("arth_rt_is_file"),
        "__arth_path_absolute" => Some("arth_rt_path_absolute"),

        // Time operations
        "__arth_time_now" => Some("arth_rt_time_now"),
        "__arth_time_now_nanos" => Some("arth_rt_time_now_nanos"),
        "__arth_time_format" => Some("arth_rt_time_format"),
        "__arth_time_parse" => Some("arth_rt_time_parse"),
        "__arth_instant_now" => Some("arth_rt_instant_now"),
        "__arth_instant_elapsed" => Some("arth_rt_instant_elapsed"),
        "__arth_instant_elapsed_nanos" => Some("arth_rt_instant_elapsed_nanos"),
        "__arth_instant_free" => Some("arth_rt_instant_free"),
        "__arth_sleep" => Some("arth_rt_sleep"),
        "__arth_sleep_nanos" => Some("arth_rt_sleep_nanos"),

        // Error handling
        "__arth_errno" => Some("arth_rt_errno"),
        "__arth_strerror" => Some("arth_rt_strerror"),

        // Struct operations
        "__arth_struct_new" => Some("arth_rt_struct_new"),
        "__arth_struct_set" => Some("arth_rt_struct_set"),
        "__arth_struct_get" => Some("arth_rt_struct_get"),
        "__arth_struct_set_named" => Some("arth_rt_struct_set_named"),
        "__arth_struct_get_named" => Some("arth_rt_struct_get_named"),
        "__arth_struct_copy" => Some("arth_rt_struct_copy"),
        "__arth_struct_type_name" => Some("arth_rt_struct_type_id"),
        "__arth_struct_free" => Some("arth_rt_struct_free"),
        "__arth_enum_new" => Some("arth_rt_enum_new"),
        "__arth_enum_set_payload" => Some("arth_rt_enum_set_payload"),
        "__arth_enum_get_payload" => Some("arth_rt_enum_get_payload"),
        "__arth_enum_get_tag" => Some("arth_rt_enum_get_tag"),

        // Exception handling
        "__arth_throw" => Some("arth_rt_throw"),
        "__arth_begin_catch" => Some("arth_rt_begin_catch"),
        "__arth_end_catch" => Some("arth_rt_end_catch"),
        "__arth_rethrow" => Some("arth_rt_rethrow"),
        "__arth_resume_unwind" => Some("arth_rt_resume_unwind"),
        "__arth_exception_type_id" => Some("arth_rt_exception_type_id"),
        "__arth_exception_payload" => Some("arth_rt_exception_payload"),
        "__arth_type_id" => Some("arth_rt_type_id"),

        // Async runtime
        "__arth_async_frame_alloc" => Some("arth_rt_async_frame_alloc"),
        "__arth_async_frame_free" => Some("arth_rt_async_frame_free"),
        "__arth_async_frame_get_state" => Some("arth_rt_async_frame_get_state"),
        "__arth_async_frame_set_state" => Some("arth_rt_async_frame_set_state"),
        "__arth_async_frame_load" => Some("arth_rt_async_frame_load"),
        "__arth_async_frame_store" => Some("arth_rt_async_frame_store"),
        "__arth_task_spawn_with_poll" => Some("arth_rt_task_spawn_with_poll"),
        "__arth_task_is_cancelled" => Some("arth_rt_task_is_cancelled"),
        "__arth_task_yield" => Some("arth_rt_task_yield"),
        "__arth_task_check_cancelled" => Some("arth_rt_task_check_cancelled"),
        "__arth_task_has_error" => Some("arth_rt_task_has_error"),
        "__arth_task_get_error" => Some("arth_rt_task_get_error"),
        "__arth_task_get_result" => Some("arth_rt_task_get_result"),
        "__arth_create_cancelled_error" => Some("arth_rt_create_cancelled_error"),
        "__arth_async_yield" => Some("arth_rt_async_yield"),
        "__arth_await" => Some("arth_rt_await"),

        // Concurrent executor runtime
        "__arth_executor_init" => Some("arth_rt_executor_init"),
        "__arth_executor_thread_count" => Some("arth_rt_executor_thread_count"),
        "__arth_executor_active_workers" => Some("arth_rt_executor_active_workers"),
        "__arth_executor_spawn" => Some("arth_rt_executor_spawn"),
        "__arth_executor_cancel" => Some("arth_rt_executor_cancel"),
        "__arth_executor_join" => Some("arth_rt_executor_join"),
        "__arth_executor_spawn_with_arg" => Some("arth_rt_executor_spawn_with_arg"),
        "__arth_executor_active_executor_count" => Some("arth_rt_executor_active_executor_count"),
        "__arth_executor_worker_task_count" => Some("arth_rt_executor_worker_task_count"),
        "__arth_executor_reset_stats" => Some("arth_rt_executor_reset_stats"),
        "__arth_executor_spawn_await" => Some("arth_rt_executor_spawn_await"),

        // Region-based allocation
        "__arth_region_enter" => Some("arth_rt_region_enter"),
        "__arth_region_exit" => Some("arth_rt_region_exit"),
        "__arth_region_alloc" => Some("arth_rt_region_alloc"),

        // Provider operations
        "__arth_provider_new" => Some("arth_rt_provider_new"),
        "__arth_provider_field_get" => Some("arth_rt_provider_field_get"),
        "__arth_provider_field_get_named" => Some("arth_rt_provider_field_get_named"),
        "__arth_provider_field_set" => Some("arth_rt_provider_field_set"),
        "__arth_provider_field_set_named" => Some("arth_rt_provider_field_set_named"),
        "__arth_provider_shared_field_init" => Some("arth_rt_provider_shared_field_init"),
        "__arth_provider_shared_get" => Some("arth_rt_provider_shared_get"),
        "__arth_provider_shared_set" => Some("arth_rt_provider_shared_set"),
        // Provider cleanup/deinit
        "__arth_provider_free" => Some("arth_rt_provider_free"),
        "__arth_provider_shared_field_free" => Some("arth_rt_provider_shared_field_free"),
        "__arth_provider_shared_field_free_named" => {
            Some("arth_rt_provider_shared_field_free_named")
        }

        // Shared memory cells
        "__arth_shared_new" => Some("arth_rt_shared_new"),
        "__arth_shared_load" => Some("arth_rt_shared_load"),
        "__arth_shared_store" => Some("arth_rt_shared_store"),
        "__arth_shared_get_named" => Some("arth_rt_shared_get_named"),
        "__arth_shared_free" => Some("arth_rt_shared_free"),
        "__arth_shared_cas" => Some("arth_rt_shared_cas"),
        "__arth_shared_add" => Some("arth_rt_shared_add"),

        // Network operations
        "__arth_socket_create" => Some("arth_rt_socket_create"),
        "__arth_socket_close" => Some("arth_rt_socket_close"),
        "__arth_socket_connect" => Some("arth_rt_socket_connect"),
        "__arth_socket_connect_host" => Some("arth_rt_socket_connect_host"),
        "__arth_socket_bind" => Some("arth_rt_socket_bind"),
        "__arth_socket_bind_port" => Some("arth_rt_socket_bind_port"),
        "__arth_socket_listen" => Some("arth_rt_socket_listen"),
        "__arth_socket_accept" => Some("arth_rt_socket_accept"),
        "__arth_socket_send" => Some("arth_rt_socket_send"),
        "__arth_socket_recv" => Some("arth_rt_socket_recv"),
        "__arth_socket_setsockopt" => Some("arth_rt_socket_setsockopt"),
        "__arth_socket_setsockopt_int" => Some("arth_rt_socket_setsockopt_int"),
        "__arth_socket_set_nonblocking" => Some("arth_rt_socket_set_nonblocking"),
        "__arth_socket_fd" => Some("arth_rt_socket_fd"),
        "__arth_getaddrinfo" => Some("arth_rt_getaddrinfo"),
        "__arth_addrinfo_next" => Some("arth_rt_addrinfo_next"),
        "__arth_freeaddrinfo" => Some("arth_rt_freeaddrinfo"),
        "__arth_addr_ipv4" => Some("arth_rt_addr_ipv4"),
        "__arth_addr_parse" => Some("arth_rt_addr_parse"),

        // No native symbol - use VM runtime
        _ => None,
    }
}

/// Returns the LLVM function signature for a native runtime function.
/// Format: (return_type, param_types)
fn native_symbol_signature(name: &str) -> (&'static str, &'static str) {
    match name {
        // Console I/O
        "arth_rt_console_write" => ("i64", "ptr, i64"), // (ptr, len) -> bytes_written
        "arth_rt_console_write_err" => ("i64", "ptr, i64"), // (ptr, len) -> bytes_written
        "arth_rt_console_read_line" => ("i64", "ptr, i64"), // (buf, buf_len) -> bytes_read
        // Simple console helpers for VM intrinsic parity
        "arth_rt_console_write_str" => ("i64", "ptr"), // (null_terminated_str) -> bytes_written
        "arth_rt_console_write_ln" => ("i64", ""),     // () -> bytes_written (prints newline)
        "arth_rt_console_write_i64" => ("i64", "i64"), // (value) -> bytes_written

        // File operations
        "arth_rt_file_open" => ("i64", "ptr, i64, i32"), // (path, path_len, mode) -> handle
        "arth_rt_file_close" => ("i32", "i64"),          // (handle) -> status
        "arth_rt_file_read" => ("i64", "i64, ptr, i64"), // (handle, buf, len) -> bytes_read
        "arth_rt_file_write" => ("i64", "i64, ptr, i64"), // (handle, buf, len) -> bytes_written
        "arth_rt_file_flush" => ("i32", "i64"),          // (handle) -> status
        "arth_rt_file_seek" => ("i64", "i64, i64, i32"), // (handle, offset, whence) -> pos
        "arth_rt_file_size" => ("i64", "i64"),           // (handle) -> size
        "arth_rt_file_exists" => ("i32", "ptr, i64"),    // (path, len) -> bool
        "arth_rt_file_delete" => ("i32", "ptr, i64"),    // (path, len) -> status
        "arth_rt_file_copy" => ("i32", "ptr, i64, ptr, i64"), // (src, src_len, dst, dst_len) -> status
        "arth_rt_file_move" => ("i32", "ptr, i64, ptr, i64"), // (src, src_len, dst, dst_len) -> status

        // Directory operations
        "arth_rt_dir_create" => ("i32", "ptr, i64"), // (path, len) -> status
        "arth_rt_dir_create_all" => ("i32", "ptr, i64"), // (path, len) -> status
        "arth_rt_dir_delete" => ("i32", "ptr, i64"), // (path, len) -> status
        "arth_rt_dir_list" => ("i64", "ptr, i64"),   // (path, len) -> handle
        "arth_rt_dir_exists" => ("i32", "ptr, i64"), // (path, len) -> bool
        "arth_rt_is_dir" => ("i32", "ptr, i64"),     // (path, len) -> bool
        "arth_rt_is_file" => ("i32", "ptr, i64"),    // (path, len) -> bool
        "arth_rt_path_absolute" => ("i32", "ptr, i64, ptr, i64"), // (path, len, buf, buf_len) -> status

        // Time operations
        "arth_rt_time_now" => ("i64", ""),       // () -> millis
        "arth_rt_time_now_nanos" => ("i64", ""), // () -> nanos
        "arth_rt_time_format" => ("i32", "i64, ptr, i64, ptr, i64"), // (millis, fmt, fmt_len, buf, buf_len) -> len
        "arth_rt_time_parse" => ("i64", "ptr, i64, ptr, i64"), // (str, str_len, fmt, fmt_len) -> millis
        "arth_rt_instant_now" => ("i64", ""),                  // () -> handle
        "arth_rt_instant_elapsed" => ("i64", "i64"),           // (handle) -> millis
        "arth_rt_instant_elapsed_nanos" => ("i64", "i64"),     // (handle) -> nanos
        "arth_rt_instant_free" => ("i32", "i64"),              // (handle) -> status
        "arth_rt_sleep" => ("i32", "i64"),                     // (millis) -> status
        "arth_rt_sleep_nanos" => ("i32", "i64"),               // (nanos) -> status

        // Error handling
        "arth_rt_errno" => ("i32", ""),            // () -> errno
        "arth_rt_strerror" => ("i32", "ptr, i64"), // (buf, buf_len) -> len

        // Struct operations
        "arth_rt_struct_new" => ("i64", "ptr, i64, i64"), // (type_name, type_name_len, field_count) -> handle
        "arth_rt_struct_set" => ("i64", "i64, i64, i64, ptr, i64"), // (handle, field_idx, value, field_name, field_name_len) -> handle
        "arth_rt_struct_get" => ("i64", "i64, i64"),                // (handle, field_idx) -> value
        "arth_rt_struct_set_named" => ("i64", "i64, ptr, i64, i64"), // (handle, field_name, field_name_len, value) -> handle
        "arth_rt_struct_get_named" => ("i64", "i64, ptr, i64"), // (handle, field_name, field_name_len) -> value
        "arth_rt_struct_copy" => ("i64", "i64, i64"),           // (dest, src) -> dest
        "arth_rt_struct_type_id" => ("i64", "i64"),             // (handle) -> type_id
        "arth_rt_struct_free" => ("i32", "i64"),                // (handle) -> status
        "arth_rt_enum_new" => ("i64", "ptr, ptr, i64, i64"), // (enum_name, variant_name, tag, payload_count) -> handle
        "arth_rt_enum_set_payload" => ("i64", "i64, i64, i64"), // (handle, index, value) -> handle
        "arth_rt_enum_get_payload" => ("i64", "i64, i64"),   // (handle, index) -> value
        "arth_rt_enum_get_tag" => ("i64", "i64"),            // (handle) -> tag

        // Exception handling
        "arth_rt_throw" => ("void", "i64, ptr, i64, ptr, i64"), // (type_id, type_name, type_name_len, payload, payload_size) -> never returns
        "arth_rt_begin_catch" => ("ptr", "ptr"),                // (exception_ptr) -> ArthException*
        "arth_rt_end_catch" => ("void", ""),                    // () -> void
        "arth_rt_rethrow" => ("void", ""),                      // () -> never returns
        "arth_rt_resume_unwind" => ("void", "ptr"),             // (exception_ptr) -> never returns
        "arth_rt_exception_type_id" => ("i64", ""),             // () -> type_id
        "arth_rt_exception_payload" => ("ptr", ""),             // () -> payload_ptr
        "arth_rt_exception_payload_size" => ("i64", ""),        // () -> size
        "arth_rt_type_id" => ("i64", "ptr, i64"),               // (type_name, len) -> type_id

        // Async runtime
        "arth_rt_async_frame_alloc" => ("ptr", "i32"), // (size) -> frame_ptr
        "arth_rt_async_frame_free" => ("void", "ptr, i32"), // (frame_ptr, size) -> void
        "arth_rt_async_frame_get_state" => ("i64", "ptr"), // (frame_ptr) -> state
        "arth_rt_async_frame_set_state" => ("void", "ptr, i32"), // (frame_ptr, state) -> void
        "arth_rt_async_frame_load" => ("i64", "ptr, i32"), // (frame_ptr, offset) -> value
        "arth_rt_async_frame_store" => ("void", "ptr, i32, i64"), // (frame_ptr, offset, value) -> void
        "arth_rt_task_spawn_with_poll" => ("i64", "ptr, i64"), // (frame_ptr, poll_fn_id) -> handle
        "arth_rt_task_is_cancelled" => ("i64", "i64"),         // (task_handle) -> bool
        "arth_rt_task_yield" => ("i64", ""),                   // () -> status
        "arth_rt_task_check_cancelled" => ("i64", ""),         // () -> bool
        "arth_rt_task_has_error" => ("i64", "i64"),            // (task_handle) -> bool
        "arth_rt_task_get_error" => ("i64", "i64"),            // (task_handle) -> exception
        "arth_rt_task_get_result" => ("i64", "i64"),           // (task_handle) -> result
        "arth_rt_create_cancelled_error" => ("i64", "i64"),    // (task_handle) -> exception
        "arth_rt_async_yield" => ("void", "i64"),              // (awaited_task) -> void
        "arth_rt_await" => ("i64", "i64"),                     // (task_handle) -> result

        // Concurrent executor runtime
        "arth_rt_executor_init" => ("i64", "i64"), // (thread_count) -> status
        "arth_rt_executor_thread_count" => ("i64", ""), // () -> thread_count
        "arth_rt_executor_active_workers" => ("i64", ""), // () -> active_workers
        "arth_rt_executor_spawn" => ("i64", "i64"), // (fn_id) -> task_handle
        "arth_rt_executor_cancel" => ("i64", "i64"), // (task_handle) -> status
        "arth_rt_executor_join" => ("i64", "i64"), // (task_handle) -> result
        "arth_rt_executor_spawn_with_arg" => ("i64", "i64, i64"), // (fn_id, arg) -> task_handle
        "arth_rt_executor_active_executor_count" => ("i64", ""), // () -> count
        "arth_rt_executor_worker_task_count" => ("i64", "i64"), // (worker_id) -> tasks_executed
        "arth_rt_executor_reset_stats" => ("i64", ""), // () -> status
        "arth_rt_executor_spawn_await" => ("i64", "i64, i64, i64"), // (sub_fn_id, sub_arg, accum) -> task_handle

        // Region-based allocation
        "arth_rt_region_enter" => ("void", "i32"), // (region_id) -> void
        "arth_rt_region_exit" => ("void", "i32"),  // (region_id) -> void
        "arth_rt_region_alloc" => ("ptr", "i64"),  // (size) -> ptr

        // Provider operations
        "arth_rt_provider_new" => ("i64", "ptr, i64, i64"), // (type_name, type_name_len, field_count) -> handle
        "arth_rt_provider_field_get" => ("i64", "i64, i64"), // (handle, field_idx) -> value
        "arth_rt_provider_field_get_named" => ("i64", "i64, ptr, i64"), // (handle, field_name, name_len) -> value
        "arth_rt_provider_field_set" => ("i64", "i64, i64, i64, ptr, i64"), // (handle, field_idx, value, field_name, name_len) -> handle
        "arth_rt_provider_field_set_named" => ("i64", "i64, ptr, i64, i64"), // (handle, field_name, name_len, value) -> handle
        "arth_rt_provider_shared_field_init" => ("i64", "i64, i64, i64, ptr, i64"), // (handle, field_idx, initial, field_name, name_len) -> cell_handle
        "arth_rt_provider_shared_get" => ("i64", "i64, i64"), // (handle, field_idx) -> value
        "arth_rt_provider_shared_set" => ("i64", "i64, i64, i64"), // (handle, field_idx, value) -> status
        // Provider cleanup/deinit
        "arth_rt_provider_free" => ("i32", "i64"), // (handle) -> status
        "arth_rt_provider_shared_field_free" => ("i32", "i64, i64"), // (handle, field_idx) -> status
        "arth_rt_provider_shared_field_free_named" => ("i32", "i64, ptr, i64"), // (handle, field_name, name_len) -> status

        // Shared memory cells
        "arth_rt_shared_new" => ("i64", ""),     // () -> handle
        "arth_rt_shared_load" => ("i64", "i64"), // (handle) -> value
        "arth_rt_shared_store" => ("i64", "i64, i64"), // (handle, value) -> status
        "arth_rt_shared_get_named" => ("i64", "ptr, i64"), // (name, name_len) -> handle
        "arth_rt_shared_free" => ("i32", "i64"), // (handle) -> status
        "arth_rt_shared_cas" => ("i64", "i64, i64, i64"), // (handle, expected, new_value) -> success
        "arth_rt_shared_add" => ("i64", "i64, i64"),      // (handle, delta) -> previous

        // Network operations
        "arth_rt_socket_create" => ("i64", "i32, i32, i32"), // (domain, type, protocol) -> handle
        "arth_rt_socket_close" => ("i32", "i64"),            // (sock) -> status
        "arth_rt_socket_connect" => ("i32", "i64, ptr, i32"), // (sock, addr, addr_len) -> status
        "arth_rt_socket_connect_host" => ("i32", "i64, ptr, i64, i32"), // (sock, host, host_len, port) -> status
        "arth_rt_socket_bind" => ("i32", "i64, ptr, i32"), // (sock, addr, addr_len) -> status
        "arth_rt_socket_bind_port" => ("i32", "i64, i32"), // (sock, port) -> status
        "arth_rt_socket_listen" => ("i32", "i64, i32"),    // (sock, backlog) -> status
        "arth_rt_socket_accept" => ("i64", "i64"),         // (sock) -> new_sock
        "arth_rt_socket_send" => ("i64", "i64, ptr, i64, i32"), // (sock, buf, len, flags) -> bytes_sent
        "arth_rt_socket_recv" => ("i64", "i64, ptr, i64, i32"), // (sock, buf, len, flags) -> bytes_recv
        "arth_rt_socket_setsockopt" => ("i32", "i64, i32, i32, ptr, i32"), // (sock, level, optname, optval, optlen) -> status
        "arth_rt_socket_setsockopt_int" => ("i32", "i64, i32, i32, i32"), // (sock, level, optname, optval) -> status
        "arth_rt_socket_set_nonblocking" => ("i32", "i64, i32"), // (sock, nonblocking) -> status
        "arth_rt_socket_fd" => ("i32", "i64"),                   // (sock) -> fd
        "arth_rt_getaddrinfo" => ("i64", "ptr, i64, i32"),       // (host, host_len, port) -> handle
        "arth_rt_addrinfo_next" => ("i32", "i64, ptr, ptr"), // (handle, addr, addr_len) -> status
        "arth_rt_freeaddrinfo" => ("i32", "i64"),            // (handle) -> status
        "arth_rt_addr_ipv4" => ("i32", "ptr, i32, ptr"),     // (ip, port, addr) -> status
        "arth_rt_addr_parse" => ("i32", "ptr, i64, i32, ptr, ptr"), // (ip_str, ip_len, port, addr, addr_len) -> status

        // Default signature for unknown functions
        _ => ("i64", "i64"),
    }
}

/// Collect all native runtime symbols used in a module.
fn collect_native_symbols(m: &Module) -> HashSet<&'static str> {
    let mut symbols = HashSet::new();
    for func in &m.funcs {
        for block in &func.blocks {
            for inst in &block.insts {
                if let InstKind::Call { name, .. } = &inst.kind {
                    if let Some(sym) = native_symbol_for_call(name) {
                        symbols.insert(sym);
                    }
                }
            }
        }
    }
    symbols
}

/// Resolve a non-runtime direct call to a module-defined function name.
/// Supports unqualified calls (e.g. `compute`) by matching a unique suffix
/// among defined functions (e.g. `Main.compute`).
fn resolve_local_callee_name(m: &Module, name: &str) -> Option<String> {
    if m.funcs.iter().any(|f| f.name == name) {
        return Some(name.to_string());
    }

    let suffix = format!(".{}", name);
    let mut matches = m.funcs.iter().filter(|f| f.name.ends_with(&suffix));
    let first = matches.next()?;
    if matches.next().is_none() {
        Some(first.name.clone())
    } else {
        None
    }
}

/// Collect unresolved direct-call targets and synthesize callable stubs for them.
///
/// Some lowered IR currently references helper/runtime functions that are not emitted
/// as module functions or extern declarations. Stubs keep native compilation moving
/// for conformance compile checks.
fn collect_unresolved_call_stubs(m: &Module) -> Vec<(String, String, Vec<String>)> {
    let defined_funcs: HashSet<String> = m.funcs.iter().map(|f| f.name.clone()).collect();
    let extern_funcs: HashSet<String> = m.extern_funcs.iter().map(|f| f.name.clone()).collect();
    let mut stubs: std::collections::BTreeMap<String, (String, Vec<String>)> =
        std::collections::BTreeMap::new();

    for func in &m.funcs {
        for block in &func.blocks {
            for inst in &block.insts {
                if let InstKind::Call { name, args, ret } = &inst.kind {
                    let resolved_local = resolve_local_callee_name(m, name);
                    let call_name_owned = if let Some(sym) = native_symbol_for_call(name) {
                        sym.to_string()
                    } else if let Some(local) = resolved_local {
                        local
                    } else {
                        name.clone()
                    };
                    let call_name = call_name_owned.as_str();
                    if call_name.starts_with("arth_rt_") || call_name == "__arth_log_emit_str" {
                        continue;
                    }
                    if defined_funcs.contains(call_name) || extern_funcs.contains(call_name) {
                        continue;
                    }

                    let (ret_ty, params) = match call_name {
                        "__arth_str_eq" => {
                            ("i1".to_string(), vec!["ptr".to_string(), "ptr".to_string()])
                        }
                        "__arth_str_concat" => (
                            "ptr".to_string(),
                            vec!["ptr".to_string(), "ptr".to_string()],
                        ),
                        _ => (ty_llvm(ret), vec!["i64".to_string(); args.len()]),
                    };
                    stubs
                        .entry(call_name.to_string())
                        .or_insert((ret_ty, params));
                }
            }
        }
    }

    stubs
        .into_iter()
        .map(|(name, (ret, params))| (name, ret, params))
        .collect()
}

/// Collect all provider names and field names used in a module.
/// Returns (provider_names, field_names) where field_names is (provider, field).
fn collect_provider_strings(m: &Module) -> (HashSet<String>, HashSet<(String, String)>) {
    let mut providers = HashSet::new();
    let mut fields = HashSet::new();
    for func in &m.funcs {
        for block in &func.blocks {
            for inst in &block.insts {
                match &inst.kind {
                    InstKind::ProviderNew { name, values } => {
                        providers.insert(name.clone());
                        for (field, _) in values {
                            fields.insert((name.clone(), field.clone()));
                        }
                    }
                    InstKind::ProviderFieldGet {
                        provider, field, ..
                    }
                    | InstKind::ProviderFieldSet {
                        provider, field, ..
                    } => {
                        fields.insert((provider.clone(), field.clone()));
                    }
                    _ => {}
                }
            }
        }
    }
    (providers, fields)
}

/// Collect all exception type names used in the module.
/// This includes types from StructAlloc instructions and __arth_struct_new calls.
/// Returns a set of exception type names for which we need to emit type info globals.
fn collect_exception_types(m: &Module) -> HashSet<String> {
    let mut types = HashSet::new();

    for func in &m.funcs {
        for block in &func.blocks {
            // Check for StructAlloc instructions
            for inst in &block.insts {
                if let InstKind::StructAlloc { type_name } = &inst.kind {
                    types.insert(type_name.clone());
                }
                // Check for __arth_struct_new calls - extract type name from first arg (string constant)
                if let InstKind::Call { name, args, .. } = &inst.kind {
                    if name == "__arth_struct_new" && !args.is_empty() {
                        // The first arg is a ConstStr value - trace it
                        if let Some(type_name) = trace_string_constant(func, args[0], m) {
                            types.insert(type_name);
                        }
                    }
                }
                // Collect exception types from LandingPad instructions
                // These are needed for DWARF exception handling type info globals
                if let InstKind::LandingPad { catch_types, .. } = &inst.kind {
                    for type_name in catch_types {
                        types.insert(type_name.clone());
                    }
                }
            }
        }
    }

    types
}

/// Trace a Value back to its ConstStr instruction and return the string.
fn trace_string_constant(
    func: &Func,
    val: crate::compiler::ir::Value,
    module: &Module,
) -> Option<String> {
    for block in &func.blocks {
        for inst in &block.insts {
            if inst.result == val {
                if let InstKind::ConstStr(idx) = &inst.kind {
                    // Get the actual string from the module's string pool
                    return module.strings.get(*idx as usize).cloned();
                }
            }
        }
    }
    None
}

/// Build a map from Value ID to struct type name for a function.
/// This traces values back to their struct creation to determine the type.
fn build_value_type_map(func: &Func, module: &Module) -> std::collections::HashMap<u32, String> {
    let mut type_map = std::collections::HashMap::new();

    for block in &func.blocks {
        for inst in &block.insts {
            match &inst.kind {
                InstKind::StructAlloc { type_name } => {
                    type_map.insert(inst.result.0, type_name.clone());
                }
                InstKind::Call { name, args, .. } if name == "__arth_struct_new" => {
                    // First arg is the type name as a ConstStr
                    if !args.is_empty() {
                        // Trace the first arg to find the string constant index
                        for b in &func.blocks {
                            for i in &b.insts {
                                if i.result == args[0] {
                                    if let InstKind::ConstStr(idx) = &i.kind {
                                        if let Some(s) = module.strings.get(*idx as usize) {
                                            type_map.insert(inst.result.0, s.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                InstKind::Copy(src) => {
                    // Propagate type through copy
                    if let Some(ty) = type_map.get(&src.0).cloned() {
                        type_map.insert(inst.result.0, ty);
                    }
                }
                InstKind::Load(ptr) => {
                    // Propagate type through load (for exceptions stored in slots)
                    if let Some(ty) = type_map.get(&ptr.0).cloned() {
                        type_map.insert(inst.result.0, ty);
                    }
                }
                InstKind::Phi(operands) => {
                    // Propagate type through phi - take type from first operand with known type
                    for (_, val) in operands {
                        if let Some(ty) = type_map.get(&val.0).cloned() {
                            type_map.insert(inst.result.0, ty);
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    type_map
}

/// Compute FNV-1a hash for a type name (matches arth-rt's hash_type_name)
fn fnv1a_hash(name: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Emit exception type info globals for DWARF exception handling.
/// Format:
///   @.str.typeinfo.TypeName = private unnamed_addr constant [N x i8] c"TypeName\00"
///   @_arth_typeinfo_TypeName = constant { i64, ptr } { i64 <hash>, ptr @.str.typeinfo.TypeName }
fn emit_exception_type_info(out: &mut String, types: &HashSet<String>) {
    use std::fmt::Write;

    if types.is_empty() {
        return;
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "; --- Exception type info globals ---");

    for type_name in types {
        let safe_name = type_name.replace('.', "_");
        let type_id = fnv1a_hash(type_name);

        // Emit string constant for type name (null-terminated)
        let bytes = type_name.as_bytes();
        let mut esc = String::new();
        for &b in bytes {
            match b {
                b'\\' => esc.push_str("\\5C"),
                b'"' => esc.push_str("\\22"),
                0x20..=0x7E => esc.push(b as char),
                _ => {
                    let _ = write!(esc, "\\{:02X}", b);
                }
            }
        }
        esc.push_str("\\00"); // null terminator

        let _ = writeln!(
            out,
            "@.str.typeinfo.{} = private unnamed_addr constant [{} x i8] c\"{}\", align 1",
            safe_name,
            bytes.len() + 1,
            esc
        );

        // Emit type info struct: { i64 type_id, ptr type_name }
        let _ = writeln!(
            out,
            "@_arth_typeinfo_{} = constant {{ i64, ptr }} {{ i64 {}, ptr @.str.typeinfo.{} }}",
            safe_name, type_id, safe_name
        );
    }
}

fn ty_llvm(t: &Ty) -> String {
    match t {
        Ty::I64 => "i64".to_string(),
        Ty::F64 => "double".to_string(),
        // LLVM backend currently treats boolean-like values as integer ABI values.
        Ty::I1 => "i64".to_string(),
        // Keep pointer-like values as integer handles at the IR function boundary.
        Ty::Ptr => "i64".to_string(),
        Ty::Void => "void".to_string(),
        Ty::Struct(_) => "i64".to_string(),
        Ty::Enum(_) => "i64".to_string(),
        Ty::Optional(_) => "i64".to_string(),
        Ty::String => "i64".to_string(),
    }
}

/// Convert an IR Ty to an ArthType for layout computation.
fn ir_ty_to_arth_type(ty: &Ty) -> ArthType {
    match ty {
        Ty::I64 => ArthType::Int,
        Ty::F64 => ArthType::Float,
        Ty::I1 => ArthType::Bool,
        Ty::Ptr => ArthType::Ptr,
        Ty::Void => ArthType::Void,
        Ty::Struct(name) => ArthType::Struct(name.clone()),
        Ty::Enum(name) => ArthType::Enum(name.clone()),
        Ty::Optional(inner) => ArthType::Optional(Box::new(ir_ty_to_arth_type(inner))),
        Ty::String => ArthType::String,
    }
}

/// Extract type dependencies from a Ty.
fn extract_type_deps(ty: &Ty, deps: &mut HashSet<String>) {
    match ty {
        Ty::Struct(name) => {
            deps.insert(name.clone());
        }
        Ty::Enum(name) => {
            deps.insert(name.clone());
        }
        Ty::Optional(inner) => {
            extract_type_deps(inner, deps);
        }
        _ => {}
    }
}

/// Build a TypeRegistry from the struct/enum definitions in an IR Module.
/// Handles type dependencies by registering types in topological order.
fn build_type_registry(m: &Module) -> TypeRegistry {
    let mut registry = TypeRegistry::new();

    // Build dependency graph for structs
    let mut struct_deps: std::collections::HashMap<String, HashSet<String>> =
        std::collections::HashMap::new();
    let mut struct_defs: std::collections::HashMap<String, &crate::compiler::ir::StructDef> =
        std::collections::HashMap::new();

    for ir_struct in &m.structs {
        let mut deps = HashSet::new();
        for field in &ir_struct.fields {
            extract_type_deps(&field.ty, &mut deps);
        }
        // Remove self-reference if any
        deps.remove(&ir_struct.name);
        struct_deps.insert(ir_struct.name.clone(), deps);
        struct_defs.insert(ir_struct.name.clone(), ir_struct);
    }

    // Build dependency graph for enums
    let mut enum_deps: std::collections::HashMap<String, HashSet<String>> =
        std::collections::HashMap::new();
    let mut enum_defs: std::collections::HashMap<String, &crate::compiler::ir::EnumDef> =
        std::collections::HashMap::new();

    for ir_enum in &m.enums {
        let mut deps = HashSet::new();
        for variant in &ir_enum.variants {
            for ty in &variant.payload_types {
                extract_type_deps(ty, &mut deps);
            }
        }
        // Remove self-reference if any
        deps.remove(&ir_enum.name);
        enum_deps.insert(ir_enum.name.clone(), deps);
        enum_defs.insert(ir_enum.name.clone(), ir_enum);
    }

    // Topological sort for structs
    let mut registered_structs: HashSet<String> = HashSet::new();
    let struct_order = topological_sort(&struct_deps);
    for name in struct_order {
        if let Some(ir_struct) = struct_defs.get(&name) {
            let llvm_struct = LlvmStructDef {
                name: ir_struct.name.clone(),
                fields: ir_struct
                    .fields
                    .iter()
                    .map(|f| FieldDef {
                        name: f.name.clone(),
                        ty: ir_ty_to_arth_type(&f.ty),
                    })
                    .collect(),
            };
            registry.register_struct(llvm_struct);
            registered_structs.insert(name);
        }
    }

    // Register any remaining structs (circular dependencies fall back to opaque pointers)
    for (name, ir_struct) in &struct_defs {
        if !registered_structs.contains(name) {
            let llvm_struct = LlvmStructDef {
                name: ir_struct.name.clone(),
                fields: ir_struct
                    .fields
                    .iter()
                    .map(|f| FieldDef {
                        name: f.name.clone(),
                        ty: ir_ty_to_arth_type(&f.ty),
                    })
                    .collect(),
            };
            registry.register_struct(llvm_struct);
        }
    }

    // Topological sort for enums
    let mut registered_enums: HashSet<String> = HashSet::new();
    let enum_order = topological_sort(&enum_deps);
    for name in enum_order {
        if let Some(ir_enum) = enum_defs.get(&name) {
            let llvm_enum = LlvmEnumDef {
                name: ir_enum.name.clone(),
                variants: ir_enum
                    .variants
                    .iter()
                    .map(|v| VariantDef {
                        name: v.name.clone(),
                        payload_types: v.payload_types.iter().map(ir_ty_to_arth_type).collect(),
                    })
                    .collect(),
            };
            registry.register_enum(llvm_enum);
            registered_enums.insert(name);
        }
    }

    // Register any remaining enums (circular dependencies)
    for (name, ir_enum) in &enum_defs {
        if !registered_enums.contains(name) {
            let llvm_enum = LlvmEnumDef {
                name: ir_enum.name.clone(),
                variants: ir_enum
                    .variants
                    .iter()
                    .map(|v| VariantDef {
                        name: v.name.clone(),
                        payload_types: v.payload_types.iter().map(ir_ty_to_arth_type).collect(),
                    })
                    .collect(),
            };
            registry.register_enum(llvm_enum);
        }
    }

    registry
}

/// Topological sort using Kahn's algorithm.
/// Returns types in dependency order (dependencies first).
fn topological_sort(deps: &std::collections::HashMap<String, HashSet<String>>) -> Vec<String> {
    let mut in_degree: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut reverse_deps: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    // Initialize in-degrees
    for (name, dep_set) in deps {
        in_degree.entry(name.clone()).or_insert(0);
        for dep in dep_set {
            // Only count dependencies that exist in the graph
            if deps.contains_key(dep) {
                *in_degree.entry(name.clone()).or_insert(0) += 1;
                reverse_deps
                    .entry(dep.clone())
                    .or_default()
                    .push(name.clone());
            }
        }
    }

    // Start with types that have no dependencies
    let mut queue: std::collections::VecDeque<String> = in_degree
        .iter()
        .filter(|&(_, deg)| *deg == 0)
        .map(|(name, _)| name.clone())
        .collect();

    let mut result = Vec::new();

    while let Some(name) = queue.pop_front() {
        result.push(name.clone());

        if let Some(dependents) = reverse_deps.get(&name) {
            for dependent in dependents {
                if let Some(deg) = in_degree.get_mut(dependent) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dependent.clone());
                    }
                }
            }
        }
    }

    result
}

/// Format a value ID as an LLVM SSA value name.
/// Parameters (IDs 0..num_params-1) are named %0, %1, etc.
/// Instruction results are named %v{id} to avoid clashing with LLVM's auto-numbering.
fn val(id: u32, num_params: usize) -> String {
    if (id as usize) < num_params {
        format!("%{}", id)
    } else {
        format!("%v{}", id)
    }
}

/// Build a mapping from original function names to mangled names.
/// `main` and lambda functions are never mangled.
fn build_mangle_map(funcs: &[Func]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for f in funcs {
        // Never mangle main or lambda functions
        if f.name == "main" || f.name.starts_with("lambda_") || f.name.contains(".lambda_") {
            continue;
        }
        // Split "Module.func" into module path and function name
        let (module_path, func_name) = if let Some(dot_pos) = f.name.rfind('.') {
            (&f.name[..dot_pos], &f.name[dot_pos + 1..])
        } else {
            ("", f.name.as_str())
        };
        let mangled = mangle_function(module_path, func_name, &f.params, &f.ret);
        map.insert(f.name.clone(), mangled);
    }
    map
}

/// Get the mangled name for a function, or the original if not in the map.
fn get_mangled_name<'a>(name: &'a str, mangle_map: &'a HashMap<String, String>) -> &'a str {
    mangle_map.get(name).map(|s| s.as_str()).unwrap_or(name)
}

fn emit_inst(
    buf: &mut String,
    inst: &Inst,
    f64_values: &std::collections::HashSet<u32>,
    blocks: &[BlockData],
    module: &Module,
    num_params: usize,
    type_registry: &TypeRegistry,
    mangle_map: &HashMap<String, String>,
) {
    match &inst.kind {
        InstKind::ConstI64(v) => {
            // Model as add with 0 to keep things simple in text demo.
            // Use 'v' prefix to avoid LLVM's sequential numbering requirement.
            let _ = writeln!(buf, "  %v{} = add i64 0, {}", inst.result.0, v);
        }
        InstKind::ConstF64(v) => {
            // Model as fadd with 0.0 to form a double constant in the demo.
            let _ = writeln!(buf, "  %v{} = fadd double 0.0, {}", inst.result.0, v);
        }
        InstKind::ConstStr(ix) => {
            // Materialize pointer to @.s<ix> with GEP [N x i8], 0, 0
            let s = module
                .strings
                .get(*ix as usize)
                .map(|x| x.as_str())
                .unwrap_or("");
            let n = s.len() + 1; // include NUL
            let _ = writeln!(
                buf,
                "  %v{}_ptr = getelementptr inbounds [{} x i8], ptr @.s{}, i64 0, i64 0",
                inst.result.0, n, ix
            );
            let _ = writeln!(
                buf,
                "  %v{} = ptrtoint ptr %v{}_ptr to i64",
                inst.result.0, inst.result.0
            );
        }
        InstKind::Copy(v) => {
            // Copy as add with 0 (no-op copy in SSA text demo).
            // Check if this is a float value
            if f64_values.contains(&v.0) {
                let _ = writeln!(
                    buf,
                    "  {} = fadd double {}, 0.0",
                    val(inst.result.0, num_params),
                    val(v.0, num_params)
                );
            } else {
                let _ = writeln!(
                    buf,
                    "  {} = add i64 {}, 0",
                    val(inst.result.0, num_params),
                    val(v.0, num_params)
                );
            }
        }
        InstKind::Binary(op, a, b) => {
            // Check if this is a floating-point operation
            let is_float = f64_values.contains(&a.0) || f64_values.contains(&b.0);

            if is_float {
                // Floating-point operations
                let op_str = match op {
                    BinOp::Add => "fadd",
                    BinOp::Sub => "fsub",
                    BinOp::Mul => "fmul",
                    BinOp::Div => "fdiv",
                    BinOp::Mod => "frem", // Floating-point remainder
                    // Bitwise operations don't apply to floats - emit as int ops after conversion
                    BinOp::Shl | BinOp::Shr | BinOp::And | BinOp::Or | BinOp::Xor => {
                        // These operations require integers - emit a warning or convert
                        // For now, we'll treat this as an error case and emit int ops
                        "fadd" // Fallback - shouldn't happen with well-typed code
                    }
                };
                let _ = writeln!(
                    buf,
                    "  {} = {} double {}, {}",
                    val(inst.result.0, num_params),
                    op_str,
                    val(a.0, num_params),
                    val(b.0, num_params)
                );
            } else {
                // Integer operations
                let op_str = match op {
                    BinOp::Add => "add",
                    BinOp::Sub => "sub",
                    BinOp::Mul => "mul",
                    BinOp::Div => "sdiv",
                    BinOp::Mod => "srem",
                    BinOp::Shl => "shl",
                    BinOp::Shr => "ashr",
                    BinOp::And => "and",
                    BinOp::Or => "or",
                    BinOp::Xor => "xor",
                };
                let _ = writeln!(
                    buf,
                    "  {} = {} i64 {}, {}",
                    val(inst.result.0, num_params),
                    op_str,
                    val(a.0, num_params),
                    val(b.0, num_params)
                );
            }
        }
        InstKind::Cmp(pred, a, b) => {
            // Check if this is a floating-point comparison
            let is_float = f64_values.contains(&a.0) || f64_values.contains(&b.0);

            if is_float {
                // Floating-point comparison with ordered predicates
                // 'o' prefix means ordered - comparison is false if either operand is NaN
                let pred_str = match pred {
                    CmpPred::Eq => "oeq",
                    CmpPred::Ne => "one",
                    CmpPred::Lt => "olt",
                    CmpPred::Le => "ole",
                    CmpPred::Gt => "ogt",
                    CmpPred::Ge => "oge",
                };
                let _ = writeln!(
                    buf,
                    "  %cmp_bool_{} = fcmp {} double {}, {}",
                    inst.result.0,
                    pred_str,
                    val(a.0, num_params),
                    val(b.0, num_params)
                );
                let _ = writeln!(
                    buf,
                    "  {} = zext i1 %cmp_bool_{} to i64",
                    val(inst.result.0, num_params),
                    inst.result.0
                );
            } else {
                // Integer comparison
                let pred_str = match pred {
                    CmpPred::Eq => "eq",
                    CmpPred::Ne => "ne",
                    CmpPred::Lt => "slt",
                    CmpPred::Le => "sle",
                    CmpPred::Gt => "sgt",
                    CmpPred::Ge => "sge",
                };
                let _ = writeln!(
                    buf,
                    "  %cmp_bool_{} = icmp {} i64 {}, {}",
                    inst.result.0,
                    pred_str,
                    val(a.0, num_params),
                    val(b.0, num_params)
                );
                let _ = writeln!(
                    buf,
                    "  {} = zext i1 %cmp_bool_{} to i64",
                    val(inst.result.0, num_params),
                    inst.result.0
                );
            }
        }
        InstKind::StrEq(a, b) => {
            // Call runtime string equality function: __arth_str_eq(ptr, ptr) -> i1
            let lhs_ptr = format!("%str_eq_lhs_{}", inst.result.0);
            let rhs_ptr = format!("%str_eq_rhs_{}", inst.result.0);
            let _ = writeln!(
                buf,
                "  {} = inttoptr i64 {} to ptr",
                lhs_ptr,
                val(a.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  {} = inttoptr i64 {} to ptr",
                rhs_ptr,
                val(b.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  %str_eq_bool_{} = call i1 @__arth_str_eq(ptr {}, ptr {})",
                inst.result.0, lhs_ptr, rhs_ptr
            );
            let _ = writeln!(
                buf,
                "  {} = zext i1 %str_eq_bool_{} to i64",
                val(inst.result.0, num_params),
                inst.result.0
            );
        }
        InstKind::StrConcat(a, b) => {
            // Call runtime string concatenation function: __arth_str_concat(ptr, ptr) -> ptr
            let lhs_ptr = format!("%str_cat_lhs_{}", inst.result.0);
            let rhs_ptr = format!("%str_cat_rhs_{}", inst.result.0);
            let _ = writeln!(
                buf,
                "  {} = inttoptr i64 {} to ptr",
                lhs_ptr,
                val(a.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  {} = inttoptr i64 {} to ptr",
                rhs_ptr,
                val(b.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  %str_cat_ptr_{} = call ptr @__arth_str_concat(ptr {}, ptr {})",
                inst.result.0, lhs_ptr, rhs_ptr
            );
            let _ = writeln!(
                buf,
                "  {} = ptrtoint ptr %str_cat_ptr_{} to i64",
                val(inst.result.0, num_params),
                inst.result.0
            );
        }
        InstKind::Alloca => {
            let _ = writeln!(buf, "  %alloca_ptr_{} = alloca i64, align 8", inst.result.0);
            let _ = writeln!(
                buf,
                "  {} = ptrtoint ptr %alloca_ptr_{} to i64",
                val(inst.result.0, num_params),
                inst.result.0
            );
        }
        InstKind::Load(p) => {
            let _ = writeln!(
                buf,
                "  %load_ptr_{} = inttoptr i64 {} to ptr",
                inst.result.0,
                val(p.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  {} = load i64, ptr %load_ptr_{}",
                val(inst.result.0, num_params),
                inst.result.0
            );
        }
        InstKind::Store(p, v) => {
            let _ = writeln!(
                buf,
                "  %store_ptr_{} = inttoptr i64 {} to ptr",
                inst.result.0,
                val(p.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  store i64 {}, ptr %store_ptr_{}",
                val(v.0, num_params),
                inst.result.0
            );
        }

        // Native struct operations with GEP-based field access
        InstKind::StructAlloc { type_name } => {
            // Allocate struct on the stack
            if let Some(layout) = type_registry.get_struct(type_name) {
                let _ = writeln!(
                    buf,
                    "  %struct_alloc_ptr_{} = alloca {}, align {}",
                    inst.result.0, layout.llvm_type_name, layout.alignment
                );
                let _ = writeln!(
                    buf,
                    "  {} = ptrtoint ptr %struct_alloc_ptr_{} to i64",
                    val(inst.result.0, num_params),
                    inst.result.0
                );
            } else {
                // Fallback: allocate as opaque bytes (shouldn't happen if types are registered)
                let _ = writeln!(
                    buf,
                    "  %struct_alloc_ptr_{} = alloca i64, align 8 ; unknown struct: {}",
                    inst.result.0, type_name
                );
                let _ = writeln!(
                    buf,
                    "  {} = ptrtoint ptr %struct_alloc_ptr_{} to i64",
                    val(inst.result.0, num_params),
                    inst.result.0
                );
            }
        }
        InstKind::StructFieldGet {
            ptr,
            type_name,
            field_name,
            field_index,
        } => {
            let ptr_reg = format!("%struct_ptr_{}", inst.result.0);
            let result_reg = val(inst.result.0, num_params);
            let _ = writeln!(
                buf,
                "  {} = inttoptr i64 {} to ptr",
                ptr_reg,
                val(ptr.0, num_params)
            );

            if let Some(layout) = type_registry.get_struct(type_name) {
                // Use the layout to get field info
                if let Some(field) = layout.fields.get(*field_index as usize) {
                    // Emit GEP to get field pointer
                    let gep_reg = format!("%gep_{}", inst.result.0);
                    let _ = writeln!(
                        buf,
                        "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 {}",
                        gep_reg, layout.llvm_type_name, ptr_reg, field_index
                    );
                    if field.llvm_ty == "i64" {
                        let _ = writeln!(buf, "  {} = load i64, ptr {}", result_reg, gep_reg);
                    } else if field.llvm_ty == "ptr" {
                        let tmp = format!("%field_ptr_{}", inst.result.0);
                        let _ = writeln!(buf, "  {} = load ptr, ptr {}", tmp, gep_reg);
                        let _ = writeln!(buf, "  {} = ptrtoint ptr {} to i64", result_reg, tmp);
                    } else if field.llvm_ty == "i32" {
                        let tmp = format!("%field_i32_{}", inst.result.0);
                        let _ = writeln!(buf, "  {} = load i32, ptr {}", tmp, gep_reg);
                        let _ = writeln!(buf, "  {} = zext i32 {} to i64", result_reg, tmp);
                    } else if field.llvm_ty == "i1" {
                        let tmp = format!("%field_i1_{}", inst.result.0);
                        let _ = writeln!(buf, "  {} = load i1, ptr {}", tmp, gep_reg);
                        let _ = writeln!(buf, "  {} = zext i1 {} to i64", result_reg, tmp);
                    } else if field.llvm_ty == "double" {
                        let tmp = format!("%field_f64_{}", inst.result.0);
                        let _ = writeln!(buf, "  {} = load double, ptr {}", tmp, gep_reg);
                        let _ = writeln!(buf, "  {} = bitcast double {} to i64", result_reg, tmp);
                    } else {
                        // Aggregate/unhandled types are lowered to a neutral i64 placeholder.
                        let _ = writeln!(
                            buf,
                            "  ; unsupported struct field load type {}, defaulting to 0",
                            field.llvm_ty
                        );
                        let _ = writeln!(buf, "  {} = add i64 0, 0", result_reg);
                    }
                } else {
                    // Field not found - emit error comment and dummy load
                    let _ = writeln!(
                        buf,
                        "  ; ERROR: field {} not found in struct {}",
                        field_name, type_name
                    );
                    let _ = writeln!(buf, "  {} = add i64 0, 0", result_reg);
                }
            } else {
                // Fallback: emit runtime call (shouldn't happen if types are registered)
                let _ = writeln!(
                    buf,
                    "  ; WARNING: struct {} not in type registry, using runtime call",
                    type_name
                );
                let _ = writeln!(
                    buf,
                    "  {} = call i64 @arth_rt_struct_get(i64 {}, i64 {})",
                    result_reg,
                    val(ptr.0, num_params),
                    field_index
                );
            }
        }
        InstKind::StructFieldSet {
            ptr,
            type_name,
            field_name,
            field_index,
            value,
        } => {
            let ptr_reg = format!("%struct_set_ptr_{}", inst.result.0);
            let value_reg = val(value.0, num_params);
            let _ = writeln!(
                buf,
                "  {} = inttoptr i64 {} to ptr",
                ptr_reg,
                val(ptr.0, num_params)
            );

            if let Some(layout) = type_registry.get_struct(type_name) {
                if let Some(field) = layout.fields.get(*field_index as usize) {
                    // Emit GEP to get field pointer
                    let gep_reg = format!("%gep_set_{}", inst.result.0);
                    let _ = writeln!(
                        buf,
                        "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 {}",
                        gep_reg, layout.llvm_type_name, ptr_reg, field_index
                    );
                    if field.llvm_ty == "i64" {
                        let _ = writeln!(buf, "  store i64 {}, ptr {}", value_reg, gep_reg);
                    } else if field.llvm_ty == "ptr" {
                        let cast = format!("%field_set_ptr_{}", inst.result.0);
                        let _ = writeln!(buf, "  {} = inttoptr i64 {} to ptr", cast, value_reg);
                        let _ = writeln!(buf, "  store ptr {}, ptr {}", cast, gep_reg);
                    } else if field.llvm_ty == "i32" {
                        let cast = format!("%field_set_i32_{}", inst.result.0);
                        let _ = writeln!(buf, "  {} = trunc i64 {} to i32", cast, value_reg);
                        let _ = writeln!(buf, "  store i32 {}, ptr {}", cast, gep_reg);
                    } else if field.llvm_ty == "i1" {
                        let cast = format!("%field_set_i1_{}", inst.result.0);
                        let _ = writeln!(buf, "  {} = trunc i64 {} to i1", cast, value_reg);
                        let _ = writeln!(buf, "  store i1 {}, ptr {}", cast, gep_reg);
                    } else if field.llvm_ty == "double" {
                        let cast = format!("%field_set_f64_{}", inst.result.0);
                        let _ = writeln!(buf, "  {} = bitcast i64 {} to double", cast, value_reg);
                        let _ = writeln!(buf, "  store double {}, ptr {}", cast, gep_reg);
                    } else {
                        let _ = writeln!(
                            buf,
                            "  ; unsupported struct field store type {}, writing zero",
                            field.llvm_ty
                        );
                        let _ = writeln!(
                            buf,
                            "  store {} zeroinitializer, ptr {}",
                            field.llvm_ty, gep_reg
                        );
                    }
                    // Result is the struct pointer (for chaining)
                    let _ = writeln!(
                        buf,
                        "  {} = add i64 0, 0 ; StructFieldSet result placeholder",
                        val(inst.result.0, num_params)
                    );
                } else {
                    let _ = writeln!(
                        buf,
                        "  ; ERROR: field {} not found in struct {}",
                        field_name, type_name
                    );
                    let _ = writeln!(buf, "  {} = add i64 0, 0", val(inst.result.0, num_params));
                }
            } else {
                let _ = writeln!(
                    buf,
                    "  ; WARNING: struct {} not in type registry",
                    type_name
                );
                let _ = writeln!(buf, "  {} = add i64 0, 0", val(inst.result.0, num_params));
            }
        }

        // Native enum operations
        InstKind::EnumAlloc { type_name } => {
            if let Some(layout) = type_registry.get_enum(type_name) {
                let _ = writeln!(
                    buf,
                    "  %enum_alloc_ptr_{} = alloca {}, align {}",
                    inst.result.0, layout.llvm_type_name, layout.alignment
                );
                let _ = writeln!(
                    buf,
                    "  {} = ptrtoint ptr %enum_alloc_ptr_{} to i64",
                    val(inst.result.0, num_params),
                    inst.result.0
                );
            } else {
                let _ = writeln!(
                    buf,
                    "  %enum_alloc_ptr_{} = alloca i64, align 8 ; unknown enum: {}",
                    inst.result.0, type_name
                );
                let _ = writeln!(
                    buf,
                    "  {} = ptrtoint ptr %enum_alloc_ptr_{} to i64",
                    val(inst.result.0, num_params),
                    inst.result.0
                );
            }
        }
        InstKind::EnumGetTag { ptr, type_name } => {
            let ptr_reg = format!("%enum_ptr_{}", inst.result.0);
            let result_reg = val(inst.result.0, num_params);
            let _ = writeln!(
                buf,
                "  {} = inttoptr i64 {} to ptr",
                ptr_reg,
                val(ptr.0, num_params)
            );

            if let Some(layout) = type_registry.get_enum(type_name) {
                // GEP to tag field (index 0)
                let tag_ptr_reg = format!("%tag_ptr_{}", inst.result.0);
                let _ = writeln!(
                    buf,
                    "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 0",
                    tag_ptr_reg, layout.llvm_type_name, ptr_reg
                );
                let _ = writeln!(
                    buf,
                    "  %tag_i32_{} = load i32, ptr {}",
                    inst.result.0, tag_ptr_reg
                );
                let _ = writeln!(
                    buf,
                    "  {} = zext i32 %tag_i32_{} to i64",
                    result_reg, inst.result.0
                );
            } else {
                let _ = writeln!(buf, "  {} = add i64 0, 0 ; unknown enum tag", result_reg);
            }
        }
        InstKind::EnumSetTag {
            ptr,
            type_name,
            tag,
        } => {
            let ptr_reg = format!("%enum_set_ptr_{}", inst.result.0);
            let _ = writeln!(
                buf,
                "  {} = inttoptr i64 {} to ptr",
                ptr_reg,
                val(ptr.0, num_params)
            );

            if let Some(layout) = type_registry.get_enum(type_name) {
                let tag_ptr_reg = format!("%tag_ptr_set_{}", inst.result.0);
                let _ = writeln!(
                    buf,
                    "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 0",
                    tag_ptr_reg, layout.llvm_type_name, ptr_reg
                );
                let _ = writeln!(buf, "  store i32 {}, ptr {}", tag, tag_ptr_reg);
            }
            let _ = writeln!(
                buf,
                "  {} = add i64 0, 0 ; EnumSetTag placeholder",
                val(inst.result.0, num_params)
            );
        }
        InstKind::EnumGetPayload {
            ptr,
            type_name,
            payload_index,
        } => {
            let ptr_reg = format!("%enum_payload_ptr_{}", inst.result.0);
            let result_reg = val(inst.result.0, num_params);
            let _ = writeln!(
                buf,
                "  {} = inttoptr i64 {} to ptr",
                ptr_reg,
                val(ptr.0, num_params)
            );

            if let Some(layout) = type_registry.get_enum(type_name) {
                if layout.max_payload_size > 0 {
                    // GEP to payload area (index 1), then offset by payload_index * 8
                    let payload_ptr_reg = format!("%payload_ptr_{}", inst.result.0);
                    let _ = writeln!(
                        buf,
                        "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 1",
                        payload_ptr_reg, layout.llvm_type_name, ptr_reg
                    );
                    // Calculate byte offset for this payload element
                    let byte_offset = payload_index * 8; // Assume 8-byte elements
                    let elem_ptr_reg = format!("%elem_ptr_{}", inst.result.0);
                    let _ = writeln!(
                        buf,
                        "  {} = getelementptr inbounds i8, ptr {}, i32 {}",
                        elem_ptr_reg, payload_ptr_reg, byte_offset
                    );
                    let _ = writeln!(buf, "  {} = load i64, ptr {}", result_reg, elem_ptr_reg);
                } else {
                    let _ = writeln!(buf, "  {} = add i64 0, 0 ; no payload", result_reg);
                }
            } else {
                let _ = writeln!(buf, "  {} = add i64 0, 0 ; unknown enum", result_reg);
            }
        }
        InstKind::EnumSetPayload {
            ptr,
            type_name,
            payload_index,
            value,
        } => {
            let ptr_reg = format!("%enum_set_payload_ptr_{}", inst.result.0);
            let value_reg = val(value.0, num_params);
            let _ = writeln!(
                buf,
                "  {} = inttoptr i64 {} to ptr",
                ptr_reg,
                val(ptr.0, num_params)
            );

            if let Some(layout) = type_registry.get_enum(type_name) {
                if layout.max_payload_size > 0 {
                    let payload_ptr_reg = format!("%payload_set_ptr_{}", inst.result.0);
                    let _ = writeln!(
                        buf,
                        "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 1",
                        payload_ptr_reg, layout.llvm_type_name, ptr_reg
                    );
                    let byte_offset = payload_index * 8;
                    let elem_ptr_reg = format!("%elem_set_ptr_{}", inst.result.0);
                    let _ = writeln!(
                        buf,
                        "  {} = getelementptr inbounds i8, ptr {}, i32 {}",
                        elem_ptr_reg, payload_ptr_reg, byte_offset
                    );
                    let _ = writeln!(buf, "  store i64 {}, ptr {}", value_reg, elem_ptr_reg);
                }
            }
            let _ = writeln!(
                buf,
                "  {} = add i64 0, 0 ; EnumSetPayload placeholder",
                val(inst.result.0, num_params)
            );
        }

        InstKind::Call { name, args, ret } => {
            // Check if this call has a native symbol for native compilation
            let resolved_local = resolve_local_callee_name(module, name);
            let call_name_owned = if let Some(sym) = native_symbol_for_call(name) {
                sym.to_string()
            } else if let Some(local) = resolved_local {
                local
            } else {
                name.clone()
            };
            // Apply mangling for user-defined functions (not runtime/intrinsics)
            let mangled_call_name;
            let call_name = if native_symbol_for_call(name).is_some()
                || call_name_owned.starts_with("arth_rt_")
                || call_name_owned.starts_with("__arth_")
            {
                call_name_owned.as_str()
            } else {
                mangled_call_name = get_mangled_name(&call_name_owned, mangle_map).to_string();
                mangled_call_name.as_str()
            };

            let mut args_s = String::new();
            if name == "__arth_log_emit_str" {
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        args_s.push_str(", ");
                    }
                    if i == 0 {
                        let _ = write!(args_s, "i64 {}", val(a.0, num_params));
                    } else {
                        let cast_reg = format!("%arg_log_ptr_{}_{}", inst.result.0, i);
                        let _ = writeln!(
                            buf,
                            "  {} = inttoptr i64 {} to ptr",
                            cast_reg,
                            val(a.0, num_params)
                        );
                        let _ = write!(args_s, "ptr {}", cast_reg);
                    }
                }
            } else if call_name.starts_with("arth_rt_") {
                // Native runtime calls use C FFI signatures
                let trace_const_string_len = |value: &crate::compiler::ir::Value| -> Option<usize> {
                    let mut cur = *value;
                    for _ in 0..8 {
                        let mut stepped = false;
                        for b in blocks {
                            for i in &b.insts {
                                if i.result == cur {
                                    match &i.kind {
                                        InstKind::ConstStr(ix) => {
                                            return module
                                                .strings
                                                .get(*ix as usize)
                                                .map(|s| s.len());
                                        }
                                        InstKind::Copy(src) => {
                                            cur = *src;
                                            stepped = true;
                                        }
                                        _ => return None,
                                    }
                                }
                            }
                        }
                        if !stepped {
                            break;
                        }
                    }
                    None
                };

                // Compatibility shims for VM-style struct intrinsic argument shapes.
                if name == "__arth_struct_new" && args.len() == 2 {
                    let ptr_reg = format!("%arg_ptr_{}_0", inst.result.0);
                    let _ = writeln!(
                        buf,
                        "  {} = inttoptr i64 {} to ptr",
                        ptr_reg,
                        val(args[0].0, num_params)
                    );
                    let type_len = trace_const_string_len(&args[0]).unwrap_or(0);
                    let _ = write!(
                        args_s,
                        "ptr {}, i64 {}, i64 {}",
                        ptr_reg,
                        type_len,
                        val(args[1].0, num_params)
                    );
                } else if name == "__arth_struct_set" && args.len() == 4 {
                    let ptr_reg = format!("%arg_ptr_{}_3", inst.result.0);
                    let _ = writeln!(
                        buf,
                        "  {} = inttoptr i64 {} to ptr",
                        ptr_reg,
                        val(args[3].0, num_params)
                    );
                    let field_len = trace_const_string_len(&args[3]).unwrap_or(0);
                    let _ = write!(
                        args_s,
                        "i64 {}, i64 {}, i64 {}, ptr {}, i64 {}",
                        val(args[0].0, num_params),
                        val(args[1].0, num_params),
                        val(args[2].0, num_params),
                        ptr_reg,
                        field_len
                    );
                } else if name == "__arth_struct_set_named" && args.len() == 3 {
                    let ptr_reg = format!("%arg_ptr_{}_1", inst.result.0);
                    let _ = writeln!(
                        buf,
                        "  {} = inttoptr i64 {} to ptr",
                        ptr_reg,
                        val(args[1].0, num_params)
                    );
                    let field_len = trace_const_string_len(&args[1]).unwrap_or(0);
                    let _ = write!(
                        args_s,
                        "i64 {}, ptr {}, i64 {}, i64 {}",
                        val(args[0].0, num_params),
                        ptr_reg,
                        field_len,
                        val(args[2].0, num_params)
                    );
                } else if name == "__arth_struct_get_named" && args.len() == 2 {
                    let ptr_reg = format!("%arg_ptr_{}_1", inst.result.0);
                    let _ = writeln!(
                        buf,
                        "  {} = inttoptr i64 {} to ptr",
                        ptr_reg,
                        val(args[1].0, num_params)
                    );
                    let field_len = trace_const_string_len(&args[1]).unwrap_or(0);
                    let _ = write!(
                        args_s,
                        "i64 {}, ptr {}, i64 {}",
                        val(args[0].0, num_params),
                        ptr_reg,
                        field_len
                    );
                } else {
                    // Get the signature to determine argument types
                    let (_, params_sig) = native_symbol_signature(call_name);
                    let param_types: Vec<&str> = if params_sig.is_empty() {
                        vec![]
                    } else {
                        params_sig.split(", ").collect()
                    };

                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            args_s.push_str(", ");
                        }
                        // Use the type from the signature if available, otherwise default to i64
                        let ty = param_types.get(i).copied().unwrap_or("i64");
                        if ty == "ptr" {
                            let cast_reg = format!("%arg_ptr_{}_{}", inst.result.0, i);
                            let _ = writeln!(
                                buf,
                                "  {} = inttoptr i64 {} to ptr",
                                cast_reg,
                                val(a.0, num_params)
                            );
                            let _ = write!(args_s, "ptr {}", cast_reg);
                        } else {
                            let _ = write!(args_s, "{} {}", ty, val(a.0, num_params));
                        }
                    }
                }
            } else {
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        args_s.push_str(", ");
                    }
                    // Assume i64 args for demo
                    let _ = write!(args_s, "i64 {}", val(a.0, num_params));
                }
            }
            if call_name.starts_with("arth_rt_") {
                let (sig_ret, _) = native_symbol_signature(call_name);
                if sig_ret == "void" {
                    let _ = writeln!(buf, "  call void @{}({})", call_name, args_s);
                    let _ = writeln!(
                        buf,
                        "  {} = add i64 0, 0  ; void runtime call result placeholder",
                        val(inst.result.0, num_params)
                    );
                } else if sig_ret == "ptr" {
                    let ptr_tmp = format!("%call_ptr_{}", inst.result.0);
                    let _ = writeln!(buf, "  {} = call ptr @{}({})", ptr_tmp, call_name, args_s);
                    let _ = writeln!(
                        buf,
                        "  {} = ptrtoint ptr {} to i64",
                        val(inst.result.0, num_params),
                        ptr_tmp
                    );
                } else if sig_ret == "i32" {
                    let i32_tmp = format!("%call_i32_{}", inst.result.0);
                    let _ = writeln!(buf, "  {} = call i32 @{}({})", i32_tmp, call_name, args_s);
                    let _ = writeln!(
                        buf,
                        "  {} = zext i32 {} to i64",
                        val(inst.result.0, num_params),
                        i32_tmp
                    );
                } else {
                    let _ = writeln!(
                        buf,
                        "  {} = call {} @{}({})",
                        val(inst.result.0, num_params),
                        sig_ret,
                        call_name,
                        args_s
                    );
                }
            } else if *ret == Ty::Void {
                let _ = writeln!(buf, "  call void @{}({})", call_name, args_s);
                let _ = writeln!(
                    buf,
                    "  {} = add i64 0, 0  ; void call result placeholder",
                    val(inst.result.0, num_params)
                );
            } else {
                let ret_s = ty_llvm(ret);
                let _ = writeln!(
                    buf,
                    "  {} = call {} @{}({})",
                    val(inst.result.0, num_params),
                    ret_s,
                    call_name,
                    args_s
                );
            }
        }
        // FFI call to external function - uses C calling convention
        InstKind::ExternCall {
            name,
            args,
            params,
            ret,
        } => {
            let mut args_s = String::new();
            for (i, (a, pty)) in args.iter().zip(params.iter()).enumerate() {
                if i > 0 {
                    args_s.push_str(", ");
                }
                let _ = write!(args_s, "{} {}", ty_llvm(pty), val(a.0, num_params));
            }
            let ret_s = ty_llvm(ret);
            if *ret == Ty::Void {
                let _ = writeln!(buf, "  call {} @{}({})", ret_s, name, args_s);
                // Emit placeholder result for void calls
                let _ = writeln!(
                    buf,
                    "  {} = add i64 0, 0  ; void extern result",
                    val(inst.result.0, num_params)
                );
            } else {
                let _ = writeln!(
                    buf,
                    "  {} = call {} @{}({})",
                    val(inst.result.0, num_params),
                    ret_s,
                    name,
                    args_s
                );
            }
        }
        InstKind::LandingPad {
            catch_types,
            is_catch_all,
        } => {
            // Emit proper LLVM landing pad for exception handling with typed catches.
            // The landingpad instruction returns { ptr, i32 } where:
            // - ptr is the exception object pointer
            // - i32 is the type selector (1-indexed into catch_types, 0 for catch-all)
            //
            // For each exception type, we emit: catch ptr @_arth_typeinfo_TypeName
            // The personality function __arth_personality_v0 will match the exception's
            // type_id against the type info and return the appropriate selector.
            let _ = writeln!(buf, "  %lpad{} = landingpad {{ ptr, i32 }}", inst.result.0);

            // Emit typed catch clauses (selector values are 1-indexed)
            for type_name in catch_types {
                let safe_name = type_name.replace('.', "_");
                let _ = writeln!(buf, "           catch ptr @_arth_typeinfo_{}", safe_name);
            }

            // Add catch-all if needed (selector will be 0 for catch-all)
            if *is_catch_all || catch_types.is_empty() {
                let _ = writeln!(buf, "           catch ptr null");
            }

            // Extract the exception pointer from the landing pad result
            let _ = writeln!(
                buf,
                "  %exn{} = extractvalue {{ ptr, i32 }} %lpad{}, 0",
                inst.result.0, inst.result.0
            );

            // Extract the type selector for catch clause dispatch
            let _ = writeln!(
                buf,
                "  %sel{} = extractvalue {{ ptr, i32 }} %lpad{}, 1",
                inst.result.0, inst.result.0
            );

            // Call arth_rt_begin_catch to register the exception as being handled.
            // This returns a pointer to the ArthException structure which contains
            // the type_id, type_name, and payload.
            let _ = writeln!(
                buf,
                "  %arth_ex{} = call ptr @arth_rt_begin_catch(ptr %exn{})",
                inst.result.0, inst.result.0
            );

            // Extract payload pointer from ArthException (offset 56 on 64-bit layout):
            // [UnwindException 32][type_id 8][type_name ptr 8][type_name_len 8][payload ptr 8]
            let _ = writeln!(
                buf,
                "  %payload_addr{} = getelementptr inbounds i8, ptr %arth_ex{}, i64 56",
                inst.result.0, inst.result.0
            );
            let _ = writeln!(
                buf,
                "  %payload_ptr{} = load ptr, ptr %payload_addr{}",
                inst.result.0, inst.result.0
            );

            // Convert payload ptr to i64 for downstream catch variable binding.
            let _ = writeln!(
                buf,
                "  {} = ptrtoint ptr %payload_ptr{} to i64",
                val(inst.result.0, num_params),
                inst.result.0
            );
        }
        InstKind::SetUnwindHandler(_) | InstKind::ClearUnwindHandler => {
            // No-op for LLVM - exception handling uses invoke/landingpad
            // SetUnwindHandler/ClearUnwindHandler are VM-specific
        }
        InstKind::Phi(ops) => {
            // Prototype: assume i64 phi type for now.
            let block_name = |idx: u32| -> &str {
                blocks
                    .get(idx as usize)
                    .map(|b| b.name.as_str())
                    .unwrap_or("unknown")
            };
            let mut parts = String::new();
            for (i, (bb, v)) in ops.iter().enumerate() {
                if i > 0 {
                    parts.push_str(", ");
                }
                let _ = write!(parts, "[ {}, %{} ]", val(v.0, num_params), block_name(bb.0));
            }
            let _ = writeln!(
                buf,
                "  {} = phi i64 {}",
                val(inst.result.0, num_params),
                parts
            );
        }
        InstKind::MakeClosure { func, captures } => {
            // Create a closure using runtime functions
            // 1. Call arth_rt_closure_new(fn_ptr, num_captures) -> closure_handle
            // 2. For each capture, call arth_rt_closure_capture(closure_handle, value)
            let num_captures = captures.len();

            // Get function pointer (apply mangling for the referenced function)
            let closure_func_name = get_mangled_name(func, mangle_map);
            let fn_ptr_tmp = format!("{}.fn_ptr", val(inst.result.0, num_params));
            let _ = writeln!(
                buf,
                "  {} = bitcast ptr @{} to ptr",
                fn_ptr_tmp, closure_func_name
            );

            // Create closure
            let _ = writeln!(
                buf,
                "  {} = call i64 @arth_rt_closure_new(ptr {}, i64 {})",
                val(inst.result.0, num_params),
                fn_ptr_tmp,
                num_captures
            );

            // Add captured values to the closure
            for cap_val in captures {
                let _ = writeln!(
                    buf,
                    "  call void @arth_rt_closure_capture(i64 {}, i64 {})",
                    val(inst.result.0, num_params),
                    val(cap_val.0, num_params)
                );
            }
        }
        InstKind::ClosureCall { closure, args, .. } => {
            // Call a closure using the appropriate arth_rt_closure_call_N function
            // The runtime function loads captures from the closure's environment
            // and calls the function with (captures..., args...)
            let num_args = args.len();
            if num_args <= 8 {
                let call_fn = match num_args {
                    0 => "arth_rt_closure_call_0",
                    1 => "arth_rt_closure_call_1",
                    2 => "arth_rt_closure_call_2",
                    3 => "arth_rt_closure_call_3",
                    4 => "arth_rt_closure_call_4",
                    5 => "arth_rt_closure_call_5",
                    6 => "arth_rt_closure_call_6",
                    7 => "arth_rt_closure_call_7",
                    _ => "arth_rt_closure_call_8",
                };

                let args_with_types = if num_args == 0 {
                    format!("i64 {}", val(closure.0, num_params))
                } else {
                    let typed_args: Vec<String> = args
                        .iter()
                        .map(|v| format!("i64 {}", val(v.0, num_params)))
                        .collect();
                    format!(
                        "i64 {}, {}",
                        val(closure.0, num_params),
                        typed_args.join(", ")
                    )
                };

                let _ = writeln!(
                    buf,
                    "  {} = call i64 @{}({})",
                    val(inst.result.0, num_params),
                    call_fn,
                    args_with_types
                );
            } else {
                // For >8 args, marshal arguments into a contiguous stack array and
                // dispatch through the variadic runtime entrypoint.
                let args_arr = format!("%closure_args_arr_{}", inst.result.0);
                let _ = writeln!(buf, "  {} = alloca [{} x i64], align 8", args_arr, num_args);
                for (idx, arg) in args.iter().enumerate() {
                    let arg_ptr = format!("%closure_arg_ptr_{}_{}", inst.result.0, idx);
                    let _ = writeln!(
                        buf,
                        "  {} = getelementptr inbounds [{} x i64], ptr {}, i32 0, i32 {}",
                        arg_ptr, num_args, args_arr, idx
                    );
                    let _ = writeln!(
                        buf,
                        "  store i64 {}, ptr {}",
                        val(arg.0, num_params),
                        arg_ptr
                    );
                }
                let args_base = format!("%closure_args_base_{}", inst.result.0);
                let _ = writeln!(
                    buf,
                    "  {} = getelementptr inbounds [{} x i64], ptr {}, i32 0, i32 0",
                    args_base, num_args, args_arr
                );
                let _ = writeln!(
                    buf,
                    "  {} = call i64 @arth_rt_closure_call_variadic(i64 {}, ptr {}, i64 {})",
                    val(inst.result.0, num_params),
                    val(closure.0, num_params),
                    args_base,
                    num_args
                );
            }
        }
        InstKind::Drop { value, ty_name } => {
            // Emit call to deinit function for the type
            // The deinit function is <TypeName>Fns.deinit(value)
            let deinit_name = format!("{}.deinit", ty_name.replace('.', "_"));
            let _ = writeln!(
                buf,
                "  call void @{}(i64 {})  ; drop {}",
                deinit_name,
                val(value.0, num_params),
                ty_name
            );
            let _ = writeln!(
                buf,
                "  {} = add i64 0, 0  ; drop result placeholder",
                val(inst.result.0, num_params)
            );
        }
        InstKind::CondDrop {
            value,
            flag,
            ty_name,
        } => {
            // Conditional drop: only call deinit if flag is 0 (not moved)
            let deinit_name = format!("{}.deinit", ty_name.replace('.', "_"));
            let skip_label = format!("conddrop_skip_{}", inst.result.0);
            let drop_label = format!("conddrop_do_{}", inst.result.0);
            let end_label = format!("conddrop_end_{}", inst.result.0);

            // Check if moved (flag != 0)
            let _ = writeln!(
                buf,
                "  %cond_v{} = icmp ne i64 {}, 0",
                inst.result.0,
                val(flag.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  br i1 %cond_v{}, label %{}, label %{}",
                inst.result.0, skip_label, drop_label
            );

            // Drop block: call deinit
            let _ = writeln!(buf, "{}:", drop_label);
            let _ = writeln!(
                buf,
                "  call void @{}(i64 {})  ; conddrop {}",
                deinit_name,
                val(value.0, num_params),
                ty_name
            );
            let _ = writeln!(buf, "  br label %{}", end_label);

            // Skip block: do nothing
            let _ = writeln!(buf, "{}:", skip_label);
            let _ = writeln!(buf, "  br label %{}", end_label);

            // End block
            let _ = writeln!(buf, "{}:", end_label);
            let _ = writeln!(
                buf,
                "  {} = add i64 0, 0  ; conddrop result placeholder",
                val(inst.result.0, num_params)
            );
        }
        InstKind::FieldDrop {
            value,
            field_name,
            ty_name,
        } => {
            // Field drop: call deinit for a specific field of a struct
            // This is used for partial moves where some fields are moved but others need dropping
            let deinit_name = format!("{}.deinit", ty_name.replace('.', "_"));
            let _ = writeln!(
                buf,
                "  ; field_drop {}.{} (value {})",
                ty_name,
                field_name,
                val(value.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  call void @{}(i64 {})  ; drop field {}",
                deinit_name,
                val(value.0, num_params),
                field_name
            );
            let _ = writeln!(
                buf,
                "  {} = add i64 0, 0  ; field_drop result placeholder",
                val(inst.result.0, num_params)
            );
        }
        // RC operations - emit as runtime calls (placeholder for actual LLVM lowering)
        InstKind::RcAlloc { initial_value } => {
            let _ = writeln!(
                buf,
                "  {} = call i64 @__arth_rc_alloc(i64 {})  ; rc_alloc",
                val(inst.result.0, num_params),
                val(initial_value.0, num_params)
            );
        }
        InstKind::RcInc { handle } => {
            let _ = writeln!(
                buf,
                "  {} = call i64 @__arth_rc_inc(i64 {})  ; rc_inc",
                val(inst.result.0, num_params),
                val(handle.0, num_params)
            );
        }
        InstKind::RcDec { handle, ty_name } => {
            if let Some(tn) = ty_name {
                let _ = writeln!(
                    buf,
                    "  {} = call i64 @__arth_rc_dec_deinit(i64 {}, ptr @{}.deinit)  ; rc_dec {}",
                    val(inst.result.0, num_params),
                    val(handle.0, num_params),
                    tn.replace('.', "_"),
                    tn
                );
            } else {
                let _ = writeln!(
                    buf,
                    "  {} = call i64 @__arth_rc_dec(i64 {})  ; rc_dec",
                    val(inst.result.0, num_params),
                    val(handle.0, num_params)
                );
            }
        }
        InstKind::RcLoad { handle } => {
            let _ = writeln!(
                buf,
                "  {} = call i64 @__arth_rc_load(i64 {})  ; rc_load",
                val(inst.result.0, num_params),
                val(handle.0, num_params)
            );
        }
        InstKind::RcStore { handle, value } => {
            let _ = writeln!(
                buf,
                "  call void @__arth_rc_store(i64 {}, i64 {})  ; rc_store",
                val(handle.0, num_params),
                val(value.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  {} = add i64 0, 0  ; rc_store result",
                val(inst.result.0, num_params)
            );
        }
        InstKind::RcGetCount { handle } => {
            let _ = writeln!(
                buf,
                "  {} = call i64 @__arth_rc_get_count(i64 {})  ; rc_get_count",
                val(inst.result.0, num_params),
                val(handle.0, num_params)
            );
        }
        // Region-based allocation operations
        InstKind::RegionEnter { region_id } => {
            let _ = writeln!(
                buf,
                "  call void @arth_rt_region_enter(i32 {})  ; region_enter",
                region_id
            );
            let _ = writeln!(
                buf,
                "  {} = add i64 0, 0  ; region_enter result",
                val(inst.result.0, num_params)
            );
        }
        InstKind::RegionExit {
            region_id,
            deinit_calls,
        } => {
            // First call deinit for each value that needs cleanup
            for (value, ty_name) in deinit_calls {
                let deinit_name = format!("{}.deinit", ty_name.replace('.', "_"));
                let _ = writeln!(
                    buf,
                    "  call void @{}(i64 {})  ; region deinit {}",
                    deinit_name,
                    val(value.0, num_params),
                    ty_name
                );
            }
            // Then exit the region (bulk deallocate)
            let _ = writeln!(
                buf,
                "  call void @arth_rt_region_exit(i32 {})  ; region_exit",
                region_id
            );
            let _ = writeln!(
                buf,
                "  {} = add i64 0, 0  ; region_exit result",
                val(inst.result.0, num_params)
            );
        }
        InstKind::GetTypeName(value) => {
            let from_landing_pad = blocks.iter().any(|b| {
                b.insts
                    .iter()
                    .any(|i| i.result == *value && matches!(i.kind, InstKind::LandingPad { .. }))
            });
            if from_landing_pad {
                // ArthException layout (64-bit): type_name pointer lives at byte offset 40.
                let type_name_addr = format!("%type_name_addr_{}", inst.result.0);
                let type_name_ptr = format!("%type_name_ptr_{}", inst.result.0);
                let _ = writeln!(
                    buf,
                    "  {} = getelementptr inbounds i8, ptr %arth_ex{}, i64 40",
                    type_name_addr, value.0
                );
                let _ = writeln!(
                    buf,
                    "  {} = load ptr, ptr {}",
                    type_name_ptr, type_name_addr
                );
                let _ = writeln!(
                    buf,
                    "  {} = ptrtoint ptr {} to i64",
                    val(inst.result.0, num_params),
                    type_name_ptr
                );
            } else {
                let _ = writeln!(
                    buf,
                    "  {} = add i64 {}, 0",
                    val(inst.result.0, num_params),
                    val(value.0, num_params)
                );
            }
        }

        // ========== Async State Machine Operations ==========
        // These emit calls to arth_rt_async_* runtime functions.
        InstKind::AsyncFrameAlloc {
            frame_name,
            frame_size,
        } => {
            // Allocate async frame via runtime; result is ptr converted to i64
            let _ = writeln!(
                buf,
                "  %frame_ptr{} = call ptr @arth_rt_async_frame_alloc(i32 {})  ; alloc frame {}",
                inst.result.0, frame_size, frame_name
            );
            let _ = writeln!(
                buf,
                "  {} = ptrtoint ptr %frame_ptr{} to i64",
                val(inst.result.0, num_params),
                inst.result.0
            );
        }
        InstKind::AsyncFrameFree { frame_ptr } => {
            // Free async frame; convert i64 back to ptr
            let _ = writeln!(
                buf,
                "  %free_ptr{} = inttoptr i64 {} to ptr",
                inst.result.0,
                val(frame_ptr.0, num_params)
            );
            // Note: We don't have frame_size here, use 0 (runtime should handle it)
            let _ = writeln!(
                buf,
                "  call void @arth_rt_async_frame_free(ptr %free_ptr{}, i32 0)  ; free frame",
                inst.result.0
            );
            let _ = writeln!(
                buf,
                "  {} = add i64 0, 0  ; frame_free result",
                val(inst.result.0, num_params)
            );
        }
        InstKind::AsyncFrameGetState { frame_ptr } => {
            let _ = writeln!(
                buf,
                "  %state_ptr{} = inttoptr i64 {} to ptr",
                inst.result.0,
                val(frame_ptr.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  {} = call i64 @arth_rt_async_frame_get_state(ptr %state_ptr{})  ; get_state",
                val(inst.result.0, num_params),
                inst.result.0
            );
        }
        InstKind::AsyncFrameSetState {
            frame_ptr,
            state_id,
        } => {
            let _ = writeln!(
                buf,
                "  %setstate_ptr{} = inttoptr i64 {} to ptr",
                inst.result.0,
                val(frame_ptr.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  call void @arth_rt_async_frame_set_state(ptr %setstate_ptr{}, i32 {})  ; set_state",
                inst.result.0, state_id
            );
            let _ = writeln!(
                buf,
                "  {} = add i64 0, 0  ; set_state result",
                val(inst.result.0, num_params)
            );
        }
        InstKind::AsyncFrameLoad {
            frame_ptr,
            field_offset,
            ..
        } => {
            let _ = writeln!(
                buf,
                "  %load_ptr{} = inttoptr i64 {} to ptr",
                inst.result.0,
                val(frame_ptr.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  {} = call i64 @arth_rt_async_frame_load(ptr %load_ptr{}, i32 {})  ; frame_load",
                val(inst.result.0, num_params),
                inst.result.0,
                field_offset
            );
        }
        InstKind::AsyncFrameStore {
            frame_ptr,
            field_offset,
            value,
        } => {
            let _ = writeln!(
                buf,
                "  %store_ptr{} = inttoptr i64 {} to ptr",
                inst.result.0,
                val(frame_ptr.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  call void @arth_rt_async_frame_store(ptr %store_ptr{}, i32 {}, i64 {})  ; frame_store",
                inst.result.0,
                field_offset,
                val(value.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  {} = add i64 0, 0  ; frame_store result",
                val(inst.result.0, num_params)
            );
        }
        InstKind::AsyncCheckCancelled { task_handle } => {
            let _ = writeln!(
                buf,
                "  {} = call i64 @arth_rt_task_is_cancelled(i64 {})  ; check_cancelled",
                val(inst.result.0, num_params),
                val(task_handle.0, num_params)
            );
        }
        InstKind::AsyncYield { awaited_task } => {
            let _ = writeln!(
                buf,
                "  call void @arth_rt_async_yield(i64 {})  ; async_yield",
                val(awaited_task.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  {} = add i64 0, 0  ; yield result",
                val(inst.result.0, num_params)
            );
        }
        InstKind::AwaitPoint { awaited_task, .. } => {
            // AwaitPoint should have been lowered to explicit state machine code.
            // If we see it here, emit a call to arth_rt_await as a fallback.
            let _ = writeln!(
                buf,
                "  {} = call i64 @arth_rt_await(i64 {})  ; await_point",
                val(inst.result.0, num_params),
                val(awaited_task.0, num_params)
            );
        }
        // Provider instructions - call arth_rt_provider_* functions
        InstKind::ProviderNew { name, values } => {
            // Create provider struct with name and field count
            let _ = writeln!(
                buf,
                "  {} = call i64 @arth_rt_provider_new(ptr @.str.provider.{}, i64 {}, i64 {})  ; provider_new {}",
                val(inst.result.0, num_params),
                name.replace('.', "_"),
                name.len(),
                values.len(),
                name
            );

            // Initialize provider fields from literal values.
            for (idx, (field, value)) in values.iter().enumerate() {
                let _ = writeln!(
                    buf,
                    "  %provider_set_{}_{} = call i64 @arth_rt_provider_field_set_named(i64 {}, ptr @.str.field.{}.{}, i64 {}, i64 {})",
                    inst.result.0,
                    idx,
                    val(inst.result.0, num_params),
                    name.replace('.', "_"),
                    field,
                    field.len(),
                    val(value.0, num_params)
                );
            }
        }
        InstKind::ProviderFieldGet {
            obj,
            provider,
            field,
            ..
        } => {
            // Use named field get with string constant for field name
            let _ = writeln!(
                buf,
                "  {} = call i64 @arth_rt_provider_field_get_named(i64 {}, ptr @.str.field.{}.{}, i64 {})  ; provider_field_get {}.{}",
                val(inst.result.0, num_params),
                val(obj.0, num_params),
                provider.replace('.', "_"),
                field,
                field.len(),
                provider,
                field
            );
        }
        InstKind::ProviderFieldSet {
            obj,
            provider,
            field,
            value,
            ..
        } => {
            // Use named field set with string constant for field name
            let _ = writeln!(
                buf,
                "  {} = call i64 @arth_rt_provider_field_set_named(i64 {}, ptr @.str.field.{}.{}, i64 {}, i64 {})  ; provider_field_set {}.{}",
                val(inst.result.0, num_params),
                val(obj.0, num_params),
                provider.replace('.', "_"),
                field,
                field.len(),
                val(value.0, num_params),
                provider,
                field
            );
        }
    }
}

/// Insert `, !dbg !N` before the trailing newline of the last line that was
/// appended to `buf` since `start_len`. If the last line is a comment or a
/// label (not a real instruction), this is a no-op.
fn append_dbg_to_last_line(buf: &mut String, start_len: usize, loc_id: u32) {
    // Find the last newline in the newly appended region
    let added = &buf[start_len..];
    if added.is_empty() {
        return;
    }
    // Find the position of the final newline
    if let Some(last_nl) = added.rfind('\n') {
        // Find the line before that newline
        let line_start = added[..last_nl].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let line = added[line_start..last_nl].trim();
        // Skip comment lines, labels, and empty lines
        if line.starts_with(';') || line.ends_with(':') || line.is_empty() {
            return;
        }
        // Don't annotate lines that already have !dbg
        if line.contains("!dbg") {
            return;
        }
        let insert_pos = start_len + last_nl;
        buf.insert_str(insert_pos, &format!(", !dbg !{}", loc_id));
    }
}

fn emit_block(
    buf: &mut String,
    b: &BlockData,
    f64_values: &std::collections::HashSet<u32>,
    func_ret: &Ty,
    value_type_map: &std::collections::HashMap<u32, String>,
    blocks: &[BlockData],
    module: &Module,
    num_params: usize,
    type_registry: &TypeRegistry,
    mangle_map: &HashMap<String, String>,
    mut debug: Option<&mut DebugInfoBuilder>,
    scope_id: Option<u32>,
) {
    let _ = writeln!(buf, "{}:", b.name);
    for inst in &b.insts {
        // Emit the instruction first
        let start_len = buf.len();
        emit_inst(
            buf,
            inst,
            f64_values,
            blocks,
            module,
            num_params,
            type_registry,
            mangle_map,
        );

        // Append !dbg metadata if debug info is available for this instruction
        if let (Some(dbg), Some(scope)) = (&mut debug, scope_id) {
            if let Some(span) = &inst.span {
                if let Some(loc) = dbg.resolve_span(span) {
                    let loc_id = dbg.create_location(loc.line, loc.col, scope);
                    // Insert ", !dbg !N" before the trailing newline of the last line
                    append_dbg_to_last_line(buf, start_len, loc_id);
                }
            }
        }
    }

    // Helper to get block name by index
    let block_name = |idx: u32| -> &str {
        blocks
            .get(idx as usize)
            .map(|b| b.name.as_str())
            .unwrap_or("unknown")
    };

    // Wrap terminator emission with debug annotation
    let term_start = buf.len();
    match &b.term {
        crate::compiler::ir::Terminator::Ret(Some(v)) => match func_ret {
            Ty::Void => {
                let _ = writeln!(buf, "  ret void");
            }
            Ty::F64 => {
                if f64_values.contains(&v.0) {
                    let _ = writeln!(buf, "  ret double {}", val(v.0, num_params));
                } else {
                    let cast = format!("%ret_f64_{}", v.0);
                    let _ = writeln!(
                        buf,
                        "  {} = bitcast i64 {} to double",
                        cast,
                        val(v.0, num_params)
                    );
                    let _ = writeln!(buf, "  ret double {}", cast);
                }
            }
            _ => {
                let _ = writeln!(buf, "  ret i64 {}", val(v.0, num_params));
            }
        },
        crate::compiler::ir::Terminator::Ret(None) => {
            if matches!(func_ret, Ty::Void) {
                let _ = writeln!(buf, "  ret void");
            } else {
                let _ = writeln!(buf, "  ret i64 0");
            }
        }
        crate::compiler::ir::Terminator::Br(bb) => {
            let _ = writeln!(buf, "  br label %{}", block_name(bb.0));
        }
        crate::compiler::ir::Terminator::CondBr {
            cond,
            then_bb,
            else_bb,
        } => {
            // Convert i64 truthy condition to i1.
            let _ = writeln!(
                buf,
                "  %cond_v{} = icmp ne i64 {}, 0",
                cond.0,
                val(cond.0, num_params)
            );
            let _ = writeln!(
                buf,
                "  br i1 %cond_v{}, label %{}, label %{}",
                cond.0,
                block_name(then_bb.0),
                block_name(else_bb.0)
            );
        }
        crate::compiler::ir::Terminator::Switch {
            scrut,
            default,
            cases,
        } => {
            let _ = writeln!(
                buf,
                "  switch i64 {}, label %{} [",
                val(scrut.0, num_params),
                block_name(default.0)
            );
            for (v, bb) in cases {
                let _ = writeln!(buf, "    i64 {}, label %{}", v, block_name(bb.0));
            }
            let _ = writeln!(buf, "  ]");
        }
        crate::compiler::ir::Terminator::Unreachable => {
            let _ = writeln!(buf, "  unreachable");
        }
        crate::compiler::ir::Terminator::Throw(value) => {
            // Throw an exception using the arth-rt exception system.
            // The value is an exception object (struct) that contains the exception data.
            // We look up the type name from the value_type_map to emit proper type info.
            if let Some(v) = value {
                let throw_id = format!("{}_{}", b.name.replace(['.', ':', '-'], "_"), v.0);
                let type_name_ptr = format!("%throw_type_name_ptr_{}", throw_id);
                let payload_ptr = format!("%throw_payload_ptr_{}", throw_id);
                // Try to get the exception type name from the value type map
                if let Some(type_name) = value_type_map.get(&v.0) {
                    // Use the pre-computed type info global
                    let safe_name = type_name.replace('.', "_");
                    let type_id = fnv1a_hash(type_name);
                    let type_name_len = type_name.len();

                    // Get pointer to the type name string from the type info global
                    let _ = writeln!(
                        buf,
                        "  {} = getelementptr inbounds [{} x i8], ptr @.str.typeinfo.{}, i64 0, i64 0",
                        type_name_ptr,
                        type_name_len + 1,
                        safe_name
                    );

                    // Call arth_rt_throw with proper type info
                    // The exception value is passed as the payload pointer
                    // For VM-style structs (i64 handles), we pass them as i64 cast to ptr
                    let _ = writeln!(
                        buf,
                        "  {} = inttoptr i64 {} to ptr",
                        payload_ptr,
                        val(v.0, num_params)
                    );
                    let _ = writeln!(
                        buf,
                        "  call void @arth_rt_throw(i64 {}, ptr {}, i64 {}, ptr {}, i64 8)",
                        type_id, type_name_ptr, type_name_len, payload_ptr
                    );
                } else {
                    // Type not found in map - use runtime type lookup
                    // This is a fallback for dynamically typed exceptions
                    let _ = writeln!(
                        buf,
                        "  {} = inttoptr i64 {} to ptr",
                        payload_ptr,
                        val(v.0, num_params)
                    );
                    // Use type ID 0 and null type name - the runtime will handle it
                    let _ = writeln!(
                        buf,
                        "  call void @arth_rt_throw(i64 0, ptr null, i64 0, ptr {}, i64 8)",
                        payload_ptr
                    );
                }
            } else {
                // Throw with no value - use a generic exception
                let _ = writeln!(
                    buf,
                    "  call void @arth_rt_throw(i64 0, ptr null, i64 0, ptr null, i64 0)"
                );
            }
            let _ = writeln!(buf, "  unreachable");
        }
        crate::compiler::ir::Terminator::Panic(msg) => {
            // Panic: call runtime panic function with message, then unreachable
            if let Some(v) = msg {
                let _ = writeln!(
                    buf,
                    "  call void @__arth_panic_str(i64 {})",
                    val(v.0, num_params)
                );
            } else {
                let _ = writeln!(buf, "  call void @__arth_panic_str(i64 0)");
            }
            let _ = writeln!(buf, "  unreachable");
        }
        crate::compiler::ir::Terminator::Invoke {
            callee,
            args,
            ret,
            result,
            normal,
            unwind,
        } => {
            // Resolve and mangle the invoke callee name
            let resolved_local = resolve_local_callee_name(module, callee);
            let resolved_name = if let Some(sym) = native_symbol_for_call(callee) {
                sym.to_string()
            } else if let Some(local) = resolved_local {
                local
            } else {
                callee.clone()
            };
            let invoke_name = if native_symbol_for_call(callee).is_some()
                || resolved_name.starts_with("arth_rt_")
                || resolved_name.starts_with("__arth_")
            {
                resolved_name.clone()
            } else {
                get_mangled_name(&resolved_name, mangle_map).to_string()
            };

            let mut args_s = String::new();
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    args_s.push_str(", ");
                }
                let _ = write!(args_s, "i64 {}", val(a.0, num_params));
            }
            let ret_s = ty_llvm(ret);
            match (ret, result) {
                (Ty::Void, _) => {
                    let _ = writeln!(
                        buf,
                        "  invoke {} @{}({}) to label %{} unwind label %{}",
                        ret_s,
                        invoke_name,
                        args_s,
                        block_name(normal.0),
                        block_name(unwind.0)
                    );
                }
                (_, Some(res)) => {
                    let _ = writeln!(
                        buf,
                        "  {} = invoke {} @{}({}) to label %{} unwind label %{}",
                        val(res.0, num_params),
                        ret_s,
                        invoke_name,
                        args_s,
                        block_name(normal.0),
                        block_name(unwind.0)
                    );
                }
                (_, None) => {
                    // Inconsistent, but keep emitting without result to avoid panic.
                    let _ = writeln!(
                        buf,
                        "  invoke {} @{}({}) to label %{} unwind label %{}",
                        ret_s,
                        invoke_name,
                        args_s,
                        block_name(normal.0),
                        block_name(unwind.0)
                    );
                }
            }
        }

        // Async poll function return - encode poll result as integer return
        crate::compiler::ir::Terminator::PollReturn { result, value } => {
            use crate::compiler::ir::PollResult;
            let result_code = match result {
                PollResult::Ready => 0,
                PollResult::Pending => 1,
                PollResult::Cancelled => 2,
                PollResult::Panicked => 3,
                PollResult::Error => 4,
            };
            // Store result code and return
            // In a full implementation, we'd also store the value to the frame
            if let Some(v) = value {
                let _ = writeln!(
                    buf,
                    "  ; poll_return {:?} with value {}",
                    result,
                    val(v.0, num_params)
                );
            }
            let _ = writeln!(buf, "  ret i64 {}", result_code);
        }
    }

    // Annotate terminator with debug location from the block's span
    if let (Some(dbg), Some(scope)) = (&mut debug, scope_id) {
        if let Some(span) = &b.span {
            if let Some(loc) = dbg.resolve_span(span) {
                let loc_id = dbg.create_location(loc.line, loc.col, scope);
                append_dbg_to_last_line(buf, term_start, loc_id);
            }
        }
    }
}

/// Check if the module uses closures (has MakeClosure or ClosureCall instructions)
fn module_has_closures(m: &Module) -> bool {
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                match &inst.kind {
                    InstKind::MakeClosure { .. } | InstKind::ClosureCall { .. } => return true,
                    _ => {}
                }
            }
        }
    }
    false
}

/// Check whether module uses string helper instructions that require runtime stubs.
fn module_has_string_helpers(m: &Module) -> bool {
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                if matches!(
                    &inst.kind,
                    InstKind::StrEq(_, _) | InstKind::StrConcat(_, _)
                ) {
                    return true;
                }
            }
        }
    }
    false
}

const CLOSURE_SLOT_ALIGNMENT: u32 = 8;

#[derive(Clone, Debug)]
struct ClosureEnvLayout {
    func_name: String,
    capture_types: Vec<Ty>,
    capture_alignments: Vec<u32>,
    issues: Vec<String>,
}

fn closure_env_llvm_name(func_name: &str) -> String {
    format!("ClosureEnv_{}", func_name.replace(['.', ':', '-'], "_"))
}

fn closure_capture_llvm_ty(ty: &Ty) -> String {
    match ty {
        Ty::I64 => "i64".to_string(),
        Ty::F64 => "double".to_string(),
        Ty::I1 => "i1".to_string(),
        Ty::Ptr => "ptr".to_string(),
        Ty::Void => "void".to_string(),
        Ty::Struct(name) | Ty::Enum(name) => format!("%{}", name.replace(['.', ':'], "_")),
        Ty::Optional(inner) => format!("{{ i8, {} }}", closure_capture_llvm_ty(inner)),
        Ty::String => "{ ptr, i64 }".to_string(),
    }
}

fn closure_capture_alignment(ty: &Ty, type_registry: &TypeRegistry) -> u32 {
    match ty {
        Ty::I64 | Ty::F64 | Ty::Ptr | Ty::String | Ty::Optional(_) => 8,
        Ty::I1 => 1,
        Ty::Void => 1,
        Ty::Struct(name) => type_registry
            .get_struct(name)
            .map(|layout| layout.alignment)
            .unwrap_or(CLOSURE_SLOT_ALIGNMENT),
        Ty::Enum(name) => type_registry
            .get_enum(name)
            .map(|layout| layout.alignment)
            .unwrap_or(CLOSURE_SLOT_ALIGNMENT),
    }
}

fn infer_closure_capture_types(
    m: &Module,
    func_name: &str,
    capture_count: usize,
) -> (Vec<Ty>, Vec<String>) {
    let mut issues = Vec::new();
    let Some(lambda) = m.funcs.iter().find(|f| f.name == func_name) else {
        issues.push(format!(
            "closure target '{}' not found while deriving capture layout",
            func_name
        ));
        return (vec![Ty::I64; capture_count], issues);
    };

    if lambda.params.len() < capture_count {
        issues.push(format!(
            "closure target '{}' has {} params but closure captures {} values",
            func_name,
            lambda.params.len(),
            capture_count
        ));
    }

    let mut capture_types = Vec::with_capacity(capture_count);
    for idx in 0..capture_count {
        capture_types.push(lambda.params.get(idx).cloned().unwrap_or(Ty::I64));
    }
    (capture_types, issues)
}

fn build_closure_env_layouts(m: &Module, type_registry: &TypeRegistry) -> Vec<ClosureEnvLayout> {
    let mut layouts = std::collections::BTreeMap::<String, ClosureEnvLayout>::new();

    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                let InstKind::MakeClosure { func, captures } = &inst.kind else {
                    continue;
                };

                if layouts.contains_key(func) {
                    continue;
                }

                let (capture_types, mut issues) =
                    infer_closure_capture_types(m, func, captures.len());
                let mut capture_alignments = Vec::with_capacity(capture_types.len());
                for (idx, ty) in capture_types.iter().enumerate() {
                    let align = closure_capture_alignment(ty, type_registry);
                    capture_alignments.push(align);
                    if align > CLOSURE_SLOT_ALIGNMENT {
                        issues.push(format!(
                            "capture {} of '{}' requires alignment {} (slot alignment is {})",
                            idx, func, align, CLOSURE_SLOT_ALIGNMENT
                        ));
                    }
                    match ty {
                        Ty::Struct(name) if type_registry.get_struct(name).is_none() => {
                            issues.push(format!(
                                "capture {} of '{}' references unknown struct layout '{}'",
                                idx, func, name
                            ));
                        }
                        Ty::Enum(name) if type_registry.get_enum(name).is_none() => {
                            issues.push(format!(
                                "capture {} of '{}' references unknown enum layout '{}'",
                                idx, func, name
                            ));
                        }
                        _ => {}
                    }
                }

                layouts.insert(
                    func.clone(),
                    ClosureEnvLayout {
                        func_name: func.clone(),
                        capture_types,
                        capture_alignments,
                        issues,
                    },
                );
            }
        }
    }

    layouts.into_values().collect()
}

/// Check if a function uses exception handling (has Invoke terminators or LandingPad instructions)
fn func_has_exception_handling(f: &Func) -> bool {
    for b in &f.blocks {
        // Check for LandingPad instructions
        for inst in &b.insts {
            if matches!(&inst.kind, InstKind::LandingPad { .. }) {
                return true;
            }
        }
        // Check for Invoke terminators
        if matches!(&b.term, crate::compiler::ir::Terminator::Invoke { .. }) {
            return true;
        }
    }
    false
}

fn emit_func(
    buf: &mut String,
    f: &Func,
    module: &Module,
    type_registry: &TypeRegistry,
    mangle_map: &HashMap<String, String>,
    mut debug: Option<&mut DebugInfoBuilder>,
) {
    // Collect all values that are f64 (floats) - propagate through operations
    let mut f64_values = std::collections::HashSet::new();

    // First pass: identify direct float sources
    // - ConstF64 produces float
    // - Function parameters with F64 type
    for (i, p) in f.params.iter().enumerate() {
        if *p == Ty::F64 {
            f64_values.insert(i as u32);
        }
    }
    for b in &f.blocks {
        for inst in &b.insts {
            if let InstKind::ConstF64(_) = &inst.kind {
                f64_values.insert(inst.result.0);
            }
        }
    }

    // Second pass: propagate float type through Binary operations
    // Keep iterating until no new floats are discovered
    loop {
        let mut changed = false;
        for b in &f.blocks {
            for inst in &b.insts {
                if let InstKind::Binary(_, a, b) = &inst.kind {
                    // If either operand is float, result is float
                    if f64_values.contains(&a.0) || f64_values.contains(&b.0) {
                        if f64_values.insert(inst.result.0) {
                            changed = true;
                        }
                    }
                }
                // Copy propagates the type
                if let InstKind::Copy(src) = &inst.kind {
                    if f64_values.contains(&src.0) {
                        if f64_values.insert(inst.result.0) {
                            changed = true;
                        }
                    }
                }
                // Load from a pointer could be float - for now, we'll handle this conservatively
                // Phi nodes propagate float if any input is float
                if let InstKind::Phi(inputs) = &inst.kind {
                    if inputs.iter().any(|(_, v)| f64_values.contains(&v.0)) {
                        if f64_values.insert(inst.result.0) {
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Build value type map for exception type tracking
    let value_type_map = build_value_type_map(f, module);

    let ret = ty_llvm(&f.ret);
    let mut sig = String::new();
    for (i, p) in f.params.iter().enumerate() {
        if i > 0 {
            sig.push_str(", ");
        }
        let _ = write!(sig, "{} %{}", ty_llvm(p), i);
    }

    // Check if function needs personality function for exception handling
    let has_eh = func_has_exception_handling(f);
    let linkage = f.linkage.to_llvm_str();
    let func_name = get_mangled_name(&f.name, mangle_map);

    // Create debug subprogram if debug info is enabled
    let subprogram_id = if let Some(ref mut dbg) = debug {
        if let Some(span) = &f.span {
            if let Some(loc) = dbg.resolve_span(span) {
                let file_id = dbg.get_or_create_file(&loc.file);
                Some(
                    dbg.create_subprogram(&f.name, func_name, file_id, loc.line, &f.ret, &f.params),
                )
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // Emit function header with optional !dbg attachment
    let dbg_suffix = subprogram_id
        .map(|id| format!(" !dbg !{}", id))
        .unwrap_or_default();

    if has_eh {
        let _ = writeln!(
            buf,
            "define {}{} @{}({}) personality ptr @__arth_personality_v0{} {{",
            linkage, ret, func_name, sig, dbg_suffix
        );
    } else {
        let _ = writeln!(
            buf,
            "define {}{} @{}({}){} {{",
            linkage, ret, func_name, sig, dbg_suffix
        );
    }
    let num_params = f.params.len();
    for b in &f.blocks {
        emit_block(
            buf,
            b,
            &f64_values,
            &f.ret,
            &value_type_map,
            &f.blocks,
            module,
            num_params,
            type_registry,
            mangle_map,
            debug.as_deref_mut(),
            subprogram_id,
        );
    }
    let _ = writeln!(buf, "}}\n");
}

/// Check if any function in the module uses exception handling
fn module_has_exception_handling(m: &Module) -> bool {
    m.funcs.iter().any(func_has_exception_handling)
}

/// Return the LLVM `target datalayout` string for a known target triple.
///
/// Only a handful of triples are supported. For an unrecognised triple the
/// function returns `None` and the emitter will simply omit the line (which
/// is valid LLVM IR — the backend infers defaults).
fn datalayout_for_triple(triple: &str) -> Option<&'static str> {
    match triple {
        // x86_64 — the three supported OS flavours differ only in the mangling
        // prefix (`m:o` = Mach-O, `m:e` = ELF, `m:w` = COFF/Windows).
        "x86_64-apple-darwin" => {
            Some("e-m:o-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128")
        }
        "x86_64-unknown-linux-gnu" => {
            Some("e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128")
        }
        "x86_64-pc-windows-msvc" => {
            Some("e-m:w-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128")
        }
        // AArch64 — macOS and Linux share the core layout; Fn32 is a Darwin
        // convention for 32-byte function alignment.
        "aarch64-apple-darwin" => Some("e-m:o-i64:64-i128:128-n32:64-S128-Fn32"),
        "aarch64-unknown-linux-gnu" => Some("e-m:e-i64:64-i128:128-n32:64-S128"),
        _ => None,
    }
}

pub fn emit_module_text(m: &Module) -> String {
    emit_module_text_for_target(m, None)
}

/// Emit the complete LLVM IR text for a module, optionally targeting a
/// specific platform triple.
///
/// When `target_triple` is `Some(...)`, the emitted IR will include
/// `target triple` and `target datalayout` directives so that LLVM's code
/// generator selects the correct calling convention (System V AMD64 on
/// macOS/Linux, Microsoft x64 on Windows) and object format.
///
/// When `target_triple` is `None` (the default used by `emit_module_text`),
/// no target metadata is emitted and LLVM will infer defaults from the host
/// or from flags passed to clang / llc.
pub fn emit_module_text_for_target(m: &Module, target_triple: Option<&str>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "; ModuleID = '{}'", m.name);

    // Emit target metadata when a triple is provided.
    if let Some(triple) = target_triple {
        if let Some(dl) = datalayout_for_triple(triple) {
            let _ = writeln!(out, "target datalayout = \"{}\"", dl);
        }
        let _ = writeln!(out, "target triple = \"{}\"", triple);
    }

    // Build type registry and emit LLVM type definitions
    let type_registry = build_type_registry(m);
    let mangle_map = build_mangle_map(&m.funcs);
    let closure_env_layouts = build_closure_env_layouts(m, &type_registry);
    let type_defs = type_registry.emit_type_definitions();
    if !type_defs.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "; --- Struct and Enum type definitions ---");
        out.push_str(&type_defs);
    }
    let _ = writeln!(out);

    // Declare runtime intrinsics
    let _ = writeln!(out, "; --- Runtime intrinsic declarations ---");
    // Logging
    let _ = writeln!(out, "declare i64 @__arth_log_emit_str(i64, ptr, ptr, ptr)");
    // Region-based allocation (for loop-local bulk deallocation)
    // These are implemented in arth-rt and linked via native runtime
    let _ = writeln!(out, "declare void @arth_rt_region_enter(i32)");
    let _ = writeln!(out, "declare void @arth_rt_region_exit(i32)");

    // Declare exception handling functions if needed
    let exception_types = collect_exception_types(m);
    let has_eh = module_has_exception_handling(m) || !exception_types.is_empty();
    if has_eh {
        let _ = writeln!(out);
        let _ = writeln!(out, "; --- Exception handling declarations ---");
        // Personality function for DWARF unwinding
        let _ = writeln!(
            out,
            "declare i32 @__arth_personality_v0(i32, i32, i64, ptr, ptr)"
        );
        // Exception runtime functions
        let _ = writeln!(out, "declare void @arth_rt_throw(i64, ptr, i64, ptr, i64)");
        let _ = writeln!(out, "declare ptr @arth_rt_begin_catch(ptr)");
        let _ = writeln!(out, "declare void @arth_rt_end_catch()");
        let _ = writeln!(out, "declare void @arth_rt_resume_unwind(ptr)");
        let _ = writeln!(out, "declare i64 @arth_rt_type_id(ptr, i64)");

        // Emit exception type info globals
        emit_exception_type_info(&mut out, &exception_types);
    }

    // Declare closure runtime functions if closures are used
    if module_has_closures(m) {
        let _ = writeln!(out);
        let _ = writeln!(out, "; --- Closure runtime declarations ---");
        if !closure_env_layouts.is_empty() {
            let _ = writeln!(
                out,
                "; Closure environment layouts (derived from lambda capture parameter metadata)"
            );
            for layout in &closure_env_layouts {
                let env_name = closure_env_llvm_name(&layout.func_name);
                if layout.capture_types.is_empty() {
                    let _ = writeln!(out, "%{} = type {{}}", env_name);
                } else {
                    let capture_types: Vec<String> = layout
                        .capture_types
                        .iter()
                        .map(closure_capture_llvm_ty)
                        .collect();
                    let _ = writeln!(
                        out,
                        "%{} = type {{ {} }}",
                        env_name,
                        capture_types.join(", ")
                    );
                }
                let alignments = layout
                    .capture_alignments
                    .iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(out, "; capture alignments: [{}]", alignments);
                for issue in &layout.issues {
                    let _ = writeln!(out, "; WARNING: closure layout validation: {}", issue);
                }
            }
        }
        // Closure type: { ptr fn_ptr, ptr env_ptr, i64 num_captures }
        let _ = writeln!(out, "%Closure = type {{ ptr, ptr, i64 }}");
        // Create a new closure: allocates closure struct and environment
        // (fn_ptr, num_captures) -> closure_handle as i64
        let _ = writeln!(out, "declare i64 @arth_rt_closure_new(ptr, i64)");
        // Add a captured value to the closure's environment
        // (closure_handle, value) -> void
        let _ = writeln!(out, "declare void @arth_rt_closure_capture(i64, i64)");
        // Call a closure with N arguments (N=0..8)
        // These specialized functions handle loading captures and calling the fn_ptr
        let _ = writeln!(out, "declare i64 @arth_rt_closure_call_0(i64)");
        let _ = writeln!(out, "declare i64 @arth_rt_closure_call_1(i64, i64)");
        let _ = writeln!(out, "declare i64 @arth_rt_closure_call_2(i64, i64, i64)");
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_3(i64, i64, i64, i64)"
        );
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_4(i64, i64, i64, i64, i64)"
        );
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_5(i64, i64, i64, i64, i64, i64)"
        );
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_6(i64, i64, i64, i64, i64, i64, i64)"
        );
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_7(i64, i64, i64, i64, i64, i64, i64, i64)"
        );
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_8(i64, i64, i64, i64, i64, i64, i64, i64, i64)"
        );
        // Variadic closure-call path used when argument count exceeds 8.
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_variadic(i64, ptr, i64)"
        );
    }

    // String helper stubs used by StrEq/StrConcat lowering.
    if module_has_string_helpers(m) {
        let _ = writeln!(out);
        let _ = writeln!(out, "; --- String helper stubs ---");
        let _ = writeln!(out, "declare i32 @strcmp(ptr, ptr)");
        let _ = writeln!(out, "define internal i1 @__arth_str_eq(ptr %a, ptr %b) {{");
        let _ = writeln!(out, "entry:");
        let _ = writeln!(out, "  %cmp = call i32 @strcmp(ptr %a, ptr %b)");
        let _ = writeln!(out, "  %eq = icmp eq i32 %cmp, 0");
        let _ = writeln!(out, "  ret i1 %eq");
        let _ = writeln!(out, "}}\n");
        let _ = writeln!(
            out,
            "define internal ptr @__arth_str_concat(ptr %a, ptr %b) {{"
        );
        let _ = writeln!(out, "entry:");
        let _ = writeln!(out, "  ret ptr %a");
        let _ = writeln!(out, "}}\n");
    }
    let _ = writeln!(out);

    // Collect and declare native runtime symbols (arth_rt_* functions)
    let native_symbols = collect_native_symbols(m);
    if !native_symbols.is_empty() {
        let _ = writeln!(
            out,
            "; --- Native runtime function declarations (arth-rt) ---"
        );
        let mut sorted_symbols: Vec<_> = native_symbols.into_iter().collect();
        sorted_symbols.sort();
        for sym in sorted_symbols {
            // Skip EH declarations already emitted in the dedicated EH block.
            if has_eh
                && matches!(
                    sym,
                    "arth_rt_throw"
                        | "arth_rt_begin_catch"
                        | "arth_rt_end_catch"
                        | "arth_rt_resume_unwind"
                        | "arth_rt_type_id"
                )
            {
                continue;
            }
            let (ret_ty, params) = native_symbol_signature(sym);
            let _ = writeln!(out, "declare {} @{}({})", ret_ty, sym, params);
        }
        let _ = writeln!(out);
    }

    // Emit extern function declarations (FFI)
    if !m.extern_funcs.is_empty() {
        let _ = writeln!(out, "; --- FFI extern function declarations ---");
        for ef in &m.extern_funcs {
            let mut params_s = String::new();
            for (i, p) in ef.params.iter().enumerate() {
                if i > 0 {
                    params_s.push_str(", ");
                }
                params_s.push_str(&ty_llvm(p));
            }
            let ret_s = ty_llvm(&ef.ret);
            let _ = writeln!(out, "declare {} @{}({})", ret_s, ef.name, params_s);
        }
        let _ = writeln!(out);
    }

    // Emit synthetic fallback stubs for unresolved direct calls.
    let unresolved_stubs = collect_unresolved_call_stubs(m);
    if !unresolved_stubs.is_empty() {
        let _ = writeln!(out, "; --- Unresolved call stubs ---");
        for (name, ret_ty, params) in unresolved_stubs {
            let mut params_s = String::new();
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    params_s.push_str(", ");
                }
                let _ = write!(params_s, "{} %p{}", p, i);
            }
            let _ = writeln!(out, "define internal {} @{}({}) {{", ret_ty, name, params_s);
            let _ = writeln!(out, "entry:");
            match ret_ty.as_str() {
                "void" => {
                    let _ = writeln!(out, "  ret void");
                }
                "ptr" => {
                    let _ = writeln!(out, "  ret ptr null");
                }
                "double" => {
                    let _ = writeln!(out, "  ret double 0.0");
                }
                "i1" => {
                    let _ = writeln!(out, "  ret i1 false");
                }
                _ => {
                    let _ = writeln!(out, "  ret {} 0", ret_ty);
                }
            }
            let _ = writeln!(out, "}}\n");
        }
    }

    // Emit global string constants
    for (i, s) in m.strings.iter().enumerate() {
        let mut bytes: Vec<u8> = s.as_bytes().to_vec();
        bytes.push(0);
        let mut esc = String::new();
        for &b in &bytes {
            match b {
                b'\\' => esc.push_str("\\5C"),
                b'"' => esc.push_str("\\22"),
                0x20..=0x7E => esc.push(b as char),
                _ => {
                    let _ = write!(esc, "\\{:02X}", b);
                }
            }
        }
        let _ = writeln!(
            out,
            "@.s{} = private unnamed_addr constant [{} x i8] c\"{}\", align 1",
            i,
            bytes.len(),
            esc
        );
    }
    if !m.strings.is_empty() {
        let _ = writeln!(out);
    }

    // Emit provider and field name string constants
    let (provider_names, field_names) = collect_provider_strings(m);
    let has_provider_strings = !provider_names.is_empty() || !field_names.is_empty();

    // Declare provider runtime functions if providers are used
    if has_provider_strings {
        let _ = writeln!(out, "; --- Provider runtime function declarations ---");
        // Provider new: (type_name_ptr, type_name_len, field_count) -> handle
        let _ = writeln!(out, "declare i64 @arth_rt_provider_new(ptr, i64, i64)");
        // Provider field get named: (handle, field_name_ptr, field_name_len) -> value
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_provider_field_get_named(i64, ptr, i64)"
        );
        // Provider field set named: (handle, field_name_ptr, field_name_len, value) -> handle
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_provider_field_set_named(i64, ptr, i64, i64)"
        );
        let _ = writeln!(out);
    }
    if has_provider_strings {
        let _ = writeln!(out, "; --- Provider/field name string constants ---");
    }
    for name in &provider_names {
        let safe_name = name.replace('.', "_");
        let bytes = name.as_bytes();
        let mut esc = String::new();
        for &b in bytes {
            match b {
                b'\\' => esc.push_str("\\5C"),
                b'"' => esc.push_str("\\22"),
                0x20..=0x7E => esc.push(b as char),
                _ => {
                    let _ = write!(esc, "\\{:02X}", b);
                }
            }
        }
        let _ = writeln!(
            out,
            "@.str.provider.{} = private unnamed_addr constant [{} x i8] c\"{}\", align 1",
            safe_name,
            bytes.len(),
            esc
        );
    }
    for (provider, field) in &field_names {
        let safe_provider = provider.replace('.', "_");
        let bytes = field.as_bytes();
        let mut esc = String::new();
        for &b in bytes {
            match b {
                b'\\' => esc.push_str("\\5C"),
                b'"' => esc.push_str("\\22"),
                0x20..=0x7E => esc.push(b as char),
                _ => {
                    let _ = write!(esc, "\\{:02X}", b);
                }
            }
        }
        let _ = writeln!(
            out,
            "@.str.field.{}.{} = private unnamed_addr constant [{} x i8] c\"{}\", align 1",
            safe_provider,
            field,
            bytes.len(),
            esc
        );
    }
    if has_provider_strings {
        let _ = writeln!(out);
    }
    for f in &m.funcs {
        emit_func(&mut out, f, m, &type_registry, &mangle_map, None);
    }
    out
}

/// Emit complete LLVM IR text with debug metadata.
///
/// The `debug` builder accumulates metadata nodes during emission.
/// After all functions are emitted, the debug metadata section is appended.
pub fn emit_module_text_with_debug(
    m: &Module,
    target_triple: Option<&str>,
    debug: &mut DebugInfoBuilder,
) -> String {
    // Reuse the header/declarations emission from emit_module_text_for_target
    // by calling it to build the preamble, then override the function emission
    let mut out = String::new();
    let _ = writeln!(out, "; ModuleID = '{}'", m.name);

    if let Some(triple) = target_triple {
        if let Some(dl) = datalayout_for_triple(triple) {
            let _ = writeln!(out, "target datalayout = \"{}\"", dl);
        }
        let _ = writeln!(out, "target triple = \"{}\"", triple);
    }

    let type_registry = build_type_registry(m);
    let mangle_map = build_mangle_map(&m.funcs);
    let closure_env_layouts = build_closure_env_layouts(m, &type_registry);
    let type_defs = type_registry.emit_type_definitions();
    if !type_defs.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "; --- Struct and Enum type definitions ---");
        out.push_str(&type_defs);
    }
    let _ = writeln!(out);

    // Declare runtime intrinsics (same as non-debug path)
    let _ = writeln!(out, "; --- Runtime intrinsic declarations ---");
    let _ = writeln!(out, "declare i64 @__arth_log_emit_str(i64, ptr, ptr, ptr)");
    let _ = writeln!(out, "declare void @arth_rt_region_enter(i32)");
    let _ = writeln!(out, "declare void @arth_rt_region_exit(i32)");

    let exception_types = collect_exception_types(m);
    let has_eh = module_has_exception_handling(m) || !exception_types.is_empty();
    if has_eh {
        let _ = writeln!(out);
        let _ = writeln!(out, "; --- Exception handling declarations ---");
        let _ = writeln!(
            out,
            "declare i32 @__arth_personality_v0(i32, i32, i64, ptr, ptr)"
        );
        let _ = writeln!(out, "declare void @arth_rt_throw(i64, ptr, i64, ptr, i64)");
        let _ = writeln!(out, "declare ptr @arth_rt_begin_catch(ptr)");
        let _ = writeln!(out, "declare void @arth_rt_end_catch()");
        let _ = writeln!(out, "declare void @arth_rt_resume_unwind(ptr)");
        let _ = writeln!(out, "declare i64 @arth_rt_type_id(ptr, i64)");
        emit_exception_type_info(&mut out, &exception_types);
    }

    if module_has_closures(m) {
        let _ = writeln!(out);
        let _ = writeln!(out, "; --- Closure runtime declarations ---");
        if !closure_env_layouts.is_empty() {
            let _ = writeln!(
                out,
                "; Closure environment layouts (derived from lambda capture parameter metadata)"
            );
            for layout in &closure_env_layouts {
                let env_name = closure_env_llvm_name(&layout.func_name);
                if layout.capture_types.is_empty() {
                    let _ = writeln!(out, "%{} = type {{}}", env_name);
                } else {
                    let capture_types: Vec<String> = layout
                        .capture_types
                        .iter()
                        .map(closure_capture_llvm_ty)
                        .collect();
                    let _ = writeln!(
                        out,
                        "%{} = type {{ {} }}",
                        env_name,
                        capture_types.join(", ")
                    );
                }
                let alignments = layout
                    .capture_alignments
                    .iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(out, "; capture alignments: [{}]", alignments);
                for issue in &layout.issues {
                    let _ = writeln!(out, "; WARNING: closure layout validation: {}", issue);
                }
            }
        }
        let _ = writeln!(out, "%Closure = type {{ ptr, ptr, i64 }}");
        let _ = writeln!(out, "declare i64 @arth_rt_closure_new(ptr, i64)");
        let _ = writeln!(out, "declare void @arth_rt_closure_capture(i64, i64)");
        let _ = writeln!(out, "declare i64 @arth_rt_closure_call_0(i64)");
        let _ = writeln!(out, "declare i64 @arth_rt_closure_call_1(i64, i64)");
        let _ = writeln!(out, "declare i64 @arth_rt_closure_call_2(i64, i64, i64)");
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_3(i64, i64, i64, i64)"
        );
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_4(i64, i64, i64, i64, i64)"
        );
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_5(i64, i64, i64, i64, i64, i64)"
        );
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_6(i64, i64, i64, i64, i64, i64, i64)"
        );
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_7(i64, i64, i64, i64, i64, i64, i64, i64)"
        );
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_8(i64, i64, i64, i64, i64, i64, i64, i64, i64)"
        );
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_closure_call_variadic(i64, ptr, i64)"
        );
    }

    if module_has_string_helpers(m) {
        let _ = writeln!(out);
        let _ = writeln!(out, "; --- String helper stubs ---");
        let _ = writeln!(out, "declare i32 @strcmp(ptr, ptr)");
        let _ = writeln!(out, "define internal i1 @__arth_str_eq(ptr %a, ptr %b) {{");
        let _ = writeln!(out, "entry:");
        let _ = writeln!(out, "  %cmp = call i32 @strcmp(ptr %a, ptr %b)");
        let _ = writeln!(out, "  %eq = icmp eq i32 %cmp, 0");
        let _ = writeln!(out, "  ret i1 %eq");
        let _ = writeln!(out, "}}\n");
        let _ = writeln!(
            out,
            "define internal ptr @__arth_str_concat(ptr %a, ptr %b) {{"
        );
        let _ = writeln!(out, "entry:");
        let _ = writeln!(out, "  ret ptr %a");
        let _ = writeln!(out, "}}\n");
    }
    let _ = writeln!(out);

    let native_symbols = collect_native_symbols(m);
    if !native_symbols.is_empty() {
        let _ = writeln!(
            out,
            "; --- Native runtime function declarations (arth-rt) ---"
        );
        let mut sorted_symbols: Vec<_> = native_symbols.into_iter().collect();
        sorted_symbols.sort();
        for sym in sorted_symbols {
            if has_eh
                && matches!(
                    sym,
                    "arth_rt_throw"
                        | "arth_rt_begin_catch"
                        | "arth_rt_end_catch"
                        | "arth_rt_resume_unwind"
                        | "arth_rt_type_id"
                )
            {
                continue;
            }
            let (ret_ty, params) = native_symbol_signature(sym);
            let _ = writeln!(out, "declare {} @{}({})", ret_ty, sym, params);
        }
        let _ = writeln!(out);
    }

    if !m.extern_funcs.is_empty() {
        let _ = writeln!(out, "; --- FFI extern function declarations ---");
        for ef in &m.extern_funcs {
            let mut params_s = String::new();
            for (i, p) in ef.params.iter().enumerate() {
                if i > 0 {
                    params_s.push_str(", ");
                }
                params_s.push_str(&ty_llvm(p));
            }
            let ret_s = ty_llvm(&ef.ret);
            let _ = writeln!(out, "declare {} @{}({})", ret_s, ef.name, params_s);
        }
        let _ = writeln!(out);
    }

    let unresolved_stubs = collect_unresolved_call_stubs(m);
    if !unresolved_stubs.is_empty() {
        let _ = writeln!(out, "; --- Unresolved call stubs ---");
        for (name, ret_ty, params) in unresolved_stubs {
            let mut params_s = String::new();
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    params_s.push_str(", ");
                }
                let _ = write!(params_s, "{} %p{}", p, i);
            }
            let _ = writeln!(out, "define internal {} @{}({}) {{", ret_ty, name, params_s);
            let _ = writeln!(out, "entry:");
            match ret_ty.as_str() {
                "void" => {
                    let _ = writeln!(out, "  ret void");
                }
                "ptr" => {
                    let _ = writeln!(out, "  ret ptr null");
                }
                "double" => {
                    let _ = writeln!(out, "  ret double 0.0");
                }
                "i1" => {
                    let _ = writeln!(out, "  ret i1 false");
                }
                _ => {
                    let _ = writeln!(out, "  ret {} 0", ret_ty);
                }
            }
            let _ = writeln!(out, "}}\n");
        }
    }

    // Emit global string constants
    for (i, s) in m.strings.iter().enumerate() {
        let mut bytes: Vec<u8> = s.as_bytes().to_vec();
        bytes.push(0);
        let mut esc = String::new();
        for &b in &bytes {
            match b {
                b'\\' => esc.push_str("\\5C"),
                b'"' => esc.push_str("\\22"),
                0x20..=0x7E => esc.push(b as char),
                _ => {
                    let _ = write!(esc, "\\{:02X}", b);
                }
            }
        }
        let _ = writeln!(
            out,
            "@.s{} = private unnamed_addr constant [{} x i8] c\"{}\", align 1",
            i,
            bytes.len(),
            esc
        );
    }
    if !m.strings.is_empty() {
        let _ = writeln!(out);
    }

    let (provider_names, field_names) = collect_provider_strings(m);
    let has_provider_strings = !provider_names.is_empty() || !field_names.is_empty();
    if has_provider_strings {
        let _ = writeln!(out, "; --- Provider runtime function declarations ---");
        let _ = writeln!(out, "declare i64 @arth_rt_provider_new(ptr, i64, i64)");
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_provider_field_get_named(i64, ptr, i64)"
        );
        let _ = writeln!(
            out,
            "declare i64 @arth_rt_provider_field_set_named(i64, ptr, i64, i64)"
        );
        let _ = writeln!(out);
    }
    if has_provider_strings {
        let _ = writeln!(out, "; --- Provider/field name string constants ---");
    }
    for name in &provider_names {
        let safe_name = name.replace('.', "_");
        let bytes = name.as_bytes();
        let mut esc = String::new();
        for &b in bytes {
            match b {
                b'\\' => esc.push_str("\\5C"),
                b'"' => esc.push_str("\\22"),
                0x20..=0x7E => esc.push(b as char),
                _ => {
                    let _ = write!(esc, "\\{:02X}", b);
                }
            }
        }
        let _ = writeln!(
            out,
            "@.str.provider.{} = private unnamed_addr constant [{} x i8] c\"{}\", align 1",
            safe_name,
            bytes.len(),
            esc
        );
    }
    for (provider, field) in &field_names {
        let safe_provider = provider.replace('.', "_");
        let bytes = field.as_bytes();
        let mut esc = String::new();
        for &b in bytes {
            match b {
                b'\\' => esc.push_str("\\5C"),
                b'"' => esc.push_str("\\22"),
                0x20..=0x7E => esc.push(b as char),
                _ => {
                    let _ = write!(esc, "\\{:02X}", b);
                }
            }
        }
        let _ = writeln!(
            out,
            "@.str.field.{}.{} = private unnamed_addr constant [{} x i8] c\"{}\", align 1",
            safe_provider,
            field,
            bytes.len(),
            esc
        );
    }
    if has_provider_strings {
        let _ = writeln!(out);
    }

    // Emit functions with debug info
    for f in &m.funcs {
        emit_func(&mut out, f, m, &type_registry, &mangle_map, Some(debug));
    }

    // Append debug metadata section
    out.push_str(&debug.finish());

    out
}

// Minimal standalone LLVM IR for a native "hello world" program.
// Uses opaque pointers (modern LLVM) and calls libc puts.
pub fn emit_hello_world_ir() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "; ModuleID = 'hello'\n");
    let _ = writeln!(out, "declare i32 @puts(ptr)\n");
    let _ = writeln!(
        out,
        "@.str = private unnamed_addr constant [15 x i8] c\"Hello, world!\\0A\\00\", align 1\n"
    );
    let _ = writeln!(out, "define i32 @main() {{");
    let _ = writeln!(out, "entry:");
    let _ = writeln!(
        out,
        "  %0 = getelementptr inbounds [15 x i8], ptr @.str, i64 0, i64 0"
    );
    let _ = writeln!(out, "  %1 = call i32 @puts(ptr %0)");
    let _ = writeln!(out, "  ret i32 0");
    let _ = writeln!(out, "}}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::ir::{BlockData, Func, Linkage, Module, Terminator, Value};
    use std::path::PathBuf;

    #[test]
    fn test_native_symbol_mapping() {
        // Test that VM intrinsic names are correctly mapped to native symbols
        assert_eq!(
            native_symbol_for_call("__arth_file_open"),
            Some("arth_rt_file_open")
        );
        assert_eq!(
            native_symbol_for_call("__arth_file_close"),
            Some("arth_rt_file_close")
        );
        assert_eq!(
            native_symbol_for_call("__arth_time_now"),
            Some("arth_rt_time_now")
        );
        assert_eq!(
            native_symbol_for_call("__arth_instant_now"),
            Some("arth_rt_instant_now")
        );
        assert_eq!(
            native_symbol_for_call("__arth_console_write"),
            Some("arth_rt_console_write")
        );

        // VM-only intrinsics should return None
        assert_eq!(native_symbol_for_call("__arth_list_new"), None);
        assert_eq!(native_symbol_for_call("__arth_map_put"), None);
    }

    #[test]
    fn test_native_symbol_signature() {
        // Test that native symbols have correct LLVM signatures
        let (ret, params) = native_symbol_signature("arth_rt_file_open");
        assert_eq!(ret, "i64");
        assert!(params.contains("ptr"));

        let (ret, params) = native_symbol_signature("arth_rt_time_now");
        assert_eq!(ret, "i64");
        assert_eq!(params, ""); // No parameters

        let (ret, params) = native_symbol_signature("arth_rt_instant_elapsed");
        assert_eq!(ret, "i64");
        assert!(params.contains("i64")); // Takes handle
    }

    #[test]
    fn test_emit_module_with_native_calls() {
        // Create a minimal IR module with a call to __arth_file_open
        let mut module = Module {
            name: "test_native".to_string(),
            funcs: Vec::new(),
            strings: Vec::new(),
            extern_funcs: Vec::new(),
            providers: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
        };

        // Add a function that calls __arth_file_open
        let func = Func {
            name: "test_fn".to_string(),
            params: vec![],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "bb0".to_string(),
                span: None,
                insts: vec![Inst {
                    result: Value(0),
                    kind: InstKind::Call {
                        name: "__arth_file_open".to_string(),
                        args: vec![],
                        ret: Ty::I64,
                    },
                    span: None,
                }],
                term: Terminator::Ret(Some(Value(0))),
            }],
            linkage: Linkage::External,
            span: None,
        };
        module.funcs.push(func);

        let ir_text = emit_module_text(&module);

        // Verify that arth_rt_file_open is declared
        assert!(
            ir_text.contains("declare i64 @arth_rt_file_open"),
            "Expected arth_rt_file_open declaration in:\n{}",
            ir_text
        );

        // Verify that the call uses arth_rt_file_open, not __arth_file_open
        assert!(
            ir_text.contains("call i64 @arth_rt_file_open"),
            "Expected call to arth_rt_file_open in:\n{}",
            ir_text
        );
    }

    #[test]
    fn test_collect_native_symbols() {
        let mut module = Module {
            name: "test".to_string(),
            funcs: Vec::new(),
            strings: Vec::new(),
            extern_funcs: Vec::new(),
            providers: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
        };

        // Add a function with multiple native calls
        let func = Func {
            name: "test_fn".to_string(),
            params: vec![],
            ret: Ty::I64,
            blocks: vec![BlockData {
                name: "bb0".to_string(),
                span: None,
                insts: vec![
                    Inst {
                        result: Value(0),
                        kind: InstKind::Call {
                            name: "__arth_file_open".to_string(),
                            args: vec![],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(1),
                        kind: InstKind::Call {
                            name: "__arth_time_now".to_string(),
                            args: vec![],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                    Inst {
                        result: Value(2),
                        kind: InstKind::Call {
                            name: "__arth_list_new".to_string(), // No native symbol
                            args: vec![],
                            ret: Ty::I64,
                        },
                        span: None,
                    },
                ],
                term: Terminator::Ret(Some(Value(2))),
            }],
            linkage: Linkage::External,
            span: None,
        };
        module.funcs.push(func);

        let symbols = collect_native_symbols(&module);

        assert!(symbols.contains("arth_rt_file_open"));
        assert!(symbols.contains("arth_rt_time_now"));
        assert!(!symbols.contains("arth_rt_list_new")); // No native symbol for this
    }

    // --- Target-triple-aware emission tests ---

    #[test]
    fn test_datalayout_for_known_triples() {
        // Every supported triple must return a datalayout string.
        let known = [
            "x86_64-apple-darwin",
            "x86_64-unknown-linux-gnu",
            "x86_64-pc-windows-msvc",
            "aarch64-apple-darwin",
            "aarch64-unknown-linux-gnu",
        ];
        for triple in &known {
            assert!(
                datalayout_for_triple(triple).is_some(),
                "missing datalayout for {triple}"
            );
        }
        // Unknown triples return None (emitter omits the line).
        assert!(datalayout_for_triple("mips-unknown-linux-gnu").is_none());
    }

    #[test]
    fn test_windows_datalayout_uses_coff_mangling() {
        let dl = datalayout_for_triple("x86_64-pc-windows-msvc").unwrap();
        // COFF mangling is indicated by "m:w" in the datalayout string.
        assert!(
            dl.contains("m:w"),
            "Windows datalayout should use COFF mangling (m:w), got: {dl}"
        );
    }

    #[test]
    fn test_macos_datalayout_uses_macho_mangling() {
        let dl = datalayout_for_triple("x86_64-apple-darwin").unwrap();
        assert!(
            dl.contains("m:o"),
            "macOS datalayout should use Mach-O mangling (m:o), got: {dl}"
        );
    }

    #[test]
    fn test_linux_datalayout_uses_elf_mangling() {
        let dl = datalayout_for_triple("x86_64-unknown-linux-gnu").unwrap();
        assert!(
            dl.contains("m:e"),
            "Linux datalayout should use ELF mangling (m:e), got: {dl}"
        );
    }

    #[test]
    fn test_emit_module_text_for_target_includes_triple() {
        let m = Module::new("test_target");
        let txt = emit_module_text_for_target(&m, Some("x86_64-pc-windows-msvc"));
        assert!(
            txt.contains("target triple = \"x86_64-pc-windows-msvc\""),
            "emitted IR should contain target triple"
        );
        assert!(
            txt.contains("target datalayout = \"e-m:w"),
            "emitted IR should contain Windows datalayout"
        );
    }

    #[test]
    fn test_emit_module_text_for_target_none_omits_triple() {
        let m = Module::new("test_no_target");
        let txt = emit_module_text_for_target(&m, None);
        assert!(
            !txt.contains("target triple"),
            "no target metadata when triple is None"
        );
        assert!(
            !txt.contains("target datalayout"),
            "no datalayout when triple is None"
        );
    }

    #[test]
    fn test_emit_module_text_backward_compat() {
        // emit_module_text (no target) must produce the same output as
        // emit_module_text_for_target(_, None).
        let m = Module::new("compat");
        let old = emit_module_text(&m);
        let new = emit_module_text_for_target(&m, None);
        assert_eq!(old, new, "emit_module_text must be a thin wrapper");
    }

    #[test]
    fn test_emit_module_text_for_target_all_platforms() {
        // Verify that each supported platform produces valid target metadata.
        let triples = [
            ("x86_64-apple-darwin", "m:o"),
            ("x86_64-unknown-linux-gnu", "m:e"),
            ("x86_64-pc-windows-msvc", "m:w"),
            ("aarch64-apple-darwin", "m:o"),
            ("aarch64-unknown-linux-gnu", "m:e"),
        ];
        let m = Module::new("multi_platform");
        for (triple, mangling) in &triples {
            let txt = emit_module_text_for_target(&m, Some(triple));
            assert!(
                txt.contains(&format!("target triple = \"{}\"", triple)),
                "missing triple for {triple}"
            );
            assert!(
                txt.contains(mangling),
                "missing mangling {mangling} for {triple}"
            );
        }
    }

    #[test]
    fn llvm_text_with_debug_attaches_dbg_to_instructions() {
        use crate::compiler::codegen::llvm_debug::{DebugInfoBuilder, SourceLineTable};
        use crate::compiler::ir::Span;
        use std::sync::Arc;

        let src = "package demo;\nfun main() {\n  val x = 1 + 2;\n}\n";
        let file = Arc::new(PathBuf::from("/project/demo.arth"));

        let a = Value(0);
        let b_val = Value(1);
        let res = Value(2);
        let inst_a = Inst {
            result: a,
            kind: InstKind::ConstI64(1),
            span: Some(Span::new(file.clone(), 28, 29)),
        };
        let inst_b = Inst {
            result: b_val,
            kind: InstKind::ConstI64(2),
            span: Some(Span::new(file.clone(), 32, 33)),
        };
        let inst_add = Inst {
            result: res,
            kind: InstKind::Binary(BinOp::Add, a, b_val),
            span: Some(Span::new(file.clone(), 28, 33)),
        };
        let block = BlockData {
            name: "entry".into(),
            insts: vec![inst_a, inst_b, inst_add],
            term: Terminator::Ret(Some(res)),
            span: Some(Span::new(file.clone(), 14, 40)),
        };
        let func = Func {
            name: "main".into(),
            params: vec![],
            ret: Ty::I64,
            blocks: vec![block],
            linkage: Linkage::External,
            span: Some(Span::new(file.clone(), 14, 40)),
        };
        let mut m = Module::new("demo");
        m.funcs.push(func);

        let table = SourceLineTable::from_sources(&[(PathBuf::from("/project/demo.arth"), src)]);
        let mut dbg = DebugInfoBuilder::new("arth test", "/project", table);
        let ir = emit_module_text_with_debug(&m, None, &mut dbg);

        // Should contain debug metadata
        assert!(ir.contains("!llvm.dbg.cu"), "missing !llvm.dbg.cu");
        assert!(ir.contains("!llvm.module.flags"), "missing module flags");
        assert!(ir.contains("DICompileUnit"), "missing DICompileUnit");
        assert!(ir.contains("DISubprogram"), "missing DISubprogram");
        assert!(ir.contains("DILocation"), "missing DILocation");
        // Instructions should have !dbg annotations
        assert!(ir.contains("!dbg"), "no !dbg annotations found");
    }

    #[test]
    fn llvm_text_with_debug_emits_subprogram_on_function() {
        use crate::compiler::codegen::llvm_debug::{DebugInfoBuilder, SourceLineTable};
        use crate::compiler::ir::Span;
        use std::sync::Arc;

        let src = "fun add(a: Int, b: Int): Int { return a + b; }\n";
        let file = Arc::new(PathBuf::from("/project/math.arth"));

        let block = BlockData {
            name: "entry".into(),
            insts: vec![],
            term: Terminator::Ret(Some(Value(2))),
            span: Some(Span::new(file.clone(), 0, 47)),
        };
        let func = Func {
            name: "add".into(),
            params: vec![Ty::I64, Ty::I64],
            ret: Ty::I64,
            blocks: vec![block],
            linkage: Linkage::External,
            span: Some(Span::new(file.clone(), 0, 47)),
        };
        let mut m = Module::new("math");
        m.funcs.push(func);

        let table = SourceLineTable::from_sources(&[(PathBuf::from("/project/math.arth"), src)]);
        let mut dbg = DebugInfoBuilder::new("arth test", "/project", table);
        let ir = emit_module_text_with_debug(&m, None, &mut dbg);

        // Function definition should have !dbg (name may be mangled)
        let define_line = ir
            .lines()
            .find(|l| l.contains("define") && l.contains("i64"))
            .expect("no define line found");
        assert!(
            define_line.contains("!dbg !"),
            "function definition missing !dbg: {}",
            define_line
        );
        // Should have a subprogram for "add"
        assert!(ir.contains("name: \"add\""), "missing subprogram for add");
    }
}
