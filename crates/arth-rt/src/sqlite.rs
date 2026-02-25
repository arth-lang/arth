//! SQLite database operations using libsqlite3 C FFI
//!
//! This module provides C FFI wrappers for SQLite3 operations.
//! All functions use opaque i64 handles for connections and statements.

use crate::error::{ErrorCode, set_last_error};
use crate::new_handle;

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::Mutex;

// -----------------------------------------------------------------------------
// SQLite3 C API Bindings
// -----------------------------------------------------------------------------

/// Opaque SQLite connection type
#[repr(C)]
pub struct sqlite3 {
    _private: [u8; 0],
}

/// Opaque SQLite statement type
#[repr(C)]
pub struct sqlite3_stmt {
    _private: [u8; 0],
}

// SQLite result codes
pub const SQLITE_OK: i32 = 0;
pub const SQLITE_ERROR: i32 = 1;
pub const SQLITE_BUSY: i32 = 5;
pub const SQLITE_LOCKED: i32 = 6;
pub const SQLITE_NOMEM: i32 = 7;
pub const SQLITE_READONLY: i32 = 8;
pub const SQLITE_CONSTRAINT: i32 = 19;
pub const SQLITE_MISMATCH: i32 = 20;
pub const SQLITE_MISUSE: i32 = 21;
pub const SQLITE_ROW: i32 = 100;
pub const SQLITE_DONE: i32 = 101;

// SQLite column types
pub const SQLITE_INTEGER: i32 = 1;
pub const SQLITE_FLOAT: i32 = 2;
pub const SQLITE_TEXT: i32 = 3;
pub const SQLITE_BLOB: i32 = 4;
pub const SQLITE_NULL: i32 = 5;

// SQLite open flags
pub const SQLITE_OPEN_READONLY: i32 = 0x00000001;
pub const SQLITE_OPEN_READWRITE: i32 = 0x00000002;
pub const SQLITE_OPEN_CREATE: i32 = 0x00000004;
pub const SQLITE_OPEN_URI: i32 = 0x00000040;
pub const SQLITE_OPEN_MEMORY: i32 = 0x00000080;
pub const SQLITE_OPEN_NOMUTEX: i32 = 0x00008000;
pub const SQLITE_OPEN_FULLMUTEX: i32 = 0x00010000;

// Transient destructor for sqlite3_bind_text/blob
pub const SQLITE_TRANSIENT: isize = -1;

