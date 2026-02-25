//! Integration tests for the connection pool functionality (Phase 5).
//!
//! Tests cover:
//! - SQLite pool creation and closing
//! - SQLite pool acquire and release
//! - SQLite pool stats
//! - Pool exhaustion and timeout
//! - Connection health checks
//! - Transaction rollback on release
//! - Multiple concurrent pool operations
//! - Capability denial (NoHostDb)

use arth_vm::{
    DbError, DbErrorKind, HostDb, NoHostDb, PoolConfig, SqliteConnectionHandle, SqlitePoolHandle,
    StdHostDb,
};
use std::sync::Arc;

/// Helper to create a StdHostDb instance for testing.
fn create_test_db() -> Arc<StdHostDb> {
    Arc::new(StdHostDb::new())
}

/// Helper to create a default pool config for testing.
fn default_pool_config() -> PoolConfig {
    PoolConfig {
        min_connections: 1,
        max_connections: 5,
        acquire_timeout_ms: 1000,
        idle_timeout_ms: 30000,
        max_lifetime_ms: 0, // No max lifetime
        test_on_acquire: false,
    }
}

// =============================================================================
// SQLite Pool Creation Tests
// =============================================================================

#[test]
fn test_sqlite_pool_create_memory() {
    let db = create_test_db();
    let config = default_pool_config();

    // Create pool for in-memory database
    let result = db.sqlite_pool_create(":memory:", &config);
    assert!(result.is_ok(), "Failed to create SQLite pool: {:?}", result);

    let pool = result.unwrap();
    assert!(pool.0 > 0, "Pool handle should be positive");

    // Close the pool
    let close_result = db.sqlite_pool_close(pool);
    assert!(close_result.is_ok(), "Failed to close pool");
}

#[test]
fn test_sqlite_pool_create_with_min_connections() {
    let db = create_test_db();
    let mut config = default_pool_config();
    config.min_connections = 3;
    config.max_connections = 5;

    let pool = db.sqlite_pool_create(":memory:", &config).unwrap();

    // Check stats - should have 3 available connections
    let stats = db.sqlite_pool_stats(pool).unwrap();
    assert_eq!(stats.available, 3, "Expected 3 available connections");
    assert_eq!(stats.in_use, 0, "Expected 0 in-use connections");
    assert_eq!(stats.total, 3, "Expected 3 total connections");

    db.sqlite_pool_close(pool).unwrap();
}

#[test]
fn test_sqlite_pool_close_invalid_handle() {
    let db = create_test_db();

    let invalid_handle = SqlitePoolHandle(99999);
    let result = db.sqlite_pool_close(invalid_handle);
    assert!(result.is_err());
}

#[test]
fn test_sqlite_pool_multiple_pools() {
    let db = create_test_db();
    let config = default_pool_config();

    // Create multiple pools
    let pool1 = db.sqlite_pool_create(":memory:", &config).unwrap();
    let pool2 = db.sqlite_pool_create(":memory:", &config).unwrap();
    let pool3 = db.sqlite_pool_create(":memory:", &config).unwrap();

    // Each should have a unique handle
    assert_ne!(pool1.0, pool2.0);
    assert_ne!(pool2.0, pool3.0);
    assert_ne!(pool1.0, pool3.0);

    // Close all
    db.sqlite_pool_close(pool1).unwrap();
    db.sqlite_pool_close(pool2).unwrap();
    db.sqlite_pool_close(pool3).unwrap();
}

// =============================================================================
// SQLite Pool Acquire/Release Tests
// =============================================================================

#[test]
fn test_sqlite_pool_acquire_release() {
    let db = create_test_db();
    let config = default_pool_config();

    let pool = db.sqlite_pool_create(":memory:", &config).unwrap();

    // Acquire a connection
    let conn = db.sqlite_pool_acquire(pool).unwrap();
    assert!(conn.0 > 0, "Connection handle should be positive");

    // Stats should show 1 in use
    let stats = db.sqlite_pool_stats(pool).unwrap();
    assert_eq!(stats.in_use, 1);

    // Release the connection
    db.sqlite_pool_release(pool, conn).unwrap();

    // Stats should show 0 in use
    let stats = db.sqlite_pool_stats(pool).unwrap();
    assert_eq!(stats.in_use, 0);

    db.sqlite_pool_close(pool).unwrap();
}

