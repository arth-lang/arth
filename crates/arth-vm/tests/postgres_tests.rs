//! Integration tests for the PostgreSQL database driver (HostDb trait).
//!
//! Tests cover:
//! - Connection management (connect/disconnect)
//! - Connection status checking
//! - DDL operations (CREATE TABLE)
//! - DML operations (INSERT, SELECT, UPDATE, DELETE)
//! - Prepared statements
//! - All supported data types
//! - Transaction management (BEGIN, COMMIT, ROLLBACK)
//! - Savepoints
//! - Error handling
//! - Capability denial (NoHostDb)
//!
//! NOTE: These tests require a running PostgreSQL server. Set the
//! `ARTH_PG_TEST_URL` environment variable to run these tests:
//!
//! ```bash
//! export ARTH_PG_TEST_URL="host=localhost user=postgres dbname=arth_test"
//! cargo test --package arth-vm --test postgres_tests -- --ignored
//! ```

use arth_vm::{DbErrorKind, HostDb, NoHostDb, PgConnectionHandle, PgResultHandle, StdHostDb};
use std::sync::Arc;

/// Get the PostgreSQL connection string from environment.
/// Returns None if not set.
fn get_test_connection_string() -> Option<String> {
    std::env::var("ARTH_PG_TEST_URL").ok()
}

/// Helper to create a StdHostDb instance for testing.
fn create_test_db() -> Arc<StdHostDb> {
    Arc::new(StdHostDb::new())
}

/// Helper to connect to test database.
/// Panics if connection fails.
fn connect_test_db(db: &StdHostDb) -> PgConnectionHandle {
    let conn_str = get_test_connection_string()
        .expect("ARTH_PG_TEST_URL environment variable must be set for PostgreSQL tests");
    db.pg_connect(&conn_str)
        .expect("Failed to connect to test database")
}

// =============================================================================
// Unit Tests (No PostgreSQL server required)
// =============================================================================

#[test]
fn test_pg_capability_denied() {
    let db = NoHostDb;

    // All PostgreSQL operations should fail with CapabilityDenied
    let result = db.pg_connect("host=localhost");
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));
}

#[test]
fn test_pg_invalid_handle() {
    let db = create_test_db();

    // Operations on invalid handles should fail
    let invalid_conn = PgConnectionHandle(99999);
    let invalid_result = PgResultHandle(99999);

    // pg_status on invalid handle
    let result = db.pg_status(invalid_conn);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::InvalidHandle
    ));

    // pg_disconnect on invalid handle
    let result = db.pg_disconnect(invalid_conn);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::InvalidHandle
    ));

    // pg_row_count on invalid result
    let result = db.pg_row_count(invalid_result);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::InvalidHandle
    ));
}

// =============================================================================
// Connection Tests (Require PostgreSQL server)
// =============================================================================

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_connect_disconnect() {
    let db = create_test_db();

    let conn = connect_test_db(&db);
    assert!(conn.0 > 0, "Connection handle should be positive");

    // Check connection status
    let status = db.pg_status(conn).unwrap();
    assert!(status, "Connection should be healthy");

    // Disconnect
    let result = db.pg_disconnect(conn);
    assert!(result.is_ok(), "Failed to disconnect");
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_connect_invalid_url() {
    let db = create_test_db();

    // Try to connect with invalid connection string
    let result = db.pg_connect("host=nonexistent_host_12345 connect_timeout=1");
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::ConnectionError
    ));
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_multiple_connections() {
    let db = create_test_db();

    let conn1 = connect_test_db(&db);
    let conn2 = connect_test_db(&db);
    let conn3 = connect_test_db(&db);

    // Each should have a unique handle
    assert_ne!(conn1.0, conn2.0);
    assert_ne!(conn2.0, conn3.0);
    assert_ne!(conn1.0, conn3.0);

    // Close all
    db.pg_disconnect(conn1).unwrap();
    db.pg_disconnect(conn2).unwrap();
    db.pg_disconnect(conn3).unwrap();
}

