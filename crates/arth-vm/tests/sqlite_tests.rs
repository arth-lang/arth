//! Integration tests for the SQLite database driver (HostDb trait).
//!
//! Tests cover:
//! - Connection management (open/close)
//! - DDL operations (CREATE TABLE)
//! - DML operations (INSERT, SELECT, UPDATE, DELETE)
//! - Prepared statements with parameter binding
//! - All supported data types
//! - Transaction management (BEGIN, COMMIT, ROLLBACK)
//! - Savepoints
//! - Error handling
//! - Capability denial (NoHostDb)

use arth_vm::{
    DbError, DbErrorKind, HostDb, NoHostDb, SqliteConnectionHandle, SqliteStatementHandle,
    StdHostDb,
};
use std::sync::Arc;

/// Helper to create a StdHostDb instance for testing.
fn create_test_db() -> Arc<StdHostDb> {
    Arc::new(StdHostDb::new())
}

// =============================================================================
// Connection Tests
// =============================================================================

#[test]
fn test_sqlite_open_close_memory() {
    let db = create_test_db();

    // Open in-memory database
    let result = db.sqlite_open(":memory:");
    assert!(result.is_ok(), "Failed to open in-memory database");

    let conn = result.unwrap();
    assert!(conn.0 > 0, "Connection handle should be positive");

    // Close the connection
    let close_result = db.sqlite_close(conn);
    assert!(close_result.is_ok(), "Failed to close connection");
}

#[test]
fn test_sqlite_open_file_database() {
    let db = create_test_db();

    // Create a temp file path
    let temp_path = std::env::temp_dir().join("arth_test_sqlite.db");
    let path_str = temp_path.to_str().unwrap();

    // Open file-based database
    let result = db.sqlite_open(path_str);
    assert!(result.is_ok(), "Failed to open file database");

    let conn = result.unwrap();

    // Close and cleanup
    db.sqlite_close(conn).unwrap();
    let _ = std::fs::remove_file(&temp_path);
}

#[test]
fn test_sqlite_close_invalid_handle() {
    let db = create_test_db();

    // Try to close an invalid handle
    let invalid_handle = SqliteConnectionHandle(99999);
    let result = db.sqlite_close(invalid_handle);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::InvalidHandle
    ));
}

#[test]
fn test_sqlite_multiple_connections() {
    let db = create_test_db();

    // Open multiple in-memory databases
    let conn1 = db.sqlite_open(":memory:").unwrap();
    let conn2 = db.sqlite_open(":memory:").unwrap();
    let conn3 = db.sqlite_open(":memory:").unwrap();

    // Each should have a unique handle
    assert_ne!(conn1.0, conn2.0);
    assert_ne!(conn2.0, conn3.0);
    assert_ne!(conn1.0, conn3.0);

    // Close all
    db.sqlite_close(conn1).unwrap();
    db.sqlite_close(conn2).unwrap();
    db.sqlite_close(conn3).unwrap();
}

// =============================================================================
// DDL Tests (CREATE TABLE, etc.)
// =============================================================================

#[test]
fn test_sqlite_create_table() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    // Create a table
    let sql = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)";
    let result = db.sqlite_execute(conn, sql);
    assert!(result.is_ok(), "Failed to create table: {:?}", result);

    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_create_table_if_not_exists() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    let sql = "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, value TEXT)";

    // Should succeed both times
    db.sqlite_execute(conn, sql).unwrap();
    db.sqlite_execute(conn, sql).unwrap();

    db.sqlite_close(conn).unwrap();
}

// =============================================================================
// DML Tests (INSERT, SELECT, UPDATE, DELETE)
// =============================================================================

#[test]
fn test_sqlite_insert_and_select() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    // Create table
    db.sqlite_execute(
        conn,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)",
    )
    .unwrap();

    // Insert data
    db.sqlite_execute(conn, "INSERT INTO users (name, age) VALUES ('Alice', 30)")
        .unwrap();
    db.sqlite_execute(conn, "INSERT INTO users (name, age) VALUES ('Bob', 25)")
        .unwrap();

    // Query data
    let stmt = db
        .sqlite_prepare(conn, "SELECT id, name, age FROM users ORDER BY id")
        .unwrap();

    // First row
    let has_row = db.sqlite_step(stmt).unwrap();
    assert!(has_row, "Expected first row");

    let id1 = db.sqlite_column_int64(stmt, 0).unwrap();
    let name1 = db.sqlite_column_text(stmt, 1).unwrap();
    let age1 = db.sqlite_column_int(stmt, 2).unwrap();

    assert_eq!(id1, 1);
    assert_eq!(name1, "Alice");
    assert_eq!(age1, 30);

    // Second row
    let has_row = db.sqlite_step(stmt).unwrap();
    assert!(has_row, "Expected second row");

    let id2 = db.sqlite_column_int64(stmt, 0).unwrap();
    let name2 = db.sqlite_column_text(stmt, 1).unwrap();
    let age2 = db.sqlite_column_int(stmt, 2).unwrap();

    assert_eq!(id2, 2);
    assert_eq!(name2, "Bob");
    assert_eq!(age2, 25);

    // No more rows
    let has_row = db.sqlite_step(stmt).unwrap();
    assert!(!has_row, "Expected no more rows");

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_update() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(
        conn,
        "CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT)",
    )
    .unwrap();
    db.sqlite_execute(conn, "INSERT INTO items (value) VALUES ('original')")
        .unwrap();

    // Update
    db.sqlite_execute(conn, "UPDATE items SET value = 'updated' WHERE id = 1")
        .unwrap();

    // Verify
    let stmt = db
        .sqlite_prepare(conn, "SELECT value FROM items WHERE id = 1")
        .unwrap();
    db.sqlite_step(stmt).unwrap();
    let value = db.sqlite_column_text(stmt, 0).unwrap();
    assert_eq!(value, "updated");

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_delete() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(
        conn,
        "CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT)",
    )
    .unwrap();
    db.sqlite_execute(conn, "INSERT INTO items (value) VALUES ('a')")
        .unwrap();
    db.sqlite_execute(conn, "INSERT INTO items (value) VALUES ('b')")
        .unwrap();

    // Delete one row
    db.sqlite_execute(conn, "DELETE FROM items WHERE id = 1")
        .unwrap();

    // Check changes count
    let changes = db.sqlite_changes(conn).unwrap();
    assert_eq!(changes, 1);

    // Verify only one row remains
    let stmt = db
        .sqlite_prepare(conn, "SELECT COUNT(*) FROM items")
        .unwrap();
    db.sqlite_step(stmt).unwrap();
    let count = db.sqlite_column_int(stmt, 0).unwrap();
    assert_eq!(count, 1);

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