unsafe extern "C" {
    fn sqlite3_open(filename: *const libc::c_char, ppDb: *mut *mut sqlite3) -> i32;
    fn sqlite3_open_v2(
        filename: *const libc::c_char,
        ppDb: *mut *mut sqlite3,
        flags: i32,
        zVfs: *const libc::c_char,
    ) -> i32;
    fn sqlite3_close(db: *mut sqlite3) -> i32;
    fn sqlite3_errmsg(db: *mut sqlite3) -> *const libc::c_char;
    #[allow(dead_code)]
    fn sqlite3_errcode(db: *mut sqlite3) -> i32;

    fn sqlite3_prepare_v2(
        db: *mut sqlite3,
        zSql: *const libc::c_char,
        nByte: i32,
        ppStmt: *mut *mut sqlite3_stmt,
        pzTail: *mut *const libc::c_char,
    ) -> i32;
    fn sqlite3_finalize(pStmt: *mut sqlite3_stmt) -> i32;
    fn sqlite3_reset(pStmt: *mut sqlite3_stmt) -> i32;
    fn sqlite3_step(pStmt: *mut sqlite3_stmt) -> i32;
    fn sqlite3_clear_bindings(pStmt: *mut sqlite3_stmt) -> i32;

    fn sqlite3_bind_int(pStmt: *mut sqlite3_stmt, idx: i32, val: i32) -> i32;
    fn sqlite3_bind_int64(pStmt: *mut sqlite3_stmt, idx: i32, val: i64) -> i32;
    fn sqlite3_bind_double(pStmt: *mut sqlite3_stmt, idx: i32, val: f64) -> i32;
    fn sqlite3_bind_text(
        pStmt: *mut sqlite3_stmt,
        idx: i32,
        val: *const libc::c_char,
        nByte: i32,
        destructor: isize,
    ) -> i32;
    fn sqlite3_bind_blob(
        pStmt: *mut sqlite3_stmt,
        idx: i32,
        val: *const libc::c_void,
        nByte: i32,
        destructor: isize,
    ) -> i32;
    fn sqlite3_bind_null(pStmt: *mut sqlite3_stmt, idx: i32) -> i32;

    fn sqlite3_column_count(pStmt: *mut sqlite3_stmt) -> i32;
    fn sqlite3_column_type(pStmt: *mut sqlite3_stmt, iCol: i32) -> i32;
    fn sqlite3_column_name(pStmt: *mut sqlite3_stmt, iCol: i32) -> *const libc::c_char;
    fn sqlite3_column_int(pStmt: *mut sqlite3_stmt, iCol: i32) -> i32;
    fn sqlite3_column_int64(pStmt: *mut sqlite3_stmt, iCol: i32) -> i64;
    fn sqlite3_column_double(pStmt: *mut sqlite3_stmt, iCol: i32) -> f64;
    fn sqlite3_column_text(pStmt: *mut sqlite3_stmt, iCol: i32) -> *const libc::c_uchar;
    fn sqlite3_column_blob(pStmt: *mut sqlite3_stmt, iCol: i32) -> *const libc::c_void;
    fn sqlite3_column_bytes(pStmt: *mut sqlite3_stmt, iCol: i32) -> i32;

    fn sqlite3_changes(db: *mut sqlite3) -> i32;
    fn sqlite3_last_insert_rowid(db: *mut sqlite3) -> i64;
    fn sqlite3_exec(
        db: *mut sqlite3,
        sql: *const libc::c_char,
        callback: *const libc::c_void,
        arg: *mut libc::c_void,
        errmsg: *mut *mut libc::c_char,
    ) -> i32;
    fn sqlite3_free(ptr: *mut libc::c_void);
}

// -----------------------------------------------------------------------------
// Handle Management
// -----------------------------------------------------------------------------

struct ConnectionData {
    db: *mut sqlite3,
}

// SAFETY: We protect all access with a mutex
unsafe impl Send for ConnectionData {}
unsafe impl Sync for ConnectionData {}

struct StatementData {
    stmt: *mut sqlite3_stmt,
    conn_handle: i64,
}

// SAFETY: We protect all access with a mutex
unsafe impl Send for StatementData {}
unsafe impl Sync for StatementData {}

lazy_static::lazy_static! {
    static ref CONNECTIONS: Mutex<HashMap<i64, ConnectionData>> = Mutex::new(HashMap::new());
    static ref STATEMENTS: Mutex<HashMap<i64, StatementData>> = Mutex::new(HashMap::new());
}

/// Convert SQLite result code to our error code
fn sqlite_to_error_code(rc: i32) -> ErrorCode {
    match rc {
        SQLITE_OK | SQLITE_ROW | SQLITE_DONE => ErrorCode::Success,
        SQLITE_BUSY | SQLITE_LOCKED => ErrorCode::Busy,
        SQLITE_NOMEM => ErrorCode::Error,
        SQLITE_CONSTRAINT => ErrorCode::DbError, // Constraint violations are query errors
        SQLITE_MISMATCH | SQLITE_MISUSE => ErrorCode::InvalidArgument,
        _ => ErrorCode::Error,
    }
}

// -----------------------------------------------------------------------------
// Connection Management
// -----------------------------------------------------------------------------