// =============================================================================
// Query Tests (Require PostgreSQL server)
// =============================================================================

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_create_table_and_insert() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Create table
    let create_result = db.pg_execute(
        conn,
        "CREATE TEMP TABLE pg_test_users (
            id SERIAL PRIMARY KEY,
            name TEXT NOT NULL,
            age INTEGER,
            score DOUBLE PRECISION,
            active BOOLEAN DEFAULT true
        )",
        &[],
    );
    assert!(create_result.is_ok(), "Failed to create table");

    // Insert data
    let insert_result = db.pg_execute(
        conn,
        "INSERT INTO pg_test_users (name, age, score, active) VALUES ('Alice', 30, 95.5, true)",
        &[],
    );
    assert!(insert_result.is_ok(), "Failed to insert data");
    assert_eq!(insert_result.unwrap(), 1);

    // Insert more data
    let insert_result = db.pg_execute(
        conn,
        "INSERT INTO pg_test_users (name, age, score, active) VALUES ('Bob', 25, 88.0, false)",
        &[],
    );
    assert!(insert_result.is_ok());

    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_query_and_result_access() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Create and populate table
    db.pg_execute(
        conn,
        "CREATE TEMP TABLE pg_test_query (id INTEGER, name TEXT, value DOUBLE PRECISION)",
        &[],
    )
    .unwrap();
    db.pg_execute(
        conn,
        "INSERT INTO pg_test_query VALUES (1, 'first', 10.5), (2, 'second', 20.5), (3, 'third', 30.5)",
        &[],
    )
    .unwrap();

    // Query data
    let result = db
        .pg_query(conn, "SELECT * FROM pg_test_query ORDER BY id", &[])
        .unwrap();

    // Check row count
    let row_count = db.pg_row_count(result).unwrap();
    assert_eq!(row_count, 3);

    // Check column count
    let col_count = db.pg_column_count(result).unwrap();
    assert_eq!(col_count, 3);

    // Check column names
    assert_eq!(db.pg_column_name(result, 0).unwrap(), "id");
    assert_eq!(db.pg_column_name(result, 1).unwrap(), "name");
    assert_eq!(db.pg_column_name(result, 2).unwrap(), "value");

    // Access data
    let id = db.pg_get_int(result, 0, 0).unwrap();
    assert_eq!(id, 1);

    let name = db.pg_get_text(result, 0, 1).unwrap();
    assert_eq!(name, "first");

    let value = db.pg_get_double(result, 0, 2).unwrap();
    assert!((value - 10.5).abs() < 0.001);

    // Check second row
    let id2 = db.pg_get_int(result, 1, 0).unwrap();
    assert_eq!(id2, 2);

    // Free result
    db.pg_free_result(result).unwrap();
    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_null_handling() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Create table with nullable columns
    db.pg_execute(
        conn,
        "CREATE TEMP TABLE pg_test_nulls (id INTEGER, nullable_text TEXT, nullable_int INTEGER)",
        &[],
    )
    .unwrap();

    // Insert row with nulls
    db.pg_execute(
        conn,
        "INSERT INTO pg_test_nulls VALUES (1, NULL, NULL)",
        &[],
    )
    .unwrap();

    // Insert row without nulls
    db.pg_execute(
        conn,
        "INSERT INTO pg_test_nulls VALUES (2, 'present', 42)",
        &[],
    )
    .unwrap();

    // Query and check null handling
    let result = db
        .pg_query(conn, "SELECT * FROM pg_test_nulls ORDER BY id", &[])
        .unwrap();

    // First row: id=1, text=NULL, int=NULL
    assert!(!db.pg_is_null(result, 0, 0).unwrap()); // id not null
    assert!(db.pg_is_null(result, 0, 1).unwrap()); // text is null
    assert!(db.pg_is_null(result, 0, 2).unwrap()); // int is null

    // Second row: id=2, text='present', int=42
    assert!(!db.pg_is_null(result, 1, 0).unwrap()); // id not null
    assert!(!db.pg_is_null(result, 1, 1).unwrap()); // text not null
    assert!(!db.pg_is_null(result, 1, 2).unwrap()); // int not null

    db.pg_free_result(result).unwrap();
    db.pg_disconnect(conn).unwrap();
}