// =============================================================================
// Prepared Statement Tests
// =============================================================================

#[test]
fn test_sqlite_prepared_statement_with_bindings() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(
        conn,
        "CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, price REAL)",
    )
    .unwrap();

    // Prepare INSERT with placeholders
    let stmt = db
        .sqlite_prepare(conn, "INSERT INTO products (name, price) VALUES (?, ?)")
        .unwrap();

    // Bind and execute first row
    db.sqlite_bind_text(stmt, 1, "Widget").unwrap();
    db.sqlite_bind_double(stmt, 2, 19.99).unwrap();
    db.sqlite_step(stmt).unwrap();

    // Reset and bind second row
    db.sqlite_reset(stmt).unwrap();
    db.sqlite_bind_text(stmt, 1, "Gadget").unwrap();
    db.sqlite_bind_double(stmt, 2, 29.99).unwrap();
    db.sqlite_step(stmt).unwrap();

    db.sqlite_finalize(stmt).unwrap();

    // Verify
    let query = db
        .sqlite_prepare(conn, "SELECT name, price FROM products ORDER BY id")
        .unwrap();

    db.sqlite_step(query).unwrap();
    assert_eq!(db.sqlite_column_text(query, 0).unwrap(), "Widget");
    assert!((db.sqlite_column_double(query, 1).unwrap() - 19.99).abs() < 0.001);

    db.sqlite_step(query).unwrap();
    assert_eq!(db.sqlite_column_text(query, 0).unwrap(), "Gadget");
    assert!((db.sqlite_column_double(query, 1).unwrap() - 29.99).abs() < 0.001);

    db.sqlite_finalize(query).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_named_parameters() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE kv (key TEXT, value INTEGER)")
        .unwrap();

    // Use named parameters
    let stmt = db
        .sqlite_prepare(conn, "INSERT INTO kv (key, value) VALUES (:key, :value)")
        .unwrap();

    // Named parameters are still bound by index (1-based)
    db.sqlite_bind_text(stmt, 1, "answer").unwrap();
    db.sqlite_bind_int(stmt, 2, 42).unwrap();
    db.sqlite_step(stmt).unwrap();

    db.sqlite_finalize(stmt).unwrap();

    // Verify
    let query = db
        .sqlite_prepare(conn, "SELECT value FROM kv WHERE key = 'answer'")
        .unwrap();
    db.sqlite_step(query).unwrap();
    assert_eq!(db.sqlite_column_int(query, 0).unwrap(), 42);

    db.sqlite_finalize(query).unwrap();
    db.sqlite_close(conn).unwrap();
}

// =============================================================================
// Data Type Tests
// =============================================================================