#[test]
fn test_sqlite_pool_acquire_multiple() {
    let db = create_test_db();
    let mut config = default_pool_config();
    config.min_connections = 0;
    config.max_connections = 3;

    let pool = db.sqlite_pool_create(":memory:", &config).unwrap();

    // Acquire 3 connections
    let conn1 = db.sqlite_pool_acquire(pool).unwrap();
    let conn2 = db.sqlite_pool_acquire(pool).unwrap();
    let conn3 = db.sqlite_pool_acquire(pool).unwrap();

    // Stats should show 3 in use
    let stats = db.sqlite_pool_stats(pool).unwrap();
    assert_eq!(stats.in_use, 3);
    assert_eq!(stats.total, 3);

    // Release all
    db.sqlite_pool_release(pool, conn1).unwrap();
    db.sqlite_pool_release(pool, conn2).unwrap();
    db.sqlite_pool_release(pool, conn3).unwrap();

    db.sqlite_pool_close(pool).unwrap();
}

#[test]
fn test_sqlite_pool_connection_reuse() {
    let db = create_test_db();
    let mut config = default_pool_config();
    config.min_connections = 1;
    config.max_connections = 1;

    let pool = db.sqlite_pool_create(":memory:", &config).unwrap();

    // Acquire and release
    let conn1 = db.sqlite_pool_acquire(pool).unwrap();
    db.sqlite_pool_release(pool, conn1).unwrap();

    // Acquire again - should get the same connection back
    let conn2 = db.sqlite_pool_acquire(pool).unwrap();

    // The handle may or may not be the same depending on implementation
    // but we should be able to use the connection
    db.sqlite_pool_release(pool, conn2).unwrap();

    db.sqlite_pool_close(pool).unwrap();
}

#[test]
fn test_sqlite_pool_use_connection() {
    let db = create_test_db();
    let config = default_pool_config();

    let pool = db.sqlite_pool_create(":memory:", &config).unwrap();
    let conn = db.sqlite_pool_acquire(pool).unwrap();

    // Use the acquired connection for database operations
    db.sqlite_execute(
        conn,
        "CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)",
    )
    .unwrap();
    db.sqlite_execute(conn, "INSERT INTO test (value) VALUES ('hello')")
        .unwrap();

    // Query to verify
    let stmt = db.sqlite_prepare(conn, "SELECT value FROM test").unwrap();
    assert!(db.sqlite_step(stmt).unwrap()); // Should have a row
    let value = db.sqlite_column_text(stmt, 0).unwrap();
    assert_eq!(value, "hello");
    db.sqlite_finalize(stmt).unwrap();

    db.sqlite_pool_release(pool, conn).unwrap();
    db.sqlite_pool_close(pool).unwrap();
}

// =============================================================================
// Pool Exhaustion Tests
// =============================================================================

#[test]
fn test_sqlite_pool_exhaustion_timeout() {
    let db = create_test_db();
    let mut config = default_pool_config();
    config.min_connections = 0;
    config.max_connections = 1;
    config.acquire_timeout_ms = 100; // 100ms timeout

    let pool = db.sqlite_pool_create(":memory:", &config).unwrap();

    // Acquire the only connection
    let _conn = db.sqlite_pool_acquire(pool).unwrap();

    // Try to acquire another - should timeout
    let start = std::time::Instant::now();
    let result = db.sqlite_pool_acquire(pool);
    let elapsed = start.elapsed();

    assert!(result.is_err(), "Expected timeout error");
    assert!(
        matches!(result.unwrap_err().kind, DbErrorKind::PoolExhausted),
        "Expected PoolExhausted error"
    );
    assert!(
        elapsed.as_millis() >= 100,
        "Should have waited at least 100ms"
    );

    db.sqlite_pool_close(pool).unwrap();
}