// =============================================================================
// Transaction Tests (Require PostgreSQL server)
// =============================================================================

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_transaction_commit() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Create table
    db.pg_execute(conn, "CREATE TEMP TABLE pg_test_tx (id INTEGER)", &[])
        .unwrap();

    // Begin transaction
    db.pg_begin(conn).unwrap();

    // Insert within transaction
    db.pg_execute(conn, "INSERT INTO pg_test_tx VALUES (1)", &[])
        .unwrap();
    db.pg_execute(conn, "INSERT INTO pg_test_tx VALUES (2)", &[])
        .unwrap();

    // Commit
    db.pg_commit(conn).unwrap();

    // Verify data persisted
    let result = db
        .pg_query(conn, "SELECT COUNT(*) FROM pg_test_tx", &[])
        .unwrap();
    let count = db.pg_get_int64(result, 0, 0).unwrap();
    assert_eq!(count, 2);

    db.pg_free_result(result).unwrap();
    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_transaction_rollback() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Create table with initial data
    db.pg_execute(conn, "CREATE TEMP TABLE pg_test_rollback (id INTEGER)", &[])
        .unwrap();
    db.pg_execute(conn, "INSERT INTO pg_test_rollback VALUES (100)", &[])
        .unwrap();

    // Begin transaction
    db.pg_begin(conn).unwrap();

    // Insert within transaction
    db.pg_execute(conn, "INSERT INTO pg_test_rollback VALUES (200)", &[])
        .unwrap();

    // Rollback
    db.pg_rollback(conn).unwrap();

    // Verify only initial data exists
    let result = db
        .pg_query(conn, "SELECT COUNT(*) FROM pg_test_rollback", &[])
        .unwrap();
    let count = db.pg_get_int64(result, 0, 0).unwrap();
    assert_eq!(count, 1); // Only the initial row

    db.pg_free_result(result).unwrap();
    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_savepoint() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Create table
    db.pg_execute(
        conn,
        "CREATE TEMP TABLE pg_test_savepoint (id INTEGER)",
        &[],
    )
    .unwrap();

    // Begin transaction
    db.pg_begin(conn).unwrap();

    // Insert first row
    db.pg_execute(conn, "INSERT INTO pg_test_savepoint VALUES (1)", &[])
        .unwrap();

    // Create savepoint
    db.pg_savepoint(conn, "sp1").unwrap();

    // Insert second row
    db.pg_execute(conn, "INSERT INTO pg_test_savepoint VALUES (2)", &[])
        .unwrap();

    // Rollback to savepoint
    db.pg_rollback_to_savepoint(conn, "sp1").unwrap();

    // Commit transaction
    db.pg_commit(conn).unwrap();

    // Verify only first row exists
    let result = db
        .pg_query(conn, "SELECT COUNT(*) FROM pg_test_savepoint", &[])
        .unwrap();
    let count = db.pg_get_int64(result, 0, 0).unwrap();
    assert_eq!(count, 1);

    db.pg_free_result(result).unwrap();
    db.pg_disconnect(conn).unwrap();
}

// =============================================================================
// Prepared Statement Tests (Require PostgreSQL server)
// =============================================================================

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_prepared_statement() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Create table
    db.pg_execute(
        conn,
        "CREATE TEMP TABLE pg_test_prepared (id INTEGER, name TEXT)",
        &[],
    )
    .unwrap();

    // Prepare statement
    let stmt = db
        .pg_prepare(
            conn,
            "insert_user",
            "INSERT INTO pg_test_prepared VALUES ($1, $2)",
        )
        .unwrap();

    // Execute prepared statement (Note: params passed via execute in current impl)
    // For now, just verify preparation works
    assert!(stmt.0 > 0);

    db.pg_disconnect(conn).unwrap();
}

// =============================================================================
// Error Handling Tests (Require PostgreSQL server)
// =============================================================================

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_query_error() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Execute invalid SQL
    let result = db.pg_execute(conn, "INVALID SQL SYNTAX HERE", &[]);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err().kind, DbErrorKind::QueryError));

    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_constraint_violation() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Create table with unique constraint
    db.pg_execute(
        conn,
        "CREATE TEMP TABLE pg_test_unique (id INTEGER PRIMARY KEY)",
        &[],
    )
    .unwrap();

    // Insert first row
    db.pg_execute(conn, "INSERT INTO pg_test_unique VALUES (1)", &[])
        .unwrap();

    // Try to insert duplicate - should fail
    let result = db.pg_execute(conn, "INSERT INTO pg_test_unique VALUES (1)", &[]);
    assert!(result.is_err());
    // Constraint violations are mapped to QueryError
    let err = result.unwrap_err();
    assert!(matches!(err.kind, DbErrorKind::QueryError));

    db.pg_disconnect(conn).unwrap();
}

// =============================================================================
// Data Type Tests (Require PostgreSQL server)
// =============================================================================

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_data_types() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Create table with various types
    db.pg_execute(
        conn,
        "CREATE TEMP TABLE pg_test_types (
            int_col INTEGER,
            bigint_col BIGINT,
            float_col DOUBLE PRECISION,
            text_col TEXT,
            bool_col BOOLEAN,
            bytes_col BYTEA
        )",
        &[],
    )
    .unwrap();

    // Insert data
    db.pg_execute(
        conn,
        "INSERT INTO pg_test_types VALUES (
            42,
            9223372036854775807,
            3.14159265359,
            'hello world',
            true,
            E'\\\\x48656C6C6F'
        )",
        &[],
    )
    .unwrap();

    // Query and verify types
    let result = db
        .pg_query(conn, "SELECT * FROM pg_test_types", &[])
        .unwrap();

    // Integer
    let int_val = db.pg_get_int(result, 0, 0).unwrap();
    assert_eq!(int_val, 42);

    // BigInt
    let bigint_val = db.pg_get_int64(result, 0, 1).unwrap();
    assert_eq!(bigint_val, 9223372036854775807i64);

    // Double
    let float_val = db.pg_get_double(result, 0, 2).unwrap();
    assert!((float_val - 3.14159265359).abs() < 0.0001);

    // Text
    let text_val = db.pg_get_text(result, 0, 3).unwrap();
    assert_eq!(text_val, "hello world");

    // Boolean
    let bool_val = db.pg_get_bool(result, 0, 4).unwrap();
    assert!(bool_val);

    db.pg_free_result(result).unwrap();
    db.pg_disconnect(conn).unwrap();
}