/// Open a SQLite database
///
/// # Arguments
/// * `path` - Path to database file (UTF-8)
/// * `path_len` - Length of path
/// * `flags` - SQLite open flags (0 for default read/write/create)
///
/// # Returns
/// * Connection handle (>= 0) on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_open(path: *const u8, path_len: usize, flags: i32) -> i64 {
    if path.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    // Convert path to CString
    let path_slice = unsafe { std::slice::from_raw_parts(path, path_len) };
    let path_cstr = match CString::new(path_slice) {
        Ok(s) => s,
        Err(_) => {
            set_last_error("Invalid path (contains null byte)");
            return ErrorCode::InvalidArgument.as_i32() as i64;
        }
    };

    let mut db: *mut sqlite3 = std::ptr::null_mut();

    let rc = if flags == 0 {
        // Default: read/write/create
        unsafe { sqlite3_open(path_cstr.as_ptr(), &mut db) }
    } else {
        unsafe { sqlite3_open_v2(path_cstr.as_ptr(), &mut db, flags, std::ptr::null()) }
    };

    if rc != SQLITE_OK {
        // Get error message before closing
        if !db.is_null() {
            let errmsg = unsafe { sqlite3_errmsg(db) };
            if !errmsg.is_null()
                && let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str()
            {
                set_last_error(s);
            }
            unsafe { sqlite3_close(db) };
        }
        return sqlite_to_error_code(rc).as_i32() as i64;
    }

    let handle = new_handle();
    CONNECTIONS
        .lock()
        .unwrap()
        .insert(handle, ConnectionData { db });

    handle
}

/// Close a SQLite database connection
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_close(conn: i64) -> i32 {
    // First, finalize all statements for this connection
    {
        let mut stmts = STATEMENTS.lock().unwrap();
        let keys_to_remove: Vec<i64> = stmts
            .iter()
            .filter(|(_, data)| data.conn_handle == conn)
            .map(|(k, _)| *k)
            .collect();

        for key in keys_to_remove {
            if let Some(data) = stmts.remove(&key) {
                unsafe { sqlite3_finalize(data.stmt) };
            }
        }
    }

    // Now close the connection
    let mut conns = CONNECTIONS.lock().unwrap();
    match conns.remove(&conn) {
        Some(data) => {
            let rc = unsafe { sqlite3_close(data.db) };
            if rc != SQLITE_OK {
                return sqlite_to_error_code(rc).as_i32();
            }
            0
        }
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Get the last error message for a connection
///
/// # Returns
/// * Number of bytes written on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_errmsg(conn: i64, buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let errmsg = unsafe { sqlite3_errmsg(data.db) };
    if errmsg.is_null() {
        return 0;
    }

    let msg = unsafe { CStr::from_ptr(errmsg) };
    let msg_bytes = msg.to_bytes();
    let copy_len = msg_bytes.len().min(buf_len - 1);

    unsafe {
        std::ptr::copy_nonoverlapping(msg_bytes.as_ptr(), buf, copy_len);
        *buf.add(copy_len) = 0;
    }

    copy_len as i32
}

// -----------------------------------------------------------------------------
// Statement Preparation
// -----------------------------------------------------------------------------

/// Prepare a SQL statement
///
/// # Returns
/// * Statement handle (>= 0) on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_prepare(conn: i64, sql: *const u8, sql_len: usize) -> i64 {
    if sql.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let conns = CONNECTIONS.lock().unwrap();
    let conn_data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    let mut stmt: *mut sqlite3_stmt = std::ptr::null_mut();

    let rc = unsafe {
        sqlite3_prepare_v2(
            conn_data.db,
            sql as *const libc::c_char,
            sql_len as i32,
            &mut stmt,
            std::ptr::null_mut(),
        )
    };

    if rc != SQLITE_OK {
        let errmsg = unsafe { sqlite3_errmsg(conn_data.db) };
        if !errmsg.is_null()
            && let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str()
        {
            set_last_error(s);
        }
        return sqlite_to_error_code(rc).as_i32() as i64;
    }

    let handle = new_handle();
    drop(conns); // Release lock before acquiring STATEMENTS lock

    STATEMENTS.lock().unwrap().insert(
        handle,
        StatementData {
            stmt,
            conn_handle: conn,
        },
    );

    handle
}