#[test]
fn test_sqlite_bind_all_types() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(
        conn,
        "CREATE TABLE types_test (
            int_val INTEGER,
            int64_val INTEGER,
            double_val REAL,
            text_val TEXT,
            blob_val BLOB,
            null_val TEXT
        )",
    )
    .unwrap();

    let stmt = db
        .sqlite_prepare(conn, "INSERT INTO types_test VALUES (?, ?, ?, ?, ?, ?)")
        .unwrap();

    // Bind different types
    db.sqlite_bind_int(stmt, 1, 42).unwrap();
    db.sqlite_bind_int64(stmt, 2, 9_000_000_000_i64).unwrap();
    db.sqlite_bind_double(stmt, 3, 3.14159).unwrap();
    db.sqlite_bind_text(stmt, 4, "hello").unwrap();
    db.sqlite_bind_blob(stmt, 5, &[0xDE, 0xAD, 0xBE, 0xEF])
        .unwrap();
    db.sqlite_bind_null(stmt, 6).unwrap();

    db.sqlite_step(stmt).unwrap();
    db.sqlite_finalize(stmt).unwrap();

    // Read back and verify
    let query = db.sqlite_prepare(conn, "SELECT * FROM types_test").unwrap();
    db.sqlite_step(query).unwrap();

    assert_eq!(db.sqlite_column_int(query, 0).unwrap(), 42);
    assert_eq!(db.sqlite_column_int64(query, 1).unwrap(), 9_000_000_000_i64);
    assert!((db.sqlite_column_double(query, 2).unwrap() - 3.14159).abs() < 0.00001);
    assert_eq!(db.sqlite_column_text(query, 3).unwrap(), "hello");
    assert_eq!(
        db.sqlite_column_blob(query, 4).unwrap(),
        vec![0xDE, 0xAD, 0xBE, 0xEF]
    );
    assert!(db.sqlite_is_null(query, 5).unwrap());

    db.sqlite_finalize(query).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_null_handling() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE nullable (id INTEGER, value TEXT)")
        .unwrap();
    db.sqlite_execute(conn, "INSERT INTO nullable VALUES (1, NULL)")
        .unwrap();
    db.sqlite_execute(conn, "INSERT INTO nullable VALUES (2, 'not null')")
        .unwrap();

    let stmt = db
        .sqlite_prepare(conn, "SELECT id, value FROM nullable ORDER BY id")
        .unwrap();

    // First row has NULL
    db.sqlite_step(stmt).unwrap();
    assert!(db.sqlite_is_null(stmt, 1).unwrap());

    // Second row is not NULL
    db.sqlite_step(stmt).unwrap();
    assert!(!db.sqlite_is_null(stmt, 1).unwrap());
    assert_eq!(db.sqlite_column_text(stmt, 1).unwrap(), "not null");

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_blob_roundtrip() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE blobs (data BLOB)")
        .unwrap();

    // Test various blob sizes
    let test_cases: Vec<Vec<u8>> = vec![
        vec![],                              // Empty
        vec![0x00],                          // Single byte
        vec![0xFF; 100],                     // 100 bytes of 0xFF
        (0..256).map(|i| i as u8).collect(), // All byte values
    ];

    for original in &test_cases {
        let stmt = db
            .sqlite_prepare(conn, "INSERT INTO blobs VALUES (?)")
            .unwrap();
        db.sqlite_bind_blob(stmt, 1, original).unwrap();
        db.sqlite_step(stmt).unwrap();
        db.sqlite_finalize(stmt).unwrap();
    }

    // Read back
    let query = db.sqlite_prepare(conn, "SELECT data FROM blobs").unwrap();

    for original in &test_cases {
        db.sqlite_step(query).unwrap();
        let retrieved = db.sqlite_column_blob(query, 0).unwrap();
        assert_eq!(&retrieved, original, "Blob roundtrip failed");
    }

    db.sqlite_finalize(query).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_column_type() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(
        conn,
        "CREATE TABLE typed (i INTEGER, r REAL, t TEXT, b BLOB, n)",
    )
    .unwrap();
    db.sqlite_execute(
        conn,
        "INSERT INTO typed VALUES (1, 1.5, 'text', X'AABB', NULL)",
    )
    .unwrap();

    let stmt = db.sqlite_prepare(conn, "SELECT * FROM typed").unwrap();
    db.sqlite_step(stmt).unwrap();

    // SQLite type codes: 1=INTEGER, 2=FLOAT, 3=TEXT, 4=BLOB, 5=NULL
    assert_eq!(db.sqlite_column_type(stmt, 0).unwrap(), 1); // INTEGER
    assert_eq!(db.sqlite_column_type(stmt, 1).unwrap(), 2); // FLOAT
    assert_eq!(db.sqlite_column_type(stmt, 2).unwrap(), 3); // TEXT
    assert_eq!(db.sqlite_column_type(stmt, 3).unwrap(), 4); // BLOB
    assert_eq!(db.sqlite_column_type(stmt, 4).unwrap(), 5); // NULL

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_column_count_and_names() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(
        conn,
        "CREATE TABLE meta (alpha INTEGER, beta TEXT, gamma REAL)",
    )
    .unwrap();

    let stmt = db.sqlite_prepare(conn, "SELECT * FROM meta").unwrap();

    assert_eq!(db.sqlite_column_count(stmt).unwrap(), 3);
    assert_eq!(db.sqlite_column_name(stmt, 0).unwrap(), "alpha");
    assert_eq!(db.sqlite_column_name(stmt, 1).unwrap(), "beta");
    assert_eq!(db.sqlite_column_name(stmt, 2).unwrap(), "gamma");

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

// =============================================================================
// Transaction Tests
// =============================================================================