// =============================================================================
// Utility Tests (Require PostgreSQL server)
// =============================================================================

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_escape_string() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Test escaping
    let escaped = db.pg_escape(conn, "it's a \"test\"").unwrap();
    assert!(escaped.contains("it''s")); // Single quotes are doubled

    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_errmsg_after_error() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Note: errmsg behavior depends on implementation
    // This is a basic test that it doesn't crash
    let _ = db.pg_errmsg(conn);

    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_affected_rows() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Create table with data
    db.pg_execute(conn, "CREATE TEMP TABLE pg_test_affected (id INTEGER)", &[])
        .unwrap();
    db.pg_execute(
        conn,
        "INSERT INTO pg_test_affected VALUES (1), (2), (3), (4), (5)",
        &[],
    )
    .unwrap();

    // Query to get result handle for affected_rows
    let result = db
        .pg_query(conn, "DELETE FROM pg_test_affected WHERE id > 2", &[])
        .unwrap();

    let affected = db.pg_affected_rows(result).unwrap();
    assert_eq!(affected, 3);

    db.pg_free_result(result).unwrap();
    db.pg_disconnect(conn).unwrap();
}

// =============================================================================
// Async PostgreSQL Unit Tests (No PostgreSQL server required)
// =============================================================================

#[test]
fn test_pg_async_capability_denied() {
    let db = NoHostDb;

    // All async operations should return CapabilityDenied
    let err = db.pg_connect_async("host=localhost").unwrap_err();
    assert_eq!(err.kind, DbErrorKind::CapabilityDenied);

    // Disconnect should also fail
    let handle = arth_vm::PgAsyncConnectionHandle(1);
    let err = db.pg_disconnect_async(handle).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::CapabilityDenied);

    // Status should fail
    let err = db.pg_status_async(handle).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::CapabilityDenied);

    // Query should fail
    let err = db.pg_query_async(handle, "SELECT 1", &[]).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::CapabilityDenied);

    // Execute should fail
    let err = db.pg_execute_async(handle, "SELECT 1", &[]).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::CapabilityDenied);

    // Transaction ops should fail
    let err = db.pg_begin_async(handle).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::CapabilityDenied);

    let err = db.pg_commit_async(handle).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::CapabilityDenied);

    let err = db.pg_rollback_async(handle).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::CapabilityDenied);
}

#[test]
fn test_pg_async_invalid_handle() {
    let db = create_test_db();

    // Test with an invalid connection handle
    let bad_handle = arth_vm::PgAsyncConnectionHandle(999999);

    // Status should fail
    let err = db.pg_status_async(bad_handle).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::ConnectionError);

    // Query should fail
    let err = db.pg_query_async(bad_handle, "SELECT 1", &[]).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::ConnectionError);

    // Execute should fail
    let err = db
        .pg_execute_async(bad_handle, "SELECT 1", &[])
        .unwrap_err();
    assert_eq!(err.kind, DbErrorKind::ConnectionError);

    // Transaction ops should fail
    let err = db.pg_begin_async(bad_handle).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::ConnectionError);
}

#[test]
fn test_pg_async_invalid_query_handle() {
    let db = create_test_db();

    // Test with an invalid query handle
    let bad_handle = arth_vm::PgAsyncQueryHandle(999999);

    // is_ready should fail
    let err = db.pg_is_ready(bad_handle).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::QueryError);

    // get_async_result should fail
    let err = db.pg_get_async_result(bad_handle).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::QueryError);

    // cancel should fail
    let err = db.pg_cancel_async(bad_handle).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::QueryError);
}

// =============================================================================
// Async PostgreSQL Integration Tests (Requires PostgreSQL server)
// =============================================================================