/// Finalize (free) a prepared statement
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_finalize(stmt: i64) -> i32 {
    let mut stmts = STATEMENTS.lock().unwrap();
    match stmts.remove(&stmt) {
        Some(data) => {
            unsafe { sqlite3_finalize(data.stmt) };
            0
        }
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Reset a prepared statement for re-execution
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_reset(stmt: i64) -> i32 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let rc = unsafe { sqlite3_reset(data.stmt) };
    if rc != SQLITE_OK {
        return sqlite_to_error_code(rc).as_i32();
    }

    // Also clear bindings
    unsafe { sqlite3_clear_bindings(data.stmt) };

    0
}

/// Execute one step of a prepared statement
///
/// # Returns
/// * 1 (SQLITE_ROW) if a row is available
/// * 0 (SQLITE_DONE) if statement completed
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_step(stmt: i64) -> i32 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let rc = unsafe { sqlite3_step(data.stmt) };

    match rc {
        SQLITE_ROW => 1,  // Row available
        SQLITE_DONE => 0, // Done
        _ => {
            // Get error message from connection
            let conns = CONNECTIONS.lock().unwrap();
            if let Some(conn_data) = conns.get(&data.conn_handle) {
                let errmsg = unsafe { sqlite3_errmsg(conn_data.db) };
                if !errmsg.is_null()
                    && let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str()
                {
                    set_last_error(s);
                }
            }
            sqlite_to_error_code(rc).as_i32()
        }
    }
}

// -----------------------------------------------------------------------------
// Parameter Binding
// -----------------------------------------------------------------------------

/// Bind an i32 value to a statement parameter
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_bind_int(stmt: i64, idx: i32, val: i32) -> i32 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let rc = unsafe { sqlite3_bind_int(data.stmt, idx, val) };
    if rc != SQLITE_OK {
        sqlite_to_error_code(rc).as_i32()
    } else {
        0
    }
}

/// Bind an i64 value to a statement parameter
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_bind_int64(stmt: i64, idx: i32, val: i64) -> i32 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let rc = unsafe { sqlite3_bind_int64(data.stmt, idx, val) };
    if rc != SQLITE_OK {
        sqlite_to_error_code(rc).as_i32()
    } else {
        0
    }
}

/// Bind a f64 value to a statement parameter
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_bind_double(stmt: i64, idx: i32, val: f64) -> i32 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let rc = unsafe { sqlite3_bind_double(data.stmt, idx, val) };
    if rc != SQLITE_OK {
        sqlite_to_error_code(rc).as_i32()
    } else {
        0
    }
}

/// Bind a text value to a statement parameter
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_bind_text(
    stmt: i64,
    idx: i32,
    val: *const u8,
    val_len: i32,
) -> i32 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let rc = unsafe {
        sqlite3_bind_text(
            data.stmt,
            idx,
            val as *const libc::c_char,
            val_len,
            SQLITE_TRANSIENT,
        )
    };

    if rc != SQLITE_OK {
        sqlite_to_error_code(rc).as_i32()
    } else {
        0
    }
}

/// Bind a blob value to a statement parameter
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_bind_blob(
    stmt: i64,
    idx: i32,
    val: *const u8,
    val_len: i32,
) -> i32 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let rc = unsafe {
        sqlite3_bind_blob(
            data.stmt,
            idx,
            val as *const libc::c_void,
            val_len,
            SQLITE_TRANSIENT,
        )
    };

    if rc != SQLITE_OK {
        sqlite_to_error_code(rc).as_i32()
    } else {
        0
    }
}

/// Bind NULL to a statement parameter
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_bind_null(stmt: i64, idx: i32) -> i32 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let rc = unsafe { sqlite3_bind_null(data.stmt, idx) };
    if rc != SQLITE_OK {
        sqlite_to_error_code(rc).as_i32()
    } else {
        0
    }
}

// -----------------------------------------------------------------------------
// Column Access
// -----------------------------------------------------------------------------

/// Get the number of columns in a result set
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_column_count(stmt: i64) -> i32 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    unsafe { sqlite3_column_count(data.stmt) }
}

