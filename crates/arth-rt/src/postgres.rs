//! PostgreSQL database operations using libpq C FFI
//!
//! This module provides C FFI wrappers for PostgreSQL operations via libpq.
//! All functions use opaque i64 handles for connections and results.

use crate::error::{ErrorCode, set_last_error};
use crate::new_handle;

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::Mutex;

// -----------------------------------------------------------------------------
// libpq C API Bindings
// -----------------------------------------------------------------------------

/// Opaque PostgreSQL connection type
#[repr(C)]
pub struct PGconn {
    _private: [u8; 0],
}

/// Opaque PostgreSQL result type
#[repr(C)]
pub struct PGresult {
    _private: [u8; 0],
}

/// PostgreSQL OID type
pub type Oid = u32;

// Connection status codes
pub const CONNECTION_OK: i32 = 0;
pub const CONNECTION_BAD: i32 = 1;

// Result status codes
pub const PGRES_EMPTY_QUERY: i32 = 0;
pub const PGRES_COMMAND_OK: i32 = 1;
pub const PGRES_TUPLES_OK: i32 = 2;
pub const PGRES_COPY_OUT: i32 = 3;
pub const PGRES_COPY_IN: i32 = 4;
pub const PGRES_BAD_RESPONSE: i32 = 5;
pub const PGRES_NONFATAL_ERROR: i32 = 6;
pub const PGRES_FATAL_ERROR: i32 = 7;
pub const PGRES_COPY_BOTH: i32 = 8;
pub const PGRES_SINGLE_TUPLE: i32 = 9;

unsafe extern "C" {
    // Connection functions
    fn PQconnectdb(conninfo: *const libc::c_char) -> *mut PGconn;
    fn PQfinish(conn: *mut PGconn);
    fn PQstatus(conn: *const PGconn) -> i32;
    fn PQerrorMessage(conn: *const PGconn) -> *const libc::c_char;

    // Query execution (synchronous)
    fn PQexec(conn: *mut PGconn, query: *const libc::c_char) -> *mut PGresult;
    fn PQexecParams(
        conn: *mut PGconn,
        command: *const libc::c_char,
        nParams: i32,
        paramTypes: *const Oid,
        paramValues: *const *const libc::c_char,
        paramLengths: *const i32,
        paramFormats: *const i32,
        resultFormat: i32,
    ) -> *mut PGresult;
    fn PQprepare(
        conn: *mut PGconn,
        stmtName: *const libc::c_char,
        query: *const libc::c_char,
        nParams: i32,
        paramTypes: *const Oid,
    ) -> *mut PGresult;
    fn PQexecPrepared(
        conn: *mut PGconn,
        stmtName: *const libc::c_char,
        nParams: i32,
        paramValues: *const *const libc::c_char,
        paramLengths: *const i32,
        paramFormats: *const i32,
        resultFormat: i32,
    ) -> *mut PGresult;

    // Async query execution
    fn PQsendQuery(conn: *mut PGconn, query: *const libc::c_char) -> i32;
    fn PQsendQueryParams(
        conn: *mut PGconn,
        command: *const libc::c_char,
        nParams: i32,
        paramTypes: *const Oid,
        paramValues: *const *const libc::c_char,
        paramLengths: *const i32,
        paramFormats: *const i32,
        resultFormat: i32,
    ) -> i32;
    fn PQsendPrepare(
        conn: *mut PGconn,
        stmtName: *const libc::c_char,
        query: *const libc::c_char,
        nParams: i32,
        paramTypes: *const Oid,
    ) -> i32;
    fn PQsendQueryPrepared(
        conn: *mut PGconn,
        stmtName: *const libc::c_char,
        nParams: i32,
        paramValues: *const *const libc::c_char,
        paramLengths: *const i32,
        paramFormats: *const i32,
        resultFormat: i32,
    ) -> i32;
    fn PQgetResult(conn: *mut PGconn) -> *mut PGresult;
    fn PQconsumeInput(conn: *mut PGconn) -> i32;
    fn PQisBusy(conn: *mut PGconn) -> i32;
    fn PQsetnonblocking(conn: *mut PGconn, arg: i32) -> i32;
    fn PQisnonblocking(conn: *const PGconn) -> i32;
    fn PQsocket(conn: *const PGconn) -> i32;
    fn PQflush(conn: *mut PGconn) -> i32;

    // Result handling
    fn PQresultStatus(res: *const PGresult) -> i32;
    fn PQntuples(res: *const PGresult) -> i32;
    fn PQnfields(res: *const PGresult) -> i32;
    fn PQfname(res: *const PGresult, field_num: i32) -> *const libc::c_char;
    fn PQftype(res: *const PGresult, field_num: i32) -> Oid;
    fn PQgetvalue(res: *const PGresult, tup_num: i32, field_num: i32) -> *const libc::c_char;
    fn PQgetisnull(res: *const PGresult, tup_num: i32, field_num: i32) -> i32;
    fn PQgetlength(res: *const PGresult, tup_num: i32, field_num: i32) -> i32;
    fn PQcmdTuples(res: *const PGresult) -> *const libc::c_char;
    fn PQclear(res: *mut PGresult);

}