#[test]
fn test_sqlite_transaction_commit() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE txn_test (value INTEGER)")
        .unwrap();

    // Begin transaction
    db.sqlite_begin(conn).unwrap();
    db.sqlite_execute(conn, "INSERT INTO txn_test VALUES (1)")
        .unwrap();
    db.sqlite_execute(conn, "INSERT INTO txn_test VALUES (2)")
        .unwrap();

    // Commit
    db.sqlite_commit(conn).unwrap();

    // Verify data persisted
    let stmt = db
        .sqlite_prepare(conn, "SELECT COUNT(*) FROM txn_test")
        .unwrap();
    db.sqlite_step(stmt).unwrap();
    assert_eq!(db.sqlite_column_int(stmt, 0).unwrap(), 2);

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_transaction_rollback() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE txn_test (value INTEGER)")
        .unwrap();

    // Insert one row outside transaction
    db.sqlite_execute(conn, "INSERT INTO txn_test VALUES (1)")
        .unwrap();

    // Begin transaction and insert more
    db.sqlite_begin(conn).unwrap();
    db.sqlite_execute(conn, "INSERT INTO txn_test VALUES (2)")
        .unwrap();
    db.sqlite_execute(conn, "INSERT INTO txn_test VALUES (3)")
        .unwrap();

    // Rollback - should undo the inserts in the transaction
    db.sqlite_rollback(conn).unwrap();

    // Verify only first row remains
    let stmt = db
        .sqlite_prepare(conn, "SELECT COUNT(*) FROM txn_test")
        .unwrap();
    db.sqlite_step(stmt).unwrap();
    assert_eq!(db.sqlite_column_int(stmt, 0).unwrap(), 1);

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_savepoint() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE sp_test (value INTEGER)")
        .unwrap();

    // Begin outer transaction
    db.sqlite_begin(conn).unwrap();
    db.sqlite_execute(conn, "INSERT INTO sp_test VALUES (1)")
        .unwrap();

    // Create savepoint
    db.sqlite_savepoint(conn, "sp1").unwrap();
    db.sqlite_execute(conn, "INSERT INTO sp_test VALUES (2)")
        .unwrap();

    // Rollback to savepoint (undoes the second insert)
    db.sqlite_rollback_to_savepoint(conn, "sp1").unwrap();

    // Insert different value
    db.sqlite_execute(conn, "INSERT INTO sp_test VALUES (3)")
        .unwrap();

    // Release savepoint and commit
    db.sqlite_release_savepoint(conn, "sp1").unwrap();
    db.sqlite_commit(conn).unwrap();

    // Should have values 1 and 3 (not 2)
    let stmt = db
        .sqlite_prepare(conn, "SELECT value FROM sp_test ORDER BY value")
        .unwrap();

    db.sqlite_step(stmt).unwrap();
    assert_eq!(db.sqlite_column_int(stmt, 0).unwrap(), 1);

    db.sqlite_step(stmt).unwrap();
    assert_eq!(db.sqlite_column_int(stmt, 0).unwrap(), 3);

    assert!(!db.sqlite_step(stmt).unwrap()); // No more rows

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

// =============================================================================
// Utility Function Tests
// =============================================================================

#[test]
fn test_sqlite_last_insert_rowid() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(
        conn,
        "CREATE TABLE auto_id (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT)",
    )
    .unwrap();

    db.sqlite_execute(conn, "INSERT INTO auto_id (name) VALUES ('first')")
        .unwrap();
    assert_eq!(db.sqlite_last_insert_rowid(conn).unwrap(), 1);

    db.sqlite_execute(conn, "INSERT INTO auto_id (name) VALUES ('second')")
        .unwrap();
    assert_eq!(db.sqlite_last_insert_rowid(conn).unwrap(), 2);

    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_changes() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE changes_test (value INTEGER)")
        .unwrap();

    // Insert multiple rows
    db.sqlite_execute(conn, "INSERT INTO changes_test VALUES (1), (2), (3)")
        .unwrap();
    assert_eq!(db.sqlite_changes(conn).unwrap(), 3);

    // Update some rows
    db.sqlite_execute(
        conn,
        "UPDATE changes_test SET value = value * 10 WHERE value > 1",
    )
    .unwrap();
    assert_eq!(db.sqlite_changes(conn).unwrap(), 2);

    // Delete one row
    db.sqlite_execute(conn, "DELETE FROM changes_test WHERE value = 1")
        .unwrap();
    assert_eq!(db.sqlite_changes(conn).unwrap(), 1);

    db.sqlite_close(conn).unwrap();
}

// =============================================================================
// Error Handling Tests
// =============================================================================

#[test]
fn test_sqlite_syntax_error() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    // Invalid SQL
    let result = db.sqlite_execute(conn, "SELEKT * FORM nowhere");
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(
        matches!(
            err.kind,
            DbErrorKind::PrepareError | DbErrorKind::QueryError
        ),
        "Expected PrepareError or QueryError, got {:?}",
        err.kind
    );

    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_constraint_violation() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(
        conn,
        "CREATE TABLE unique_test (id INTEGER PRIMARY KEY, value TEXT UNIQUE)",
    )
    .unwrap();
    db.sqlite_execute(conn, "INSERT INTO unique_test VALUES (1, 'unique')")
        .unwrap();

    // Try to insert duplicate
    let result = db.sqlite_execute(conn, "INSERT INTO unique_test VALUES (2, 'unique')");
    assert!(result.is_err());

    let err = result.unwrap_err();
    // Constraint violations come as QueryError
    assert!(
        matches!(err.kind, DbErrorKind::QueryError),
        "Expected QueryError, got {:?}",
        err.kind
    );

    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_invalid_statement_handle() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    let invalid_stmt = SqliteStatementHandle(99999);

    // All statement operations should fail
    assert!(db.sqlite_step(invalid_stmt).is_err());
    assert!(db.sqlite_reset(invalid_stmt).is_err());
    assert!(db.sqlite_finalize(invalid_stmt).is_err());
    assert!(db.sqlite_bind_int(invalid_stmt, 1, 42).is_err());
    assert!(db.sqlite_column_int(invalid_stmt, 0).is_err());

    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_errmsg() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    // Cause an error
    let _ = db.sqlite_execute(conn, "INVALID SQL SYNTAX HERE");

    // Get error message - this uses arth_rt C FFI to libsqlite3.
    // The important thing is it doesn't fail.
    let result = db.sqlite_errmsg(conn);
    assert!(result.is_ok(), "sqlite_errmsg should not fail");

    db.sqlite_close(conn).unwrap();
}