/// Get the type of a column
///
/// # Returns
/// * SQLITE_INTEGER (1), SQLITE_FLOAT (2), SQLITE_TEXT (3), SQLITE_BLOB (4), SQLITE_NULL (5)
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_column_type(stmt: i64, idx: i32) -> i32 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    unsafe { sqlite3_column_type(data.stmt, idx) }
}

/// Get the name of a column
///
/// # Returns
/// * Number of bytes written on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_column_name(
    stmt: i64,
    idx: i32,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let name = unsafe { sqlite3_column_name(data.stmt, idx) };
    if name.is_null() {
        return 0;
    }

    let name_cstr = unsafe { CStr::from_ptr(name) };
    let name_bytes = name_cstr.to_bytes();
    let copy_len = name_bytes.len().min(buf_len - 1);

    unsafe {
        std::ptr::copy_nonoverlapping(name_bytes.as_ptr(), buf, copy_len);
        *buf.add(copy_len) = 0;
    }

    copy_len as i32
}

/// Get an i32 value from a column
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_column_int(stmt: i64, idx: i32) -> i32 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return 0, // SQLite returns 0 for invalid access
    };

    unsafe { sqlite3_column_int(data.stmt, idx) }
}

/// Get an i64 value from a column
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_column_int64(stmt: i64, idx: i32) -> i64 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return 0,
    };

    unsafe { sqlite3_column_int64(data.stmt, idx) }
}

/// Get a f64 value from a column
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_column_double(stmt: i64, idx: i32) -> f64 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return 0.0,
    };

    unsafe { sqlite3_column_double(data.stmt, idx) }
}

/// Get a text value from a column
///
/// # Returns
/// * Number of bytes written on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_column_text(
    stmt: i64,
    idx: i32,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let text = unsafe { sqlite3_column_text(data.stmt, idx) };
    if text.is_null() {
        // NULL value - return 0 bytes
        unsafe { *buf = 0 };
        return 0;
    }

    let len = unsafe { sqlite3_column_bytes(data.stmt, idx) } as usize;
    let copy_len = len.min(buf_len - 1);

    unsafe {
        std::ptr::copy_nonoverlapping(text, buf, copy_len);
        *buf.add(copy_len) = 0;
    }

    if len > buf_len - 1 {
        // Data was truncated
        ErrorCode::BufferTooSmall.as_i32()
    } else {
        copy_len as i32
    }
}

/// Get a blob value from a column
///
/// # Returns
/// * Number of bytes written on success (may be 0 for empty blob)
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_column_blob(
    stmt: i64,
    idx: i32,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    if buf.is_null() && buf_len > 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let blob = unsafe { sqlite3_column_blob(data.stmt, idx) };
    let len = unsafe { sqlite3_column_bytes(data.stmt, idx) } as usize;

    if blob.is_null() || len == 0 {
        return 0;
    }

    let copy_len = len.min(buf_len);
    unsafe {
        std::ptr::copy_nonoverlapping(blob as *const u8, buf, copy_len);
    }

    if len > buf_len {
        ErrorCode::BufferTooSmall.as_i32()
    } else {
        copy_len as i32
    }
}

/// Check if a column value is NULL
///
/// # Returns
/// * 1 if NULL, 0 if not NULL
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_is_null(stmt: i64, idx: i32) -> i32 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let col_type = unsafe { sqlite3_column_type(data.stmt, idx) };
    if col_type == SQLITE_NULL { 1 } else { 0 }
}

/// Get the size of a blob/text column in bytes
///
/// # Returns
/// * Size in bytes on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_column_bytes(stmt: i64, idx: i32) -> i32 {
    let stmts = STATEMENTS.lock().unwrap();
    let data = match stmts.get(&stmt) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    unsafe { sqlite3_column_bytes(data.stmt, idx) }
}

// -----------------------------------------------------------------------------
// Utility Functions
// -----------------------------------------------------------------------------

/// Get the number of rows changed by the last INSERT/UPDATE/DELETE
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_changes(conn: i64) -> i64 {
    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    unsafe { sqlite3_changes(data.db) as i64 }
}