// -----------------------------------------------------------------------------
// Handle Management
// -----------------------------------------------------------------------------

/// Connection handle data
struct ConnectionData {
    conn: *mut PGconn,
}

// SAFETY: PGconn pointers are thread-safe in libpq when used correctly
unsafe impl Send for ConnectionData {}

/// Result handle data
struct ResultData {
    result: *mut PGresult,
}

// SAFETY: PGresult pointers are thread-safe in libpq when used correctly
unsafe impl Send for ResultData {}

lazy_static::lazy_static! {
    static ref CONNECTIONS: Mutex<HashMap<i64, ConnectionData>> = Mutex::new(HashMap::new());
    static ref RESULTS: Mutex<HashMap<i64, ResultData>> = Mutex::new(HashMap::new());
}

// -----------------------------------------------------------------------------
// Connection Management
// -----------------------------------------------------------------------------

/// Connect to a PostgreSQL database
///
/// # Arguments
/// * `conninfo` - Connection string (UTF-8)
/// * `conninfo_len` - Length of connection string
///
/// # Returns
/// * Positive connection handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_connect(conninfo: *const u8, conninfo_len: usize) -> i64 {
    if conninfo.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    // Convert to CString
    let conninfo_slice = unsafe { std::slice::from_raw_parts(conninfo, conninfo_len) };
    let conninfo_cstr = match CString::new(conninfo_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
    };

    let conn = unsafe { PQconnectdb(conninfo_cstr.as_ptr()) };
    if conn.is_null() {
        set_last_error("Failed to allocate connection");
        return ErrorCode::Error.as_i32() as i64;
    }

    let status = unsafe { PQstatus(conn) };
    if status != CONNECTION_OK {
        // Get error message before finishing
        let errmsg = unsafe { PQerrorMessage(conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
        unsafe { PQfinish(conn) };
        return ErrorCode::ConnectionRefused.as_i32() as i64;
    }

    let handle = new_handle();
    CONNECTIONS
        .lock()
        .unwrap()
        .insert(handle, ConnectionData { conn });
    handle
}

/// Close a PostgreSQL connection
///
/// # Arguments
/// * `conn` - Connection handle
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_finish(conn: i64) -> i32 {
    let mut conns = CONNECTIONS.lock().unwrap();
    match conns.remove(&conn) {
        Some(data) => {
            unsafe { PQfinish(data.conn) };
            0
        }
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Get connection status
///
/// # Returns
/// * CONNECTION_OK (0) if connected
/// * CONNECTION_BAD (1) if not connected
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_status(conn: i64) -> i32 {
    let conns = CONNECTIONS.lock().unwrap();
    match conns.get(&conn) {
        Some(data) => unsafe { PQstatus(data.conn) },
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Get the last error message for a connection
///
/// # Arguments
/// * `conn` - Connection handle
/// * `buf` - Buffer to write error message
/// * `buf_len` - Length of buffer
///
/// # Returns
/// * Number of bytes written on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_error_message(conn: i64, buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let errmsg = unsafe { PQerrorMessage(data.conn) };
    if errmsg.is_null() {
        unsafe { *buf = 0 };
        return 0;
    }

    let msg = match unsafe { CStr::from_ptr(errmsg) }.to_str() {
        Ok(s) => s.trim(),
        Err(_) => {
            unsafe { *buf = 0 };
            return 0;
        }
    };

    let msg_bytes = msg.as_bytes();
    if msg_bytes.len() >= buf_len {
        return ErrorCode::BufferTooSmall.as_i32();
    }

    unsafe {
        std::ptr::copy_nonoverlapping(msg_bytes.as_ptr(), buf, msg_bytes.len());
        *buf.add(msg_bytes.len()) = 0;
    }

    msg_bytes.len() as i32
}

// -----------------------------------------------------------------------------
// Query Execution
// -----------------------------------------------------------------------------

/// Execute a SQL command
///
/// # Arguments
/// * `conn` - Connection handle
/// * `sql` - SQL query (UTF-8)
/// * `sql_len` - Length of SQL query
///
/// # Returns
/// * Positive result handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_exec(conn: i64, sql: *const u8, sql_len: usize) -> i64 {
    if sql.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    // Convert SQL to CString
    let sql_slice = unsafe { std::slice::from_raw_parts(sql, sql_len) };
    let sql_cstr = match CString::new(sql_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
    };

    let result = unsafe { PQexec(data.conn, sql_cstr.as_ptr()) };
    if result.is_null() {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
        return ErrorCode::DbError.as_i32() as i64;
    }

    let status = unsafe { PQresultStatus(result) };
    if status == PGRES_FATAL_ERROR || status == PGRES_BAD_RESPONSE {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
        unsafe { PQclear(result) };
        return ErrorCode::DbError.as_i32() as i64;
    }

    let handle = new_handle();
    RESULTS
        .lock()
        .unwrap()
        .insert(handle, ResultData { result });
    handle
}

/// Execute a parameterized SQL command
///
/// # Arguments
/// * `conn` - Connection handle
/// * `sql` - SQL query (UTF-8)
/// * `sql_len` - Length of SQL query
/// * `nparams` - Number of parameters
/// * `param_values` - Array of parameter value pointers
/// * `param_lengths` - Array of parameter lengths
/// * `param_formats` - Array of parameter formats (0=text, 1=binary)
///
/// # Returns
/// * Positive result handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_exec_params(
    conn: i64,
    sql: *const u8,
    sql_len: usize,
    nparams: i32,
    param_values: *const *const libc::c_char,
    param_lengths: *const i32,
    param_formats: *const i32,
) -> i64 {
    if sql.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    // Convert SQL to CString
    let sql_slice = unsafe { std::slice::from_raw_parts(sql, sql_len) };
    let sql_cstr = match CString::new(sql_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
    };

    let result = unsafe {
        PQexecParams(
            data.conn,
            sql_cstr.as_ptr(),
            nparams,
            std::ptr::null(), // paramTypes - let server infer
            param_values,
            param_lengths,
            param_formats,
            0, // resultFormat - text
        )
    };

    if result.is_null() {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
        return ErrorCode::DbError.as_i32() as i64;
    }

    let status = unsafe { PQresultStatus(result) };
    if status == PGRES_FATAL_ERROR || status == PGRES_BAD_RESPONSE {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
        unsafe { PQclear(result) };
        return ErrorCode::DbError.as_i32() as i64;
    }

    let handle = new_handle();
    RESULTS
        .lock()
        .unwrap()
        .insert(handle, ResultData { result });
    handle
}

/// Prepare a statement
///
/// # Arguments
/// * `conn` - Connection handle
/// * `name` - Statement name (UTF-8)
/// * `name_len` - Length of statement name
/// * `sql` - SQL query (UTF-8)
/// * `sql_len` - Length of SQL query
/// * `nparams` - Number of parameters
///
/// # Returns
/// * Positive result handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_prepare(
    conn: i64,
    name: *const u8,
    name_len: usize,
    sql: *const u8,
    sql_len: usize,
    nparams: i32,
) -> i64 {
    if sql.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    // Convert name to CString
    let name_cstr = if name.is_null() || name_len == 0 {
        CString::new("").unwrap()
    } else {
        let name_slice = unsafe { std::slice::from_raw_parts(name, name_len) };
        match CString::new(name_slice) {
            Ok(s) => s,
            Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
        }
    };

    // Convert SQL to CString
    let sql_slice = unsafe { std::slice::from_raw_parts(sql, sql_len) };
    let sql_cstr = match CString::new(sql_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
    };

    let result = unsafe {
        PQprepare(
            data.conn,
            name_cstr.as_ptr(),
            sql_cstr.as_ptr(),
            nparams,
            std::ptr::null(), // paramTypes - let server infer
        )
    };

    if result.is_null() {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
        return ErrorCode::DbError.as_i32() as i64;
    }

    let status = unsafe { PQresultStatus(result) };
    if status == PGRES_FATAL_ERROR || status == PGRES_BAD_RESPONSE {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
        unsafe { PQclear(result) };
        return ErrorCode::DbError.as_i32() as i64;
    }

    let handle = new_handle();
    RESULTS
        .lock()
        .unwrap()
        .insert(handle, ResultData { result });
    handle
}

/// Execute a prepared statement
///
/// # Arguments
/// * `conn` - Connection handle
/// * `name` - Statement name (UTF-8)
/// * `name_len` - Length of statement name
/// * `nparams` - Number of parameters
/// * `param_values` - Array of parameter value pointers
/// * `param_lengths` - Array of parameter lengths
/// * `param_formats` - Array of parameter formats (0=text, 1=binary)
///
/// # Returns
/// * Positive result handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_exec_prepared(
    conn: i64,
    name: *const u8,
    name_len: usize,
    nparams: i32,
    param_values: *const *const libc::c_char,
    param_lengths: *const i32,
    param_formats: *const i32,
) -> i64 {
    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    // Convert name to CString
    let name_cstr = if name.is_null() || name_len == 0 {
        CString::new("").unwrap()
    } else {
        let name_slice = unsafe { std::slice::from_raw_parts(name, name_len) };
        match CString::new(name_slice) {
            Ok(s) => s,
            Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
        }
    };

    let result = unsafe {
        PQexecPrepared(
            data.conn,
            name_cstr.as_ptr(),
            nparams,
            param_values,
            param_lengths,
            param_formats,
            0, // resultFormat - text
        )
    };

    if result.is_null() {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
        return ErrorCode::DbError.as_i32() as i64;
    }

    let status = unsafe { PQresultStatus(result) };
    if status == PGRES_FATAL_ERROR || status == PGRES_BAD_RESPONSE {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
        unsafe { PQclear(result) };
        return ErrorCode::DbError.as_i32() as i64;
    }

    let handle = new_handle();
    RESULTS
        .lock()
        .unwrap()
        .insert(handle, ResultData { result });
    handle
}