// =============================================================================
// NoHostDb (Capability Denial) Tests
// =============================================================================

#[test]
fn test_no_host_db_all_operations_denied() {
    let db = NoHostDb;

    // Macro to check that an operation returns CapabilityDenied
    macro_rules! check_denied {
        ($result:expr) => {
            let result = $result;
            assert!(result.is_err(), "Expected error, got Ok");
            assert!(
                matches!(result.unwrap_err().kind, DbErrorKind::CapabilityDenied),
                "Expected CapabilityDenied"
            );
        };
    }

    check_denied!(db.sqlite_open(":memory:"));

    let fake_conn = SqliteConnectionHandle(1);
    let fake_stmt = SqliteStatementHandle(1);

    check_denied!(db.sqlite_close(fake_conn));
    check_denied!(db.sqlite_prepare(fake_conn, "SELECT 1"));
    check_denied!(db.sqlite_execute(fake_conn, "SELECT 1"));
    check_denied!(db.sqlite_query(fake_conn, "SELECT 1"));
    check_denied!(db.sqlite_step(fake_stmt));
    check_denied!(db.sqlite_reset(fake_stmt));
    check_denied!(db.sqlite_finalize(fake_stmt));
    check_denied!(db.sqlite_bind_int(fake_stmt, 1, 1));
    check_denied!(db.sqlite_bind_int64(fake_stmt, 1, 1));
    check_denied!(db.sqlite_bind_double(fake_stmt, 1, 1.0));
    check_denied!(db.sqlite_bind_text(fake_stmt, 1, ""));
    check_denied!(db.sqlite_bind_blob(fake_stmt, 1, &[]));
    check_denied!(db.sqlite_bind_null(fake_stmt, 1));
    check_denied!(db.sqlite_column_int(fake_stmt, 0));
    check_denied!(db.sqlite_column_int64(fake_stmt, 0));
    check_denied!(db.sqlite_column_double(fake_stmt, 0));
    check_denied!(db.sqlite_column_text(fake_stmt, 0));
    check_denied!(db.sqlite_column_blob(fake_stmt, 0));
    check_denied!(db.sqlite_column_type(fake_stmt, 0));
    check_denied!(db.sqlite_column_count(fake_stmt));
    check_denied!(db.sqlite_column_name(fake_stmt, 0));
    check_denied!(db.sqlite_is_null(fake_stmt, 0));
    check_denied!(db.sqlite_changes(fake_conn));
    check_denied!(db.sqlite_last_insert_rowid(fake_conn));
    check_denied!(db.sqlite_errmsg(fake_conn));
    check_denied!(db.sqlite_begin(fake_conn));
    check_denied!(db.sqlite_commit(fake_conn));
    check_denied!(db.sqlite_rollback(fake_conn));
    check_denied!(db.sqlite_savepoint(fake_conn, "sp"));
    check_denied!(db.sqlite_release_savepoint(fake_conn, "sp"));
    check_denied!(db.sqlite_rollback_to_savepoint(fake_conn, "sp"));
}

// =============================================================================
// High-Level Query API Tests
// =============================================================================

#[test]
fn test_sqlite_query_convenience() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE simple (id INTEGER, name TEXT)")
        .unwrap();
    db.sqlite_execute(conn, "INSERT INTO simple VALUES (1, 'one'), (2, 'two')")
        .unwrap();

    // sqlite_query is a convenience for prepare + step loop
    let stmt = db.sqlite_query(conn, "SELECT * FROM simple").unwrap();

    let count = db.sqlite_column_count(stmt).unwrap();
    assert_eq!(count, 2);

    // Can iterate rows
    assert!(db.sqlite_step(stmt).unwrap());
    assert!(db.sqlite_step(stmt).unwrap());
    assert!(!db.sqlite_step(stmt).unwrap());

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

// =============================================================================
// Edge Case Tests
// =============================================================================