/// Get the rowid of the last inserted row
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_last_insert_rowid(conn: i64) -> i64 {
    let conns = CONNECTIONS.lock().unwrap();
    let data = match conns.get(&conn) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    unsafe { sqlite3_last_insert_rowid(data.db) }
}

/// Execute a SQL statement directly (no result set)
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
#[allow(clippy::collapsible_if)] // Can't collapse: sqlite3_free needs to run regardless of CStr conversion
pub extern "C" fn arth_rt_sqlite_execute(conn: i64, sql: *const u8, sql_len: usize) -> i32 {
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

    let mut errmsg: *mut libc::c_char = std::ptr::null_mut();

    let rc = unsafe {
        sqlite3_exec(
            data.db,
            sql_cstr.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut errmsg,
        )
    };

    if rc != SQLITE_OK {
        if !errmsg.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(errmsg) }.to_str() {
                set_last_error(s);
            }
            unsafe { sqlite3_free(errmsg as *mut libc::c_void) };
        }
        return sqlite_to_error_code(rc).as_i32();
    }

    0
}

// -----------------------------------------------------------------------------
// Transaction Helpers
// -----------------------------------------------------------------------------

/// Begin a transaction
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_begin(conn: i64) -> i32 {
    let sql = b"BEGIN TRANSACTION";
    arth_rt_sqlite_execute(conn, sql.as_ptr(), sql.len())
}

/// Commit a transaction
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_commit(conn: i64) -> i32 {
    let sql = b"COMMIT";
    arth_rt_sqlite_execute(conn, sql.as_ptr(), sql.len())
}

/// Rollback a transaction
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_rollback(conn: i64) -> i32 {
    let sql = b"ROLLBACK";
    arth_rt_sqlite_execute(conn, sql.as_ptr(), sql.len())
}

/// Create a savepoint
///
/// # Arguments
/// * `conn` - Connection handle
/// * `name` - Savepoint name (UTF-8)
/// * `name_len` - Length of name
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_savepoint(conn: i64, name: *const u8, name_len: usize) -> i32 {
    if name.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let name_slice = unsafe { std::slice::from_raw_parts(name, name_len) };
    let name_str = match std::str::from_utf8(name_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };

    let sql = format!("SAVEPOINT {}", name_str);
    let sql_bytes = sql.as_bytes();
    arth_rt_sqlite_execute(conn, sql_bytes.as_ptr(), sql_bytes.len())
}

/// Release a savepoint
///
/// # Arguments
/// * `conn` - Connection handle
/// * `name` - Savepoint name (UTF-8)
/// * `name_len` - Length of name
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_release_savepoint(
    conn: i64,
    name: *const u8,
    name_len: usize,
) -> i32 {
    if name.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let name_slice = unsafe { std::slice::from_raw_parts(name, name_len) };
    let name_str = match std::str::from_utf8(name_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };

    let sql = format!("RELEASE SAVEPOINT {}", name_str);
    let sql_bytes = sql.as_bytes();
    arth_rt_sqlite_execute(conn, sql_bytes.as_ptr(), sql_bytes.len())
}