/// Helper to connect to test database asynchronously.
fn connect_test_db_async(db: &StdHostDb) -> arth_vm::PgAsyncConnectionHandle {
    let conn_str = get_test_connection_string()
        .expect("ARTH_PG_TEST_URL environment variable must be set for PostgreSQL tests");
    db.pg_connect_async(&conn_str)
        .expect("Failed to connect async to test database")
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_async_connect_disconnect() {
    let db = create_test_db();

    let conn = connect_test_db_async(&db);

    // Check connection status
    let is_connected = db.pg_status_async(conn).unwrap();
    assert!(is_connected, "Connection should be active");

    // Disconnect
    db.pg_disconnect_async(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_async_simple_query() {
    let db = create_test_db();
    let conn = connect_test_db_async(&db);

    // Execute a simple query
    let query_handle = db.pg_query_async(conn, "SELECT 1 as value", &[]).unwrap();

    // Check if ready (should complete quickly with block_on)
    let is_ready = db.pg_is_ready(query_handle).unwrap();
    assert!(is_ready, "Query should be ready");

    // Get result
    let result = db.pg_get_async_result(query_handle).unwrap();

    // Note: Due to the tokio_postgres/postgres row conversion limitation,
    // we can't directly read rows. But we can verify the result handle is valid.
    db.pg_free_result(result).unwrap();
    db.pg_disconnect_async(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_async_execute() {
    let db = create_test_db();
    let conn = connect_test_db_async(&db);

    // Create a temp table
    let query_handle = db
        .pg_execute_async(conn, "CREATE TEMP TABLE pg_async_test (id INTEGER)", &[])
        .unwrap();

    let is_ready = db.pg_is_ready(query_handle).unwrap();
    assert!(is_ready);

    // Get result to verify completion
    let result = db.pg_get_async_result(query_handle).unwrap();
    db.pg_free_result(result).unwrap();

    // Insert some data
    let insert_handle = db
        .pg_execute_async(conn, "INSERT INTO pg_async_test VALUES (1), (2), (3)", &[])
        .unwrap();
    let result = db.pg_get_async_result(insert_handle).unwrap();
    let affected = db.pg_affected_rows(result).unwrap();
    assert_eq!(affected, 3);

    db.pg_free_result(result).unwrap();
    db.pg_disconnect_async(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_async_transaction() {
    let db = create_test_db();
    let conn = connect_test_db_async(&db);

    // Begin transaction
    let begin_handle = db.pg_begin_async(conn).unwrap();
    assert!(db.pg_is_ready(begin_handle).unwrap());
    let result = db.pg_get_async_result(begin_handle).unwrap();
    db.pg_free_result(result).unwrap();

    // Create table and insert
    let create_handle = db
        .pg_execute_async(conn, "CREATE TEMP TABLE pg_async_txn (id INTEGER)", &[])
        .unwrap();
    let result = db.pg_get_async_result(create_handle).unwrap();
    db.pg_free_result(result).unwrap();

    let insert_handle = db
        .pg_execute_async(conn, "INSERT INTO pg_async_txn VALUES (42)", &[])
        .unwrap();
    let result = db.pg_get_async_result(insert_handle).unwrap();
    db.pg_free_result(result).unwrap();

    // Commit transaction
    let commit_handle = db.pg_commit_async(conn).unwrap();
    assert!(db.pg_is_ready(commit_handle).unwrap());
    let result = db.pg_get_async_result(commit_handle).unwrap();
    db.pg_free_result(result).unwrap();

    db.pg_disconnect_async(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_async_rollback() {
    let db = create_test_db();
    let conn = connect_test_db_async(&db);

    // Begin transaction
    let begin_handle = db.pg_begin_async(conn).unwrap();
    let result = db.pg_get_async_result(begin_handle).unwrap();
    db.pg_free_result(result).unwrap();

    // Create table
    let create_handle = db
        .pg_execute_async(
            conn,
            "CREATE TEMP TABLE pg_async_rollback (id INTEGER)",
            &[],
        )
        .unwrap();
    let result = db.pg_get_async_result(create_handle).unwrap();
    db.pg_free_result(result).unwrap();

    // Rollback transaction
    let rollback_handle = db.pg_rollback_async(conn).unwrap();
    assert!(db.pg_is_ready(rollback_handle).unwrap());
    let result = db.pg_get_async_result(rollback_handle).unwrap();
    db.pg_free_result(result).unwrap();

    // Table should not exist since we rolled back
    // (We can't easily check this without sync queries)

    db.pg_disconnect_async(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_async_prepare_execute() {
    let db = create_test_db();
    let conn = connect_test_db_async(&db);

    // Create table
    let create_handle = db
        .pg_execute_async(conn, "CREATE TEMP TABLE pg_async_prep (id INTEGER)", &[])
        .unwrap();
    let result = db.pg_get_async_result(create_handle).unwrap();
    db.pg_free_result(result).unwrap();

    // Prepare statement
    let prepare_handle = db
        .pg_prepare_async(
            conn,
            "async_insert_stmt",
            "INSERT INTO pg_async_prep VALUES ($1)",
        )
        .unwrap();
    assert!(db.pg_is_ready(prepare_handle).unwrap());
    let result = db.pg_get_async_result(prepare_handle).unwrap();
    db.pg_free_result(result).unwrap();

    // Execute prepared statement (note: params are not fully supported yet)
    // This is a simplified test
    let exec_handle = db
        .pg_execute_prepared_async(conn, "async_insert_stmt", &[])
        .unwrap_err(); // Will fail without proper params

    db.pg_disconnect_async(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_async_cancel() {
    let db = create_test_db();
    let conn = connect_test_db_async(&db);

    // Start a query (this will complete synchronously with block_on)
    let query_handle = db.pg_query_async(conn, "SELECT 1", &[]).unwrap();

    // Cancel should work (even if already complete)
    db.pg_cancel_async(query_handle).unwrap();

    // Query handle should now be invalid
    let err = db.pg_is_ready(query_handle).unwrap_err();
    assert_eq!(err.kind, DbErrorKind::QueryError);

    db.pg_disconnect_async(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_async_multiple_connections() {
    let db = create_test_db();

    let conn1 = connect_test_db_async(&db);
    let conn2 = connect_test_db_async(&db);

    // Both connections should be active
    assert!(db.pg_status_async(conn1).unwrap());
    assert!(db.pg_status_async(conn2).unwrap());

    // Execute on both
    let q1 = db.pg_execute_async(conn1, "SELECT 1", &[]).unwrap();
    let q2 = db.pg_execute_async(conn2, "SELECT 2", &[]).unwrap();

    // Both should complete
    assert!(db.pg_is_ready(q1).unwrap());
    assert!(db.pg_is_ready(q2).unwrap());

    let r1 = db.pg_get_async_result(q1).unwrap();
    let r2 = db.pg_get_async_result(q2).unwrap();

    db.pg_free_result(r1).unwrap();
    db.pg_free_result(r2).unwrap();

    db.pg_disconnect_async(conn1).unwrap();
    db.pg_disconnect_async(conn2).unwrap();
}

// =============================================================================
// PostgreSQL Transaction Helper (Scope) Unit Tests (No server required)
// =============================================================================

#[test]
fn test_pg_tx_scope_capability_denied() {
    let db = NoHostDb;
    let fake_conn = PgConnectionHandle(1);

    // All transaction scope operations should be denied
    assert!(matches!(
        db.pg_tx_scope_begin(fake_conn).unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));
    assert!(matches!(
        db.pg_tx_scope_end(fake_conn, 1, true).unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));
    assert!(matches!(
        db.pg_tx_depth(fake_conn).unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));
    assert!(matches!(
        db.pg_tx_active(fake_conn).unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));
}

#[test]
fn test_pg_tx_scope_invalid_handle() {
    let db = create_test_db();
    let invalid = PgConnectionHandle(99999);

    // begin fails with InvalidHandle (tries to execute BEGIN on invalid connection)
    assert!(matches!(
        db.pg_tx_scope_begin(invalid).unwrap_err().kind,
        DbErrorKind::InvalidHandle
    ));
    // scope_end fails with TransactionError (no transaction state exists for invalid handle)
    assert!(matches!(
        db.pg_tx_scope_end(invalid, 1, true).unwrap_err().kind,
        DbErrorKind::TransactionError
    ));
    // depth returns 0 for invalid handle (no tx state entry)
    assert_eq!(db.pg_tx_depth(invalid).unwrap(), 0);
    // active returns false for invalid handle (no tx state entry)
    assert!(!db.pg_tx_active(invalid).unwrap());
}

// =============================================================================
// PostgreSQL Transaction Helper Integration Tests (Require PostgreSQL server)
// =============================================================================

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_tx_scope_commit_on_success() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Create temp table
    db.pg_execute(
        conn,
        "CREATE TEMP TABLE pg_tx_scope_test (value INTEGER)",
        &[],
    )
    .unwrap();

    // Verify no transaction active initially
    assert!(!db.pg_tx_active(conn).unwrap());
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 0);

    // Begin transaction scope
    let scope_id = db.pg_tx_scope_begin(conn).unwrap();
    assert!(scope_id > 0, "Scope ID should be positive");

    // Verify transaction is now active
    assert!(db.pg_tx_active(conn).unwrap());
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 1);

    // Insert data within scope
    db.pg_execute(conn, "INSERT INTO pg_tx_scope_test VALUES (1)", &[])
        .unwrap();
    db.pg_execute(conn, "INSERT INTO pg_tx_scope_test VALUES (2)", &[])
        .unwrap();

    // End scope with success (commit)
    db.pg_tx_scope_end(conn, scope_id, true).unwrap();

    // Verify transaction is no longer active
    assert!(!db.pg_tx_active(conn).unwrap());
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 0);

    // Verify data persisted
    let result = db
        .pg_query(conn, "SELECT COUNT(*) FROM pg_tx_scope_test", &[])
        .unwrap();
    let count = db.pg_get_int64(result, 0, 0).unwrap();
    assert_eq!(count, 2);

    db.pg_free_result(result).unwrap();
    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_tx_scope_rollback_on_failure() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Create temp table
    db.pg_execute(
        conn,
        "CREATE TEMP TABLE pg_tx_scope_fail (value INTEGER)",
        &[],
    )
    .unwrap();

    // Insert initial data outside transaction
    db.pg_execute(conn, "INSERT INTO pg_tx_scope_fail VALUES (100)", &[])
        .unwrap();

    // Begin transaction scope
    let scope_id = db.pg_tx_scope_begin(conn).unwrap();
    assert!(db.pg_tx_active(conn).unwrap());

    // Insert more data within scope
    db.pg_execute(conn, "INSERT INTO pg_tx_scope_fail VALUES (200)", &[])
        .unwrap();
    db.pg_execute(conn, "INSERT INTO pg_tx_scope_fail VALUES (300)", &[])
        .unwrap();

    // End scope with failure (rollback)
    db.pg_tx_scope_end(conn, scope_id, false).unwrap();

    // Verify transaction is no longer active
    assert!(!db.pg_tx_active(conn).unwrap());
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 0);

    // Verify only initial data remains
    let result = db
        .pg_query(conn, "SELECT COUNT(*) FROM pg_tx_scope_fail", &[])
        .unwrap();
    let count = db.pg_get_int64(result, 0, 0).unwrap();
    assert_eq!(count, 1);

    db.pg_free_result(result).unwrap();
    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_tx_scope_nested_with_savepoints() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Create temp table
    db.pg_execute(conn, "CREATE TEMP TABLE pg_tx_nested (value INTEGER)", &[])
        .unwrap();

    // Begin outer transaction
    let outer_scope = db.pg_tx_scope_begin(conn).unwrap();
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 1);

    // Insert in outer scope
    db.pg_execute(conn, "INSERT INTO pg_tx_nested VALUES (1)", &[])
        .unwrap();

    // Begin nested transaction (creates savepoint)
    let inner_scope = db.pg_tx_scope_begin(conn).unwrap();
    assert_ne!(inner_scope, outer_scope, "Scope IDs should be unique");
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 2);

    // Insert in inner scope
    db.pg_execute(conn, "INSERT INTO pg_tx_nested VALUES (2)", &[])
        .unwrap();

    // Rollback inner scope (rolls back to savepoint)
    db.pg_tx_scope_end(conn, inner_scope, false).unwrap();
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 1);
    assert!(db.pg_tx_active(conn).unwrap());

    // Begin another nested transaction
    let inner_scope2 = db.pg_tx_scope_begin(conn).unwrap();
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 2);

    // Insert different value
    db.pg_execute(conn, "INSERT INTO pg_tx_nested VALUES (3)", &[])
        .unwrap();

    // Commit inner scope 2
    db.pg_tx_scope_end(conn, inner_scope2, true).unwrap();
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 1);

    // Commit outer scope
    db.pg_tx_scope_end(conn, outer_scope, true).unwrap();
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 0);
    assert!(!db.pg_tx_active(conn).unwrap());

    // Verify: should have values 1 and 3 (not 2)
    let result = db
        .pg_query(conn, "SELECT value FROM pg_tx_nested ORDER BY value", &[])
        .unwrap();

    let val1 = db.pg_get_int(result, 0, 0).unwrap();
    let val2 = db.pg_get_int(result, 1, 0).unwrap();
    assert_eq!(val1, 1);
    assert_eq!(val2, 3);

    db.pg_free_result(result).unwrap();
    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_tx_scope_deeply_nested() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    db.pg_execute(conn, "CREATE TEMP TABLE pg_tx_deep (level INTEGER)", &[])
        .unwrap();

    // Create 5 levels of nested transactions
    let mut scopes = Vec::new();
    for i in 0..5 {
        let scope = db.pg_tx_scope_begin(conn).unwrap();
        scopes.push(scope);
        assert_eq!(db.pg_tx_depth(conn).unwrap(), (i + 1) as u32);

        // Insert at each level
        db.pg_execute(conn, &format!("INSERT INTO pg_tx_deep VALUES ({})", i), &[])
            .unwrap();
    }

    // All transactions should be active
    assert!(db.pg_tx_active(conn).unwrap());
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 5);

    // Commit all from innermost to outermost
    for i in (0..5).rev() {
        db.pg_tx_scope_end(conn, scopes[i], true).unwrap();
        assert_eq!(db.pg_tx_depth(conn).unwrap(), i as u32);
    }

    // All committed
    assert!(!db.pg_tx_active(conn).unwrap());

    // Verify all 5 values present
    let result = db
        .pg_query(conn, "SELECT COUNT(*) FROM pg_tx_deep", &[])
        .unwrap();
    let count = db.pg_get_int64(result, 0, 0).unwrap();
    assert_eq!(count, 5);

    db.pg_free_result(result).unwrap();
    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_tx_scope_rollback_inner_keeps_outer() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    db.pg_execute(conn, "CREATE TEMP TABLE pg_tx_partial (value TEXT)", &[])
        .unwrap();

    // Begin outer transaction
    let outer = db.pg_tx_scope_begin(conn).unwrap();
    db.pg_execute(conn, "INSERT INTO pg_tx_partial VALUES ('outer')", &[])
        .unwrap();

    // Begin inner transaction
    let inner = db.pg_tx_scope_begin(conn).unwrap();
    db.pg_execute(conn, "INSERT INTO pg_tx_partial VALUES ('inner')", &[])
        .unwrap();

    // Rollback inner (only inner work lost)
    db.pg_tx_scope_end(conn, inner, false).unwrap();

    // Commit outer
    db.pg_tx_scope_end(conn, outer, true).unwrap();

    // Verify only 'outer' was committed
    let result = db
        .pg_query(conn, "SELECT value FROM pg_tx_partial", &[])
        .unwrap();

    let val = db.pg_get_text(result, 0, 0).unwrap();
    assert_eq!(val, "outer");

    let row_count = db.pg_row_count(result).unwrap();
    assert_eq!(row_count, 1);

    db.pg_free_result(result).unwrap();
    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_tx_depth_and_active_queries() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Initial state
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 0);
    assert!(!db.pg_tx_active(conn).unwrap());

    // After begin
    let scope1 = db.pg_tx_scope_begin(conn).unwrap();
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 1);
    assert!(db.pg_tx_active(conn).unwrap());

    // Nested
    let scope2 = db.pg_tx_scope_begin(conn).unwrap();
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 2);
    assert!(db.pg_tx_active(conn).unwrap());

    // End inner
    db.pg_tx_scope_end(conn, scope2, true).unwrap();
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 1);
    assert!(db.pg_tx_active(conn).unwrap());

    // End outer
    db.pg_tx_scope_end(conn, scope1, true).unwrap();
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 0);
    assert!(!db.pg_tx_active(conn).unwrap());

    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_tx_scope_id_mismatch() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // Begin transaction
    let scope = db.pg_tx_scope_begin(conn).unwrap();

    // Try to end with wrong scope ID
    let wrong_scope = scope + 1000;
    let result = db.pg_tx_scope_end(conn, wrong_scope, true);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::TransactionError
    ));

    // Transaction should still be active
    assert!(db.pg_tx_active(conn).unwrap());
    assert_eq!(db.pg_tx_depth(conn).unwrap(), 1);

    // End with correct scope ID
    db.pg_tx_scope_end(conn, scope, true).unwrap();
    assert!(!db.pg_tx_active(conn).unwrap());

    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_tx_scope_end_no_active_transaction() {
    let db = create_test_db();
    let conn = connect_test_db(&db);

    // No transaction active
    assert!(!db.pg_tx_active(conn).unwrap());

    // Try to end a non-existent transaction
    let result = db.pg_tx_scope_end(conn, 12345, true);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::TransactionError
    ));

    db.pg_disconnect(conn).unwrap();
}