#[test]
fn test_sqlite_empty_string() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE strings (s TEXT)")
        .unwrap();

    let stmt = db
        .sqlite_prepare(conn, "INSERT INTO strings VALUES (?)")
        .unwrap();
    db.sqlite_bind_text(stmt, 1, "").unwrap();
    db.sqlite_step(stmt).unwrap();
    db.sqlite_finalize(stmt).unwrap();

    let query = db.sqlite_prepare(conn, "SELECT s FROM strings").unwrap();
    db.sqlite_step(query).unwrap();
    let s = db.sqlite_column_text(query, 0).unwrap();
    assert_eq!(s, "");
    assert!(!db.sqlite_is_null(query, 0).unwrap()); // Empty string is not NULL

    db.sqlite_finalize(query).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_unicode_text() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE unicode (text TEXT)")
        .unwrap();

    let test_strings = vec![
        "Hello, 世界",
        "Привет мир",
        "مرحبا بالعالم",
        "🎉🎊🎁",
        "한국어 테스트",
    ];

    for s in &test_strings {
        let stmt = db
            .sqlite_prepare(conn, "INSERT INTO unicode VALUES (?)")
            .unwrap();
        db.sqlite_bind_text(stmt, 1, s).unwrap();
        db.sqlite_step(stmt).unwrap();
        db.sqlite_finalize(stmt).unwrap();
    }

    let query = db.sqlite_prepare(conn, "SELECT text FROM unicode").unwrap();
    for expected in &test_strings {
        db.sqlite_step(query).unwrap();
        let actual = db.sqlite_column_text(query, 0).unwrap();
        assert_eq!(&actual, *expected);
    }

    db.sqlite_finalize(query).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_large_blob() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE large_blobs (data BLOB)")
        .unwrap();

    // Create a 1MB blob
    let large_data: Vec<u8> = (0..1_000_000).map(|i| (i % 256) as u8).collect();

    let stmt = db
        .sqlite_prepare(conn, "INSERT INTO large_blobs VALUES (?)")
        .unwrap();
    db.sqlite_bind_blob(stmt, 1, &large_data).unwrap();
    db.sqlite_step(stmt).unwrap();
    db.sqlite_finalize(stmt).unwrap();

    let query = db
        .sqlite_prepare(conn, "SELECT data FROM large_blobs")
        .unwrap();
    db.sqlite_step(query).unwrap();
    let retrieved = db.sqlite_column_blob(query, 0).unwrap();
    assert_eq!(retrieved.len(), large_data.len());
    assert_eq!(retrieved, large_data);

    db.sqlite_finalize(query).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_concurrent_statements() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE multi (id INTEGER)")
        .unwrap();
    db.sqlite_execute(conn, "INSERT INTO multi VALUES (1), (2), (3)")
        .unwrap();

    // Open multiple statements on same connection
    let stmt1 = db.sqlite_prepare(conn, "SELECT id FROM multi").unwrap();
    let stmt2 = db
        .sqlite_prepare(conn, "SELECT id FROM multi WHERE id > 1")
        .unwrap();

    // Interleave stepping
    db.sqlite_step(stmt1).unwrap();
    db.sqlite_step(stmt2).unwrap();
    db.sqlite_step(stmt1).unwrap();

    let val1 = db.sqlite_column_int(stmt1, 0).unwrap();
    let val2 = db.sqlite_column_int(stmt2, 0).unwrap();

    // stmt1 should be on row 2, stmt2 should be on first matching row (2)
    assert_eq!(val1, 2);
    assert_eq!(val2, 2);

    db.sqlite_finalize(stmt1).unwrap();
    db.sqlite_finalize(stmt2).unwrap();
    db.sqlite_close(conn).unwrap();
}

// =============================================================================
// Transaction Helper (Scope) Tests
// =============================================================================