// =============================================================================
// Transaction Rollback on Release Tests
// =============================================================================

#[test]
fn test_sqlite_pool_rollback_on_release() {
    let db = create_test_db();
    let config = default_pool_config();

    let pool = db.sqlite_pool_create(":memory:", &config).unwrap();

    // Acquire a connection and start a transaction
    let conn = db.sqlite_pool_acquire(pool).unwrap();
    db.sqlite_execute(conn, "CREATE TABLE test (id INTEGER PRIMARY KEY)")
        .unwrap();
    db.sqlite_begin(conn).unwrap();
    db.sqlite_execute(conn, "INSERT INTO test (id) VALUES (1)")
        .unwrap();

    // Release without committing - should rollback
    db.sqlite_pool_release(pool, conn).unwrap();

    // Acquire again and check if insert was rolled back
    let conn2 = db.sqlite_pool_acquire(pool).unwrap();
    let stmt = db
        .sqlite_prepare(conn2, "SELECT COUNT(*) FROM test")
        .unwrap();
    assert!(db.sqlite_step(stmt).unwrap());
    let count = db.sqlite_column_int(stmt, 0).unwrap();
    db.sqlite_finalize(stmt).unwrap();

    // The count should be 0 because the transaction was rolled back
    assert_eq!(count, 0, "Transaction should have been rolled back");

    db.sqlite_pool_release(pool, conn2).unwrap();
    db.sqlite_pool_close(pool).unwrap();
}

// =============================================================================
// Health Check Tests
// =============================================================================

#[test]
fn test_sqlite_pool_with_health_check() {
    let db = create_test_db();
    let mut config = default_pool_config();
    config.test_on_acquire = true;

    let pool = db.sqlite_pool_create(":memory:", &config).unwrap();

    // Acquire should work and pass health check
    let conn = db.sqlite_pool_acquire(pool).unwrap();
    db.sqlite_pool_release(pool, conn).unwrap();

    // Acquire again - health check should pass
    let conn2 = db.sqlite_pool_acquire(pool).unwrap();
    db.sqlite_pool_release(pool, conn2).unwrap();

    db.sqlite_pool_close(pool).unwrap();
}

// =============================================================================
// Stats Tests
// =============================================================================

#[test]
fn test_sqlite_pool_stats() {
    let db = create_test_db();
    let mut config = default_pool_config();
    config.min_connections = 2;
    config.max_connections = 5;

    let pool = db.sqlite_pool_create(":memory:", &config).unwrap();

    // Initial stats
    let stats = db.sqlite_pool_stats(pool).unwrap();
    assert_eq!(stats.available, 2);
    assert_eq!(stats.in_use, 0);
    assert_eq!(stats.total, 2);

    // Acquire one
    let conn1 = db.sqlite_pool_acquire(pool).unwrap();
    let stats = db.sqlite_pool_stats(pool).unwrap();
    assert_eq!(stats.available, 1);
    assert_eq!(stats.in_use, 1);
    assert_eq!(stats.total, 2);

    // Acquire another
    let conn2 = db.sqlite_pool_acquire(pool).unwrap();
    let stats = db.sqlite_pool_stats(pool).unwrap();
    assert_eq!(stats.available, 0);
    assert_eq!(stats.in_use, 2);
    assert_eq!(stats.total, 2);

    // Acquire a third (creates new connection)
    let conn3 = db.sqlite_pool_acquire(pool).unwrap();
    let stats = db.sqlite_pool_stats(pool).unwrap();
    assert_eq!(stats.available, 0);
    assert_eq!(stats.in_use, 3);
    assert_eq!(stats.total, 3);

    // Release all
    db.sqlite_pool_release(pool, conn1).unwrap();
    db.sqlite_pool_release(pool, conn2).unwrap();
    db.sqlite_pool_release(pool, conn3).unwrap();

    db.sqlite_pool_close(pool).unwrap();
}