// -----------------------------------------------------------------------------
// Result Handling
// -----------------------------------------------------------------------------

/// Get result status
///
/// # Returns
/// * Result status code (PGRES_*)
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_result_status(result: i64) -> i32 {
    let results = RESULTS.lock().unwrap();
    match results.get(&result) {
        Some(data) => unsafe { PQresultStatus(data.result) },
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Get number of rows in result
///
/// # Returns
/// * Number of rows (>= 0) on success
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_ntuples(result: i64) -> i32 {
    let results = RESULTS.lock().unwrap();
    match results.get(&result) {
        Some(data) => unsafe { PQntuples(data.result) },
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Get number of columns in result
///
/// # Returns
/// * Number of columns (>= 0) on success
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_nfields(result: i64) -> i32 {
    let results = RESULTS.lock().unwrap();
    match results.get(&result) {
        Some(data) => unsafe { PQnfields(data.result) },
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Get column name
///
/// # Arguments
/// * `result` - Result handle
/// * `col` - Column index (0-based)
/// * `buf` - Buffer to write column name
/// * `buf_len` - Length of buffer
///
/// # Returns
/// * Number of bytes written on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_fname(result: i64, col: i32, buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let results = RESULTS.lock().unwrap();
    let data = match results.get(&result) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let name = unsafe { PQfname(data.result, col) };
    if name.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let name_str = match unsafe { CStr::from_ptr(name) }.to_str() {
        Ok(s) => s,
        Err(_) => return ErrorCode::Error.as_i32(),
    };

    let name_bytes = name_str.as_bytes();
    if name_bytes.len() >= buf_len {
        return ErrorCode::BufferTooSmall.as_i32();
    }

    unsafe {
        std::ptr::copy_nonoverlapping(name_bytes.as_ptr(), buf, name_bytes.len());
        *buf.add(name_bytes.len()) = 0;
    }

    name_bytes.len() as i32
}

/// Get column type OID
///
/// # Returns
/// * Type OID on success
/// * 0 if column doesn't exist
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_ftype(result: i64, col: i32) -> i32 {
    let results = RESULTS.lock().unwrap();
    match results.get(&result) {
        Some(data) => unsafe { PQftype(data.result, col) as i32 },
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Get a value from the result
///
/// # Arguments
/// * `result` - Result handle
/// * `row` - Row index (0-based)
/// * `col` - Column index (0-based)
/// * `buf` - Buffer to write value
/// * `buf_len` - Length of buffer
///
/// # Returns
/// * Number of bytes written on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_getvalue(
    result: i64,
    row: i32,
    col: i32,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let results = RESULTS.lock().unwrap();
    let data = match results.get(&result) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    // Check if null
    if unsafe { PQgetisnull(data.result, row, col) } != 0 {
        unsafe { *buf = 0 };
        return 0;
    }

    let value = unsafe { PQgetvalue(data.result, row, col) };
    if value.is_null() {
        unsafe { *buf = 0 };
        return 0;
    }

    let value_len = unsafe { PQgetlength(data.result, row, col) } as usize;
    if value_len >= buf_len {
        return ErrorCode::BufferTooSmall.as_i32();
    }

    unsafe {
        std::ptr::copy_nonoverlapping(value as *const u8, buf, value_len);
        *buf.add(value_len) = 0;
    }

    value_len as i32
}

/// Check if a value is NULL
///
/// # Returns
/// * 1 if NULL, 0 if not NULL
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_getisnull(result: i64, row: i32, col: i32) -> i32 {
    let results = RESULTS.lock().unwrap();
    match results.get(&result) {
        Some(data) => unsafe { PQgetisnull(data.result, row, col) },
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Get the length of a value
///
/// # Returns
/// * Length in bytes on success
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_getlength(result: i64, row: i32, col: i32) -> i32 {
    let results = RESULTS.lock().unwrap();
    match results.get(&result) {
        Some(data) => unsafe { PQgetlength(data.result, row, col) },
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Get the number of rows affected by a command
///
/// # Returns
/// * Number of rows affected on success (as i64)
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_cmd_tuples(result: i64) -> i64 {
    let results = RESULTS.lock().unwrap();
    let data = match results.get(&result) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    let tuples_str = unsafe { PQcmdTuples(data.result) };
    if tuples_str.is_null() {
        return 0;
    }

    let s = match unsafe { CStr::from_ptr(tuples_str) }.to_str() {
        Ok(s) => s,
        Err(_) => return 0,
    };

    s.parse::<i64>().unwrap_or(0)
}

/// Clear/free a result
///
/// # Returns
/// * 0 on success
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_clear(result: i64) -> i32 {
    let mut results = RESULTS.lock().unwrap();
    match results.remove(&result) {
        Some(data) => {
            unsafe { PQclear(data.result) };
            0
        }
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

// -----------------------------------------------------------------------------
// Transaction Helpers
// -----------------------------------------------------------------------------

/// Begin a transaction
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_begin(conn: i64) -> i64 {
    let sql = b"BEGIN";
    arth_rt_pg_exec(conn, sql.as_ptr(), sql.len())
}

/// Commit a transaction
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_commit(conn: i64) -> i64 {
    let sql = b"COMMIT";
    arth_rt_pg_exec(conn, sql.as_ptr(), sql.len())
}

/// Rollback a transaction
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_rollback(conn: i64) -> i64 {
    let sql = b"ROLLBACK";
    arth_rt_pg_exec(conn, sql.as_ptr(), sql.len())
}

// -----------------------------------------------------------------------------
// Async Query Execution
// -----------------------------------------------------------------------------

/// Send a query asynchronously (non-blocking)
///
/// # Arguments
/// * `conn` - Connection handle
/// * `sql` - SQL query (UTF-8)
/// * `sql_len` - Length of SQL query
///
/// # Returns
/// * 1 on success (query dispatched)
/// * 0 on failure (query not dispatched)
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_send_query(conn: i64, sql: *const u8, sql_len: usize) -> i32 {
    if sql.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    // Convert SQL to CString
    let sql_slice = unsafe { std::slice::from_raw_parts(sql, sql_len) };
    let sql_cstr = match CString::new(sql_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };

    let result = unsafe { PQsendQuery(data.conn, sql_cstr.as_ptr()) };
    if result == 0 {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
    }
    result
}

/// Send a parameterized query asynchronously (non-blocking)
///
/// # Returns
/// * 1 on success
/// * 0 on failure
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_send_query_params(
    conn: i64,
    sql: *const u8,
    sql_len: usize,
    nparams: i32,
    param_values: *const *const libc::c_char,
    param_lengths: *const i32,
    param_formats: *const i32,
) -> i32 {
    if sql.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let sql_slice = unsafe { std::slice::from_raw_parts(sql, sql_len) };
    let sql_cstr = match CString::new(sql_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };

    let result = unsafe {
        PQsendQueryParams(
            data.conn,
            sql_cstr.as_ptr(),
            nparams,
            std::ptr::null(), // paramTypes - let server infer
            param_values,
            param_lengths,
            param_formats,
            0, // resultFormat - text
        )
    };

    if result == 0 {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
    }
    result
}

/// Get the result of an async query
///
/// This function should be called repeatedly until it returns 0 (no more results).
/// Each call returns one result from the query.
///
/// # Returns
/// * Positive result handle if a result is available
/// * 0 if no more results (query complete)
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_get_result(conn: i64) -> i64 {
    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    let result = unsafe { PQgetResult(data.conn) };
    if result.is_null() {
        return 0; // No more results
    }

    let status = unsafe { PQresultStatus(result) };
    if status == PGRES_FATAL_ERROR || status == PGRES_BAD_RESPONSE {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
        unsafe { PQclear(result) };
        return ErrorCode::DbError.as_i32() as i64;
    }

    let handle = new_handle();
    RESULTS
        .lock()
        .unwrap()
        .insert(handle, ResultData { result });
    handle
}

/// Check if connection is busy processing a query
///
/// # Returns
/// * 1 if busy (call PQconsumeInput and try again)
/// * 0 if not busy (results are available)
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_is_busy(conn: i64) -> i32 {
    let conns = CONNECTIONS.lock().unwrap();
    match conns.get(&conn) {
        Some(data) => unsafe { PQisBusy(data.conn) },
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Consume any available input from the server
///
/// This should be called when the connection socket is readable,
/// before checking PQisBusy or calling PQgetResult.
///
/// # Returns
/// * 1 on success
/// * 0 on failure (connection is bad)
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_consume_input(conn: i64) -> i32 {
    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let result = unsafe { PQconsumeInput(data.conn) };
    if result == 0 {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
    }
    result
}

/// Get the socket file descriptor for the connection
///
/// This can be used with select/poll/epoll for async I/O.
///
/// # Returns
/// * Socket fd (>= 0) on success
/// * -1 if no socket
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_socket(conn: i64) -> i32 {
    let conns = CONNECTIONS.lock().unwrap();
    match conns.get(&conn) {
        Some(data) => unsafe { PQsocket(data.conn) },
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Set the connection to non-blocking mode
///
/// # Arguments
/// * `conn` - Connection handle
/// * `nonblocking` - 1 for non-blocking, 0 for blocking
///
/// # Returns
/// * 0 on success
/// * -1 on failure
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_set_nonblocking(conn: i64, nonblocking: i32) -> i32 {
    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    unsafe { PQsetnonblocking(data.conn, nonblocking) }
}

/// Check if the connection is in non-blocking mode
///
/// # Returns
/// * 1 if non-blocking
/// * 0 if blocking
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_is_nonblocking(conn: i64) -> i32 {
    let conns = CONNECTIONS.lock().unwrap();
    match conns.get(&conn) {
        Some(data) => unsafe { PQisnonblocking(data.conn) },
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Flush any queued output data to the server
///
/// # Returns
/// * 0 if successful (or no data to send)
/// * 1 if unable to send all data (call again when socket is writable)
/// * -1 on failure
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_flush(conn: i64) -> i32 {
    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let result = unsafe { PQflush(data.conn) };
    if result == -1 {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
    }
    result
}

/// Send a PREPARE statement asynchronously (non-blocking)
///
/// # Arguments
/// * `conn` - Connection handle
/// * `stmt_name` - Statement name (UTF-8)
/// * `stmt_name_len` - Length of statement name
/// * `sql` - SQL query to prepare (UTF-8)
/// * `sql_len` - Length of SQL query
///
/// # Returns
/// * 1 on success (prepare dispatched)
/// * 0 on failure (prepare not dispatched)
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_send_prepare(
    conn: i64,
    stmt_name: *const u8,
    stmt_name_len: usize,
    sql: *const u8,
    sql_len: usize,
) -> i32 {
    if stmt_name.is_null() || sql.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let stmt_name_slice = unsafe { std::slice::from_raw_parts(stmt_name, stmt_name_len) };
    let stmt_name_cstr = match CString::new(stmt_name_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };

    let sql_slice = unsafe { std::slice::from_raw_parts(sql, sql_len) };
    let sql_cstr = match CString::new(sql_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };

    let result = unsafe {
        PQsendPrepare(
            data.conn,
            stmt_name_cstr.as_ptr(),
            sql_cstr.as_ptr(),
            0,                // nParams - let server infer
            std::ptr::null(), // paramTypes
        )
    };

    if result == 0 {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
    }
    result
}

/// Send a prepared query execution asynchronously (non-blocking)
///
/// # Arguments
/// * `conn` - Connection handle
/// * `stmt_name` - Prepared statement name (UTF-8)
/// * `stmt_name_len` - Length of statement name
/// * `nparams` - Number of parameters
/// * `param_values` - Array of parameter values (as C strings)
/// * `param_lengths` - Array of parameter lengths
/// * `param_formats` - Array of parameter formats (0=text, 1=binary)
///
/// # Returns
/// * 1 on success (query dispatched)
/// * 0 on failure (query not dispatched)
/// * Negative error code if invalid handle
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_pg_send_query_prepared(
    conn: i64,
    stmt_name: *const u8,
    stmt_name_len: usize,
    nparams: i32,
    param_values: *const *const libc::c_char,
    param_lengths: *const i32,
    param_formats: *const i32,
) -> i32 {
    if stmt_name.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let stmt_name_slice = unsafe { std::slice::from_raw_parts(stmt_name, stmt_name_len) };
    let stmt_name_cstr = match CString::new(stmt_name_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };

    let result = unsafe {
        PQsendQueryPrepared(
            data.conn,
            stmt_name_cstr.as_ptr(),
            nparams,
            param_values,
            param_lengths,
            param_formats,
            0, // resultFormat - text
        )
    };

    if result == 0 {
        let errmsg = unsafe { PQerrorMessage(data.conn) };
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s.trim());
            }
        }
    }
    result
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // Note: PostgreSQL tests require a running PostgreSQL server
    // These tests are integration tests and should be run with:
    // cargo test --package arth-rt --features postgres -- --ignored
    // with a PostgreSQL server available

    use super::*;

    #[test]
    #[ignore] // Requires PostgreSQL server
    fn test_connect_disconnect() {
        let conninfo = b"host=localhost dbname=postgres";
        let conn = arth_rt_pg_connect(conninfo.as_ptr(), conninfo.len());
        assert!(conn > 0, "Failed to connect");

        let status = arth_rt_pg_status(conn);
        assert_eq!(status, CONNECTION_OK);

        let rc = arth_rt_pg_finish(conn);
        assert_eq!(rc, 0);
    }

    #[test]
    #[ignore] // Requires PostgreSQL server
    fn test_simple_query() {
        let conninfo = b"host=localhost dbname=postgres";
        let conn = arth_rt_pg_connect(conninfo.as_ptr(), conninfo.len());
        assert!(conn > 0);

        let sql = b"SELECT 1 as num";
        let result = arth_rt_pg_exec(conn, sql.as_ptr(), sql.len());
        assert!(result > 0, "Query failed");

        let status = arth_rt_pg_result_status(result);
        assert_eq!(status, PGRES_TUPLES_OK);

        let ntuples = arth_rt_pg_ntuples(result);
        assert_eq!(ntuples, 1);

        let nfields = arth_rt_pg_nfields(result);
        assert_eq!(nfields, 1);

        let mut buf = [0u8; 256];
        let len = arth_rt_pg_getvalue(result, 0, 0, buf.as_mut_ptr(), buf.len());
        assert!(len > 0);
        assert_eq!(&buf[..len as usize], b"1");

        arth_rt_pg_clear(result);
        arth_rt_pg_finish(conn);
    }

    #[test]
    fn test_invalid_handle() {
        let status = arth_rt_pg_status(99999);
        assert!(status < 0);

        let ntuples = arth_rt_pg_ntuples(99999);
        assert!(ntuples < 0);
    }
}