#[test]
fn test_sqlite_tx_scope_commit_on_success() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE tx_scope_test (value INTEGER)")
        .unwrap();

    // Verify no transaction active initially
    assert!(!db.sqlite_tx_active(conn).unwrap());
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 0);

    // Begin transaction scope
    let scope_id = db.sqlite_tx_scope_begin(conn).unwrap();
    assert!(scope_id > 0, "Scope ID should be positive");

    // Verify transaction is now active
    assert!(db.sqlite_tx_active(conn).unwrap());
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 1);

    // Insert data within scope
    db.sqlite_execute(conn, "INSERT INTO tx_scope_test VALUES (1)")
        .unwrap();
    db.sqlite_execute(conn, "INSERT INTO tx_scope_test VALUES (2)")
        .unwrap();

    // End scope with success (commit)
    db.sqlite_tx_scope_end(conn, scope_id, true).unwrap();

    // Verify transaction is no longer active
    assert!(!db.sqlite_tx_active(conn).unwrap());
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 0);

    // Verify data persisted
    let stmt = db
        .sqlite_prepare(conn, "SELECT COUNT(*) FROM tx_scope_test")
        .unwrap();
    db.sqlite_step(stmt).unwrap();
    assert_eq!(db.sqlite_column_int(stmt, 0).unwrap(), 2);

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_tx_scope_rollback_on_failure() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE tx_scope_fail (value INTEGER)")
        .unwrap();

    // Insert initial data outside transaction
    db.sqlite_execute(conn, "INSERT INTO tx_scope_fail VALUES (100)")
        .unwrap();

    // Begin transaction scope
    let scope_id = db.sqlite_tx_scope_begin(conn).unwrap();
    assert!(db.sqlite_tx_active(conn).unwrap());

    // Insert more data within scope
    db.sqlite_execute(conn, "INSERT INTO tx_scope_fail VALUES (200)")
        .unwrap();
    db.sqlite_execute(conn, "INSERT INTO tx_scope_fail VALUES (300)")
        .unwrap();

    // End scope with failure (rollback)
    db.sqlite_tx_scope_end(conn, scope_id, false).unwrap();

    // Verify transaction is no longer active
    assert!(!db.sqlite_tx_active(conn).unwrap());
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 0);

    // Verify only initial data remains (200 and 300 were rolled back)
    let stmt = db
        .sqlite_prepare(conn, "SELECT COUNT(*) FROM tx_scope_fail")
        .unwrap();
    db.sqlite_step(stmt).unwrap();
    assert_eq!(db.sqlite_column_int(stmt, 0).unwrap(), 1);

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_tx_scope_nested_with_savepoints() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE tx_nested (value INTEGER)")
        .unwrap();

    // Begin outer transaction
    let outer_scope = db.sqlite_tx_scope_begin(conn).unwrap();
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 1);

    // Insert in outer scope
    db.sqlite_execute(conn, "INSERT INTO tx_nested VALUES (1)")
        .unwrap();

    // Begin nested transaction (creates savepoint)
    let inner_scope = db.sqlite_tx_scope_begin(conn).unwrap();
    assert_ne!(inner_scope, outer_scope, "Scope IDs should be unique");
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 2);

    // Insert in inner scope
    db.sqlite_execute(conn, "INSERT INTO tx_nested VALUES (2)")
        .unwrap();

    // Rollback inner scope (rolls back to savepoint)
    db.sqlite_tx_scope_end(conn, inner_scope, false).unwrap();
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 1);
    assert!(db.sqlite_tx_active(conn).unwrap());

    // Begin another nested transaction
    let inner_scope2 = db.sqlite_tx_scope_begin(conn).unwrap();
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 2);

    // Insert different value
    db.sqlite_execute(conn, "INSERT INTO tx_nested VALUES (3)")
        .unwrap();

    // Commit inner scope 2
    db.sqlite_tx_scope_end(conn, inner_scope2, true).unwrap();
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 1);

    // Commit outer scope
    db.sqlite_tx_scope_end(conn, outer_scope, true).unwrap();
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 0);
    assert!(!db.sqlite_tx_active(conn).unwrap());

    // Verify: should have values 1 and 3 (not 2)
    let stmt = db
        .sqlite_prepare(conn, "SELECT value FROM tx_nested ORDER BY value")
        .unwrap();

    db.sqlite_step(stmt).unwrap();
    assert_eq!(db.sqlite_column_int(stmt, 0).unwrap(), 1);

    db.sqlite_step(stmt).unwrap();
    assert_eq!(db.sqlite_column_int(stmt, 0).unwrap(), 3);

    assert!(!db.sqlite_step(stmt).unwrap()); // No more rows

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_tx_scope_deeply_nested() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE tx_deep (level INTEGER)")
        .unwrap();

    // Create 5 levels of nested transactions
    let mut scopes = Vec::new();
    for i in 0..5 {
        let scope = db.sqlite_tx_scope_begin(conn).unwrap();
        scopes.push(scope);
        assert_eq!(db.sqlite_tx_depth(conn).unwrap(), (i + 1) as u32);

        // Insert at each level
        db.sqlite_execute(conn, &format!("INSERT INTO tx_deep VALUES ({})", i))
            .unwrap();
    }

    // All transactions should be active
    assert!(db.sqlite_tx_active(conn).unwrap());
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 5);

    // Commit all from innermost to outermost
    for i in (0..5).rev() {
        db.sqlite_tx_scope_end(conn, scopes[i], true).unwrap();
        assert_eq!(db.sqlite_tx_depth(conn).unwrap(), i as u32);
    }

    // All committed
    assert!(!db.sqlite_tx_active(conn).unwrap());

    // Verify all 5 values present
    let stmt = db
        .sqlite_prepare(conn, "SELECT COUNT(*) FROM tx_deep")
        .unwrap();
    db.sqlite_step(stmt).unwrap();
    assert_eq!(db.sqlite_column_int(stmt, 0).unwrap(), 5);

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_tx_scope_rollback_inner_keeps_outer() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    db.sqlite_execute(conn, "CREATE TABLE tx_partial (value TEXT)")
        .unwrap();

    // Begin outer transaction
    let outer = db.sqlite_tx_scope_begin(conn).unwrap();
    db.sqlite_execute(conn, "INSERT INTO tx_partial VALUES ('outer')")
        .unwrap();

    // Begin inner transaction
    let inner = db.sqlite_tx_scope_begin(conn).unwrap();
    db.sqlite_execute(conn, "INSERT INTO tx_partial VALUES ('inner')")
        .unwrap();

    // Rollback inner (only inner work lost)
    db.sqlite_tx_scope_end(conn, inner, false).unwrap();

    // Commit outer
    db.sqlite_tx_scope_end(conn, outer, true).unwrap();

    // Verify only 'outer' was committed
    let stmt = db
        .sqlite_prepare(conn, "SELECT value FROM tx_partial")
        .unwrap();

    db.sqlite_step(stmt).unwrap();
    assert_eq!(db.sqlite_column_text(stmt, 0).unwrap(), "outer");
    assert!(!db.sqlite_step(stmt).unwrap());

    db.sqlite_finalize(stmt).unwrap();
    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_tx_depth_and_active_queries() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    // Initial state
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 0);
    assert!(!db.sqlite_tx_active(conn).unwrap());

    // After begin
    let scope1 = db.sqlite_tx_scope_begin(conn).unwrap();
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 1);
    assert!(db.sqlite_tx_active(conn).unwrap());

    // Nested
    let scope2 = db.sqlite_tx_scope_begin(conn).unwrap();
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 2);
    assert!(db.sqlite_tx_active(conn).unwrap());

    // End inner
    db.sqlite_tx_scope_end(conn, scope2, true).unwrap();
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 1);
    assert!(db.sqlite_tx_active(conn).unwrap());

    // End outer
    db.sqlite_tx_scope_end(conn, scope1, true).unwrap();
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 0);
    assert!(!db.sqlite_tx_active(conn).unwrap());

    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_tx_scope_id_mismatch() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    // Begin transaction
    let scope = db.sqlite_tx_scope_begin(conn).unwrap();

    // Try to end with wrong scope ID
    let wrong_scope = scope + 1000;
    let result = db.sqlite_tx_scope_end(conn, wrong_scope, true);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::TransactionError
    ));

    // Transaction should still be active
    assert!(db.sqlite_tx_active(conn).unwrap());
    assert_eq!(db.sqlite_tx_depth(conn).unwrap(), 1);

    // End with correct scope ID
    db.sqlite_tx_scope_end(conn, scope, true).unwrap();
    assert!(!db.sqlite_tx_active(conn).unwrap());

    db.sqlite_close(conn).unwrap();
}