#[test]
#[ignore] // Requires PostgreSQL server
fn test_pg_tx_scope_multiple_connections_independent() {
    let db = create_test_db();
    let conn1 = connect_test_db(&db);
    let conn2 = connect_test_db(&db);

    // Begin transaction on conn1
    let scope1 = db.pg_tx_scope_begin(conn1).unwrap();
    assert!(db.pg_tx_active(conn1).unwrap());
    assert_eq!(db.pg_tx_depth(conn1).unwrap(), 1);

    // conn2 should not be affected
    assert!(!db.pg_tx_active(conn2).unwrap());
    assert_eq!(db.pg_tx_depth(conn2).unwrap(), 0);

    // Begin transaction on conn2
    let scope2 = db.pg_tx_scope_begin(conn2).unwrap();
    assert!(db.pg_tx_active(conn2).unwrap());
    assert_eq!(db.pg_tx_depth(conn2).unwrap(), 1);

    // Both independent
    assert_ne!(scope1, scope2);

    // End conn1
    db.pg_tx_scope_end(conn1, scope1, true).unwrap();
    assert!(!db.pg_tx_active(conn1).unwrap());

    // conn2 still active
    assert!(db.pg_tx_active(conn2).unwrap());

    db.pg_tx_scope_end(conn2, scope2, true).unwrap();

    db.pg_disconnect(conn1).unwrap();
    db.pg_disconnect(conn2).unwrap();
}