/// Rollback to a savepoint
///
/// # Arguments
/// * `conn` - Connection handle
/// * `name` - Savepoint name (UTF-8)
/// * `name_len` - Length of name
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_sqlite_rollback_to_savepoint(
    conn: i64,
    name: *const u8,
    name_len: usize,
) -> i32 {
    if name.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let name_slice = unsafe { std::slice::from_raw_parts(name, name_len) };
    let name_str = match std::str::from_utf8(name_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };

    let sql = format!("ROLLBACK TO SAVEPOINT {}", name_str);
    let sql_bytes = sql.as_bytes();
    arth_rt_sqlite_execute(conn, sql_bytes.as_ptr(), sql_bytes.len())
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_close_memory() {
        let path = b":memory:";
        let conn = arth_rt_sqlite_open(path.as_ptr(), path.len(), 0);
        assert!(conn >= 0, "Failed to open database: {}", conn);

        let rc = arth_rt_sqlite_close(conn);
        assert_eq!(rc, 0, "Failed to close database");
    }

    #[test]
    fn test_execute_and_query() {
        let path = b":memory:";
        let conn = arth_rt_sqlite_open(path.as_ptr(), path.len(), 0);
        assert!(conn >= 0);

        // Create table
        let sql = b"CREATE TABLE test (id INTEGER PRIMARY KEY, name TEXT)";
        let rc = arth_rt_sqlite_execute(conn, sql.as_ptr(), sql.len());
        assert_eq!(rc, 0, "Failed to create table");

        // Insert data
        let sql = b"INSERT INTO test (name) VALUES ('hello')";
        let rc = arth_rt_sqlite_execute(conn, sql.as_ptr(), sql.len());
        assert_eq!(rc, 0, "Failed to insert data");

        // Query data
        let sql = b"SELECT id, name FROM test";
        let stmt = arth_rt_sqlite_prepare(conn, sql.as_ptr(), sql.len());
        assert!(stmt >= 0, "Failed to prepare statement: {}", stmt);

        // Check column count
        let count = arth_rt_sqlite_column_count(stmt);
        assert_eq!(count, 2);

        // Step to first row
        let rc = arth_rt_sqlite_step(stmt);
        assert_eq!(rc, 1, "Expected row");

        // Get values
        let id = arth_rt_sqlite_column_int64(stmt, 0);
        assert_eq!(id, 1);

        let mut buf = [0u8; 32];
        let len = arth_rt_sqlite_column_text(stmt, 1, buf.as_mut_ptr(), buf.len());
        assert!(len >= 0);
        let name = std::str::from_utf8(&buf[..len as usize]).unwrap();
        assert_eq!(name, "hello");

        // Step to end
        let rc = arth_rt_sqlite_step(stmt);
        assert_eq!(rc, 0, "Expected done");

        arth_rt_sqlite_finalize(stmt);
        arth_rt_sqlite_close(conn);
    }

    #[test]
    fn test_bind_parameters() {
        let path = b":memory:";
        let conn = arth_rt_sqlite_open(path.as_ptr(), path.len(), 0);
        assert!(conn >= 0);

        let sql = b"CREATE TABLE test (i INTEGER, r REAL, t TEXT, b BLOB)";
        arth_rt_sqlite_execute(conn, sql.as_ptr(), sql.len());

        let sql = b"INSERT INTO test VALUES (?, ?, ?, ?)";
        let stmt = arth_rt_sqlite_prepare(conn, sql.as_ptr(), sql.len());
        assert!(stmt >= 0);

        // Bind values
        arth_rt_sqlite_bind_int64(stmt, 1, 42);
        arth_rt_sqlite_bind_double(stmt, 2, 3.14);
        let text = b"test";
        arth_rt_sqlite_bind_text(stmt, 3, text.as_ptr(), text.len() as i32);
        let blob = [1u8, 2, 3, 4];
        arth_rt_sqlite_bind_blob(stmt, 4, blob.as_ptr(), blob.len() as i32);

        let rc = arth_rt_sqlite_step(stmt);
        assert_eq!(rc, 0, "Expected done");

        arth_rt_sqlite_finalize(stmt);

        // Verify
        let sql = b"SELECT * FROM test";
        let stmt = arth_rt_sqlite_prepare(conn, sql.as_ptr(), sql.len());
        let rc = arth_rt_sqlite_step(stmt);
        assert_eq!(rc, 1);

        assert_eq!(arth_rt_sqlite_column_int64(stmt, 0), 42);
        assert!((arth_rt_sqlite_column_double(stmt, 1) - 3.14).abs() < 0.001);

        let mut buf = [0u8; 32];
        let len = arth_rt_sqlite_column_text(stmt, 2, buf.as_mut_ptr(), buf.len());
        assert_eq!(&buf[..len as usize], b"test");

        let mut blob_buf = [0u8; 32];
        let len = arth_rt_sqlite_column_blob(stmt, 3, blob_buf.as_mut_ptr(), blob_buf.len());
        assert_eq!(&blob_buf[..len as usize], &[1, 2, 3, 4]);

        arth_rt_sqlite_finalize(stmt);
        arth_rt_sqlite_close(conn);
    }

    #[test]
    fn test_null_handling() {
        let path = b":memory:";
        let conn = arth_rt_sqlite_open(path.as_ptr(), path.len(), 0);
        assert!(conn >= 0);

        let create_sql = b"CREATE TABLE test (val TEXT)";
        let rc = arth_rt_sqlite_execute(conn, create_sql.as_ptr(), create_sql.len());
        assert_eq!(rc, 0, "Failed to create table");

        let sql = b"INSERT INTO test VALUES (?)";
        let stmt = arth_rt_sqlite_prepare(conn, sql.as_ptr(), sql.len());
        assert!(stmt >= 0, "Failed to prepare insert: {}", stmt);
        arth_rt_sqlite_bind_null(stmt, 1);
        let rc = arth_rt_sqlite_step(stmt);
        assert_eq!(rc, 0, "Expected done for insert");
        arth_rt_sqlite_finalize(stmt);

        let sql = b"SELECT val FROM test";
        let stmt = arth_rt_sqlite_prepare(conn, sql.as_ptr(), sql.len());
        assert!(stmt >= 0, "Failed to prepare select: {}", stmt);
        let rc = arth_rt_sqlite_step(stmt);
        assert_eq!(rc, 1, "Expected row");

        assert_eq!(arth_rt_sqlite_is_null(stmt, 0), 1);
        assert_eq!(arth_rt_sqlite_column_type(stmt, 0), SQLITE_NULL);

        arth_rt_sqlite_finalize(stmt);
        arth_rt_sqlite_close(conn);
    }

    #[test]
    fn test_changes_and_rowid() {
        let path = b":memory:";
        let conn = arth_rt_sqlite_open(path.as_ptr(), path.len(), 0);
        assert!(conn >= 0);

        arth_rt_sqlite_execute(
            conn,
            b"CREATE TABLE test (id INTEGER PRIMARY KEY, val TEXT)".as_ptr(),
            52,
        );

        arth_rt_sqlite_execute(conn, b"INSERT INTO test (val) VALUES ('a')".as_ptr(), 35);
        assert_eq!(arth_rt_sqlite_changes(conn), 1);
        assert_eq!(arth_rt_sqlite_last_insert_rowid(conn), 1);

        arth_rt_sqlite_execute(conn, b"INSERT INTO test (val) VALUES ('b')".as_ptr(), 35);
        assert_eq!(arth_rt_sqlite_last_insert_rowid(conn), 2);

        arth_rt_sqlite_close(conn);
    }

    #[test]
    fn test_invalid_handle() {
        assert_eq!(arth_rt_sqlite_close(999), ErrorCode::InvalidHandle.as_i32());
        assert_eq!(
            arth_rt_sqlite_finalize(999),
            ErrorCode::InvalidHandle.as_i32()
        );
        assert_eq!(arth_rt_sqlite_step(999), ErrorCode::InvalidHandle.as_i32());
    }

    #[test]
    fn test_error_message() {
        let path = b":memory:";
        let conn = arth_rt_sqlite_open(path.as_ptr(), path.len(), 0);
        assert!(conn >= 0);

        // Execute invalid SQL
        let sql = b"SELECT * FROM nonexistent_table";
        let rc = arth_rt_sqlite_execute(conn, sql.as_ptr(), sql.len());
        assert!(rc < 0, "Expected error for nonexistent table");

        // Get error message
        let mut buf = [0u8; 256];
        let len = arth_rt_sqlite_errmsg(conn, buf.as_mut_ptr(), buf.len());
        assert!(len > 0, "Expected error message");

        let msg = std::str::from_utf8(&buf[..len as usize]).unwrap();
        assert!(
            msg.contains("no such table"),
            "Expected 'no such table' error, got: {}",
            msg
        );

        arth_rt_sqlite_close(conn);
    }
}