#[test]
fn test_sqlite_pool_stats_invalid_handle() {
    let db = create_test_db();

    let invalid_handle = SqlitePoolHandle(99999);
    let result = db.sqlite_pool_stats(invalid_handle);
    assert!(result.is_err());
}

// =============================================================================
// Release Validation Tests
// =============================================================================

#[test]
fn test_sqlite_pool_release_invalid_pool() {
    let db = create_test_db();
    let config = default_pool_config();

    let pool = db.sqlite_pool_create(":memory:", &config).unwrap();
    let conn = db.sqlite_pool_acquire(pool).unwrap();

    // Try to release to wrong pool
    let invalid_pool = SqlitePoolHandle(99999);
    let result = db.sqlite_pool_release(invalid_pool, conn);
    assert!(result.is_err());

    // Proper cleanup
    db.sqlite_pool_release(pool, conn).unwrap();
    db.sqlite_pool_close(pool).unwrap();
}

#[test]
fn test_sqlite_pool_release_not_from_pool() {
    let db = create_test_db();
    let config = default_pool_config();

    let pool = db.sqlite_pool_create(":memory:", &config).unwrap();

    // Create a connection directly (not from pool)
    let direct_conn = db.sqlite_open(":memory:").unwrap();

    // Try to release it to the pool - should fail
    let result = db.sqlite_pool_release(pool, direct_conn);
    assert!(result.is_err());

    // Cleanup
    db.sqlite_close(direct_conn).unwrap();
    db.sqlite_pool_close(pool).unwrap();
}

// =============================================================================
// Capability Denial Tests (NoHostDb)
// =============================================================================

#[test]
fn test_pool_capability_denied() {
    let no_db = NoHostDb;
    let config = default_pool_config();

    // All pool operations should fail with capability denied
    let result = no_db.sqlite_pool_create(":memory:", &config);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));

    let result = no_db.sqlite_pool_close(SqlitePoolHandle(1));
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));

    let result = no_db.sqlite_pool_acquire(SqlitePoolHandle(1));
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));

    let result = no_db.sqlite_pool_release(SqlitePoolHandle(1), SqliteConnectionHandle(1));
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));

    let result = no_db.sqlite_pool_stats(SqlitePoolHandle(1));
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().kind,
        DbErrorKind::CapabilityDenied
    ));
}

// =============================================================================
// Edge Cases
// =============================================================================

#[test]
fn test_sqlite_pool_zero_min_connections() {
    let db = create_test_db();
    let mut config = default_pool_config();
    config.min_connections = 0;
    config.max_connections = 2;

    let pool = db.sqlite_pool_create(":memory:", &config).unwrap();

    // Should start with no connections
    let stats = db.sqlite_pool_stats(pool).unwrap();
    assert_eq!(stats.total, 0);

    // Acquire should create a new connection
    let conn = db.sqlite_pool_acquire(pool).unwrap();
    let stats = db.sqlite_pool_stats(pool).unwrap();
    assert_eq!(stats.total, 1);
    assert_eq!(stats.in_use, 1);

    db.sqlite_pool_release(pool, conn).unwrap();
    db.sqlite_pool_close(pool).unwrap();
}

#[test]
fn test_sqlite_pool_acquire_at_max_then_release() {
    let db = create_test_db();
    let mut config = default_pool_config();
    config.min_connections = 0;
    config.max_connections = 2;
    config.acquire_timeout_ms = 500;

    let pool = db.sqlite_pool_create(":memory:", &config).unwrap();

    // Acquire up to max
    let conn1 = db.sqlite_pool_acquire(pool).unwrap();
    let conn2 = db.sqlite_pool_acquire(pool).unwrap();

    // Spawn a thread to release after a short delay
    let db_clone = db.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        db_clone.sqlite_pool_release(pool, conn1).unwrap();
    });

    // This acquire should succeed after the release
    let conn3 = db.sqlite_pool_acquire(pool).unwrap();
    handle.join().unwrap();

    db.sqlite_pool_release(pool, conn2).unwrap();
    db.sqlite_pool_release(pool, conn3).unwrap();
    db.sqlite_pool_close(pool).unwrap();
}