#[test]
fn test_sqlite_tx_scope_operations_on_invalid_handle() {
    let db = create_test_db();
    let invalid = SqliteConnectionHandle(99999);

    // begin fails with InvalidHandle (tries to execute BEGIN on invalid connection)
    assert!(matches!(
        db.sqlite_tx_scope_begin(invalid).unwrap_err().kind,
        DbErrorKind::InvalidHandle
    ));
    // scope_end fails with TransactionError (no transaction state exists for invalid handle)
    assert!(matches!(
        db.sqlite_tx_scope_end(invalid, 1, true).unwrap_err().kind,
        DbErrorKind::TransactionError
    ));
    // depth returns 0 for invalid handle (no tx state entry)
    assert_eq!(db.sqlite_tx_depth(invalid).unwrap(), 0);
    // active returns false for invalid handle (no tx state entry)
    assert!(!db.sqlite_tx_active(invalid).unwrap());
}

#[test]
fn test_sqlite_tx_scope_no_host_db_denied() {
    let db = NoHostDb;
    let fake_conn = SqliteConnectionHandle(1);

    // All transaction scope operations should be denied
    assert!(matches!(
        db.sqlite_tx_scope_begin(fake_conn).unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));
    assert!(matches!(
        db.sqlite_tx_scope_end(fake_conn, 1, true).unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));
    assert!(matches!(
        db.sqlite_tx_depth(fake_conn).unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));
    assert!(matches!(
        db.sqlite_tx_active(fake_conn).unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));
}

#[test]
fn test_sqlite_tx_scope_multiple_connections_independent() {
    let db = create_test_db();
    let conn1 = db.sqlite_open(":memory:").unwrap();
    let conn2 = db.sqlite_open(":memory:").unwrap();

    // Begin transaction on conn1
    let scope1 = db.sqlite_tx_scope_begin(conn1).unwrap();
    assert!(db.sqlite_tx_active(conn1).unwrap());
    assert_eq!(db.sqlite_tx_depth(conn1).unwrap(), 1);

    // conn2 should not be affected
    assert!(!db.sqlite_tx_active(conn2).unwrap());
    assert_eq!(db.sqlite_tx_depth(conn2).unwrap(), 0);

    // Begin transaction on conn2
    let scope2 = db.sqlite_tx_scope_begin(conn2).unwrap();
    assert!(db.sqlite_tx_active(conn2).unwrap());
    assert_eq!(db.sqlite_tx_depth(conn2).unwrap(), 1);

    // Both independent
    assert_ne!(scope1, scope2);

    // End conn1
    db.sqlite_tx_scope_end(conn1, scope1, true).unwrap();
    assert!(!db.sqlite_tx_active(conn1).unwrap());

    // conn2 still active
    assert!(db.sqlite_tx_active(conn2).unwrap());

    db.sqlite_tx_scope_end(conn2, scope2, true).unwrap();

    db.sqlite_close(conn1).unwrap();
    db.sqlite_close(conn2).unwrap();
}

#[test]
fn test_sqlite_tx_scope_end_no_active_transaction() {
    let db = create_test_db();
    let conn = db.sqlite_open(":memory:").unwrap();

    // No transaction active
    assert!(!db.sqlite_tx_active(conn).unwrap());

    // Try to end a non-existent transaction
    let result = db.sqlite_tx_scope_end(conn, 12345, true);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::TransactionError
    ));

    db.sqlite_close(conn).unwrap();
}
