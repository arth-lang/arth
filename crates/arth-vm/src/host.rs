//! Host-provided functions for IO, networking, and time operations.
//!
//! This module defines traits that abstract host capabilities away from the core VM.
//! Embedders can provide their own implementations to customize or restrict behavior.
//!
//! # Architecture
//!
//! The host function layer replaces hard-wired VM intrinsics for:
//! - **IO**: File, directory, path, and console operations (`HostIo`)
//! - **Networking**: HTTP, WebSocket, and SSE operations (`HostNet`)
//! - **Time**: Wall-clock, monotonic clock, and timer operations (`HostTime`)
//!
//! # Default Implementations
//!
//! - `StdHostIo`: Uses `std::fs` and `std::io` for real filesystem access
//! - `StdHostNet`: Uses Tokio/hyper for real networking
//! - `StdHostTime`: Uses `std::time` for real clock access
//!
//! # Sandboxed Implementations
//!
//! - `NoHostIo`: Errors on all IO operations (for sandboxed guests)
//! - `NoHostNet`: Errors on all network operations (for sandboxed guests)
//! - `MockHostTime`: Deterministic time for testing

use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, SeekFrom, Write};
use std::sync::Arc;

// ============================================================================
// HostConfig - Capability Configuration
// ============================================================================

/// Configuration for host capabilities.
///
/// Used to enable/disable specific capability domains for sandboxed execution.
/// This allows fine-grained control over what operations a guest program can perform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostConfig {
    /// Allow IO operations (file, directory, console, logging)
    pub allow_io: bool,
    /// Allow networking operations (HTTP, WebSocket, SSE)
    pub allow_net: bool,
    /// Allow time operations (DateTime, Instant)
    pub allow_time: bool,
    /// Allow database operations (SQLite, PostgreSQL)
    pub allow_db: bool,
    /// Allow mail operations (SMTP, IMAP, POP3)
    pub allow_mail: bool,
    /// Allow cryptographic operations (hashing, encryption, signatures)
    pub allow_crypto: bool,
    /// Allow WISP Calendar operations (wisp.calendar.*)
    pub allow_wisp_calendar: bool,
    /// Allow WISP Spreadsheet operations (wisp.sheet.*)
    pub allow_wisp_sheet: bool,
    /// Allow WISP Document operations (wisp.doc.*)
    pub allow_wisp_doc: bool,
    /// Allow WISP Presentation operations (wisp.pres.*)
    pub allow_wisp_pres: bool,
}

impl HostConfig {
    /// All capabilities enabled (for CLI/trusted code).
    pub fn full() -> Self {
        Self {
            allow_io: true,
            allow_net: true,
            allow_time: true,
            allow_db: true,
            allow_mail: true,
            allow_crypto: true,
            allow_wisp_calendar: true,
            allow_wisp_sheet: true,
            allow_wisp_doc: true,
            allow_wisp_pres: true,
        }
    }

    /// All capabilities disabled (fully sandboxed).
    pub fn sandboxed() -> Self {
        Self {
            allow_io: false,
            allow_net: false,
            allow_time: false,
            allow_db: false,
            allow_mail: false,
            allow_crypto: false,
            allow_wisp_calendar: false,
            allow_wisp_sheet: false,
            allow_wisp_doc: false,
            allow_wisp_pres: false,
        }
    }

    /// Create from a list of allowed capability names.
    ///
    /// Recognized capability names: "io", "net", "time", "db", "mail", "crypto",
    /// "wisp.calendar", "wisp.sheet", "wisp.doc", "wisp.pres"
    #[allow(clippy::cmp_owned)] // Can't use contains() because &[String].contains(&str) doesn't work
    pub fn from_capabilities(caps: &[String]) -> Self {
        Self {
            allow_io: caps.iter().any(|c| c == "io"),
            allow_net: caps.iter().any(|c| c == "net"),
            allow_time: caps.iter().any(|c| c == "time"),
            allow_db: caps.iter().any(|c| c == "db"),
            allow_mail: caps.iter().any(|c| c == "mail"),
            allow_crypto: caps.iter().any(|c| c == "crypto"),
            allow_wisp_calendar: caps.iter().any(|c| c == "wisp.calendar"),
            allow_wisp_sheet: caps.iter().any(|c| c == "wisp.sheet"),
            allow_wisp_doc: caps.iter().any(|c| c == "wisp.doc"),
            allow_wisp_pres: caps.iter().any(|c| c == "wisp.pres"),
        }
    }

    /// Create from a slice of string slices.
    pub fn from_capability_strs(caps: &[&str]) -> Self {
        Self {
            allow_io: caps.contains(&"io"),
            allow_net: caps.contains(&"net"),
            allow_time: caps.contains(&"time"),
            allow_db: caps.contains(&"db"),
            allow_mail: caps.contains(&"mail"),
            allow_crypto: caps.contains(&"crypto"),
            allow_wisp_calendar: caps.contains(&"wisp.calendar"),
            allow_wisp_sheet: caps.contains(&"wisp.sheet"),
            allow_wisp_doc: caps.contains(&"wisp.doc"),
            allow_wisp_pres: caps.contains(&"wisp.pres"),
        }
    }
}

impl Default for HostConfig {
    fn default() -> Self {
        Self::full()
    }
}

// ============================================================================
// Base64 Encoding/Decoding Helpers (using arth_rt)
// ============================================================================

/// Base64 encode data using arth_rt's implementation.
pub(crate) fn base64_encode(data: &[u8]) -> String {
    if data.is_empty() {
        return String::new();
    }
    let enc_len = arth_rt::encoding::arth_rt_base64_encode_len(data.len());
    let mut encoded = vec![0u8; enc_len];
    let len = arth_rt::encoding::arth_rt_base64_encode(
        data.as_ptr(),
        data.len(),
        encoded.as_mut_ptr(),
        encoded.len(),
    );
    if len > 0 {
        encoded.truncate(len as usize);
        // Base64 output is always ASCII, but use safe conversion anyway
        String::from_utf8(encoded).unwrap_or_default()
    } else {
        String::new()
    }
}

/// Base64 decode data using arth_rt's implementation.
pub(crate) fn base64_decode(encoded: &str) -> Result<Vec<u8>, String> {
    if encoded.is_empty() {
        return Ok(Vec::new());
    }
    let bytes = encoded.as_bytes();
    let dec_len = arth_rt::encoding::arth_rt_base64_decode_len(bytes.len());
    let mut decoded = vec![0u8; dec_len];
    let len = arth_rt::encoding::arth_rt_base64_decode(
        bytes.as_ptr(),
        bytes.len(),
        decoded.as_mut_ptr(),
        decoded.len(),
    );
    if len >= 0 {
        decoded.truncate(len as usize);
        Ok(decoded)
    } else {
        Err(match len {
            -1 => "buffer too small".to_string(),
            -3 => "invalid base64 input".to_string(),
            _ => "base64 decode error".to_string(),
        })
    }
}

// ============================================================================
// Error Types
// ============================================================================

/// Error type for IO operations.
#[derive(Clone, Debug)]
pub struct IoError {
    pub kind: IoErrorKind,
    pub message: String,
}

impl IoError {
    pub fn new(kind: IoErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn not_found(path: &str) -> Self {
        Self::new(IoErrorKind::NotFound, format!("not found: {}", path))
    }

    pub fn permission_denied(path: &str) -> Self {
        Self::new(
            IoErrorKind::PermissionDenied,
            format!("permission denied: {}", path),
        )
    }

    pub fn invalid_handle() -> Self {
        Self::new(IoErrorKind::InvalidHandle, "invalid file handle")
    }

    pub fn not_supported(op: &str) -> Self {
        Self::new(IoErrorKind::NotSupported, format!("not supported: {}", op))
    }

    pub fn capability_denied(cap: &str) -> Self {
        Self::new(
            IoErrorKind::CapabilityDenied,
            format!("capability denied: {}", cap),
        )
    }
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for IoError {}

impl From<std::io::Error> for IoError {
    fn from(err: std::io::Error) -> Self {
        let kind = match err.kind() {
            std::io::ErrorKind::NotFound => IoErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied => IoErrorKind::PermissionDenied,
            std::io::ErrorKind::AlreadyExists => IoErrorKind::AlreadyExists,
            std::io::ErrorKind::InvalidInput => IoErrorKind::InvalidInput,
            std::io::ErrorKind::InvalidData => IoErrorKind::InvalidData,
            std::io::ErrorKind::UnexpectedEof => IoErrorKind::UnexpectedEof,
            std::io::ErrorKind::Interrupted => IoErrorKind::Interrupted,
            _ => IoErrorKind::Other,
        };
        Self::new(kind, err.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoErrorKind {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    InvalidInput,
    InvalidData,
    InvalidHandle,
    UnexpectedEof,
    Interrupted,
    NotSupported,
    CapabilityDenied,
    Other,
}

/// Error type for networking operations.
#[derive(Clone, Debug)]
pub struct NetError {
    pub kind: NetErrorKind,
    pub message: String,
}

impl NetError {
    pub fn new(kind: NetErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn connection_refused(addr: &str) -> Self {
        Self::new(
            NetErrorKind::ConnectionRefused,
            format!("connection refused: {}", addr),
        )
    }

    pub fn timeout() -> Self {
        Self::new(NetErrorKind::Timeout, "operation timed out")
    }

    pub fn invalid_handle() -> Self {
        Self::new(NetErrorKind::InvalidHandle, "invalid handle")
    }

    pub fn capability_denied(cap: &str) -> Self {
        Self::new(
            NetErrorKind::CapabilityDenied,
            format!("capability denied: {}", cap),
        )
    }

    pub fn connection_failed(msg: &str) -> Self {
        Self::new(NetErrorKind::ConnectionRefused, msg)
    }

    pub fn protocol_error(msg: &str) -> Self {
        Self::new(NetErrorKind::Other, format!("protocol error: {}", msg))
    }

    pub fn not_implemented(msg: &str) -> Self {
        Self::new(NetErrorKind::Other, format!("not implemented: {}", msg))
    }

    pub fn invalid_url(msg: &str) -> Self {
        Self::new(NetErrorKind::InvalidUrl, msg)
    }
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for NetError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetErrorKind {
    ConnectionRefused,
    ConnectionReset,
    ConnectionClosed,
    Timeout,
    InvalidUrl,
    InvalidHandle,
    AddressInUse,
    CapabilityDenied,
    Other,
}

/// Error type for time operations.
#[derive(Clone, Debug)]
pub struct TimeError {
    pub kind: TimeErrorKind,
    pub message: String,
}

impl TimeError {
    pub fn new(kind: TimeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn parse_error(input: &str, format: &str) -> Self {
        Self::new(
            TimeErrorKind::ParseError,
            format!("failed to parse '{}' with format '{}'", input, format),
        )
    }

    pub fn invalid_handle() -> Self {
        Self::new(TimeErrorKind::InvalidHandle, "invalid instant handle")
    }

    pub fn capability_denied(cap: &str) -> Self {
        Self::new(
            TimeErrorKind::CapabilityDenied,
            format!("capability denied: {}", cap),
        )
    }
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for TimeError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeErrorKind {
    ParseError,
    FormatError,
    InvalidHandle,
    CapabilityDenied,
    Other,
}

/// Error type for database operations.
#[derive(Clone, Debug)]
pub struct DbError {
    pub kind: DbErrorKind,
    pub message: String,
    pub sqlite_code: Option<i32>,
}

impl DbError {
    pub fn new(kind: DbErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            sqlite_code: None,
        }
    }

    pub fn with_code(kind: DbErrorKind, message: impl Into<String>, code: i32) -> Self {
        Self {
            kind,
            message: message.into(),
            sqlite_code: Some(code),
        }
    }

    pub fn connection_error(message: impl Into<String>) -> Self {
        Self::new(DbErrorKind::ConnectionError, message)
    }

    pub fn query_error(message: impl Into<String>) -> Self {
        Self::new(DbErrorKind::QueryError, message)
    }

    pub fn prepare_error(message: impl Into<String>) -> Self {
        Self::new(DbErrorKind::PrepareError, message)
    }

    pub fn bind_error(message: impl Into<String>) -> Self {
        Self::new(DbErrorKind::BindError, message)
    }

    pub fn transaction_error(message: impl Into<String>) -> Self {
        Self::new(DbErrorKind::TransactionError, message)
    }

    pub fn type_mismatch(message: impl Into<String>) -> Self {
        Self::new(DbErrorKind::TypeMismatch, message)
    }

    pub fn invalid_handle() -> Self {
        Self::new(DbErrorKind::InvalidHandle, "invalid database handle")
    }

    pub fn capability_denied(cap: &str) -> Self {
        Self::new(
            DbErrorKind::CapabilityDenied,
            format!("capability denied: {}", cap),
        )
    }

    pub fn pool_exhausted(message: impl Into<String>) -> Self {
        Self::new(DbErrorKind::PoolExhausted, message)
    }
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(code) = self.sqlite_code {
            write!(f, "{:?} (code {}): {}", self.kind, code, self.message)
        } else {
            write!(f, "{:?}: {}", self.kind, self.message)
        }
    }
}

impl std::error::Error for DbError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbErrorKind {
    ConnectionError,
    QueryError,
    PrepareError,
    BindError,
    TransactionError,
    TypeMismatch,
    InvalidHandle,
    CapabilityDenied,
    PoolExhausted,
    Other,
}

/// Error type for mail operations (SMTP, IMAP, POP3).
#[derive(Clone, Debug)]
pub struct MailError {
    pub kind: MailErrorKind,
    pub message: String,
    /// SMTP response code (e.g., 250, 550) if available.
    pub response_code: Option<u16>,
}

impl MailError {
    pub fn new(kind: MailErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            response_code: None,
        }
    }

    pub fn with_code(kind: MailErrorKind, message: impl Into<String>, code: u16) -> Self {
        Self {
            kind,
            message: message.into(),
            response_code: Some(code),
        }
    }

    pub fn connection_error(message: impl Into<String>) -> Self {
        Self::new(MailErrorKind::ConnectionError, message)
    }

    pub fn auth_error(message: impl Into<String>) -> Self {
        Self::new(MailErrorKind::AuthenticationError, message)
    }

    pub fn protocol_error(message: impl Into<String>) -> Self {
        Self::new(MailErrorKind::ProtocolError, message)
    }

    pub fn tls_error(message: impl Into<String>) -> Self {
        Self::new(MailErrorKind::TlsError, message)
    }

    pub fn timeout() -> Self {
        Self::new(MailErrorKind::Timeout, "operation timed out")
    }

    pub fn invalid_handle() -> Self {
        Self::new(MailErrorKind::InvalidHandle, "invalid mail handle")
    }

    pub fn capability_denied(cap: &str) -> Self {
        Self::new(
            MailErrorKind::CapabilityDenied,
            format!("capability denied: {}", cap),
        )
    }

    pub fn invalid_address(addr: &str) -> Self {
        Self::new(
            MailErrorKind::InvalidAddress,
            format!("invalid email address: {}", addr),
        )
    }

    pub fn message_error(message: impl Into<String>) -> Self {
        Self::new(MailErrorKind::MessageError, message)
    }
}

impl fmt::Display for MailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(code) = self.response_code {
            write!(f, "{:?} (code {}): {}", self.kind, code, self.message)
        } else {
            write!(f, "{:?}: {}", self.kind, self.message)
        }
    }
}

impl std::error::Error for MailError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailErrorKind {
    /// Failed to establish connection to mail server.
    ConnectionError,
    /// Authentication failed (wrong credentials, etc.).
    AuthenticationError,
    /// Protocol-level error (unexpected response, malformed command).
    ProtocolError,
    /// TLS/SSL handshake or encryption error.
    TlsError,
    /// Operation timed out.
    Timeout,
    /// Invalid handle (connection closed, etc.).
    InvalidHandle,
    /// Capability/permission denied.
    CapabilityDenied,
    /// Invalid email address format.
    InvalidAddress,
    /// Error building or parsing MIME message.
    MessageError,
    /// Recipient rejected by server.
    RecipientRejected,
    /// Mailbox not found (IMAP/POP3).
    MailboxNotFound,
    /// Message not found.
    MessageNotFound,
    /// Server does not support required feature.
    NotSupported,
    /// Other error.
    Other,
}

// ============================================================================
// Handle Types
// ============================================================================

/// Opaque handle to an open file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileHandle(pub i64);

/// Opaque handle to an HTTP server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HttpServerHandle(pub i64);

/// Opaque handle to an HTTP request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HttpRequestHandle(pub i64);

/// Opaque handle to an HTTP response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HttpResponseHandle(pub i64);

/// Opaque handle to HTTP headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HeadersHandle(pub i64);

/// Opaque handle to an async task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TaskHandle(pub i64);

/// Opaque handle to a WebSocket server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WsServerHandle(pub i64);

/// Opaque handle to a WebSocket connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WsConnectionHandle(pub i64);

/// Opaque handle to an SSE server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SseServerHandle(pub i64);

/// Opaque handle to an SSE emitter (client connection).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SseEmitterHandle(pub i64);

/// Opaque handle to a monotonic instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InstantHandle(pub i64);

/// Opaque handle to a SQLite database connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SqliteConnectionHandle(pub i64);

/// Opaque handle to a SQLite prepared statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SqliteStatementHandle(pub i64);

/// Opaque handle to a PostgreSQL database connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PgConnectionHandle(pub i64);

/// Opaque handle to a PostgreSQL query result set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PgResultHandle(pub i64);

/// Opaque handle to a PostgreSQL prepared statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PgStatementHandle(pub i64);

/// Opaque handle to an async PostgreSQL connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PgAsyncConnectionHandle(pub i64);

/// Opaque handle to a pending async PostgreSQL query.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PgAsyncQueryHandle(pub i64);

/// Opaque handle to a SQLite connection pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SqlitePoolHandle(pub i64);

/// Opaque handle to a PostgreSQL connection pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PgPoolHandle(pub i64);

/// Opaque handle to an SMTP connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SmtpConnectionHandle(pub i64);

/// Opaque handle to an IMAP connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImapConnectionHandle(pub i64);

/// Opaque handle to an IMAP folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImapFolderHandle(pub i64);

/// Opaque handle to a POP3 connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pop3ConnectionHandle(pub i64);

/// Opaque handle to a MIME message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MimeMessageHandle(pub i64);

/// Opaque handle to a TLS context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TlsContextHandle(pub i64);

/// Opaque handle to a TLS stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TlsStreamHandle(pub i64);

// ============================================================================
// Pool Configuration
// ============================================================================

/// Configuration for a database connection pool.
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// Minimum number of connections to keep in the pool.
    pub min_connections: u32,
    /// Maximum number of connections allowed in the pool.
    pub max_connections: u32,
    /// Timeout in milliseconds to wait for a connection when pool is exhausted.
    /// 0 means no timeout (wait indefinitely).
    pub acquire_timeout_ms: u64,
    /// Maximum idle time in milliseconds before a connection is closed.
    /// 0 means connections are never closed due to idle time.
    pub idle_timeout_ms: u64,
    /// Maximum lifetime of a connection in milliseconds.
    /// 0 means connections have no maximum lifetime.
    pub max_lifetime_ms: u64,
    /// Whether to test connections on acquire (health check).
    pub test_on_acquire: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            min_connections: 1,
            max_connections: 10,
            acquire_timeout_ms: 30_000, // 30 seconds
            idle_timeout_ms: 600_000,   // 10 minutes
            max_lifetime_ms: 0,         // no limit
            test_on_acquire: true,
        }
    }
}

impl PoolConfig {
    /// Create a new PoolConfig with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a pool config with just max connections specified.
    pub fn with_max_connections(max: u32) -> Self {
        Self {
            max_connections: max,
            ..Default::default()
        }
    }
}

/// Statistics about a connection pool.
#[derive(Clone, Debug, Default)]
pub struct PoolStats {
    /// Number of connections currently available in the pool.
    pub available: u32,
    /// Number of connections currently in use.
    pub in_use: u32,
    /// Total number of connections (available + in_use).
    pub total: u32,
    /// Number of waiters in the queue (when pool is exhausted).
    pub waiters: u32,
}

// ============================================================================
// Enums
// ============================================================================

/// File open mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileMode {
    Read,
    Write,
    Append,
    ReadWrite,
}

impl FileMode {
    pub fn from_i64(n: i64) -> Option<Self> {
        match n {
            0 => Some(FileMode::Read),
            1 => Some(FileMode::Write),
            2 => Some(FileMode::Append),
            3 => Some(FileMode::ReadWrite),
            _ => None,
        }
    }
}

/// Seek position for file operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeekPosition {
    Start(u64),
    End(i64),
    Current(i64),
}

impl SeekPosition {
    pub fn from_whence(offset: i64, whence: i64) -> Option<Self> {
        match whence {
            0 => Some(SeekPosition::Start(offset as u64)),
            1 => Some(SeekPosition::Current(offset)),
            2 => Some(SeekPosition::End(offset)),
            _ => None,
        }
    }
}

impl From<SeekPosition> for SeekFrom {
    fn from(pos: SeekPosition) -> Self {
        match pos {
            SeekPosition::Start(n) => SeekFrom::Start(n),
            SeekPosition::Current(n) => SeekFrom::Current(n),
            SeekPosition::End(n) => SeekFrom::End(n),
        }
    }
}

// ============================================================================
// HostIo Trait
// ============================================================================

/// Trait for host-provided IO operations.
///
/// Implementations can provide real filesystem access, in-memory filesystems,
/// or deny all access for sandboxed execution.
pub trait HostIo: Send + Sync + 'static {
    // --- File Operations ---

    /// Open a file with the specified mode.
    fn file_open(&self, path: &str, mode: FileMode) -> Result<FileHandle, IoError>;

    /// Close an open file.
    fn file_close(&self, handle: FileHandle) -> Result<(), IoError>;

    /// Read up to `max_bytes` from a file.
    fn file_read(&self, handle: FileHandle, max_bytes: usize) -> Result<Vec<u8>, IoError>;

    /// Write data to a file. Returns number of bytes written.
    fn file_write(&self, handle: FileHandle, data: &[u8]) -> Result<usize, IoError>;

    /// Write a string to a file. Returns number of bytes written.
    fn file_write_str(&self, handle: FileHandle, s: &str) -> Result<usize, IoError> {
        self.file_write(handle, s.as_bytes())
    }

    /// Flush buffered data to disk.
    fn file_flush(&self, handle: FileHandle) -> Result<(), IoError>;

    /// Seek to a position in the file. Returns new position.
    fn file_seek(&self, handle: FileHandle, pos: SeekPosition) -> Result<i64, IoError>;

    /// Get the size of an open file in bytes.
    fn file_size(&self, handle: FileHandle) -> Result<i64, IoError>;

    /// Check if a file exists at the given path.
    fn file_exists(&self, path: &str) -> Result<bool, IoError>;

    /// Delete a file.
    fn file_delete(&self, path: &str) -> Result<(), IoError>;

    /// Copy a file from src to dst.
    fn file_copy(&self, src: &str, dst: &str) -> Result<(), IoError>;

    /// Move/rename a file from src to dst.
    fn file_move(&self, src: &str, dst: &str) -> Result<(), IoError>;

    // --- Directory Operations ---

    /// Create a directory.
    fn dir_create(&self, path: &str) -> Result<(), IoError>;

    /// Create a directory and all parent directories.
    fn dir_create_all(&self, path: &str) -> Result<(), IoError>;

    /// Delete an empty directory.
    fn dir_delete(&self, path: &str) -> Result<(), IoError>;

    /// List entries in a directory.
    fn dir_list(&self, path: &str) -> Result<Vec<String>, IoError>;

    /// Check if a directory exists at the given path.
    fn dir_exists(&self, path: &str) -> Result<bool, IoError>;

    /// Check if the path is a directory.
    fn is_dir(&self, path: &str) -> Result<bool, IoError>;

    /// Check if the path is a regular file.
    fn is_file(&self, path: &str) -> Result<bool, IoError>;

    // --- Path Operations ---

    /// Convert a path to an absolute path.
    fn path_absolute(&self, path: &str) -> Result<String, IoError>;

    // --- Console Operations ---

    /// Read a line from stdin.
    fn console_read_line(&self) -> Result<String, IoError>;

    /// Write a string to stdout.
    fn console_write(&self, s: &str);

    /// Write a string to stderr.
    fn console_write_err(&self, s: &str);
}

// ============================================================================
// HostNet Trait
// ============================================================================

/// Trait for host-provided networking operations.
///
/// Implementations can provide real networking, mock networking for tests,
/// or deny all access for sandboxed execution.
pub trait HostNet: Send + Sync + 'static {
    // --- HTTP Client ---

    /// Perform an HTTP fetch request. Returns a task handle for async completion.
    fn http_fetch(
        &self,
        url: &str,
        method: &str,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<TaskHandle, NetError>;

    // --- HTTP Server ---

    /// Start an HTTP server on the given port.
    fn http_serve(&self, port: u16) -> Result<HttpServerHandle, NetError>;

    /// Accept the next HTTP request from a server. Returns a task handle.
    fn http_accept(&self, server: HttpServerHandle) -> Result<TaskHandle, NetError>;

    /// Send an HTTP response.
    fn http_respond(
        &self,
        request: HttpRequestHandle,
        status: u16,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<(), NetError>;

    // --- WebSocket Server ---

    /// Start a WebSocket server on the given port and path.
    fn ws_serve(&self, port: u16, path: &str) -> Result<WsServerHandle, NetError>;

    /// Accept the next WebSocket connection.
    fn ws_accept(&self, server: WsServerHandle) -> Result<WsConnectionHandle, NetError>;

    /// Send a text message over a WebSocket.
    fn ws_send_text(&self, conn: WsConnectionHandle, text: &str) -> Result<(), NetError>;

    /// Send a binary message over a WebSocket.
    fn ws_send_binary(&self, conn: WsConnectionHandle, data: &[u8]) -> Result<(), NetError>;

    /// Receive the next message from a WebSocket. Blocks until message arrives.
    fn ws_recv(&self, conn: WsConnectionHandle) -> Result<WsMessage, NetError>;

    /// Close a WebSocket connection.
    fn ws_close(&self, conn: WsConnectionHandle, code: u16, reason: &str) -> Result<(), NetError>;

    /// Check if a WebSocket connection is open.
    fn ws_is_open(&self, conn: WsConnectionHandle) -> Result<bool, NetError>;

    // --- SSE Server ---

    /// Start an SSE server on the given port and path.
    fn sse_serve(&self, port: u16, path: &str) -> Result<SseServerHandle, NetError>;

    /// Accept the next SSE client connection.
    fn sse_accept(&self, server: SseServerHandle) -> Result<SseEmitterHandle, NetError>;

    /// Send an SSE event.
    fn sse_send(
        &self,
        emitter: SseEmitterHandle,
        event: &str,
        data: &str,
        id: &str,
    ) -> Result<(), NetError>;

    /// Close an SSE connection.
    fn sse_close(&self, emitter: SseEmitterHandle) -> Result<(), NetError>;

    /// Check if an SSE client is still connected.
    fn sse_is_open(&self, emitter: SseEmitterHandle) -> Result<bool, NetError>;
}

/// WebSocket message type.
#[derive(Clone, Debug)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
    Close(u16, String),
}

// ============================================================================
// HostTime Trait
// ============================================================================

/// Trait for host-provided time operations.
///
/// Implementations can provide real time, mock time for deterministic testing,
/// or deny access for sandboxed execution.
pub trait HostTime: Send + Sync + 'static {
    /// Get current wall-clock time as milliseconds since Unix epoch.
    fn now_realtime(&self) -> i64;

    /// Parse a datetime string with the given format. Returns millis since epoch.
    fn parse(&self, format: &str, input: &str) -> Result<i64, TimeError>;

    /// Format a datetime (millis since epoch) with the given format.
    fn format(&self, millis: i64, format: &str) -> Result<String, TimeError>;

    /// Get a monotonic instant handle.
    fn instant_now(&self) -> InstantHandle;

    /// Get elapsed time in milliseconds since an instant was created.
    fn instant_elapsed(&self, instant: InstantHandle) -> Result<i64, TimeError>;

    /// Sleep for the given number of milliseconds. Blocks the current thread.
    fn sleep(&self, millis: i64);
}

// ============================================================================
// HostDb Trait
// ============================================================================

/// Trait for host-provided database operations.
///
/// Implementations can provide real SQLite/PostgreSQL access, in-memory databases,
/// or deny all access for sandboxed execution.
pub trait HostDb: Send + Sync + 'static {
    // --- Connection Operations ---

    /// Open a SQLite database connection.
    fn sqlite_open(&self, path: &str) -> Result<SqliteConnectionHandle, DbError>;

    /// Close a SQLite database connection.
    fn sqlite_close(&self, conn: SqliteConnectionHandle) -> Result<(), DbError>;

    // --- Statement Operations ---

    /// Prepare a SQL statement.
    fn sqlite_prepare(
        &self,
        conn: SqliteConnectionHandle,
        sql: &str,
    ) -> Result<SqliteStatementHandle, DbError>;

    /// Execute a step of a prepared statement.
    /// Returns true if there is a row available, false if done.
    fn sqlite_step(&self, stmt: SqliteStatementHandle) -> Result<bool, DbError>;

    /// Finalize (destroy) a prepared statement.
    fn sqlite_finalize(&self, stmt: SqliteStatementHandle) -> Result<(), DbError>;

    /// Reset a prepared statement for re-execution.
    fn sqlite_reset(&self, stmt: SqliteStatementHandle) -> Result<(), DbError>;

    // --- Binding Operations ---

    /// Bind an integer value to a parameter.
    fn sqlite_bind_int(
        &self,
        stmt: SqliteStatementHandle,
        idx: i32,
        val: i32,
    ) -> Result<(), DbError>;

    /// Bind an int64 value to a parameter.
    fn sqlite_bind_int64(
        &self,
        stmt: SqliteStatementHandle,
        idx: i32,
        val: i64,
    ) -> Result<(), DbError>;

    /// Bind a double value to a parameter.
    fn sqlite_bind_double(
        &self,
        stmt: SqliteStatementHandle,
        idx: i32,
        val: f64,
    ) -> Result<(), DbError>;

    /// Bind a text value to a parameter.
    fn sqlite_bind_text(
        &self,
        stmt: SqliteStatementHandle,
        idx: i32,
        val: &str,
    ) -> Result<(), DbError>;

    /// Bind a blob value to a parameter.
    fn sqlite_bind_blob(
        &self,
        stmt: SqliteStatementHandle,
        idx: i32,
        val: &[u8],
    ) -> Result<(), DbError>;

    /// Bind NULL to a parameter.
    fn sqlite_bind_null(&self, stmt: SqliteStatementHandle, idx: i32) -> Result<(), DbError>;

    // --- Column Access Operations ---

    /// Get an integer column value.
    fn sqlite_column_int(&self, stmt: SqliteStatementHandle, idx: i32) -> Result<i32, DbError>;

    /// Get an int64 column value.
    fn sqlite_column_int64(&self, stmt: SqliteStatementHandle, idx: i32) -> Result<i64, DbError>;

    /// Get a double column value.
    fn sqlite_column_double(&self, stmt: SqliteStatementHandle, idx: i32) -> Result<f64, DbError>;

    /// Get a text column value.
    fn sqlite_column_text(&self, stmt: SqliteStatementHandle, idx: i32) -> Result<String, DbError>;

    /// Get a blob column value.
    fn sqlite_column_blob(&self, stmt: SqliteStatementHandle, idx: i32)
    -> Result<Vec<u8>, DbError>;

    /// Get the type of a column (0=integer, 1=float, 2=text, 3=blob, 4=null).
    fn sqlite_column_type(&self, stmt: SqliteStatementHandle, idx: i32) -> Result<i32, DbError>;

    /// Get the number of columns in the result set.
    fn sqlite_column_count(&self, stmt: SqliteStatementHandle) -> Result<i32, DbError>;

    /// Get the name of a column.
    fn sqlite_column_name(&self, stmt: SqliteStatementHandle, idx: i32) -> Result<String, DbError>;

    /// Check if a column value is NULL.
    fn sqlite_is_null(&self, stmt: SqliteStatementHandle, idx: i32) -> Result<bool, DbError>;

    // --- Utility Operations ---

    /// Get the number of rows changed by the last statement.
    fn sqlite_changes(&self, conn: SqliteConnectionHandle) -> Result<i32, DbError>;

    /// Get the rowid of the last inserted row.
    fn sqlite_last_insert_rowid(&self, conn: SqliteConnectionHandle) -> Result<i64, DbError>;

    /// Get the last error message for a connection.
    fn sqlite_errmsg(&self, conn: SqliteConnectionHandle) -> Result<String, DbError>;

    // --- High-Level Convenience Operations ---

    /// Execute a SQL statement (prepare + step + finalize).
    /// Used for INSERT, UPDATE, DELETE, CREATE TABLE, etc.
    fn sqlite_execute(&self, conn: SqliteConnectionHandle, sql: &str) -> Result<(), DbError>;

    /// Prepare and return a statement handle for a query.
    /// This is an alias for sqlite_prepare - used for SELECT queries.
    fn sqlite_query(
        &self,
        conn: SqliteConnectionHandle,
        sql: &str,
    ) -> Result<SqliteStatementHandle, DbError>;

    // --- Transaction Operations ---

    /// Begin a transaction.
    fn sqlite_begin(&self, conn: SqliteConnectionHandle) -> Result<(), DbError>;

    /// Commit a transaction.
    fn sqlite_commit(&self, conn: SqliteConnectionHandle) -> Result<(), DbError>;

    /// Rollback a transaction.
    fn sqlite_rollback(&self, conn: SqliteConnectionHandle) -> Result<(), DbError>;

    /// Create a savepoint.
    fn sqlite_savepoint(&self, conn: SqliteConnectionHandle, name: &str) -> Result<(), DbError>;

    /// Release a savepoint.
    fn sqlite_release_savepoint(
        &self,
        conn: SqliteConnectionHandle,
        name: &str,
    ) -> Result<(), DbError>;

    /// Rollback to a savepoint.
    fn sqlite_rollback_to_savepoint(
        &self,
        conn: SqliteConnectionHandle,
        name: &str,
    ) -> Result<(), DbError>;

    // =========================================================================
    // PostgreSQL Operations
    // =========================================================================

    // --- Connection Operations ---

    /// Connect to a PostgreSQL database.
    fn pg_connect(&self, connection_string: &str) -> Result<PgConnectionHandle, DbError>;

    /// Disconnect from a PostgreSQL database.
    fn pg_disconnect(&self, conn: PgConnectionHandle) -> Result<(), DbError>;

    /// Check connection status. Returns true if connected.
    fn pg_status(&self, conn: PgConnectionHandle) -> Result<bool, DbError>;

    // --- Query Operations ---

    /// Execute a query and return a result handle.
    fn pg_query(
        &self,
        conn: PgConnectionHandle,
        sql: &str,
        params: &[PgValue],
    ) -> Result<PgResultHandle, DbError>;

    /// Execute a statement (INSERT/UPDATE/DELETE) and return affected rows.
    fn pg_execute(
        &self,
        conn: PgConnectionHandle,
        sql: &str,
        params: &[PgValue],
    ) -> Result<u64, DbError>;

    /// Prepare a statement for later execution.
    fn pg_prepare(
        &self,
        conn: PgConnectionHandle,
        name: &str,
        sql: &str,
    ) -> Result<PgStatementHandle, DbError>;

    /// Execute a prepared statement.
    fn pg_execute_prepared(
        &self,
        conn: PgConnectionHandle,
        stmt: PgStatementHandle,
        params: &[PgValue],
    ) -> Result<PgResultHandle, DbError>;

    // --- Result Operations ---

    /// Get the number of rows in a result set.
    fn pg_row_count(&self, result: PgResultHandle) -> Result<i64, DbError>;

    /// Get the number of columns in a result set.
    fn pg_column_count(&self, result: PgResultHandle) -> Result<i32, DbError>;

    /// Get the name of a column.
    fn pg_column_name(&self, result: PgResultHandle, col: i32) -> Result<String, DbError>;

    /// Get the type OID of a column.
    fn pg_column_type(&self, result: PgResultHandle, col: i32) -> Result<i32, DbError>;

    /// Get a value as a string (generic accessor).
    fn pg_get_value(&self, result: PgResultHandle, row: i64, col: i32) -> Result<String, DbError>;

    /// Get an integer value.
    fn pg_get_int(&self, result: PgResultHandle, row: i64, col: i32) -> Result<i32, DbError>;

    /// Get an int64 value.
    fn pg_get_int64(&self, result: PgResultHandle, row: i64, col: i32) -> Result<i64, DbError>;

    /// Get a double value.
    fn pg_get_double(&self, result: PgResultHandle, row: i64, col: i32) -> Result<f64, DbError>;

    /// Get a text value.
    fn pg_get_text(&self, result: PgResultHandle, row: i64, col: i32) -> Result<String, DbError>;

    /// Get a bytea (binary) value.
    fn pg_get_bytes(&self, result: PgResultHandle, row: i64, col: i32) -> Result<Vec<u8>, DbError>;

    /// Get a boolean value.
    fn pg_get_bool(&self, result: PgResultHandle, row: i64, col: i32) -> Result<bool, DbError>;

    /// Check if a value is NULL.
    fn pg_is_null(&self, result: PgResultHandle, row: i64, col: i32) -> Result<bool, DbError>;

    /// Get the number of affected rows from an execute.
    fn pg_affected_rows(&self, result: PgResultHandle) -> Result<u64, DbError>;

    /// Free a result handle.
    fn pg_free_result(&self, result: PgResultHandle) -> Result<(), DbError>;

    // --- Transaction Operations ---

    /// Begin a transaction.
    fn pg_begin(&self, conn: PgConnectionHandle) -> Result<(), DbError>;

    /// Commit a transaction.
    fn pg_commit(&self, conn: PgConnectionHandle) -> Result<(), DbError>;

    /// Rollback a transaction.
    fn pg_rollback(&self, conn: PgConnectionHandle) -> Result<(), DbError>;

    /// Create a savepoint.
    fn pg_savepoint(&self, conn: PgConnectionHandle, name: &str) -> Result<(), DbError>;

    /// Release a savepoint.
    fn pg_release_savepoint(&self, conn: PgConnectionHandle, name: &str) -> Result<(), DbError>;

    /// Rollback to a savepoint.
    fn pg_rollback_to_savepoint(&self, conn: PgConnectionHandle, name: &str)
    -> Result<(), DbError>;

    // --- Utility Operations ---

    /// Get the last error message.
    fn pg_errmsg(&self, conn: PgConnectionHandle) -> Result<String, DbError>;

    /// Escape a string for use in SQL.
    fn pg_escape(&self, conn: PgConnectionHandle, s: &str) -> Result<String, DbError>;

    // =========================================================================
    // Async PostgreSQL Operations
    // =========================================================================

    // --- Async Connection Operations ---

    /// Connect to PostgreSQL asynchronously.
    /// Returns an async connection handle immediately; the connection runs in a background task.
    fn pg_connect_async(&self, connection_string: &str)
    -> Result<PgAsyncConnectionHandle, DbError>;

    /// Disconnect from an async PostgreSQL connection.
    fn pg_disconnect_async(&self, conn: PgAsyncConnectionHandle) -> Result<(), DbError>;

    /// Check async connection status. Returns true if connected.
    fn pg_status_async(&self, conn: PgAsyncConnectionHandle) -> Result<bool, DbError>;

    // --- Async Query Operations ---

    /// Execute a query asynchronously.
    /// Returns a query handle that can be polled for completion.
    fn pg_query_async(
        &self,
        conn: PgAsyncConnectionHandle,
        sql: &str,
        params: &[PgValue],
    ) -> Result<PgAsyncQueryHandle, DbError>;

    /// Execute a statement asynchronously.
    /// Returns a query handle that can be polled for completion.
    fn pg_execute_async(
        &self,
        conn: PgAsyncConnectionHandle,
        sql: &str,
        params: &[PgValue],
    ) -> Result<PgAsyncQueryHandle, DbError>;

    /// Prepare a statement asynchronously.
    fn pg_prepare_async(
        &self,
        conn: PgAsyncConnectionHandle,
        name: &str,
        sql: &str,
    ) -> Result<PgAsyncQueryHandle, DbError>;

    /// Execute a prepared statement asynchronously.
    fn pg_execute_prepared_async(
        &self,
        conn: PgAsyncConnectionHandle,
        stmt_name: &str,
        params: &[PgValue],
    ) -> Result<PgAsyncQueryHandle, DbError>;

    // --- Async Result Polling ---

    /// Check if an async query is ready.
    /// Returns true if the query has completed.
    fn pg_is_ready(&self, query: PgAsyncQueryHandle) -> Result<bool, DbError>;

    /// Get the result from a completed async query.
    /// Returns a result handle for SELECT queries or affected row count for DML.
    fn pg_get_async_result(&self, query: PgAsyncQueryHandle) -> Result<PgResultHandle, DbError>;

    /// Cancel an in-progress async query.
    fn pg_cancel_async(&self, query: PgAsyncQueryHandle) -> Result<(), DbError>;

    // --- Async Transaction Operations ---

    /// Begin a transaction asynchronously.
    fn pg_begin_async(&self, conn: PgAsyncConnectionHandle) -> Result<PgAsyncQueryHandle, DbError>;

    /// Commit a transaction asynchronously.
    fn pg_commit_async(&self, conn: PgAsyncConnectionHandle)
    -> Result<PgAsyncQueryHandle, DbError>;

    /// Rollback a transaction asynchronously.
    fn pg_rollback_async(
        &self,
        conn: PgAsyncConnectionHandle,
    ) -> Result<PgAsyncQueryHandle, DbError>;

    // =========================================================================
    // Connection Pool Operations
    // =========================================================================

    // --- SQLite Pool Operations ---

    /// Create a new SQLite connection pool.
    fn sqlite_pool_create(
        &self,
        connection_string: &str,
        config: &PoolConfig,
    ) -> Result<SqlitePoolHandle, DbError>;

    /// Close a SQLite connection pool and all its connections.
    fn sqlite_pool_close(&self, pool: SqlitePoolHandle) -> Result<(), DbError>;

    /// Acquire a connection from a SQLite pool.
    /// Blocks until a connection is available or timeout is reached.
    fn sqlite_pool_acquire(
        &self,
        pool: SqlitePoolHandle,
    ) -> Result<SqliteConnectionHandle, DbError>;

    /// Release a connection back to a SQLite pool.
    fn sqlite_pool_release(
        &self,
        pool: SqlitePoolHandle,
        conn: SqliteConnectionHandle,
    ) -> Result<(), DbError>;

    /// Get statistics for a SQLite pool.
    fn sqlite_pool_stats(&self, pool: SqlitePoolHandle) -> Result<PoolStats, DbError>;

    // --- PostgreSQL Pool Operations ---

    /// Create a new PostgreSQL connection pool.
    fn pg_pool_create(
        &self,
        connection_string: &str,
        config: &PoolConfig,
    ) -> Result<PgPoolHandle, DbError>;

    /// Close a PostgreSQL connection pool and all its connections.
    fn pg_pool_close(&self, pool: PgPoolHandle) -> Result<(), DbError>;

    /// Acquire a connection from a PostgreSQL pool.
    /// Blocks until a connection is available or timeout is reached.
    fn pg_pool_acquire(&self, pool: PgPoolHandle) -> Result<PgConnectionHandle, DbError>;

    /// Release a connection back to a PostgreSQL pool.
    fn pg_pool_release(&self, pool: PgPoolHandle, conn: PgConnectionHandle) -> Result<(), DbError>;

    /// Get statistics for a PostgreSQL pool.
    fn pg_pool_stats(&self, pool: PgPoolHandle) -> Result<PoolStats, DbError>;

    // ========================================================================
    // SQLite Transaction Helpers
    // ========================================================================

    /// Begin a managed transaction scope.
    /// If already in a transaction, creates a savepoint for nested transactions.
    /// Returns a scope ID that must be passed to `sqlite_tx_scope_end`.
    fn sqlite_tx_scope_begin(&self, conn: SqliteConnectionHandle) -> Result<i64, DbError>;

    /// End a managed transaction scope.
    /// If `success` is true, commits (or releases savepoint for nested).
    /// If `success` is false, rolls back (or rolls back to savepoint for nested).
    fn sqlite_tx_scope_end(
        &self,
        conn: SqliteConnectionHandle,
        scope_id: i64,
        success: bool,
    ) -> Result<(), DbError>;

    /// Get the current transaction depth for a SQLite connection.
    /// Returns 0 if not in a transaction.
    fn sqlite_tx_depth(&self, conn: SqliteConnectionHandle) -> Result<u32, DbError>;

    /// Check if a SQLite connection is in a transaction.
    fn sqlite_tx_active(&self, conn: SqliteConnectionHandle) -> Result<bool, DbError>;

    // ========================================================================
    // PostgreSQL Transaction Helpers
    // ========================================================================

    /// Begin a managed transaction scope.
    /// If already in a transaction, creates a savepoint for nested transactions.
    /// Returns a scope ID that must be passed to `pg_tx_scope_end`.
    fn pg_tx_scope_begin(&self, conn: PgConnectionHandle) -> Result<i64, DbError>;

    /// End a managed transaction scope.
    /// If `success` is true, commits (or releases savepoint for nested).
    /// If `success` is false, rolls back (or rolls back to savepoint for nested).
    fn pg_tx_scope_end(
        &self,
        conn: PgConnectionHandle,
        scope_id: i64,
        success: bool,
    ) -> Result<(), DbError>;

    /// Get the current transaction depth for a PostgreSQL connection.
    /// Returns 0 if not in a transaction.
    fn pg_tx_depth(&self, conn: PgConnectionHandle) -> Result<u32, DbError>;

    /// Check if a PostgreSQL connection is in a transaction.
    fn pg_tx_active(&self, conn: PgConnectionHandle) -> Result<bool, DbError>;
}

/// PostgreSQL parameter value.
#[derive(Clone, Debug)]
pub enum PgValue {
    Null,
    Bool(bool),
    Int(i32),
    Int64(i64),
    Float(f32),
    Double(f64),
    Text(String),
    Bytes(Vec<u8>),
}

// ============================================================================
// HostMail Trait
// ============================================================================

/// Trait for host-provided mail operations (SMTP, IMAP, POP3).
///
/// Implementations can provide real mail server access, mock servers for tests,
/// or deny all access for sandboxed execution.
pub trait HostMail: Send + Sync + 'static {
    // =========================================================================
    // SMTP Operations
    // =========================================================================

    /// Connect to an SMTP server.
    fn smtp_connect(
        &self,
        host: &str,
        port: u16,
        use_tls: bool,
        timeout_ms: u64,
    ) -> Result<SmtpConnectionHandle, MailError>;

    /// Upgrade an SMTP connection to TLS (STARTTLS).
    fn smtp_start_tls(&self, conn: SmtpConnectionHandle) -> Result<(), MailError>;

    /// Authenticate with the SMTP server.
    fn smtp_auth(
        &self,
        conn: SmtpConnectionHandle,
        mechanism: &str,
        username: &str,
        password: &str,
    ) -> Result<(), MailError>;

    /// Send EHLO/HELO command.
    fn smtp_ehlo(
        &self,
        conn: SmtpConnectionHandle,
        hostname: &str,
    ) -> Result<Vec<String>, MailError>;

    /// Send QUIT command and disconnect.
    fn smtp_quit(&self, conn: SmtpConnectionHandle) -> Result<(), MailError>;

    /// Send NOOP command to keep connection alive.
    fn smtp_noop(&self, conn: SmtpConnectionHandle) -> Result<(), MailError>;

    /// Reset session state (RSET command).
    fn smtp_reset(&self, conn: SmtpConnectionHandle) -> Result<(), MailError>;

    /// Set sender (MAIL FROM command).
    fn smtp_mail_from(&self, conn: SmtpConnectionHandle, sender: &str) -> Result<(), MailError>;

    /// Add recipient (RCPT TO command).
    fn smtp_rcpt_to(&self, conn: SmtpConnectionHandle, recipient: &str) -> Result<(), MailError>;

    /// Start data transfer (DATA command).
    fn smtp_data(&self, conn: SmtpConnectionHandle) -> Result<(), MailError>;

    /// Send message content.
    fn smtp_send_data(&self, conn: SmtpConnectionHandle, data: &[u8]) -> Result<(), MailError>;

    /// End data transfer (CRLF.CRLF).
    fn smtp_end_data(&self, conn: SmtpConnectionHandle) -> Result<(), MailError>;

    /// Read response from server.
    fn smtp_read_response(&self, conn: SmtpConnectionHandle) -> Result<SmtpResponse, MailError>;

    /// Get server capabilities from last EHLO response.
    fn smtp_get_capabilities(&self, conn: SmtpConnectionHandle) -> Result<Vec<String>, MailError>;

    /// Send a complete message (high-level convenience wrapper).
    fn smtp_send_message(
        &self,
        conn: SmtpConnectionHandle,
        from: &str,
        to: &[&str],
        message_data: &[u8],
    ) -> Result<(), MailError>;

    // =========================================================================
    // IMAP Operations
    // =========================================================================

    /// Connect to an IMAP server.
    fn imap_connect(
        &self,
        host: &str,
        port: u16,
        use_tls: bool,
        timeout_ms: u64,
    ) -> Result<ImapConnectionHandle, MailError>;

    /// Upgrade an IMAP connection to TLS (STARTTLS).
    fn imap_start_tls(&self, conn: ImapConnectionHandle) -> Result<(), MailError>;

    /// Authenticate with username/password.
    fn imap_auth(
        &self,
        conn: ImapConnectionHandle,
        username: &str,
        password: &str,
    ) -> Result<(), MailError>;

    /// Authenticate with OAuth2 token.
    fn imap_auth_oauth(
        &self,
        conn: ImapConnectionHandle,
        username: &str,
        access_token: &str,
    ) -> Result<(), MailError>;

    /// Logout and disconnect.
    fn imap_logout(&self, conn: ImapConnectionHandle) -> Result<(), MailError>;

    /// Send NOOP command.
    fn imap_noop(&self, conn: ImapConnectionHandle) -> Result<(), MailError>;

    /// Get server capabilities.
    fn imap_capability(&self, conn: ImapConnectionHandle) -> Result<Vec<String>, MailError>;

    /// Select a mailbox (read-write).
    fn imap_select(
        &self,
        conn: ImapConnectionHandle,
        mailbox: &str,
    ) -> Result<ImapFolderHandle, MailError>;

    /// Examine a mailbox (read-only).
    fn imap_examine(
        &self,
        conn: ImapConnectionHandle,
        mailbox: &str,
    ) -> Result<ImapFolderHandle, MailError>;

    /// List mailboxes matching a pattern.
    fn imap_list(
        &self,
        conn: ImapConnectionHandle,
        reference: &str,
        pattern: &str,
    ) -> Result<Vec<ImapMailbox>, MailError>;

    /// Fetch message data.
    fn imap_fetch(
        &self,
        conn: ImapConnectionHandle,
        sequence: &str,
        items: &str,
    ) -> Result<Vec<ImapFetchResult>, MailError>;

    /// Search for messages.
    fn imap_search(
        &self,
        conn: ImapConnectionHandle,
        criteria: &str,
    ) -> Result<Vec<u32>, MailError>;

    /// Store message flags.
    fn imap_store(
        &self,
        conn: ImapConnectionHandle,
        sequence: &str,
        flags: &str,
        action: &str, // "+FLAGS", "-FLAGS", "FLAGS"
    ) -> Result<(), MailError>;

    /// Expunge deleted messages.
    fn imap_expunge(&self, conn: ImapConnectionHandle) -> Result<Vec<u32>, MailError>;

    /// Enter IDLE mode for push notifications.
    fn imap_idle(
        &self,
        conn: ImapConnectionHandle,
        timeout_ms: u64,
    ) -> Result<ImapIdleEvent, MailError>;

    // =========================================================================
    // POP3 Operations
    // =========================================================================

    /// Connect to a POP3 server.
    fn pop3_connect(
        &self,
        host: &str,
        port: u16,
        use_tls: bool,
        timeout_ms: u64,
    ) -> Result<Pop3ConnectionHandle, MailError>;

    /// Upgrade a POP3 connection to TLS.
    fn pop3_start_tls(&self, conn: Pop3ConnectionHandle) -> Result<(), MailError>;

    /// Authenticate with USER/PASS.
    fn pop3_auth(
        &self,
        conn: Pop3ConnectionHandle,
        username: &str,
        password: &str,
    ) -> Result<(), MailError>;

    /// Authenticate with APOP (more secure).
    fn pop3_auth_apop(
        &self,
        conn: Pop3ConnectionHandle,
        username: &str,
        password: &str,
    ) -> Result<(), MailError>;

    /// Quit and disconnect.
    fn pop3_quit(&self, conn: Pop3ConnectionHandle) -> Result<(), MailError>;

    /// Get mailbox statistics (STAT).
    fn pop3_stat(&self, conn: Pop3ConnectionHandle) -> Result<Pop3Stat, MailError>;

    /// List messages (LIST).
    fn pop3_list(&self, conn: Pop3ConnectionHandle) -> Result<Vec<Pop3MessageInfo>, MailError>;

    /// Get message UIDs (UIDL).
    fn pop3_uidl(&self, conn: Pop3ConnectionHandle) -> Result<Vec<Pop3Uid>, MailError>;

    /// Retrieve message (RETR).
    fn pop3_retr(&self, conn: Pop3ConnectionHandle, msg_num: u32) -> Result<Vec<u8>, MailError>;

    /// Delete message (DELE).
    fn pop3_dele(&self, conn: Pop3ConnectionHandle, msg_num: u32) -> Result<(), MailError>;

    /// Reset deletion marks (RSET).
    fn pop3_reset(&self, conn: Pop3ConnectionHandle) -> Result<(), MailError>;

    // =========================================================================
    // MIME Operations
    // =========================================================================

    /// Base64 encode data.
    fn mime_base64_encode(&self, data: &[u8]) -> String;

    /// Base64 decode data.
    fn mime_base64_decode(&self, encoded: &str) -> Result<Vec<u8>, MailError>;

    /// Quoted-Printable encode data.
    fn mime_quoted_printable_encode(&self, data: &[u8]) -> String;

    /// Quoted-Printable decode data.
    fn mime_quoted_printable_decode(&self, encoded: &str) -> Result<Vec<u8>, MailError>;

    /// Encode header value (RFC 2047).
    fn mime_encode_header(&self, value: &str, charset: &str) -> String;

    /// Decode header value (RFC 2047).
    fn mime_decode_header(&self, encoded: &str) -> Result<String, MailError>;

    /// Create a new MIME message.
    fn mime_message_new(&self) -> MimeMessageHandle;

    /// Set message header.
    fn mime_message_set_header(
        &self,
        msg: MimeMessageHandle,
        name: &str,
        value: &str,
    ) -> Result<(), MailError>;

    /// Set message body.
    fn mime_message_set_body(
        &self,
        msg: MimeMessageHandle,
        content_type: &str,
        body: &[u8],
    ) -> Result<(), MailError>;

    /// Add attachment to message.
    fn mime_message_add_attachment(
        &self,
        msg: MimeMessageHandle,
        filename: &str,
        content_type: &str,
        data: &[u8],
    ) -> Result<(), MailError>;

    /// Serialize message to RFC 5322 format.
    fn mime_message_serialize(&self, msg: MimeMessageHandle) -> Result<Vec<u8>, MailError>;

    /// Parse RFC 5322 message.
    fn mime_message_parse(&self, data: &[u8]) -> Result<MimeMessageHandle, MailError>;

    /// Get header from parsed message.
    fn mime_message_get_header(
        &self,
        msg: MimeMessageHandle,
        name: &str,
    ) -> Result<Option<String>, MailError>;

    /// Get body from parsed message.
    fn mime_message_get_body(&self, msg: MimeMessageHandle) -> Result<Vec<u8>, MailError>;

    /// Free MIME message resources.
    fn mime_message_free(&self, msg: MimeMessageHandle);

    // =========================================================================
    // TLS Operations (used internally by mail protocols)
    // =========================================================================

    /// Create a new TLS context.
    fn tls_context_new(&self, verify_certs: bool) -> Result<TlsContextHandle, MailError>;

    /// Upgrade a stream to TLS.
    fn tls_upgrade(
        &self,
        ctx: TlsContextHandle,
        hostname: &str,
    ) -> Result<TlsStreamHandle, MailError>;

    /// Close TLS stream.
    fn tls_close(&self, stream: TlsStreamHandle) -> Result<(), MailError>;
}

/// SMTP response from server.
#[derive(Clone, Debug)]
pub struct SmtpResponse {
    /// Response code (e.g., 250, 550).
    pub code: u16,
    /// Response message.
    pub message: String,
    /// Whether this is a multi-line response.
    pub is_multiline: bool,
}

/// IMAP mailbox information.
#[derive(Clone, Debug)]
pub struct ImapMailbox {
    /// Mailbox name.
    pub name: String,
    /// Mailbox attributes (e.g., \Noselect, \HasChildren).
    pub attributes: Vec<String>,
    /// Delimiter character.
    pub delimiter: Option<char>,
}

/// IMAP fetch result.
#[derive(Clone, Debug)]
pub struct ImapFetchResult {
    /// Message sequence number.
    pub seq_num: u32,
    /// Fetched data items.
    pub items: HashMap<String, String>,
}

/// IMAP IDLE event.
#[derive(Clone, Debug)]
pub enum ImapIdleEvent {
    /// New message arrived.
    Exists(u32),
    /// Message expunged.
    Expunge(u32),
    /// Flags changed.
    Fetch { seq_num: u32, flags: Vec<String> },
    /// Timeout (no event).
    Timeout,
}

/// POP3 mailbox statistics.
#[derive(Clone, Debug)]
pub struct Pop3Stat {
    /// Number of messages.
    pub message_count: u32,
    /// Total size in bytes.
    pub total_size: u64,
}

/// POP3 message information.
#[derive(Clone, Debug)]
pub struct Pop3MessageInfo {
    /// Message number (1-based).
    pub msg_num: u32,
    /// Message size in bytes.
    pub size: u64,
}

/// POP3 message UID.
#[derive(Clone, Debug)]
pub struct Pop3Uid {
    /// Message number (1-based).
    pub msg_num: u32,
    /// Unique identifier string.
    pub uid: String,
}

// ============================================================================
// HostGenericCall Trait
// ============================================================================

/// Trait for generic host call operations.
///
/// This allows embedding hosts (rune-server, rune-scene, etc.) to provide
/// their own implementations of host functions called via HostCallGeneric opcode.
/// The VM delegates all HostCallGeneric calls to this trait, making the VM
/// itself generic and not tied to any specific application (like WAID).
///
/// The payload format is JSON: `{"fn": "function_name", "args": {...}}`
pub trait HostGenericCall: Send + Sync + 'static {
    /// Handle a generic host call.
    ///
    /// # Arguments
    /// * `fn_name` - The function name from the JSON payload
    /// * `args` - The args object from the JSON payload
    /// * `local_state` - Shared state storage for passing data between host and VM
    ///
    /// # Returns
    /// A JSON string response to push onto the VM stack.
    fn call(
        &self,
        fn_name: &str,
        args: &serde_json::Value,
        local_state: &std::sync::RwLock<std::collections::HashMap<String, Vec<u8>>>,
    ) -> String;
}

/// Standard host generic call implementation.
///
/// Provides basic functionality:
/// - log_info, log_debug, log_warn, log_error: Console logging
/// - get_time_ms: Current timestamp in milliseconds
///
/// For WAID/Rune-specific functions (set_data, get_current_event, load_partial),
/// use ServerHostGenericCall from rune-server-arth or a custom implementation.
pub struct StdHostGenericCall;

impl StdHostGenericCall {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StdHostGenericCall {
    fn default() -> Self {
        Self::new()
    }
}

impl HostGenericCall for StdHostGenericCall {
    fn call(
        &self,
        fn_name: &str,
        args: &serde_json::Value,
        local_state: &std::sync::RwLock<std::collections::HashMap<String, Vec<u8>>>,
    ) -> String {
        match fn_name {
            "log_info" | "log_debug" | "log_warn" | "log_error" => {
                let msg = args.get("message").and_then(|v| v.as_str()).unwrap_or("");
                eprintln!("[arth:{}] {}", fn_name, msg);
                "{}".to_string()
            }
            "get_time_ms" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                ms.to_string()
            }
            "core_dispatch_mutation" => {
                // Append mutation JSON to __pending_mutations in local_state.
                // The embedding runtime (e.g. ArthRuntime) drains these after execution.
                if let Ok(mut state) = local_state.write() {
                    let key = "__pending_mutations".to_string();
                    let mut mutations: Vec<serde_json::Value> = state
                        .get(&key)
                        .and_then(|bytes| serde_json::from_slice(bytes).ok())
                        .unwrap_or_default();
                    mutations.push(args.clone());
                    if let Ok(bytes) = serde_json::to_vec(&mutations) {
                        state.insert(key, bytes);
                    }
                }
                "{}".to_string()
            }
            "set_local_state" => {
                let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
                let value = args.get("value").and_then(|v| v.as_str()).unwrap_or("");
                if !key.is_empty() {
                    if let Ok(mut state) = local_state.write() {
                        state.insert(key.to_string(), value.as_bytes().to_vec());
                    }
                }
                "{}".to_string()
            }
            "get_local_state" => {
                let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
                if let Ok(state) = local_state.read() {
                    if let Some(bytes) = state.get(key) {
                        let value = String::from_utf8_lossy(bytes);
                        format!(
                            "{{\"has_value\":true,\"value\":{}}}",
                            serde_json::json!(value.as_ref())
                        )
                    } else {
                        "{\"has_value\":false,\"value\":null}".to_string()
                    }
                } else {
                    "{\"has_value\":false,\"value\":null}".to_string()
                }
            }
            _ => {
                format!("{{\"error\":\"unknown function: {}\"}}", fn_name)
            }
        }
    }
}

// ============================================================================
// StdHostIo Implementation
// ============================================================================

/// Standard host I/O implementation using arth-rt C FFI layer.
///
/// This implementation uses the arth_rt_* functions which wrap libc directly,
/// making the same code usable for both VM and native compilation.
pub struct StdHostIo;

impl StdHostIo {
    pub fn new() -> Self {
        Self
    }

    /// Convert an arth_rt error code to IoError
    fn error_from_code(code: i32) -> IoError {
        use arth_rt::error::ErrorCode;

        let kind = match code {
            c if c == ErrorCode::NotFound.as_i32() => IoErrorKind::NotFound,
            c if c == ErrorCode::PermissionDenied.as_i32() => IoErrorKind::PermissionDenied,
            c if c == ErrorCode::AlreadyExists.as_i32() => IoErrorKind::AlreadyExists,
            c if c == ErrorCode::InvalidArgument.as_i32() => IoErrorKind::InvalidInput,
            c if c == ErrorCode::InvalidHandle.as_i32() => IoErrorKind::InvalidHandle,
            c if c == ErrorCode::Eof.as_i32() => IoErrorKind::UnexpectedEof,
            c if c == ErrorCode::Interrupted.as_i32() => IoErrorKind::Interrupted,
            c if c == ErrorCode::NotSupported.as_i32() => IoErrorKind::NotSupported,
            c if c == ErrorCode::CapabilityDenied.as_i32() => IoErrorKind::CapabilityDenied,
            _ => IoErrorKind::Other,
        };

        // Try to get detailed error message from arth_rt
        let msg =
            arth_rt::error::get_last_error().unwrap_or_else(|| format!("IO error (code {})", code));

        IoError::new(kind, msg)
    }

    /// Check result and convert to IoError if negative
    fn check_result(result: i32) -> Result<(), IoError> {
        if result < 0 {
            Err(Self::error_from_code(result))
        } else {
            Ok(())
        }
    }

    /// Check i64 result and convert to IoError if negative
    fn check_result_i64(result: i64) -> Result<i64, IoError> {
        if result < 0 {
            Err(Self::error_from_code(result as i32))
        } else {
            Ok(result)
        }
    }
}

impl Default for StdHostIo {
    fn default() -> Self {
        Self::new()
    }
}

impl HostIo for StdHostIo {
    fn file_open(&self, path: &str, mode: FileMode) -> Result<FileHandle, IoError> {
        use arth_rt::io::{
            FILE_MODE_APPEND, FILE_MODE_READ, FILE_MODE_READ_WRITE, FILE_MODE_WRITE,
            arth_rt_file_open,
        };

        let rt_mode = match mode {
            FileMode::Read => FILE_MODE_READ,
            FileMode::Write => FILE_MODE_WRITE,
            FileMode::Append => FILE_MODE_APPEND,
            FileMode::ReadWrite => FILE_MODE_READ_WRITE,
        };

        let fd = arth_rt_file_open(path.as_ptr(), path.len(), rt_mode);
        let fd = Self::check_result_i64(fd)?;
        Ok(FileHandle(fd))
    }

    fn file_close(&self, handle: FileHandle) -> Result<(), IoError> {
        let result = arth_rt::io::arth_rt_file_close(handle.0);
        Self::check_result(result)
    }

    fn file_read(&self, handle: FileHandle, max_bytes: usize) -> Result<Vec<u8>, IoError> {
        let mut buf = vec![0u8; max_bytes];
        let n = arth_rt::io::arth_rt_file_read(handle.0, buf.as_mut_ptr(), max_bytes);
        let n = Self::check_result_i64(n)?;
        buf.truncate(n as usize);
        Ok(buf)
    }

    fn file_write(&self, handle: FileHandle, data: &[u8]) -> Result<usize, IoError> {
        let n = arth_rt::io::arth_rt_file_write(handle.0, data.as_ptr(), data.len());
        let n = Self::check_result_i64(n)?;
        Ok(n as usize)
    }

    fn file_flush(&self, handle: FileHandle) -> Result<(), IoError> {
        let result = arth_rt::io::arth_rt_file_flush(handle.0);
        Self::check_result(result)
    }

    fn file_seek(&self, handle: FileHandle, pos: SeekPosition) -> Result<i64, IoError> {
        use arth_rt::io::{SEEK_CUR, SEEK_END, SEEK_SET};

        let (offset, whence) = match pos {
            SeekPosition::Start(n) => (n as i64, SEEK_SET),
            SeekPosition::Current(n) => (n, SEEK_CUR),
            SeekPosition::End(n) => (n, SEEK_END),
        };

        let new_pos = arth_rt::io::arth_rt_file_seek(handle.0, offset, whence);
        Self::check_result_i64(new_pos)
    }

    fn file_size(&self, handle: FileHandle) -> Result<i64, IoError> {
        let size = arth_rt::io::arth_rt_file_size(handle.0);
        Self::check_result_i64(size)
    }

    fn file_exists(&self, path: &str) -> Result<bool, IoError> {
        let result = arth_rt::io::arth_rt_file_exists(path.as_ptr(), path.len());
        if result < 0 {
            Err(Self::error_from_code(result))
        } else {
            Ok(result == 1)
        }
    }

    fn file_delete(&self, path: &str) -> Result<(), IoError> {
        let result = arth_rt::io::arth_rt_file_delete(path.as_ptr(), path.len());
        Self::check_result(result)
    }

    fn file_copy(&self, src: &str, dst: &str) -> Result<(), IoError> {
        let result =
            arth_rt::io::arth_rt_file_copy(src.as_ptr(), src.len(), dst.as_ptr(), dst.len());
        Self::check_result(result)
    }

    fn file_move(&self, src: &str, dst: &str) -> Result<(), IoError> {
        let result =
            arth_rt::io::arth_rt_file_move(src.as_ptr(), src.len(), dst.as_ptr(), dst.len());
        Self::check_result(result)
    }

    fn dir_create(&self, path: &str) -> Result<(), IoError> {
        let result = arth_rt::io::arth_rt_dir_create(path.as_ptr(), path.len());
        Self::check_result(result)
    }

    fn dir_create_all(&self, path: &str) -> Result<(), IoError> {
        let result = arth_rt::io::arth_rt_dir_create_all(path.as_ptr(), path.len());
        Self::check_result(result)
    }

    fn dir_delete(&self, path: &str) -> Result<(), IoError> {
        let result = arth_rt::io::arth_rt_dir_delete(path.as_ptr(), path.len());
        Self::check_result(result)
    }

    fn dir_list(&self, path: &str) -> Result<Vec<String>, IoError> {
        let handle = arth_rt::io::arth_rt_dir_list(path.as_ptr(), path.len());
        let handle = Self::check_result_i64(handle)?;

        let mut names = Vec::new();
        let mut buf = [0u8; 1024];

        loop {
            let len = arth_rt::io::arth_rt_dir_next(handle, buf.as_mut_ptr(), buf.len());
            if len == 0 {
                break; // No more entries
            }
            if len < 0 {
                // Close handle before returning error
                arth_rt::io::arth_rt_dir_close(handle);
                return Err(Self::error_from_code(len));
            }

            if let Ok(name) = std::str::from_utf8(&buf[..len as usize]) {
                names.push(name.to_string());
            }
        }

        arth_rt::io::arth_rt_dir_close(handle);
        Ok(names)
    }

    fn dir_exists(&self, path: &str) -> Result<bool, IoError> {
        let result = arth_rt::io::arth_rt_dir_exists(path.as_ptr(), path.len());
        if result < 0 {
            Err(Self::error_from_code(result))
        } else {
            Ok(result == 1)
        }
    }

    fn is_dir(&self, path: &str) -> Result<bool, IoError> {
        let result = arth_rt::io::arth_rt_is_dir(path.as_ptr(), path.len());
        if result < 0 {
            Err(Self::error_from_code(result))
        } else {
            Ok(result == 1)
        }
    }

    fn is_file(&self, path: &str) -> Result<bool, IoError> {
        let result = arth_rt::io::arth_rt_is_file(path.as_ptr(), path.len());
        if result < 0 {
            Err(Self::error_from_code(result))
        } else {
            Ok(result == 1)
        }
    }

    fn path_absolute(&self, path: &str) -> Result<String, IoError> {
        // arth_rt doesn't have path_absolute yet, use std::fs::canonicalize
        // TODO: Add arth_rt_path_absolute when needed
        let abs = std::fs::canonicalize(path)?;
        abs.to_str()
            .map(|s| s.to_string())
            .ok_or_else(|| IoError::new(IoErrorKind::InvalidData, "path is not valid UTF-8"))
    }

    fn console_read_line(&self) -> Result<String, IoError> {
        let mut buf = vec![0u8; 4096];
        let n = arth_rt::io::arth_rt_console_read_line(buf.as_mut_ptr(), buf.len());
        let n = Self::check_result_i64(n)?;

        // Remove trailing newline
        let mut len = n as usize;
        if len > 0 && buf[len - 1] == b'\n' {
            len -= 1;
            if len > 0 && buf[len - 1] == b'\r' {
                len -= 1;
            }
        }

        String::from_utf8(buf[..len].to_vec())
            .map_err(|_| IoError::new(IoErrorKind::InvalidData, "input is not valid UTF-8"))
    }

    fn console_write(&self, s: &str) {
        arth_rt::io::arth_rt_console_write(s.as_ptr(), s.len());
    }

    fn console_write_err(&self, s: &str) {
        arth_rt::io::arth_rt_console_write_err(s.as_ptr(), s.len());
    }
}

// ============================================================================
// NoHostIo Implementation (Sandboxed)
// ============================================================================

/// Host IO implementation that denies all operations.
/// Use this for sandboxed guest execution.
pub struct NoHostIo;

impl NoHostIo {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoHostIo {
    fn default() -> Self {
        Self::new()
    }
}

impl HostIo for NoHostIo {
    fn file_open(&self, _path: &str, _mode: FileMode) -> Result<FileHandle, IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn file_close(&self, _handle: FileHandle) -> Result<(), IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn file_read(&self, _handle: FileHandle, _max_bytes: usize) -> Result<Vec<u8>, IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn file_write(&self, _handle: FileHandle, _data: &[u8]) -> Result<usize, IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn file_flush(&self, _handle: FileHandle) -> Result<(), IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn file_seek(&self, _handle: FileHandle, _pos: SeekPosition) -> Result<i64, IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn file_size(&self, _handle: FileHandle) -> Result<i64, IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn file_exists(&self, _path: &str) -> Result<bool, IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn file_delete(&self, _path: &str) -> Result<(), IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn file_copy(&self, _src: &str, _dst: &str) -> Result<(), IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn file_move(&self, _src: &str, _dst: &str) -> Result<(), IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn dir_create(&self, _path: &str) -> Result<(), IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn dir_create_all(&self, _path: &str) -> Result<(), IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn dir_delete(&self, _path: &str) -> Result<(), IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn dir_list(&self, _path: &str) -> Result<Vec<String>, IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn dir_exists(&self, _path: &str) -> Result<bool, IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn is_dir(&self, _path: &str) -> Result<bool, IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn is_file(&self, _path: &str) -> Result<bool, IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn path_absolute(&self, _path: &str) -> Result<String, IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn console_read_line(&self) -> Result<String, IoError> {
        Err(IoError::capability_denied("io"))
    }

    fn console_write(&self, _s: &str) {
        // Silently ignore console writes in sandboxed mode
    }

    fn console_write_err(&self, _s: &str) {
        // Silently ignore console writes in sandboxed mode
    }
}

// ============================================================================
// NoHostNet Implementation (Sandboxed)
// ============================================================================

/// Host networking implementation that denies all operations.
/// Use this for sandboxed guest execution.
pub struct NoHostNet;

impl NoHostNet {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoHostNet {
    fn default() -> Self {
        Self::new()
    }
}

impl HostNet for NoHostNet {
    fn http_fetch(
        &self,
        _url: &str,
        _method: &str,
        _headers: &HashMap<String, String>,
        _body: &[u8],
    ) -> Result<TaskHandle, NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn http_serve(&self, _port: u16) -> Result<HttpServerHandle, NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn http_accept(&self, _server: HttpServerHandle) -> Result<TaskHandle, NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn http_respond(
        &self,
        _request: HttpRequestHandle,
        _status: u16,
        _headers: &HashMap<String, String>,
        _body: &[u8],
    ) -> Result<(), NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn ws_serve(&self, _port: u16, _path: &str) -> Result<WsServerHandle, NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn ws_accept(&self, _server: WsServerHandle) -> Result<WsConnectionHandle, NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn ws_send_text(&self, _conn: WsConnectionHandle, _text: &str) -> Result<(), NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn ws_send_binary(&self, _conn: WsConnectionHandle, _data: &[u8]) -> Result<(), NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn ws_recv(&self, _conn: WsConnectionHandle) -> Result<WsMessage, NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn ws_close(
        &self,
        _conn: WsConnectionHandle,
        _code: u16,
        _reason: &str,
    ) -> Result<(), NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn ws_is_open(&self, _conn: WsConnectionHandle) -> Result<bool, NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn sse_serve(&self, _port: u16, _path: &str) -> Result<SseServerHandle, NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn sse_accept(&self, _server: SseServerHandle) -> Result<SseEmitterHandle, NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn sse_send(
        &self,
        _emitter: SseEmitterHandle,
        _event: &str,
        _data: &str,
        _id: &str,
    ) -> Result<(), NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn sse_close(&self, _emitter: SseEmitterHandle) -> Result<(), NetError> {
        Err(NetError::capability_denied("net"))
    }

    fn sse_is_open(&self, _emitter: SseEmitterHandle) -> Result<bool, NetError> {
        Err(NetError::capability_denied("net"))
    }
}

// ============================================================================
// StdHostNet Implementation (Real Networking via arth-rt)
// ============================================================================

/// Host networking implementation using arth-rt C FFI wrappers.
/// This enables the same implementation to work for VM mode and native compilation.
pub struct StdHostNet {
    /// Counter for generating task handles (for async responses)
    next_task_handle: std::sync::atomic::AtomicI64,
    /// Completed HTTP responses stored by task handle
    http_responses: Mutex<HashMap<i64, HttpResponseData>>,
    /// HTTP servers by handle
    http_servers: Mutex<HashMap<i64, HttpServerData>>,
    /// HTTP requests by handle (pending requests to be responded to)
    http_requests: Mutex<HashMap<i64, HttpRequestData>>,
    /// WebSocket servers by handle
    ws_servers: Mutex<HashMap<i64, WsServerData>>,
    /// WebSocket connections by handle
    ws_connections: Mutex<HashMap<i64, WsConnectionData>>,
    /// SSE servers by handle
    sse_servers: Mutex<HashMap<i64, SseServerData>>,
    /// SSE emitters (client connections) by handle
    sse_emitters: Mutex<HashMap<i64, SseEmitterData>>,
}

/// Stored HTTP response data
struct HttpResponseData {
    status: i32,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// HTTP server state
struct HttpServerData {
    socket_handle: i64,
    port: u16,
}

/// HTTP request state (for responding)
struct HttpRequestData {
    client_socket: i64,
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// WebSocket server state
struct WsServerData {
    socket_handle: i64,
    port: u16,
    path: String,
}

/// WebSocket connection state
struct WsConnectionData {
    socket_handle: i64,
    is_open: bool,
}

/// SSE server state
struct SseServerData {
    socket_handle: i64,
    port: u16,
    path: String,
}

/// SSE emitter (client connection) state
struct SseEmitterData {
    socket_handle: i64,
    is_open: bool,
}

impl StdHostNet {
    pub fn new() -> Self {
        Self {
            next_task_handle: std::sync::atomic::AtomicI64::new(1),
            http_responses: Mutex::new(HashMap::new()),
            http_servers: Mutex::new(HashMap::new()),
            http_requests: Mutex::new(HashMap::new()),
            ws_servers: Mutex::new(HashMap::new()),
            ws_connections: Mutex::new(HashMap::new()),
            sse_servers: Mutex::new(HashMap::new()),
            sse_emitters: Mutex::new(HashMap::new()),
        }
    }

    fn next_task_handle(&self) -> i64 {
        self.next_task_handle
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Create a listening socket on the given port
    fn create_server_socket(&self, port: u16) -> Result<i64, NetError> {
        // Create TCP socket (AF_INET = 2, SOCK_STREAM = 1)
        let sock = arth_rt::net::arth_rt_socket_create(2, 1, 0);
        if sock < 0 {
            return Err(NetError::connection_failed("Failed to create socket"));
        }

        // Set SO_REUSEADDR
        let result = arth_rt::net::arth_rt_socket_setsockopt_int(
            sock,
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            1,
        );
        if result < 0 {
            arth_rt::net::arth_rt_socket_close(sock);
            return Err(NetError::connection_failed("Failed to set socket options"));
        }

        // Bind to port
        let result = arth_rt::net::arth_rt_socket_bind_port(sock, port);
        if result < 0 {
            arth_rt::net::arth_rt_socket_close(sock);
            return Err(NetError::connection_failed(&format!(
                "Failed to bind to port {}",
                port
            )));
        }

        // Listen
        let result = arth_rt::net::arth_rt_socket_listen(sock, 128);
        if result < 0 {
            arth_rt::net::arth_rt_socket_close(sock);
            return Err(NetError::connection_failed("Failed to listen on socket"));
        }

        Ok(sock)
    }

    /// Read a line from socket (up to \r\n or \n)
    fn read_line(&self, sock: i64, buf: &mut Vec<u8>) -> Result<usize, NetError> {
        buf.clear();
        let mut byte = [0u8; 1];
        loop {
            let n = arth_rt::net::arth_rt_socket_recv(sock, byte.as_mut_ptr(), 1, 0);
            if n <= 0 {
                if buf.is_empty() {
                    return Err(NetError::connection_failed("Connection closed"));
                }
                break;
            }
            if byte[0] == b'\n' {
                break;
            }
            if byte[0] != b'\r' {
                buf.push(byte[0]);
            }
        }
        Ok(buf.len())
    }

    /// Read exactly n bytes from socket
    fn read_exact(&self, sock: i64, buf: &mut [u8]) -> Result<(), NetError> {
        let mut read = 0;
        while read < buf.len() {
            let n = arth_rt::net::arth_rt_socket_recv(
                sock,
                buf[read..].as_mut_ptr(),
                buf.len() - read,
                0,
            );
            if n <= 0 {
                return Err(NetError::connection_failed("Connection closed"));
            }
            read += n as usize;
        }
        Ok(())
    }

    /// Write all bytes to socket
    fn write_all(&self, sock: i64, data: &[u8]) -> Result<(), NetError> {
        let mut written = 0;
        while written < data.len() {
            let n = arth_rt::net::arth_rt_socket_send(
                sock,
                data[written..].as_ptr(),
                data.len() - written,
                0,
            );
            if n <= 0 {
                return Err(NetError::connection_failed("Failed to write to socket"));
            }
            written += n as usize;
        }
        Ok(())
    }

    /// Parse HTTP request from socket
    fn parse_http_request(&self, sock: i64) -> Result<HttpRequestData, NetError> {
        let mut line_buf = Vec::with_capacity(1024);

        // Read request line
        self.read_line(sock, &mut line_buf)?;
        let request_line = String::from_utf8_lossy(&line_buf).to_string();
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            return Err(NetError::protocol_error("Invalid request line"));
        }
        let method = parts[0].to_string();
        let path = parts[1].to_string();

        // Read headers
        let mut headers = HashMap::new();
        let mut content_length = 0usize;
        loop {
            self.read_line(sock, &mut line_buf)?;
            if line_buf.is_empty() {
                break;
            }
            let header_line = String::from_utf8_lossy(&line_buf).to_string();
            if let Some(pos) = header_line.find(':') {
                let name = header_line[..pos].trim().to_string();
                let value = header_line[pos + 1..].trim().to_string();
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.parse().unwrap_or(0);
                }
                headers.insert(name, value);
            }
        }

        // Read body if present
        let body = if content_length > 0 {
            let mut body = vec![0u8; content_length];
            self.read_exact(sock, &mut body)?;
            body
        } else {
            Vec::new()
        };

        Ok(HttpRequestData {
            client_socket: sock,
            method,
            path,
            headers,
            body,
        })
    }

    /// Send a WebSocket frame
    fn ws_send_frame(
        &self,
        conn: WsConnectionHandle,
        opcode: u8,
        data: &[u8],
    ) -> Result<(), NetError> {
        let connections = self.ws_connections.lock().unwrap();
        let conn_data = connections.get(&conn.0).ok_or_else(|| {
            NetError::new(
                NetErrorKind::InvalidHandle,
                "Invalid WebSocket connection handle",
            )
        })?;

        if !conn_data.is_open {
            return Err(NetError::connection_failed(
                "WebSocket connection is closed",
            ));
        }

        let sock = conn_data.socket_handle;
        drop(connections);

        // Build frame header
        let mut frame = Vec::with_capacity(10 + data.len());

        // First byte: FIN + opcode
        frame.push(0x80 | opcode);

        // Second byte: payload length (server doesn't mask)
        if data.len() < 126 {
            frame.push(data.len() as u8);
        } else if data.len() < 65536 {
            frame.push(126);
            frame.extend_from_slice(&(data.len() as u16).to_be_bytes());
        } else {
            frame.push(127);
            frame.extend_from_slice(&(data.len() as u64).to_be_bytes());
        }

        // Payload
        frame.extend_from_slice(data);

        self.write_all(sock, &frame)
    }
}

impl Default for StdHostNet {
    fn default() -> Self {
        Self::new()
    }
}

impl HostNet for StdHostNet {
    fn http_fetch(
        &self,
        url: &str,
        method: &str,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<TaskHandle, NetError> {
        // Build headers string
        let mut headers_str = String::new();
        for (key, value) in headers {
            headers_str.push_str(&format!("{}: {}\r\n", key, value));
        }

        // Parse URL to get host, port, path, and TLS flag
        let (host, port, path, use_tls) = parse_http_url(url)?;

        // Connect to server
        let session = arth_rt::http::arth_rt_http_connect(
            host.as_ptr(),
            host.len(),
            port,
            if use_tls { 1 } else { 0 },
        );
        if session < 0 {
            return Err(NetError::connection_failed(&format!(
                "Failed to connect: error code {}",
                session
            )));
        }

        // Send request
        let response_handle = arth_rt::http::arth_rt_http_request(
            session,
            method.as_ptr(),
            method.len(),
            path.as_ptr(),
            path.len(),
            if headers_str.is_empty() {
                std::ptr::null()
            } else {
                headers_str.as_ptr()
            },
            headers_str.len(),
            if body.is_empty() {
                std::ptr::null()
            } else {
                body.as_ptr()
            },
            body.len(),
        );

        // Close session (we don't reuse connections for simplicity)
        arth_rt::http::arth_rt_http_close(session);

        if response_handle < 0 {
            return Err(NetError::connection_failed(&format!(
                "HTTP request failed: error code {}",
                response_handle
            )));
        }

        // Read response data
        let status = arth_rt::http::arth_rt_http_response_status(response_handle);
        if status < 0 {
            arth_rt::http::arth_rt_http_response_free(response_handle);
            return Err(NetError::protocol_error("Failed to get response status"));
        }

        // Read headers
        let mut resp_headers = HashMap::new();
        let header_count = arth_rt::http::arth_rt_http_response_header_count(response_handle);
        if header_count > 0 {
            let mut name_buf = vec![0u8; 256];
            let mut value_buf = vec![0u8; 4096];
            for i in 0..header_count {
                let name_len = arth_rt::http::arth_rt_http_response_header_name(
                    response_handle,
                    i,
                    name_buf.as_mut_ptr(),
                    name_buf.len(),
                );
                if name_len > 0 {
                    let name = String::from_utf8_lossy(&name_buf[..name_len as usize]).to_string();
                    let value_len = arth_rt::http::arth_rt_http_response_header_value(
                        response_handle,
                        i,
                        value_buf.as_mut_ptr(),
                        value_buf.len(),
                    );
                    if value_len > 0 {
                        let value =
                            String::from_utf8_lossy(&value_buf[..value_len as usize]).to_string();
                        resp_headers.insert(name, value);
                    }
                }
            }
        }

        // Read body
        let body_len = arth_rt::http::arth_rt_http_response_body_len(response_handle);
        let resp_body = if body_len > 0 {
            let mut body_buf = vec![0u8; body_len as usize];
            let read_len = arth_rt::http::arth_rt_http_response_body(
                response_handle,
                body_buf.as_mut_ptr(),
                body_buf.len(),
            );
            if read_len > 0 {
                body_buf.truncate(read_len as usize);
                body_buf
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        // Free the response handle
        arth_rt::http::arth_rt_http_response_free(response_handle);

        // Store response and return task handle
        let task_handle = self.next_task_handle();
        self.http_responses.lock().unwrap().insert(
            task_handle,
            HttpResponseData {
                status,
                headers: resp_headers,
                body: resp_body,
            },
        );

        Ok(TaskHandle(task_handle))
    }

    fn http_serve(&self, port: u16) -> Result<HttpServerHandle, NetError> {
        let socket_handle = self.create_server_socket(port)?;
        let handle = self.next_task_handle();
        self.http_servers.lock().unwrap().insert(
            handle,
            HttpServerData {
                socket_handle,
                port,
            },
        );
        Ok(HttpServerHandle(handle))
    }

    fn http_accept(&self, server: HttpServerHandle) -> Result<TaskHandle, NetError> {
        let servers = self.http_servers.lock().unwrap();
        let server_data = servers.get(&server.0).ok_or_else(|| {
            NetError::new(NetErrorKind::InvalidHandle, "Invalid HTTP server handle")
        })?;

        // Accept connection
        let client_sock = arth_rt::net::arth_rt_socket_accept(server_data.socket_handle);
        if client_sock < 0 {
            return Err(NetError::connection_failed("Failed to accept connection"));
        }
        drop(servers);

        // Parse HTTP request
        let request = match self.parse_http_request(client_sock) {
            Ok(req) => req,
            Err(e) => {
                arth_rt::net::arth_rt_socket_close(client_sock);
                return Err(e);
            }
        };

        // Store request and return handle
        let handle = self.next_task_handle();
        self.http_requests.lock().unwrap().insert(handle, request);
        Ok(TaskHandle(handle))
    }

    fn http_respond(
        &self,
        request: HttpRequestHandle,
        status: u16,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<(), NetError> {
        let mut requests = self.http_requests.lock().unwrap();
        let req = requests.remove(&request.0).ok_or_else(|| {
            NetError::new(NetErrorKind::InvalidHandle, "Invalid HTTP request handle")
        })?;

        // Build response
        let status_text = match status {
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            301 => "Moved Permanently",
            302 => "Found",
            304 => "Not Modified",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            500 => "Internal Server Error",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            _ => "Unknown",
        };

        let mut response = format!("HTTP/1.1 {} {}\r\n", status, status_text);

        // Add Content-Length if not present
        let has_content_length = headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("content-length"));
        if !has_content_length {
            response.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }

        // Add headers
        for (name, value) in headers {
            response.push_str(&format!("{}: {}\r\n", name, value));
        }
        response.push_str("\r\n");

        // Send response
        let result = self.write_all(req.client_socket, response.as_bytes());
        if result.is_ok() && !body.is_empty() {
            let _ = self.write_all(req.client_socket, body);
        }

        // Close connection
        arth_rt::net::arth_rt_socket_close(req.client_socket);

        result
    }

    fn ws_serve(&self, port: u16, path: &str) -> Result<WsServerHandle, NetError> {
        let socket_handle = self.create_server_socket(port)?;
        let handle = self.next_task_handle();
        self.ws_servers.lock().unwrap().insert(
            handle,
            WsServerData {
                socket_handle,
                port,
                path: path.to_string(),
            },
        );
        Ok(WsServerHandle(handle))
    }

    fn ws_accept(&self, server: WsServerHandle) -> Result<WsConnectionHandle, NetError> {
        let servers = self.ws_servers.lock().unwrap();
        let server_data = servers.get(&server.0).ok_or_else(|| {
            NetError::new(
                NetErrorKind::InvalidHandle,
                "Invalid WebSocket server handle",
            )
        })?;
        let expected_path = server_data.path.clone();

        // Accept TCP connection
        let client_sock = arth_rt::net::arth_rt_socket_accept(server_data.socket_handle);
        if client_sock < 0 {
            return Err(NetError::connection_failed("Failed to accept connection"));
        }
        drop(servers);

        // Parse HTTP upgrade request
        let request = match self.parse_http_request(client_sock) {
            Ok(req) => req,
            Err(e) => {
                arth_rt::net::arth_rt_socket_close(client_sock);
                return Err(e);
            }
        };

        // Verify this is a WebSocket upgrade request
        let upgrade = request
            .headers
            .get("Upgrade")
            .or_else(|| request.headers.get("upgrade"));
        let connection = request
            .headers
            .get("Connection")
            .or_else(|| request.headers.get("connection"));
        let ws_key = request
            .headers
            .get("Sec-WebSocket-Key")
            .or_else(|| request.headers.get("sec-websocket-key"));

        let is_upgrade = upgrade
            .map(|u| u.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false);
        let is_connection_upgrade = connection
            .map(|c| c.to_lowercase().contains("upgrade"))
            .unwrap_or(false);

        if !is_upgrade || !is_connection_upgrade || ws_key.is_none() {
            // Send 400 Bad Request
            let response = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";
            let _ = self.write_all(client_sock, response);
            arth_rt::net::arth_rt_socket_close(client_sock);
            return Err(NetError::protocol_error("Not a WebSocket upgrade request"));
        }

        // Check path matches
        if !expected_path.is_empty() && request.path != expected_path {
            let response = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
            let _ = self.write_all(client_sock, response);
            arth_rt::net::arth_rt_socket_close(client_sock);
            return Err(NetError::protocol_error("WebSocket path mismatch"));
        }

        // Calculate Sec-WebSocket-Accept
        let ws_key = ws_key.unwrap();
        let accept_key = compute_ws_accept_key(ws_key);

        // Send upgrade response
        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {}\r\n\r\n",
            accept_key
        );
        if self.write_all(client_sock, response.as_bytes()).is_err() {
            arth_rt::net::arth_rt_socket_close(client_sock);
            return Err(NetError::connection_failed(
                "Failed to send upgrade response",
            ));
        }

        // Store connection
        let handle = self.next_task_handle();
        self.ws_connections.lock().unwrap().insert(
            handle,
            WsConnectionData {
                socket_handle: client_sock,
                is_open: true,
            },
        );
        Ok(WsConnectionHandle(handle))
    }

    fn ws_send_text(&self, conn: WsConnectionHandle, text: &str) -> Result<(), NetError> {
        self.ws_send_frame(conn, 0x01, text.as_bytes())
    }

    fn ws_send_binary(&self, conn: WsConnectionHandle, data: &[u8]) -> Result<(), NetError> {
        self.ws_send_frame(conn, 0x02, data)
    }

    fn ws_recv(&self, conn: WsConnectionHandle) -> Result<WsMessage, NetError> {
        let connections = self.ws_connections.lock().unwrap();
        let conn_data = connections.get(&conn.0).ok_or_else(|| {
            NetError::new(
                NetErrorKind::InvalidHandle,
                "Invalid WebSocket connection handle",
            )
        })?;

        if !conn_data.is_open {
            return Err(NetError::connection_failed(
                "WebSocket connection is closed",
            ));
        }

        let sock = conn_data.socket_handle;
        drop(connections);

        // Read WebSocket frame
        let mut header = [0u8; 2];
        self.read_exact(sock, &mut header)?;

        let fin = (header[0] & 0x80) != 0;
        let opcode = header[0] & 0x0F;
        let masked = (header[1] & 0x80) != 0;
        let mut payload_len = (header[1] & 0x7F) as u64;

        // Extended payload length
        if payload_len == 126 {
            let mut ext = [0u8; 2];
            self.read_exact(sock, &mut ext)?;
            payload_len = u16::from_be_bytes(ext) as u64;
        } else if payload_len == 127 {
            let mut ext = [0u8; 8];
            self.read_exact(sock, &mut ext)?;
            payload_len = u64::from_be_bytes(ext);
        }

        // Read masking key if present
        let mask = if masked {
            let mut m = [0u8; 4];
            self.read_exact(sock, &mut m)?;
            Some(m)
        } else {
            None
        };

        // Read payload
        let mut payload = vec![0u8; payload_len as usize];
        if payload_len > 0 {
            self.read_exact(sock, &mut payload)?;
        }

        // Unmask if needed
        if let Some(mask) = mask {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[i % 4];
            }
        }

        // Handle opcode
        match opcode {
            0x01 => {
                // Text
                let text = String::from_utf8_lossy(&payload).to_string();
                Ok(WsMessage::Text(text))
            }
            0x02 => {
                // Binary
                Ok(WsMessage::Binary(payload))
            }
            0x08 => {
                // Close
                let code = if payload.len() >= 2 {
                    u16::from_be_bytes([payload[0], payload[1]])
                } else {
                    1000
                };
                let reason = if payload.len() > 2 {
                    String::from_utf8_lossy(&payload[2..]).to_string()
                } else {
                    String::new()
                };
                // Mark as closed
                if let Some(c) = self.ws_connections.lock().unwrap().get_mut(&conn.0) {
                    c.is_open = false;
                }
                Ok(WsMessage::Close(code, reason))
            }
            0x09 => {
                // Ping - send pong automatically
                let _ = self.ws_send_frame(conn, 0x0A, &payload);
                // Recurse to get next message
                if fin {
                    self.ws_recv(conn)
                } else {
                    Ok(WsMessage::Binary(payload))
                }
            }
            0x0A => {
                // Pong - ignore and get next message
                if fin {
                    self.ws_recv(conn)
                } else {
                    Ok(WsMessage::Binary(payload))
                }
            }
            _ => Err(NetError::protocol_error(&format!(
                "Unknown WebSocket opcode: {}",
                opcode
            ))),
        }
    }

    fn ws_close(&self, conn: WsConnectionHandle, code: u16, reason: &str) -> Result<(), NetError> {
        // Send close frame
        let mut payload = Vec::with_capacity(2 + reason.len());
        payload.extend_from_slice(&code.to_be_bytes());
        payload.extend_from_slice(reason.as_bytes());
        let _ = self.ws_send_frame(conn, 0x08, &payload);

        // Close socket
        let mut connections = self.ws_connections.lock().unwrap();
        if let Some(conn_data) = connections.remove(&conn.0) {
            arth_rt::net::arth_rt_socket_close(conn_data.socket_handle);
        }
        Ok(())
    }

    fn ws_is_open(&self, conn: WsConnectionHandle) -> Result<bool, NetError> {
        let connections = self.ws_connections.lock().unwrap();
        connections.get(&conn.0).map(|c| c.is_open).ok_or_else(|| {
            NetError::new(
                NetErrorKind::InvalidHandle,
                "Invalid WebSocket connection handle",
            )
        })
    }

    fn sse_serve(&self, port: u16, path: &str) -> Result<SseServerHandle, NetError> {
        let socket_handle = self.create_server_socket(port)?;
        let handle = self.next_task_handle();
        self.sse_servers.lock().unwrap().insert(
            handle,
            SseServerData {
                socket_handle,
                port,
                path: path.to_string(),
            },
        );
        Ok(SseServerHandle(handle))
    }

    fn sse_accept(&self, server: SseServerHandle) -> Result<SseEmitterHandle, NetError> {
        let servers = self.sse_servers.lock().unwrap();
        let server_data = servers.get(&server.0).ok_or_else(|| {
            NetError::new(NetErrorKind::InvalidHandle, "Invalid SSE server handle")
        })?;
        let expected_path = server_data.path.clone();

        // Accept TCP connection
        let client_sock = arth_rt::net::arth_rt_socket_accept(server_data.socket_handle);
        if client_sock < 0 {
            return Err(NetError::connection_failed("Failed to accept connection"));
        }
        drop(servers);

        // Parse HTTP request
        let request = match self.parse_http_request(client_sock) {
            Ok(req) => req,
            Err(e) => {
                arth_rt::net::arth_rt_socket_close(client_sock);
                return Err(e);
            }
        };

        // Check path matches
        if !expected_path.is_empty() && request.path != expected_path {
            let response = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
            let _ = self.write_all(client_sock, response);
            arth_rt::net::arth_rt_socket_close(client_sock);
            return Err(NetError::protocol_error("SSE path mismatch"));
        }

        // Send SSE response headers
        let response = "HTTP/1.1 200 OK\r\n\
                        Content-Type: text/event-stream\r\n\
                        Cache-Control: no-cache\r\n\
                        Connection: keep-alive\r\n\
                        Access-Control-Allow-Origin: *\r\n\r\n";
        if self.write_all(client_sock, response.as_bytes()).is_err() {
            arth_rt::net::arth_rt_socket_close(client_sock);
            return Err(NetError::connection_failed("Failed to send SSE headers"));
        }

        // Store emitter
        let handle = self.next_task_handle();
        self.sse_emitters.lock().unwrap().insert(
            handle,
            SseEmitterData {
                socket_handle: client_sock,
                is_open: true,
            },
        );
        Ok(SseEmitterHandle(handle))
    }

    fn sse_send(
        &self,
        emitter: SseEmitterHandle,
        event: &str,
        data: &str,
        id: &str,
    ) -> Result<(), NetError> {
        let emitters = self.sse_emitters.lock().unwrap();
        let emitter_data = emitters.get(&emitter.0).ok_or_else(|| {
            NetError::new(NetErrorKind::InvalidHandle, "Invalid SSE emitter handle")
        })?;

        if !emitter_data.is_open {
            return Err(NetError::connection_failed("SSE connection is closed"));
        }

        let sock = emitter_data.socket_handle;
        drop(emitters);

        // Build SSE message
        let mut message = String::new();
        if !id.is_empty() {
            message.push_str(&format!("id: {}\n", id));
        }
        if !event.is_empty() {
            message.push_str(&format!("event: {}\n", event));
        }
        // Split data by newlines
        for line in data.lines() {
            message.push_str(&format!("data: {}\n", line));
        }
        if data.is_empty() {
            message.push_str("data: \n");
        }
        message.push('\n');

        let result = self.write_all(sock, message.as_bytes());
        if result.is_err() {
            // Mark as closed
            if let Some(e) = self.sse_emitters.lock().unwrap().get_mut(&emitter.0) {
                e.is_open = false;
            }
        }
        result
    }

    fn sse_close(&self, emitter: SseEmitterHandle) -> Result<(), NetError> {
        let mut emitters = self.sse_emitters.lock().unwrap();
        if let Some(emitter_data) = emitters.remove(&emitter.0) {
            arth_rt::net::arth_rt_socket_close(emitter_data.socket_handle);
        }
        Ok(())
    }

    fn sse_is_open(&self, emitter: SseEmitterHandle) -> Result<bool, NetError> {
        let emitters = self.sse_emitters.lock().unwrap();
        emitters
            .get(&emitter.0)
            .map(|e| e.is_open)
            .ok_or_else(|| NetError::new(NetErrorKind::InvalidHandle, "Invalid SSE emitter handle"))
    }
}

/// Parse HTTP/HTTPS URL into components
fn parse_http_url(url: &str) -> Result<(String, u16, String, bool), NetError> {
    let (scheme, rest) = if url.starts_with("https://") {
        ("https", &url[8..])
    } else if url.starts_with("http://") {
        ("http", &url[7..])
    } else {
        return Err(NetError::invalid_url(
            "URL must start with http:// or https://",
        ));
    };

    let use_tls = scheme == "https";
    let default_port: u16 = if use_tls { 443 } else { 80 };

    // Split host and path
    let (host_port, path) = match rest.find('/') {
        Some(pos) => (&rest[..pos], rest[pos..].to_string()),
        None => (rest, "/".to_string()),
    };

    // Split host and port
    let (host, port) = match host_port.rfind(':') {
        Some(pos) => {
            let port_str = &host_port[pos + 1..];
            let port: u16 = port_str
                .parse()
                .map_err(|_| NetError::invalid_url("Invalid port number"))?;
            (host_port[..pos].to_string(), port)
        }
        None => (host_port.to_string(), default_port),
    };

    Ok((host, port, path, use_tls))
}

/// Compute WebSocket Sec-WebSocket-Accept header value.
/// This concatenates the client key with the magic GUID and computes SHA-1 + base64.
fn compute_ws_accept_key(client_key: &str) -> String {
    use sha1::{Digest, Sha1};

    const WS_MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut hasher = Sha1::new();
    hasher.update(client_key.as_bytes());
    hasher.update(WS_MAGIC.as_bytes());
    let hash = hasher.finalize();
    base64_encode(hash.as_slice())
}

// ============================================================================
// NoHostDb Implementation (Sandboxed)
// ============================================================================

/// Host database implementation that denies all operations.
/// Use this for sandboxed guest execution.
pub struct NoHostDb;

impl NoHostDb {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoHostDb {
    fn default() -> Self {
        Self::new()
    }
}

impl HostDb for NoHostDb {
    fn sqlite_open(&self, _path: &str) -> Result<SqliteConnectionHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_close(&self, _conn: SqliteConnectionHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_prepare(
        &self,
        _conn: SqliteConnectionHandle,
        _sql: &str,
    ) -> Result<SqliteStatementHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_step(&self, _stmt: SqliteStatementHandle) -> Result<bool, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_finalize(&self, _stmt: SqliteStatementHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_reset(&self, _stmt: SqliteStatementHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_bind_int(
        &self,
        _stmt: SqliteStatementHandle,
        _idx: i32,
        _val: i32,
    ) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_bind_int64(
        &self,
        _stmt: SqliteStatementHandle,
        _idx: i32,
        _val: i64,
    ) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_bind_double(
        &self,
        _stmt: SqliteStatementHandle,
        _idx: i32,
        _val: f64,
    ) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_bind_text(
        &self,
        _stmt: SqliteStatementHandle,
        _idx: i32,
        _val: &str,
    ) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_bind_blob(
        &self,
        _stmt: SqliteStatementHandle,
        _idx: i32,
        _val: &[u8],
    ) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_bind_null(&self, _stmt: SqliteStatementHandle, _idx: i32) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_column_int(&self, _stmt: SqliteStatementHandle, _idx: i32) -> Result<i32, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_column_int64(&self, _stmt: SqliteStatementHandle, _idx: i32) -> Result<i64, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_column_double(
        &self,
        _stmt: SqliteStatementHandle,
        _idx: i32,
    ) -> Result<f64, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_column_text(
        &self,
        _stmt: SqliteStatementHandle,
        _idx: i32,
    ) -> Result<String, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_column_blob(
        &self,
        _stmt: SqliteStatementHandle,
        _idx: i32,
    ) -> Result<Vec<u8>, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_column_type(&self, _stmt: SqliteStatementHandle, _idx: i32) -> Result<i32, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_column_count(&self, _stmt: SqliteStatementHandle) -> Result<i32, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_column_name(
        &self,
        _stmt: SqliteStatementHandle,
        _idx: i32,
    ) -> Result<String, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_is_null(&self, _stmt: SqliteStatementHandle, _idx: i32) -> Result<bool, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_changes(&self, _conn: SqliteConnectionHandle) -> Result<i32, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_last_insert_rowid(&self, _conn: SqliteConnectionHandle) -> Result<i64, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_errmsg(&self, _conn: SqliteConnectionHandle) -> Result<String, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_execute(&self, _conn: SqliteConnectionHandle, _sql: &str) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_query(
        &self,
        _conn: SqliteConnectionHandle,
        _sql: &str,
    ) -> Result<SqliteStatementHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_begin(&self, _conn: SqliteConnectionHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_commit(&self, _conn: SqliteConnectionHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_rollback(&self, _conn: SqliteConnectionHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_savepoint(&self, _conn: SqliteConnectionHandle, _name: &str) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_release_savepoint(
        &self,
        _conn: SqliteConnectionHandle,
        _name: &str,
    ) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_rollback_to_savepoint(
        &self,
        _conn: SqliteConnectionHandle,
        _name: &str,
    ) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    // =========================================================================
    // PostgreSQL - NoHostDb stubs (all return CapabilityDenied)
    // =========================================================================

    fn pg_connect(&self, _connection_string: &str) -> Result<PgConnectionHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_disconnect(&self, _conn: PgConnectionHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_status(&self, _conn: PgConnectionHandle) -> Result<bool, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_query(
        &self,
        _conn: PgConnectionHandle,
        _sql: &str,
        _params: &[PgValue],
    ) -> Result<PgResultHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_execute(
        &self,
        _conn: PgConnectionHandle,
        _sql: &str,
        _params: &[PgValue],
    ) -> Result<u64, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_prepare(
        &self,
        _conn: PgConnectionHandle,
        _name: &str,
        _sql: &str,
    ) -> Result<PgStatementHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_execute_prepared(
        &self,
        _conn: PgConnectionHandle,
        _stmt: PgStatementHandle,
        _params: &[PgValue],
    ) -> Result<PgResultHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_row_count(&self, _result: PgResultHandle) -> Result<i64, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_column_count(&self, _result: PgResultHandle) -> Result<i32, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_column_name(&self, _result: PgResultHandle, _col: i32) -> Result<String, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_column_type(&self, _result: PgResultHandle, _col: i32) -> Result<i32, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_get_value(
        &self,
        _result: PgResultHandle,
        _row: i64,
        _col: i32,
    ) -> Result<String, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_get_int(&self, _result: PgResultHandle, _row: i64, _col: i32) -> Result<i32, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_get_int64(&self, _result: PgResultHandle, _row: i64, _col: i32) -> Result<i64, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_get_double(&self, _result: PgResultHandle, _row: i64, _col: i32) -> Result<f64, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_get_text(
        &self,
        _result: PgResultHandle,
        _row: i64,
        _col: i32,
    ) -> Result<String, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_get_bytes(
        &self,
        _result: PgResultHandle,
        _row: i64,
        _col: i32,
    ) -> Result<Vec<u8>, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_get_bool(&self, _result: PgResultHandle, _row: i64, _col: i32) -> Result<bool, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_is_null(&self, _result: PgResultHandle, _row: i64, _col: i32) -> Result<bool, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_affected_rows(&self, _result: PgResultHandle) -> Result<u64, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_free_result(&self, _result: PgResultHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_begin(&self, _conn: PgConnectionHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_commit(&self, _conn: PgConnectionHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_rollback(&self, _conn: PgConnectionHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_savepoint(&self, _conn: PgConnectionHandle, _name: &str) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_release_savepoint(&self, _conn: PgConnectionHandle, _name: &str) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_rollback_to_savepoint(
        &self,
        _conn: PgConnectionHandle,
        _name: &str,
    ) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_errmsg(&self, _conn: PgConnectionHandle) -> Result<String, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_escape(&self, _conn: PgConnectionHandle, _s: &str) -> Result<String, DbError> {
        Err(DbError::capability_denied("db"))
    }

    // --- Async PostgreSQL Operations (all denied) ---

    fn pg_connect_async(
        &self,
        _connection_string: &str,
    ) -> Result<PgAsyncConnectionHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_disconnect_async(&self, _conn: PgAsyncConnectionHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_status_async(&self, _conn: PgAsyncConnectionHandle) -> Result<bool, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_query_async(
        &self,
        _conn: PgAsyncConnectionHandle,
        _sql: &str,
        _params: &[PgValue],
    ) -> Result<PgAsyncQueryHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_execute_async(
        &self,
        _conn: PgAsyncConnectionHandle,
        _sql: &str,
        _params: &[PgValue],
    ) -> Result<PgAsyncQueryHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_prepare_async(
        &self,
        _conn: PgAsyncConnectionHandle,
        _name: &str,
        _sql: &str,
    ) -> Result<PgAsyncQueryHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_execute_prepared_async(
        &self,
        _conn: PgAsyncConnectionHandle,
        _stmt_name: &str,
        _params: &[PgValue],
    ) -> Result<PgAsyncQueryHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_is_ready(&self, _query: PgAsyncQueryHandle) -> Result<bool, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_get_async_result(&self, _query: PgAsyncQueryHandle) -> Result<PgResultHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_cancel_async(&self, _query: PgAsyncQueryHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_begin_async(
        &self,
        _conn: PgAsyncConnectionHandle,
    ) -> Result<PgAsyncQueryHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_commit_async(
        &self,
        _conn: PgAsyncConnectionHandle,
    ) -> Result<PgAsyncQueryHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_rollback_async(
        &self,
        _conn: PgAsyncConnectionHandle,
    ) -> Result<PgAsyncQueryHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    // --- Connection Pool Operations (all denied) ---

    fn sqlite_pool_create(
        &self,
        _connection_string: &str,
        _config: &PoolConfig,
    ) -> Result<SqlitePoolHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_pool_close(&self, _pool: SqlitePoolHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_pool_acquire(
        &self,
        _pool: SqlitePoolHandle,
    ) -> Result<SqliteConnectionHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_pool_release(
        &self,
        _pool: SqlitePoolHandle,
        _conn: SqliteConnectionHandle,
    ) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_pool_stats(&self, _pool: SqlitePoolHandle) -> Result<PoolStats, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_pool_create(
        &self,
        _connection_string: &str,
        _config: &PoolConfig,
    ) -> Result<PgPoolHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_pool_close(&self, _pool: PgPoolHandle) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_pool_acquire(&self, _pool: PgPoolHandle) -> Result<PgConnectionHandle, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_pool_release(
        &self,
        _pool: PgPoolHandle,
        _conn: PgConnectionHandle,
    ) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_pool_stats(&self, _pool: PgPoolHandle) -> Result<PoolStats, DbError> {
        Err(DbError::capability_denied("db"))
    }

    // Transaction helpers
    fn sqlite_tx_scope_begin(&self, _conn: SqliteConnectionHandle) -> Result<i64, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_tx_scope_end(
        &self,
        _conn: SqliteConnectionHandle,
        _scope_id: i64,
        _success: bool,
    ) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_tx_depth(&self, _conn: SqliteConnectionHandle) -> Result<u32, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn sqlite_tx_active(&self, _conn: SqliteConnectionHandle) -> Result<bool, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_tx_scope_begin(&self, _conn: PgConnectionHandle) -> Result<i64, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_tx_scope_end(
        &self,
        _conn: PgConnectionHandle,
        _scope_id: i64,
        _success: bool,
    ) -> Result<(), DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_tx_depth(&self, _conn: PgConnectionHandle) -> Result<u32, DbError> {
        Err(DbError::capability_denied("db"))
    }

    fn pg_tx_active(&self, _conn: PgConnectionHandle) -> Result<bool, DbError> {
        Err(DbError::capability_denied("db"))
    }
}

// ============================================================================
// NoHostMail Implementation
// ============================================================================

/// Host mail implementation that denies all operations.
/// Use this for sandboxed guest execution.
pub struct NoHostMail;

impl NoHostMail {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoHostMail {
    fn default() -> Self {
        Self::new()
    }
}

impl HostMail for NoHostMail {
    fn smtp_connect(
        &self,
        _host: &str,
        _port: u16,
        _use_tls: bool,
        _timeout_ms: u64,
    ) -> Result<SmtpConnectionHandle, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_start_tls(&self, _conn: SmtpConnectionHandle) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_auth(
        &self,
        _conn: SmtpConnectionHandle,
        _mechanism: &str,
        _username: &str,
        _password: &str,
    ) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_ehlo(
        &self,
        _conn: SmtpConnectionHandle,
        _hostname: &str,
    ) -> Result<Vec<String>, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_quit(&self, _conn: SmtpConnectionHandle) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_noop(&self, _conn: SmtpConnectionHandle) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_reset(&self, _conn: SmtpConnectionHandle) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_mail_from(&self, _conn: SmtpConnectionHandle, _sender: &str) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_rcpt_to(&self, _conn: SmtpConnectionHandle, _recipient: &str) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_data(&self, _conn: SmtpConnectionHandle) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_send_data(&self, _conn: SmtpConnectionHandle, _data: &[u8]) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_end_data(&self, _conn: SmtpConnectionHandle) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_read_response(&self, _conn: SmtpConnectionHandle) -> Result<SmtpResponse, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_get_capabilities(&self, _conn: SmtpConnectionHandle) -> Result<Vec<String>, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn smtp_send_message(
        &self,
        _conn: SmtpConnectionHandle,
        _from: &str,
        _to: &[&str],
        _message_data: &[u8],
    ) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_connect(
        &self,
        _host: &str,
        _port: u16,
        _use_tls: bool,
        _timeout_ms: u64,
    ) -> Result<ImapConnectionHandle, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_start_tls(&self, _conn: ImapConnectionHandle) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_auth(
        &self,
        _conn: ImapConnectionHandle,
        _username: &str,
        _password: &str,
    ) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_auth_oauth(
        &self,
        _conn: ImapConnectionHandle,
        _username: &str,
        _access_token: &str,
    ) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_logout(&self, _conn: ImapConnectionHandle) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_noop(&self, _conn: ImapConnectionHandle) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_capability(&self, _conn: ImapConnectionHandle) -> Result<Vec<String>, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_select(
        &self,
        _conn: ImapConnectionHandle,
        _mailbox: &str,
    ) -> Result<ImapFolderHandle, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_examine(
        &self,
        _conn: ImapConnectionHandle,
        _mailbox: &str,
    ) -> Result<ImapFolderHandle, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_list(
        &self,
        _conn: ImapConnectionHandle,
        _reference: &str,
        _pattern: &str,
    ) -> Result<Vec<ImapMailbox>, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_fetch(
        &self,
        _conn: ImapConnectionHandle,
        _sequence: &str,
        _items: &str,
    ) -> Result<Vec<ImapFetchResult>, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_search(
        &self,
        _conn: ImapConnectionHandle,
        _criteria: &str,
    ) -> Result<Vec<u32>, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_store(
        &self,
        _conn: ImapConnectionHandle,
        _sequence: &str,
        _flags: &str,
        _action: &str,
    ) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_expunge(&self, _conn: ImapConnectionHandle) -> Result<Vec<u32>, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn imap_idle(
        &self,
        _conn: ImapConnectionHandle,
        _timeout_ms: u64,
    ) -> Result<ImapIdleEvent, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn pop3_connect(
        &self,
        _host: &str,
        _port: u16,
        _use_tls: bool,
        _timeout_ms: u64,
    ) -> Result<Pop3ConnectionHandle, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn pop3_start_tls(&self, _conn: Pop3ConnectionHandle) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn pop3_auth(
        &self,
        _conn: Pop3ConnectionHandle,
        _username: &str,
        _password: &str,
    ) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn pop3_auth_apop(
        &self,
        _conn: Pop3ConnectionHandle,
        _username: &str,
        _password: &str,
    ) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn pop3_quit(&self, _conn: Pop3ConnectionHandle) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn pop3_stat(&self, _conn: Pop3ConnectionHandle) -> Result<Pop3Stat, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn pop3_list(&self, _conn: Pop3ConnectionHandle) -> Result<Vec<Pop3MessageInfo>, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn pop3_uidl(&self, _conn: Pop3ConnectionHandle) -> Result<Vec<Pop3Uid>, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn pop3_retr(&self, _conn: Pop3ConnectionHandle, _msg_num: u32) -> Result<Vec<u8>, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn pop3_dele(&self, _conn: Pop3ConnectionHandle, _msg_num: u32) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn pop3_reset(&self, _conn: Pop3ConnectionHandle) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn mime_base64_encode(&self, data: &[u8]) -> String {
        base64_encode(data)
    }
    fn mime_base64_decode(&self, encoded: &str) -> Result<Vec<u8>, MailError> {
        base64_decode(encoded).map_err(|e| MailError::message_error(e))
    }
    fn mime_quoted_printable_encode(&self, data: &[u8]) -> String {
        // Simple quoted-printable encoding
        let mut result = String::new();
        for &byte in data {
            if byte == b'=' || byte < 32 || byte > 126 {
                result.push_str(&format!("={:02X}", byte));
            } else {
                result.push(byte as char);
            }
        }
        result
    }
    fn mime_quoted_printable_decode(&self, encoded: &str) -> Result<Vec<u8>, MailError> {
        // Simple quoted-printable decoding
        let mut result = Vec::new();
        let bytes = encoded.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'=' && i + 2 < bytes.len() {
                if let Ok(val) = u8::from_str_radix(&encoded[i + 1..i + 3], 16) {
                    result.push(val);
                    i += 3;
                    continue;
                }
            }
            result.push(bytes[i]);
            i += 1;
        }
        Ok(result)
    }
    fn mime_encode_header(&self, value: &str, _charset: &str) -> String {
        // Simple RFC 2047 Q-encoding
        if value.is_ascii() {
            return value.to_string();
        }
        format!(
            "=?UTF-8?Q?{}?=",
            self.mime_quoted_printable_encode(value.as_bytes())
        )
    }
    fn mime_decode_header(&self, encoded: &str) -> Result<String, MailError> {
        // Simple RFC 2047 decoding
        if encoded.starts_with("=?") && encoded.ends_with("?=") {
            // Very simplified - just return as-is for now
            Ok(encoded.to_string())
        } else {
            Ok(encoded.to_string())
        }
    }
    fn mime_message_new(&self) -> MimeMessageHandle {
        MimeMessageHandle(0) // Stub
    }
    fn mime_message_set_header(
        &self,
        _msg: MimeMessageHandle,
        _name: &str,
        _value: &str,
    ) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn mime_message_set_body(
        &self,
        _msg: MimeMessageHandle,
        _content_type: &str,
        _body: &[u8],
    ) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn mime_message_add_attachment(
        &self,
        _msg: MimeMessageHandle,
        _filename: &str,
        _content_type: &str,
        _data: &[u8],
    ) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn mime_message_serialize(&self, _msg: MimeMessageHandle) -> Result<Vec<u8>, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn mime_message_parse(&self, _data: &[u8]) -> Result<MimeMessageHandle, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn mime_message_get_header(
        &self,
        _msg: MimeMessageHandle,
        _name: &str,
    ) -> Result<Option<String>, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn mime_message_get_body(&self, _msg: MimeMessageHandle) -> Result<Vec<u8>, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn mime_message_free(&self, _msg: MimeMessageHandle) {}
    fn tls_context_new(&self, _verify_certs: bool) -> Result<TlsContextHandle, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn tls_upgrade(
        &self,
        _ctx: TlsContextHandle,
        _hostname: &str,
    ) -> Result<TlsStreamHandle, MailError> {
        Err(MailError::capability_denied("mail"))
    }
    fn tls_close(&self, _stream: TlsStreamHandle) -> Result<(), MailError> {
        Err(MailError::capability_denied("mail"))
    }
}

// ============================================================================
// StdHostMail Implementation
// ============================================================================

use std::io::{BufReader, Read as IoRead, Write as IoWrite};

// ============================================================================
// SMTP Stream using arth_rt C FFI
// ============================================================================

/// Stream type for SMTP connection using arth_rt handles.
/// This enables the same implementation for VM and native compilation.
enum SmtpStream {
    /// Plain TCP connection (socket handle)
    Plain { socket_handle: i64 },
    /// TLS connection (TLS stream handle, socket handle is consumed by TLS)
    Tls { tls_handle: i64 },
}

impl SmtpStream {
    /// Create a new plain TCP stream
    fn new_plain(socket_handle: i64) -> Self {
        SmtpStream::Plain { socket_handle }
    }

    /// Upgrade a plain connection to TLS
    fn upgrade_to_tls(&mut self, host: &str) -> Result<(), std::io::Error> {
        match self {
            SmtpStream::Plain { socket_handle } => {
                // Create TLS connector
                let connector = arth_rt::tls::arth_rt_tls_connector_new();
                if connector < 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Failed to create TLS connector",
                    ));
                }

                // Perform TLS handshake
                let tls_handle = arth_rt::tls::arth_rt_tls_connect(
                    connector,
                    *socket_handle,
                    host.as_ptr(),
                    host.len(),
                );

                // Free connector (no longer needed)
                arth_rt::tls::arth_rt_tls_connector_free(connector);

                if tls_handle < 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "TLS handshake failed",
                    ));
                }

                // Transition to TLS mode
                *self = SmtpStream::Tls { tls_handle };
                Ok(())
            }
            SmtpStream::Tls { .. } => Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Connection already using TLS",
            )),
        }
    }

    /// Check if this is a TLS connection
    fn is_tls(&self) -> bool {
        matches!(self, SmtpStream::Tls { .. })
    }
}

impl Drop for SmtpStream {
    fn drop(&mut self) {
        match self {
            SmtpStream::Plain { socket_handle } => {
                arth_rt::net::arth_rt_socket_close(*socket_handle);
            }
            SmtpStream::Tls { tls_handle } => {
                arth_rt::tls::arth_rt_tls_close(*tls_handle);
            }
        }
    }
}

impl IoRead for SmtpStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let result = match self {
            SmtpStream::Plain { socket_handle } => {
                arth_rt::net::arth_rt_socket_recv(*socket_handle, buf.as_mut_ptr(), buf.len(), 0)
            }
            SmtpStream::Tls { tls_handle } => {
                arth_rt::tls::arth_rt_tls_read(*tls_handle, buf.as_mut_ptr(), buf.len())
            }
        };

        if result < 0 {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("read failed: error code {}", result),
            ))
        } else {
            Ok(result as usize)
        }
    }
}

impl IoWrite for SmtpStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let result = match self {
            SmtpStream::Plain { socket_handle } => {
                arth_rt::net::arth_rt_socket_send(*socket_handle, buf.as_ptr(), buf.len(), 0)
            }
            SmtpStream::Tls { tls_handle } => {
                arth_rt::tls::arth_rt_tls_write(*tls_handle, buf.as_ptr(), buf.len())
            }
        };

        if result < 0 {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("write failed: error code {}", result),
            ))
        } else {
            Ok(result as usize)
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let SmtpStream::Tls { tls_handle } = self {
            let result = arth_rt::tls::arth_rt_tls_flush(*tls_handle);
            if result < 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("flush failed: error code {}", result),
                ));
            }
        }
        Ok(())
    }
}

/// SMTP connection state.
struct SmtpConnection {
    stream: SmtpStream,
    host: String,
    capabilities: Vec<String>,
    last_response: Option<SmtpResponse>,
}

impl SmtpConnection {
    fn read_response(&mut self) -> Result<SmtpResponse, MailError> {
        let mut reader = BufReader::new(&mut self.stream);
        let mut full_message = String::new();
        let mut code: u16 = 0;
        let mut is_multiline = false;

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => return Err(MailError::connection_error("connection closed")),
                Ok(_) => {
                    // Parse response code from first 3 characters
                    if line.len() < 4 {
                        return Err(MailError::protocol_error("invalid response format"));
                    }
                    let line_code = line[..3]
                        .parse::<u16>()
                        .map_err(|_| MailError::protocol_error("invalid response code"))?;

                    if code == 0 {
                        code = line_code;
                    }

                    // Check if this is a continuation line (4th char is '-')
                    let is_continuation = line.len() > 3 && line.chars().nth(3) == Some('-');
                    if is_continuation {
                        is_multiline = true;
                    }

                    // Append message part (skip code and separator)
                    if line.len() > 4 {
                        if !full_message.is_empty() {
                            full_message.push('\n');
                        }
                        full_message.push_str(line[4..].trim_end());
                    }

                    if !is_continuation {
                        break;
                    }
                }
                Err(e) => return Err(MailError::connection_error(e.to_string())),
            }
        }

        let response = SmtpResponse {
            code,
            message: full_message,
            is_multiline,
        };
        self.last_response = Some(response.clone());
        Ok(response)
    }

    fn send_command(&mut self, cmd: &str) -> Result<(), MailError> {
        let line = format!("{}\r\n", cmd);
        self.stream
            .write_all(line.as_bytes())
            .map_err(|e| MailError::connection_error(e.to_string()))?;
        self.stream
            .flush()
            .map_err(|e| MailError::connection_error(e.to_string()))
    }

    fn send_and_read(&mut self, cmd: &str) -> Result<SmtpResponse, MailError> {
        self.send_command(cmd)?;
        self.read_response()
    }
}

// ============================================================================
// IMAP Stream using arth_rt C FFI
// ============================================================================

/// Stream type for IMAP connection using arth_rt handles.
enum ImapStream {
    /// Plain TCP connection (socket handle)
    Plain { socket_handle: i64 },
    /// TLS connection (TLS stream handle)
    Tls { tls_handle: i64 },
}

impl ImapStream {
    fn new_plain(socket_handle: i64) -> Self {
        ImapStream::Plain { socket_handle }
    }

    fn upgrade_to_tls(&mut self, host: &str) -> Result<(), std::io::Error> {
        match self {
            ImapStream::Plain { socket_handle } => {
                let connector = arth_rt::tls::arth_rt_tls_connector_new();
                if connector < 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Failed to create TLS connector",
                    ));
                }

                let tls_handle = arth_rt::tls::arth_rt_tls_connect(
                    connector,
                    *socket_handle,
                    host.as_ptr(),
                    host.len(),
                );

                arth_rt::tls::arth_rt_tls_connector_free(connector);

                if tls_handle < 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "TLS handshake failed",
                    ));
                }

                *self = ImapStream::Tls { tls_handle };
                Ok(())
            }
            ImapStream::Tls { .. } => Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Connection already using TLS",
            )),
        }
    }
}

impl Drop for ImapStream {
    fn drop(&mut self) {
        match self {
            ImapStream::Plain { socket_handle } => {
                arth_rt::net::arth_rt_socket_close(*socket_handle);
            }
            ImapStream::Tls { tls_handle } => {
                arth_rt::tls::arth_rt_tls_close(*tls_handle);
            }
        }
    }
}

impl IoRead for ImapStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let result = match self {
            ImapStream::Plain { socket_handle } => {
                arth_rt::net::arth_rt_socket_recv(*socket_handle, buf.as_mut_ptr(), buf.len(), 0)
            }
            ImapStream::Tls { tls_handle } => {
                arth_rt::tls::arth_rt_tls_read(*tls_handle, buf.as_mut_ptr(), buf.len())
            }
        };

        if result < 0 {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("read failed: error code {}", result),
            ))
        } else {
            Ok(result as usize)
        }
    }
}

impl IoWrite for ImapStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let result = match self {
            ImapStream::Plain { socket_handle } => {
                arth_rt::net::arth_rt_socket_send(*socket_handle, buf.as_ptr(), buf.len(), 0)
            }
            ImapStream::Tls { tls_handle } => {
                arth_rt::tls::arth_rt_tls_write(*tls_handle, buf.as_ptr(), buf.len())
            }
        };

        if result < 0 {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("write failed: error code {}", result),
            ))
        } else {
            Ok(result as usize)
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let ImapStream::Tls { tls_handle } = self {
            let result = arth_rt::tls::arth_rt_tls_flush(*tls_handle);
            if result < 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("flush failed: error code {}", result),
                ));
            }
        }
        Ok(())
    }
}

/// IMAP connection state.
struct ImapConnection {
    stream: ImapStream,
    host: String,
    tag_counter: u32,
    capabilities: Vec<String>,
    selected_mailbox: Option<String>,
    folder_handles: HashMap<i64, ImapFolderState>,
    next_folder_handle: i64,
}

/// IMAP folder state.
struct ImapFolderState {
    name: String,
    exists: u32,
    recent: u32,
    uidvalidity: u32,
    uidnext: u32,
    read_only: bool,
}

/// IMAP response data.
#[derive(Debug)]
struct ImapResponse {
    tag: String,
    status: String,
    message: String,
    untagged: Vec<String>,
}

impl ImapConnection {
    fn new(stream: ImapStream, host: String) -> Self {
        Self {
            stream,
            host,
            tag_counter: 0,
            capabilities: Vec::new(),
            selected_mailbox: None,
            folder_handles: HashMap::new(),
            next_folder_handle: 1,
        }
    }

    fn next_tag(&mut self) -> String {
        self.tag_counter += 1;
        format!("A{:04}", self.tag_counter)
    }

    fn send_command(&mut self, tag: &str, cmd: &str) -> Result<(), MailError> {
        let line = format!("{} {}\r\n", tag, cmd);
        self.stream
            .write_all(line.as_bytes())
            .map_err(|e| MailError::connection_error(e.to_string()))?;
        self.stream
            .flush()
            .map_err(|e| MailError::connection_error(e.to_string()))
    }

    fn read_response(&mut self, expected_tag: &str) -> Result<ImapResponse, MailError> {
        let mut reader = BufReader::new(&mut self.stream);
        let mut untagged = Vec::new();

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => return Err(MailError::connection_error("connection closed")),
                Ok(_) => {
                    let line = line.trim_end();

                    // Untagged response (starts with '*')
                    if line.starts_with("* ") {
                        untagged.push(line[2..].to_string());
                        continue;
                    }

                    // Continuation request (starts with '+')
                    if line.starts_with("+ ") {
                        // Return early for continuation
                        return Ok(ImapResponse {
                            tag: "+".to_string(),
                            status: "CONTINUE".to_string(),
                            message: line[2..].to_string(),
                            untagged,
                        });
                    }

                    // Tagged response
                    if line.starts_with(expected_tag) {
                        let rest = &line[expected_tag.len()..].trim_start();
                        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                        let status = parts.first().unwrap_or(&"").to_string();
                        let message = parts.get(1).unwrap_or(&"").to_string();

                        return Ok(ImapResponse {
                            tag: expected_tag.to_string(),
                            status,
                            message,
                            untagged,
                        });
                    }

                    // Unexpected tagged response
                    return Err(MailError::protocol_error(format!(
                        "unexpected tag: expected {}, got: {}",
                        expected_tag, line
                    )));
                }
                Err(e) => return Err(MailError::connection_error(e.to_string())),
            }
        }
    }

    fn read_greeting(&mut self) -> Result<(), MailError> {
        let mut reader = BufReader::new(&mut self.stream);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| MailError::connection_error(e.to_string()))?;

        let line = line.trim();
        if !line.starts_with("* OK") && !line.starts_with("* PREAUTH") {
            return Err(MailError::protocol_error(format!(
                "unexpected greeting: {}",
                line
            )));
        }

        // Parse capabilities from greeting if present
        if let Some(caps_start) = line.find("[CAPABILITY ") {
            if let Some(caps_end) = line[caps_start..].find(']') {
                let caps = &line[caps_start + 12..caps_start + caps_end];
                self.capabilities = caps.split(' ').map(|s| s.to_string()).collect();
            }
        }

        Ok(())
    }

    fn send_and_read(&mut self, cmd: &str) -> Result<ImapResponse, MailError> {
        let tag = self.next_tag();
        self.send_command(&tag, cmd)?;
        self.read_response(&tag)
    }

    fn check_ok(&self, response: &ImapResponse) -> Result<(), MailError> {
        if response.status != "OK" {
            Err(MailError::protocol_error(format!(
                "{}: {}",
                response.status, response.message
            )))
        } else {
            Ok(())
        }
    }

    fn alloc_folder_handle(&mut self) -> i64 {
        let handle = self.next_folder_handle;
        self.next_folder_handle += 1;
        handle
    }
}

// ============================================================================
// POP3 Stream using arth_rt C FFI
// ============================================================================

/// Stream type for POP3 connection using arth_rt handles.
enum Pop3Stream {
    /// Plain TCP connection (socket handle)
    Plain { socket_handle: i64 },
    /// TLS connection (TLS stream handle)
    Tls { tls_handle: i64 },
}

impl Pop3Stream {
    fn new_plain(socket_handle: i64) -> Self {
        Pop3Stream::Plain { socket_handle }
    }

    fn upgrade_to_tls(&mut self, host: &str) -> Result<(), std::io::Error> {
        match self {
            Pop3Stream::Plain { socket_handle } => {
                let connector = arth_rt::tls::arth_rt_tls_connector_new();
                if connector < 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Failed to create TLS connector",
                    ));
                }

                let tls_handle = arth_rt::tls::arth_rt_tls_connect(
                    connector,
                    *socket_handle,
                    host.as_ptr(),
                    host.len(),
                );

                arth_rt::tls::arth_rt_tls_connector_free(connector);

                if tls_handle < 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "TLS handshake failed",
                    ));
                }

                *self = Pop3Stream::Tls { tls_handle };
                Ok(())
            }
            Pop3Stream::Tls { .. } => Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Connection already using TLS",
            )),
        }
    }
}

impl Drop for Pop3Stream {
    fn drop(&mut self) {
        match self {
            Pop3Stream::Plain { socket_handle } => {
                arth_rt::net::arth_rt_socket_close(*socket_handle);
            }
            Pop3Stream::Tls { tls_handle } => {
                arth_rt::tls::arth_rt_tls_close(*tls_handle);
            }
        }
    }
}

impl IoRead for Pop3Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let result = match self {
            Pop3Stream::Plain { socket_handle } => {
                arth_rt::net::arth_rt_socket_recv(*socket_handle, buf.as_mut_ptr(), buf.len(), 0)
            }
            Pop3Stream::Tls { tls_handle } => {
                arth_rt::tls::arth_rt_tls_read(*tls_handle, buf.as_mut_ptr(), buf.len())
            }
        };

        if result < 0 {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("read failed: error code {}", result),
            ))
        } else {
            Ok(result as usize)
        }
    }
}

impl IoWrite for Pop3Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let result = match self {
            Pop3Stream::Plain { socket_handle } => {
                arth_rt::net::arth_rt_socket_send(*socket_handle, buf.as_ptr(), buf.len(), 0)
            }
            Pop3Stream::Tls { tls_handle } => {
                arth_rt::tls::arth_rt_tls_write(*tls_handle, buf.as_ptr(), buf.len())
            }
        };

        if result < 0 {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("write failed: error code {}", result),
            ))
        } else {
            Ok(result as usize)
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Pop3Stream::Tls { tls_handle } = self {
            let result = arth_rt::tls::arth_rt_tls_flush(*tls_handle);
            if result < 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("flush failed: error code {}", result),
                ));
            }
        }
        Ok(())
    }
}

/// POP3 connection state.
struct Pop3Connection {
    stream: Pop3Stream,
    host: String,
}

impl Pop3Connection {
    fn new(stream: Pop3Stream, host: String) -> Self {
        Self { stream, host }
    }

    fn send_command(&mut self, cmd: &str) -> Result<(), MailError> {
        let line = format!("{}\r\n", cmd);
        self.stream
            .write_all(line.as_bytes())
            .map_err(|e| MailError::connection_error(e.to_string()))?;
        self.stream
            .flush()
            .map_err(|e| MailError::connection_error(e.to_string()))
    }

    fn read_response(&mut self) -> Result<(bool, String), MailError> {
        let mut reader = BufReader::new(&mut self.stream);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| MailError::connection_error(e.to_string()))?;

        let line = line.trim();
        if line.starts_with("+OK") {
            Ok((true, line[3..].trim().to_string()))
        } else if line.starts_with("-ERR") {
            Ok((false, line[4..].trim().to_string()))
        } else {
            Err(MailError::protocol_error(format!(
                "unexpected response: {}",
                line
            )))
        }
    }

    fn read_multiline_response(&mut self) -> Result<Vec<String>, MailError> {
        let mut reader = BufReader::new(&mut self.stream);
        let mut lines = Vec::new();

        loop {
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .map_err(|e| MailError::connection_error(e.to_string()))?;

            let line = line.trim_end_matches("\r\n").trim_end_matches('\n');

            // End of multiline response
            if line == "." {
                break;
            }

            // Byte-stuffed line (starts with ..)
            let content = if line.starts_with("..") {
                &line[1..]
            } else {
                line
            };

            lines.push(content.to_string());
        }

        Ok(lines)
    }

    fn send_and_read(&mut self, cmd: &str) -> Result<(bool, String), MailError> {
        self.send_command(cmd)?;
        self.read_response()
    }
}

/// Standard host mail implementation with actual SMTP, IMAP, and POP3 support.
pub struct StdHostMail {
    next_handle: std::sync::atomic::AtomicI64,
    smtp_connections: std::sync::Mutex<HashMap<i64, SmtpConnection>>,
    imap_connections: std::sync::Mutex<HashMap<i64, ImapConnection>>,
    pop3_connections: std::sync::Mutex<HashMap<i64, Pop3Connection>>,
    mime_messages: std::sync::Mutex<HashMap<i64, MimeMessageData>>,
}

/// Internal MIME message data.
struct MimeMessageData {
    headers: HashMap<String, String>,
    body: Vec<u8>,
    content_type: String,
    attachments: Vec<MimeAttachment>,
}

struct MimeAttachment {
    filename: String,
    content_type: String,
    data: Vec<u8>,
}

impl StdHostMail {
    pub fn new() -> Self {
        Self {
            next_handle: std::sync::atomic::AtomicI64::new(1),
            smtp_connections: std::sync::Mutex::new(HashMap::new()),
            imap_connections: std::sync::Mutex::new(HashMap::new()),
            pop3_connections: std::sync::Mutex::new(HashMap::new()),
            mime_messages: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn alloc_handle(&self) -> i64 {
        self.next_handle
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// Create a TCP socket connection to a host:port using arth_rt
    fn connect_to_host(host: &str, port: u16) -> Result<i64, MailError> {
        let socket_handle = arth_rt::net::arth_rt_socket_create(
            arth_rt::net::AF_INET,
            arth_rt::net::SOCK_STREAM,
            0,
        );
        if socket_handle < 0 {
            return Err(MailError::connection_error("Failed to create socket"));
        }

        let connect_result = arth_rt::net::arth_rt_socket_connect_host(
            socket_handle,
            host.as_ptr(),
            host.len(),
            port,
        );
        if connect_result < 0 {
            arth_rt::net::arth_rt_socket_close(socket_handle);
            return Err(MailError::connection_error(&format!(
                "Failed to connect to {}:{}",
                host, port
            )));
        }

        Ok(socket_handle)
    }

    /// Parse an IMAP LIST response line
    /// Format: (\flags) "delimiter" "mailbox-name"
    fn parse_list_response(line: &str) -> Option<ImapMailbox> {
        let line = line.trim();

        // Parse attributes in parentheses
        if !line.starts_with('(') {
            return None;
        }

        let attr_end = line.find(')')?;
        let attrs_str = &line[1..attr_end];
        let attributes: Vec<String> = attrs_str
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();

        let rest = &line[attr_end + 1..].trim_start();

        // Parse delimiter (quoted or NIL)
        let (delimiter, name_start) = if rest.starts_with("NIL") {
            (None, 3)
        } else if rest.starts_with('"') {
            let end = rest[1..].find('"')?;
            let delim = rest[1..end + 1].chars().next();
            (delim, end + 3)
        } else {
            return None;
        };

        // Parse mailbox name (quoted)
        let name_rest = rest[name_start..].trim_start();
        if !name_rest.starts_with('"') {
            return None;
        }
        let name_end = name_rest[1..].find('"')?;
        let name = name_rest[1..name_end + 1].to_string();

        Some(ImapMailbox {
            name,
            attributes,
            delimiter,
        })
    }

    /// Parse an IMAP FETCH response line
    /// Format: seq_num FETCH (data...)
    fn parse_fetch_response(line: &str) -> Option<ImapFetchResult> {
        let line = line.trim();

        // Parse sequence number
        let parts: Vec<&str> = line.splitn(2, ' ').collect();
        if parts.len() < 2 {
            return None;
        }

        let seq_num: u32 = parts[0].parse().ok()?;
        let rest = parts[1].trim();

        if !rest.starts_with("FETCH ") {
            return None;
        }

        let fetch_data = &rest[6..].trim();

        // Simple parsing: extract key-value pairs from parentheses
        // This is a simplified parser that handles common cases
        let mut items = HashMap::new();

        if fetch_data.starts_with('(') && fetch_data.ends_with(')') {
            let inner = &fetch_data[1..fetch_data.len() - 1];

            // Parse space-separated key-value pairs
            let tokens: Vec<&str> = inner.split_whitespace().collect();
            let mut i = 0;
            while i < tokens.len() {
                let key = tokens[i].to_uppercase();
                i += 1;

                // Handle different value formats
                if i < tokens.len() {
                    let value = if tokens[i].starts_with('{') {
                        // Literal format {size}
                        tokens[i].to_string()
                    } else if tokens[i].starts_with('"') {
                        // Quoted string
                        let mut val = tokens[i].to_string();
                        while !val.ends_with('"') && i + 1 < tokens.len() {
                            i += 1;
                            val.push(' ');
                            val.push_str(tokens[i]);
                        }
                        val
                    } else if tokens[i].starts_with('(') {
                        // Parenthesized list
                        let mut val = tokens[i].to_string();
                        let mut depth = val.chars().filter(|&c| c == '(').count() as i32
                            - val.chars().filter(|&c| c == ')').count() as i32;
                        while depth > 0 && i + 1 < tokens.len() {
                            i += 1;
                            val.push(' ');
                            val.push_str(tokens[i]);
                            depth += tokens[i].chars().filter(|&c| c == '(').count() as i32
                                - tokens[i].chars().filter(|&c| c == ')').count() as i32;
                        }
                        val
                    } else {
                        tokens[i].to_string()
                    };
                    items.insert(key, value);
                    i += 1;
                }
            }
        }

        Some(ImapFetchResult { seq_num, items })
    }
}

impl Default for StdHostMail {
    fn default() -> Self {
        Self::new()
    }
}

impl HostMail for StdHostMail {
    fn smtp_connect(
        &self,
        host: &str,
        port: u16,
        use_tls: bool,
        _timeout_ms: u64,
    ) -> Result<SmtpConnectionHandle, MailError> {
        // Create TCP socket using arth_rt
        let socket_handle = arth_rt::net::arth_rt_socket_create(
            arth_rt::net::AF_INET,
            arth_rt::net::SOCK_STREAM,
            0,
        );
        if socket_handle < 0 {
            return Err(MailError::connection_error("Failed to create socket"));
        }

        // Connect to host:port using arth_rt
        let connect_result = arth_rt::net::arth_rt_socket_connect_host(
            socket_handle,
            host.as_ptr(),
            host.len(),
            port,
        );
        if connect_result < 0 {
            arth_rt::net::arth_rt_socket_close(socket_handle);
            return Err(MailError::connection_error(&format!(
                "Failed to connect to {}:{}",
                host, port
            )));
        }

        // Create the stream (plain or TLS)
        let mut stream = SmtpStream::new_plain(socket_handle);

        // Upgrade to TLS if requested (implicit TLS, typically port 465)
        if use_tls {
            stream
                .upgrade_to_tls(host)
                .map_err(|e| MailError::tls_error(e.to_string()))?;
        }

        let mut conn = SmtpConnection {
            stream,
            host: host.to_string(),
            capabilities: Vec::new(),
            last_response: None,
        };

        // Read server greeting
        let response = conn.read_response()?;
        if response.code != 220 {
            return Err(MailError::with_code(
                MailErrorKind::ConnectionError,
                response.message,
                response.code,
            ));
        }

        let handle = self.alloc_handle();
        self.smtp_connections.lock().unwrap().insert(handle, conn);

        Ok(SmtpConnectionHandle(handle))
    }

    fn smtp_start_tls(&self, conn: SmtpConnectionHandle) -> Result<(), MailError> {
        let mut conns = self.smtp_connections.lock().unwrap();
        let smtp_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Send STARTTLS command
        let response = smtp_conn.send_and_read("STARTTLS")?;
        if response.code != 220 {
            return Err(MailError::with_code(
                MailErrorKind::TlsError,
                response.message,
                response.code,
            ));
        }

        // Get host for TLS handshake
        let host = smtp_conn.host.clone();

        // Upgrade the connection to TLS using arth_rt
        smtp_conn
            .stream
            .upgrade_to_tls(&host)
            .map_err(|e| MailError::tls_error(e.to_string()))?;

        // Need to re-EHLO after STARTTLS
        smtp_conn.capabilities.clear();

        Ok(())
    }

    fn smtp_auth(
        &self,
        conn: SmtpConnectionHandle,
        mechanism: &str,
        username: &str,
        password: &str,
    ) -> Result<(), MailError> {
        let mut conns = self.smtp_connections.lock().unwrap();
        let smtp_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        match mechanism.to_uppercase().as_str() {
            "PLAIN" => {
                // AUTH PLAIN: base64("\0username\0password")
                let auth_string = format!("\x00{}\x00{}", username, password);
                let encoded = base64_encode(auth_string.as_bytes());
                let response = smtp_conn.send_and_read(&format!("AUTH PLAIN {}", encoded))?;
                if response.code != 235 {
                    return Err(MailError::with_code(
                        MailErrorKind::AuthenticationError,
                        response.message,
                        response.code,
                    ));
                }
            }
            "LOGIN" => {
                // AUTH LOGIN is a challenge-response mechanism
                let response = smtp_conn.send_and_read("AUTH LOGIN")?;
                if response.code != 334 {
                    return Err(MailError::with_code(
                        MailErrorKind::AuthenticationError,
                        response.message,
                        response.code,
                    ));
                }

                // Send username (base64 encoded)
                let encoded_user = base64_encode(username.as_bytes());
                let response = smtp_conn.send_and_read(&encoded_user)?;
                if response.code != 334 {
                    return Err(MailError::with_code(
                        MailErrorKind::AuthenticationError,
                        response.message,
                        response.code,
                    ));
                }

                // Send password (base64 encoded)
                let encoded_pass = base64_encode(password.as_bytes());
                let response = smtp_conn.send_and_read(&encoded_pass)?;
                if response.code != 235 {
                    return Err(MailError::with_code(
                        MailErrorKind::AuthenticationError,
                        response.message,
                        response.code,
                    ));
                }
            }
            _ => {
                return Err(MailError::new(
                    MailErrorKind::NotSupported,
                    format!("authentication mechanism '{}' not supported", mechanism),
                ));
            }
        }

        Ok(())
    }

    fn smtp_ehlo(
        &self,
        conn: SmtpConnectionHandle,
        hostname: &str,
    ) -> Result<Vec<String>, MailError> {
        let mut conns = self.smtp_connections.lock().unwrap();
        let smtp_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let response = smtp_conn.send_and_read(&format!("EHLO {}", hostname))?;
        if response.code != 250 {
            // Fall back to HELO
            let response = smtp_conn.send_and_read(&format!("HELO {}", hostname))?;
            if response.code != 250 {
                return Err(MailError::with_code(
                    MailErrorKind::ProtocolError,
                    response.message,
                    response.code,
                ));
            }
            smtp_conn.capabilities.clear();
            return Ok(Vec::new());
        }

        // Parse capabilities from multiline response
        let caps: Vec<String> = response.message.lines().map(|s| s.to_string()).collect();
        smtp_conn.capabilities = caps.clone();

        Ok(caps)
    }
    fn smtp_quit(&self, conn: SmtpConnectionHandle) -> Result<(), MailError> {
        let mut conns = self.smtp_connections.lock().unwrap();
        let smtp_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Send QUIT command, expect 221
        let response = smtp_conn.send_and_read("QUIT")?;
        if response.code != 221 {
            return Err(MailError::with_code(
                MailErrorKind::ProtocolError,
                format!("QUIT failed: {}", response.message),
                response.code,
            ));
        }

        // Remove connection from map
        conns.remove(&conn.0);
        Ok(())
    }

    fn smtp_noop(&self, conn: SmtpConnectionHandle) -> Result<(), MailError> {
        let mut conns = self.smtp_connections.lock().unwrap();
        let smtp_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Send NOOP command, expect 250
        let response = smtp_conn.send_and_read("NOOP")?;
        if response.code != 250 {
            return Err(MailError::with_code(
                MailErrorKind::ProtocolError,
                format!("NOOP failed: {}", response.message),
                response.code,
            ));
        }
        Ok(())
    }

    fn smtp_reset(&self, conn: SmtpConnectionHandle) -> Result<(), MailError> {
        let mut conns = self.smtp_connections.lock().unwrap();
        let smtp_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Send RSET command, expect 250
        let response = smtp_conn.send_and_read("RSET")?;
        if response.code != 250 {
            return Err(MailError::with_code(
                MailErrorKind::ProtocolError,
                format!("RSET failed: {}", response.message),
                response.code,
            ));
        }
        Ok(())
    }

    fn smtp_mail_from(&self, conn: SmtpConnectionHandle, sender: &str) -> Result<(), MailError> {
        let mut conns = self.smtp_connections.lock().unwrap();
        let smtp_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Send MAIL FROM:<sender>, expect 250
        // If sender doesn't have angle brackets, add them
        let addr = if sender.starts_with('<') && sender.ends_with('>') {
            sender.to_string()
        } else {
            format!("<{}>", sender)
        };
        let response = smtp_conn.send_and_read(&format!("MAIL FROM:{}", addr))?;
        if response.code != 250 {
            return Err(MailError::with_code(
                MailErrorKind::ProtocolError,
                format!("MAIL FROM failed: {}", response.message),
                response.code,
            ));
        }
        Ok(())
    }

    fn smtp_rcpt_to(&self, conn: SmtpConnectionHandle, recipient: &str) -> Result<(), MailError> {
        let mut conns = self.smtp_connections.lock().unwrap();
        let smtp_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Send RCPT TO:<recipient>, expect 250 or 251 (forwarding)
        let addr = if recipient.starts_with('<') && recipient.ends_with('>') {
            recipient.to_string()
        } else {
            format!("<{}>", recipient)
        };
        let response = smtp_conn.send_and_read(&format!("RCPT TO:{}", addr))?;
        if response.code != 250 && response.code != 251 {
            return Err(MailError::with_code(
                MailErrorKind::ProtocolError,
                format!("RCPT TO failed: {}", response.message),
                response.code,
            ));
        }
        Ok(())
    }

    fn smtp_data(&self, conn: SmtpConnectionHandle) -> Result<(), MailError> {
        let mut conns = self.smtp_connections.lock().unwrap();
        let smtp_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Send DATA command, expect 354 "Start mail input"
        let response = smtp_conn.send_and_read("DATA")?;
        if response.code != 354 {
            return Err(MailError::with_code(
                MailErrorKind::ProtocolError,
                format!("DATA failed: {}", response.message),
                response.code,
            ));
        }
        Ok(())
    }

    fn smtp_send_data(&self, conn: SmtpConnectionHandle, data: &[u8]) -> Result<(), MailError> {
        let mut conns = self.smtp_connections.lock().unwrap();
        let smtp_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Write raw data to the stream (no CRLF added automatically)
        // Caller is responsible for proper line endings
        smtp_conn
            .stream
            .write_all(data)
            .map_err(|e| MailError::connection_error(e.to_string()))?;
        smtp_conn
            .stream
            .flush()
            .map_err(|e| MailError::connection_error(e.to_string()))
    }

    fn smtp_end_data(&self, conn: SmtpConnectionHandle) -> Result<(), MailError> {
        let mut conns = self.smtp_connections.lock().unwrap();
        let smtp_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Send <CRLF>.<CRLF> to end data, expect 250
        smtp_conn
            .stream
            .write_all(b"\r\n.\r\n")
            .map_err(|e| MailError::connection_error(e.to_string()))?;
        smtp_conn
            .stream
            .flush()
            .map_err(|e| MailError::connection_error(e.to_string()))?;

        let response = smtp_conn.read_response()?;
        if response.code != 250 {
            return Err(MailError::with_code(
                MailErrorKind::ProtocolError,
                format!("DATA end failed: {}", response.message),
                response.code,
            ));
        }
        Ok(())
    }

    fn smtp_read_response(&self, conn: SmtpConnectionHandle) -> Result<SmtpResponse, MailError> {
        let mut conns = self.smtp_connections.lock().unwrap();
        let smtp_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        smtp_conn.read_response()
    }

    fn smtp_get_capabilities(&self, conn: SmtpConnectionHandle) -> Result<Vec<String>, MailError> {
        let conns = self.smtp_connections.lock().unwrap();
        let smtp_conn = conns
            .get(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        Ok(smtp_conn.capabilities.clone())
    }

    fn smtp_send_message(
        &self,
        conn: SmtpConnectionHandle,
        from: &str,
        to: &[&str],
        message_data: &[u8],
    ) -> Result<(), MailError> {
        // High-level message sending: MAIL FROM, RCPT TO (multiple), DATA, message, end
        // Note: We can't hold the lock across multiple calls, so we do each step directly

        // Validate recipients
        if to.is_empty() {
            return Err(MailError::message_error("no recipients specified"));
        }

        // MAIL FROM
        self.smtp_mail_from(conn, from)?;

        // RCPT TO for each recipient
        for recipient in to {
            self.smtp_rcpt_to(conn, recipient)?;
        }

        // DATA
        self.smtp_data(conn)?;

        // Send message data with proper dot-stuffing
        // Lines starting with a period must be escaped with another period
        let message = String::from_utf8_lossy(message_data);
        let mut stuffed = String::new();
        for line in message.lines() {
            if line.starts_with('.') {
                stuffed.push('.');
            }
            stuffed.push_str(line);
            stuffed.push_str("\r\n");
        }

        self.smtp_send_data(conn, stuffed.as_bytes())?;

        // End DATA
        self.smtp_end_data(conn)
    }
    fn imap_connect(
        &self,
        host: &str,
        port: u16,
        use_tls: bool,
        _timeout_ms: u64,
    ) -> Result<ImapConnectionHandle, MailError> {
        // Create TCP socket connection using arth_rt
        let socket_handle = Self::connect_to_host(host, port)?;

        // Create the stream (plain or TLS)
        let mut stream = ImapStream::new_plain(socket_handle);

        // Upgrade to TLS if requested (implicit TLS, typically port 993)
        if use_tls {
            stream
                .upgrade_to_tls(host)
                .map_err(|e| MailError::tls_error(e.to_string()))?;
        }

        let mut conn = ImapConnection::new(stream, host.to_string());

        // Read server greeting
        conn.read_greeting()?;

        let handle = self.alloc_handle();
        self.imap_connections.lock().unwrap().insert(handle, conn);

        Ok(ImapConnectionHandle(handle))
    }

    fn imap_start_tls(&self, conn: ImapConnectionHandle) -> Result<(), MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Send STARTTLS command
        let response = imap_conn.send_and_read("STARTTLS")?;
        imap_conn.check_ok(&response)?;

        // Upgrade the connection to TLS
        let host = imap_conn.host.clone();
        imap_conn
            .stream
            .upgrade_to_tls(&host)
            .map_err(|e| MailError::tls_error(e.to_string()))?;

        // Clear cached capabilities after TLS upgrade
        imap_conn.capabilities.clear();

        Ok(())
    }

    fn imap_auth(
        &self,
        conn: ImapConnectionHandle,
        username: &str,
        password: &str,
    ) -> Result<(), MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Use LOGIN command (simple username/password auth)
        let cmd = format!(
            "LOGIN \"{}\" \"{}\"",
            username.replace('\\', "\\\\").replace('"', "\\\""),
            password.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let response = imap_conn.send_and_read(&cmd)?;

        if response.status != "OK" {
            return Err(MailError::with_code(
                MailErrorKind::AuthenticationError,
                response.message,
                0,
            ));
        }

        Ok(())
    }

    fn imap_auth_oauth(
        &self,
        conn: ImapConnectionHandle,
        username: &str,
        access_token: &str,
    ) -> Result<(), MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // XOAUTH2 authentication
        // Format: user=<username>\x01auth=Bearer <token>\x01\x01
        let oauth_string = format!("user={}\x01auth=Bearer {}\x01\x01", username, access_token);
        let encoded = base64_encode(oauth_string.as_bytes());

        let tag = imap_conn.next_tag();
        imap_conn.send_command(&tag, &format!("AUTHENTICATE XOAUTH2 {}", encoded))?;
        let response = imap_conn.read_response(&tag)?;

        if response.status != "OK" {
            return Err(MailError::with_code(
                MailErrorKind::AuthenticationError,
                response.message,
                0,
            ));
        }

        Ok(())
    }

    fn imap_logout(&self, conn: ImapConnectionHandle) -> Result<(), MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Send LOGOUT command
        let response = imap_conn.send_and_read("LOGOUT")?;

        // Remove from connections (stream will be closed on drop)
        conns.remove(&conn.0);

        // LOGOUT may return BYE (expected) before OK
        if response.status != "OK" && response.status != "BYE" {
            return Err(MailError::protocol_error(response.message));
        }

        Ok(())
    }

    fn imap_noop(&self, conn: ImapConnectionHandle) -> Result<(), MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let response = imap_conn.send_and_read("NOOP")?;
        imap_conn.check_ok(&response)
    }

    fn imap_capability(&self, conn: ImapConnectionHandle) -> Result<Vec<String>, MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let response = imap_conn.send_and_read("CAPABILITY")?;
        imap_conn.check_ok(&response)?;

        // Parse capabilities from untagged response
        for line in &response.untagged {
            if line.starts_with("CAPABILITY ") {
                imap_conn.capabilities = line[11..].split(' ').map(|s| s.to_string()).collect();
                return Ok(imap_conn.capabilities.clone());
            }
        }

        Ok(imap_conn.capabilities.clone())
    }

    fn imap_select(
        &self,
        conn: ImapConnectionHandle,
        mailbox: &str,
    ) -> Result<ImapFolderHandle, MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let cmd = format!(
            "SELECT \"{}\"",
            mailbox.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let response = imap_conn.send_and_read(&cmd)?;
        imap_conn.check_ok(&response)?;

        // Parse mailbox info from untagged responses
        let mut exists = 0u32;
        let mut recent = 0u32;
        let mut uidvalidity = 0u32;
        let mut uidnext = 0u32;

        for line in &response.untagged {
            if line.ends_with(" EXISTS") {
                exists = line.trim_end_matches(" EXISTS").parse().unwrap_or(0);
            } else if line.ends_with(" RECENT") {
                recent = line.trim_end_matches(" RECENT").parse().unwrap_or(0);
            } else if line.contains("UIDVALIDITY ") {
                if let Some(start) = line.find("UIDVALIDITY ") {
                    let rest = &line[start + 12..];
                    if let Some(end) = rest.find(']') {
                        uidvalidity = rest[..end].parse().unwrap_or(0);
                    }
                }
            } else if line.contains("UIDNEXT ") {
                if let Some(start) = line.find("UIDNEXT ") {
                    let rest = &line[start + 8..];
                    if let Some(end) = rest.find(']') {
                        uidnext = rest[..end].parse().unwrap_or(0);
                    }
                }
            }
        }

        imap_conn.selected_mailbox = Some(mailbox.to_string());

        let folder_handle = imap_conn.alloc_folder_handle();
        imap_conn.folder_handles.insert(
            folder_handle,
            ImapFolderState {
                name: mailbox.to_string(),
                exists,
                recent,
                uidvalidity,
                uidnext,
                read_only: false,
            },
        );

        Ok(ImapFolderHandle(folder_handle))
    }

    fn imap_examine(
        &self,
        conn: ImapConnectionHandle,
        mailbox: &str,
    ) -> Result<ImapFolderHandle, MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let cmd = format!(
            "EXAMINE \"{}\"",
            mailbox.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let response = imap_conn.send_and_read(&cmd)?;
        imap_conn.check_ok(&response)?;

        // Parse mailbox info (same as SELECT)
        let mut exists = 0u32;
        let mut recent = 0u32;
        let mut uidvalidity = 0u32;
        let mut uidnext = 0u32;

        for line in &response.untagged {
            if line.ends_with(" EXISTS") {
                exists = line.trim_end_matches(" EXISTS").parse().unwrap_or(0);
            } else if line.ends_with(" RECENT") {
                recent = line.trim_end_matches(" RECENT").parse().unwrap_or(0);
            } else if line.contains("UIDVALIDITY ") {
                if let Some(start) = line.find("UIDVALIDITY ") {
                    let rest = &line[start + 12..];
                    if let Some(end) = rest.find(']') {
                        uidvalidity = rest[..end].parse().unwrap_or(0);
                    }
                }
            } else if line.contains("UIDNEXT ") {
                if let Some(start) = line.find("UIDNEXT ") {
                    let rest = &line[start + 8..];
                    if let Some(end) = rest.find(']') {
                        uidnext = rest[..end].parse().unwrap_or(0);
                    }
                }
            }
        }

        imap_conn.selected_mailbox = Some(mailbox.to_string());

        let folder_handle = imap_conn.alloc_folder_handle();
        imap_conn.folder_handles.insert(
            folder_handle,
            ImapFolderState {
                name: mailbox.to_string(),
                exists,
                recent,
                uidvalidity,
                uidnext,
                read_only: true,
            },
        );

        Ok(ImapFolderHandle(folder_handle))
    }

    fn imap_list(
        &self,
        conn: ImapConnectionHandle,
        reference: &str,
        pattern: &str,
    ) -> Result<Vec<ImapMailbox>, MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let cmd = format!(
            "LIST \"{}\" \"{}\"",
            reference.replace('\\', "\\\\").replace('"', "\\\""),
            pattern.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let response = imap_conn.send_and_read(&cmd)?;
        imap_conn.check_ok(&response)?;

        let mut mailboxes = Vec::new();

        for line in &response.untagged {
            if line.starts_with("LIST ") {
                // Parse: LIST (\flags) "delimiter" "mailbox-name"
                if let Some(parsed) = Self::parse_list_response(&line[5..]) {
                    mailboxes.push(parsed);
                }
            }
        }

        Ok(mailboxes)
    }

    fn imap_fetch(
        &self,
        conn: ImapConnectionHandle,
        sequence: &str,
        items: &str,
    ) -> Result<Vec<ImapFetchResult>, MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let cmd = format!("FETCH {} {}", sequence, items);
        let response = imap_conn.send_and_read(&cmd)?;
        imap_conn.check_ok(&response)?;

        let mut results = Vec::new();

        for line in &response.untagged {
            // Parse: seq_num FETCH (data...)
            if let Some(parsed) = Self::parse_fetch_response(line) {
                results.push(parsed);
            }
        }

        Ok(results)
    }

    fn imap_search(
        &self,
        conn: ImapConnectionHandle,
        criteria: &str,
    ) -> Result<Vec<u32>, MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let cmd = format!("SEARCH {}", criteria);
        let response = imap_conn.send_and_read(&cmd)?;
        imap_conn.check_ok(&response)?;

        let mut results = Vec::new();

        for line in &response.untagged {
            if line.starts_with("SEARCH") {
                let nums = line[6..].trim();
                for num_str in nums.split_whitespace() {
                    if let Ok(num) = num_str.parse::<u32>() {
                        results.push(num);
                    }
                }
            }
        }

        Ok(results)
    }

    fn imap_store(
        &self,
        conn: ImapConnectionHandle,
        sequence: &str,
        flags: &str,
        action: &str,
    ) -> Result<(), MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // action: +FLAGS, -FLAGS, or FLAGS
        let cmd = format!("STORE {} {} ({})", sequence, action, flags);
        let response = imap_conn.send_and_read(&cmd)?;
        imap_conn.check_ok(&response)
    }

    fn imap_expunge(&self, conn: ImapConnectionHandle) -> Result<Vec<u32>, MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let response = imap_conn.send_and_read("EXPUNGE")?;
        imap_conn.check_ok(&response)?;

        let mut expunged = Vec::new();

        for line in &response.untagged {
            if line.ends_with(" EXPUNGE") {
                if let Ok(num) = line.trim_end_matches(" EXPUNGE").parse::<u32>() {
                    expunged.push(num);
                }
            }
        }

        Ok(expunged)
    }

    fn imap_idle(
        &self,
        conn: ImapConnectionHandle,
        _timeout_ms: u64,
    ) -> Result<ImapIdleEvent, MailError> {
        let mut conns = self.imap_connections.lock().unwrap();
        let imap_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Send IDLE command
        let tag = imap_conn.next_tag();
        imap_conn.send_command(&tag, "IDLE")?;

        // Wait for continuation
        let cont_response = imap_conn.read_response(&tag)?;
        if cont_response.tag != "+" {
            return Err(MailError::protocol_error("expected continuation for IDLE"));
        }

        // Read untagged responses until we get an event or timeout
        // For now, we do a simplified implementation that returns immediately
        // A full implementation would use select/poll with timeout

        // Send DONE to exit IDLE
        let line = "DONE\r\n";
        imap_conn
            .stream
            .write_all(line.as_bytes())
            .map_err(|e| MailError::connection_error(e.to_string()))?;
        imap_conn
            .stream
            .flush()
            .map_err(|e| MailError::connection_error(e.to_string()))?;

        // Read the tagged response
        let response = imap_conn.read_response(&tag)?;
        imap_conn.check_ok(&response)?;

        // Check for any events in untagged responses
        for line in &cont_response.untagged {
            if line.ends_with(" EXISTS") {
                if let Ok(num) = line.trim_end_matches(" EXISTS").parse::<u32>() {
                    return Ok(ImapIdleEvent::Exists(num));
                }
            } else if line.ends_with(" EXPUNGE") {
                if let Ok(num) = line.trim_end_matches(" EXPUNGE").parse::<u32>() {
                    return Ok(ImapIdleEvent::Expunge(num));
                }
            }
        }

        Ok(ImapIdleEvent::Timeout)
    }
    fn pop3_connect(
        &self,
        host: &str,
        port: u16,
        use_tls: bool,
        _timeout_ms: u64,
    ) -> Result<Pop3ConnectionHandle, MailError> {
        // Create TCP socket connection using arth_rt
        let socket_handle = Self::connect_to_host(host, port)?;

        // Create the stream (plain or TLS)
        let mut stream = Pop3Stream::new_plain(socket_handle);

        // Upgrade to TLS if requested (implicit TLS, typically port 995)
        if use_tls {
            stream
                .upgrade_to_tls(host)
                .map_err(|e| MailError::tls_error(e.to_string()))?;
        }

        let mut conn = Pop3Connection::new(stream, host.to_string());

        // Read server greeting
        let (ok, _msg) = conn.read_response()?;
        if !ok {
            return Err(MailError::connection_error(
                "POP3 server rejected connection",
            ));
        }

        let handle = self.alloc_handle();
        self.pop3_connections.lock().unwrap().insert(handle, conn);

        Ok(Pop3ConnectionHandle(handle))
    }

    fn pop3_start_tls(&self, conn: Pop3ConnectionHandle) -> Result<(), MailError> {
        let mut conns = self.pop3_connections.lock().unwrap();
        let pop3_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Send STLS command
        let (ok, msg) = pop3_conn.send_and_read("STLS")?;
        if !ok {
            return Err(MailError::protocol_error(msg));
        }

        // Upgrade the connection to TLS
        let host = pop3_conn.host.clone();
        pop3_conn
            .stream
            .upgrade_to_tls(&host)
            .map_err(|e| MailError::tls_error(e.to_string()))?;

        Ok(())
    }

    fn pop3_auth(
        &self,
        conn: Pop3ConnectionHandle,
        username: &str,
        password: &str,
    ) -> Result<(), MailError> {
        let mut conns = self.pop3_connections.lock().unwrap();
        let pop3_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // USER command
        let (ok, msg) = pop3_conn.send_and_read(&format!("USER {}", username))?;
        if !ok {
            return Err(MailError::with_code(
                MailErrorKind::AuthenticationError,
                msg,
                0,
            ));
        }

        // PASS command
        let (ok, msg) = pop3_conn.send_and_read(&format!("PASS {}", password))?;
        if !ok {
            return Err(MailError::with_code(
                MailErrorKind::AuthenticationError,
                msg,
                0,
            ));
        }

        Ok(())
    }

    fn pop3_auth_apop(
        &self,
        conn: Pop3ConnectionHandle,
        username: &str,
        _password: &str,
    ) -> Result<(), MailError> {
        // APOP requires the server timestamp from the greeting
        // For now, we'll return an error as this requires storing the timestamp
        let mut conns = self.pop3_connections.lock().unwrap();
        let _pop3_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // APOP authentication requires MD5 hash of timestamp + password
        // This is a simplified implementation
        Err(MailError::new(
            MailErrorKind::NotSupported,
            format!(
                "APOP authentication not implemented (username: {})",
                username
            ),
        ))
    }

    fn pop3_quit(&self, conn: Pop3ConnectionHandle) -> Result<(), MailError> {
        let mut conns = self.pop3_connections.lock().unwrap();
        let pop3_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Send QUIT command
        let (ok, msg) = pop3_conn.send_and_read("QUIT")?;

        // Remove from connections (stream will be closed on drop)
        conns.remove(&conn.0);

        if !ok {
            return Err(MailError::protocol_error(msg));
        }

        Ok(())
    }

    fn pop3_stat(&self, conn: Pop3ConnectionHandle) -> Result<Pop3Stat, MailError> {
        let mut conns = self.pop3_connections.lock().unwrap();
        let pop3_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let (ok, msg) = pop3_conn.send_and_read("STAT")?;
        if !ok {
            return Err(MailError::protocol_error(msg));
        }

        // Parse: count size
        let parts: Vec<&str> = msg.split_whitespace().collect();
        if parts.len() < 2 {
            return Err(MailError::protocol_error("invalid STAT response"));
        }

        let message_count = parts[0]
            .parse::<u32>()
            .map_err(|_| MailError::protocol_error("invalid message count"))?;
        let total_size = parts[1]
            .parse::<u64>()
            .map_err(|_| MailError::protocol_error("invalid total size"))?;

        Ok(Pop3Stat {
            message_count,
            total_size,
        })
    }

    fn pop3_list(&self, conn: Pop3ConnectionHandle) -> Result<Vec<Pop3MessageInfo>, MailError> {
        let mut conns = self.pop3_connections.lock().unwrap();
        let pop3_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let (ok, msg) = pop3_conn.send_and_read("LIST")?;
        if !ok {
            return Err(MailError::protocol_error(msg));
        }

        // Read multiline response
        let lines = pop3_conn.read_multiline_response()?;

        let mut messages = Vec::new();
        for line in lines {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                if let (Ok(msg_num), Ok(size)) = (parts[0].parse::<u32>(), parts[1].parse::<u64>())
                {
                    messages.push(Pop3MessageInfo { msg_num, size });
                }
            }
        }

        Ok(messages)
    }

    fn pop3_uidl(&self, conn: Pop3ConnectionHandle) -> Result<Vec<Pop3Uid>, MailError> {
        let mut conns = self.pop3_connections.lock().unwrap();
        let pop3_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let (ok, msg) = pop3_conn.send_and_read("UIDL")?;
        if !ok {
            return Err(MailError::protocol_error(msg));
        }

        // Read multiline response
        let lines = pop3_conn.read_multiline_response()?;

        let mut uids = Vec::new();
        for line in lines {
            let parts: Vec<&str> = line.splitn(2, ' ').collect();
            if parts.len() >= 2 {
                if let Ok(msg_num) = parts[0].parse::<u32>() {
                    uids.push(Pop3Uid {
                        msg_num,
                        uid: parts[1].to_string(),
                    });
                }
            }
        }

        Ok(uids)
    }

    fn pop3_retr(&self, conn: Pop3ConnectionHandle, msg_num: u32) -> Result<Vec<u8>, MailError> {
        let mut conns = self.pop3_connections.lock().unwrap();
        let pop3_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let (ok, msg) = pop3_conn.send_and_read(&format!("RETR {}", msg_num))?;
        if !ok {
            return Err(MailError::protocol_error(msg));
        }

        // Read multiline response
        let lines = pop3_conn.read_multiline_response()?;

        // Join lines with CRLF
        let mut data = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                data.extend_from_slice(b"\r\n");
            }
            data.extend_from_slice(line.as_bytes());
        }

        Ok(data)
    }

    fn pop3_dele(&self, conn: Pop3ConnectionHandle, msg_num: u32) -> Result<(), MailError> {
        let mut conns = self.pop3_connections.lock().unwrap();
        let pop3_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let (ok, msg) = pop3_conn.send_and_read(&format!("DELE {}", msg_num))?;
        if !ok {
            return Err(MailError::protocol_error(msg));
        }

        Ok(())
    }

    fn pop3_reset(&self, conn: Pop3ConnectionHandle) -> Result<(), MailError> {
        let mut conns = self.pop3_connections.lock().unwrap();
        let pop3_conn = conns
            .get_mut(&conn.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let (ok, msg) = pop3_conn.send_and_read("RSET")?;
        if !ok {
            return Err(MailError::protocol_error(msg));
        }

        Ok(())
    }
    fn mime_base64_encode(&self, data: &[u8]) -> String {
        base64_encode(data)
    }
    fn mime_base64_decode(&self, encoded: &str) -> Result<Vec<u8>, MailError> {
        base64_decode(encoded).map_err(|e| MailError::message_error(e))
    }
    fn mime_quoted_printable_encode(&self, data: &[u8]) -> String {
        let mut result = String::new();
        for &byte in data {
            if byte == b'=' || byte < 32 || byte > 126 {
                result.push_str(&format!("={:02X}", byte));
            } else {
                result.push(byte as char);
            }
        }
        result
    }
    fn mime_quoted_printable_decode(&self, encoded: &str) -> Result<Vec<u8>, MailError> {
        let mut result = Vec::new();
        let bytes = encoded.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'=' && i + 2 < bytes.len() {
                if let Ok(val) = u8::from_str_radix(&encoded[i + 1..i + 3], 16) {
                    result.push(val);
                    i += 3;
                    continue;
                }
            }
            result.push(bytes[i]);
            i += 1;
        }
        Ok(result)
    }
    fn mime_encode_header(&self, value: &str, _charset: &str) -> String {
        if value.is_ascii() {
            return value.to_string();
        }
        format!(
            "=?UTF-8?Q?{}?=",
            self.mime_quoted_printable_encode(value.as_bytes())
        )
    }
    fn mime_decode_header(&self, encoded: &str) -> Result<String, MailError> {
        if encoded.starts_with("=?") && encoded.ends_with("?=") {
            Ok(encoded.to_string())
        } else {
            Ok(encoded.to_string())
        }
    }
    fn mime_message_new(&self) -> MimeMessageHandle {
        let handle = self.alloc_handle();
        let msg_data = MimeMessageData {
            headers: HashMap::new(),
            body: Vec::new(),
            content_type: "text/plain; charset=utf-8".to_string(),
            attachments: Vec::new(),
        };
        self.mime_messages.lock().unwrap().insert(handle, msg_data);
        MimeMessageHandle(handle)
    }

    fn mime_message_set_header(
        &self,
        msg: MimeMessageHandle,
        name: &str,
        value: &str,
    ) -> Result<(), MailError> {
        let mut msgs = self.mime_messages.lock().unwrap();
        let msg_data = msgs
            .get_mut(&msg.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        // Encode header value if it contains non-ASCII characters
        let encoded_value = self.mime_encode_header(value, "UTF-8");
        msg_data.headers.insert(name.to_string(), encoded_value);
        Ok(())
    }

    fn mime_message_set_body(
        &self,
        msg: MimeMessageHandle,
        content_type: &str,
        body: &[u8],
    ) -> Result<(), MailError> {
        let mut msgs = self.mime_messages.lock().unwrap();
        let msg_data = msgs
            .get_mut(&msg.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        msg_data.content_type = content_type.to_string();
        msg_data.body = body.to_vec();
        Ok(())
    }

    fn mime_message_add_attachment(
        &self,
        msg: MimeMessageHandle,
        filename: &str,
        content_type: &str,
        data: &[u8],
    ) -> Result<(), MailError> {
        let mut msgs = self.mime_messages.lock().unwrap();
        let msg_data = msgs
            .get_mut(&msg.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        msg_data.attachments.push(MimeAttachment {
            filename: filename.to_string(),
            content_type: content_type.to_string(),
            data: data.to_vec(),
        });
        Ok(())
    }

    fn mime_message_serialize(&self, msg: MimeMessageHandle) -> Result<Vec<u8>, MailError> {
        let msgs = self.mime_messages.lock().unwrap();
        let msg_data = msgs
            .get(&msg.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        let mut output = String::new();

        // If we have attachments, create a multipart message
        if !msg_data.attachments.is_empty() {
            // Generate a unique boundary
            let boundary = format!(
                "----=_Part_{:016x}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );

            // Write headers (except Content-Type which we'll override)
            for (name, value) in &msg_data.headers {
                if name.to_lowercase() != "content-type" {
                    output.push_str(&format!("{}: {}\r\n", name, value));
                }
            }
            output.push_str("MIME-Version: 1.0\r\n");
            output.push_str(&format!(
                "Content-Type: multipart/mixed; boundary=\"{}\"\r\n",
                boundary
            ));
            output.push_str("\r\n");

            // Body part
            output.push_str(&format!("--{}\r\n", boundary));
            output.push_str(&format!("Content-Type: {}\r\n", msg_data.content_type));
            output.push_str("Content-Transfer-Encoding: base64\r\n");
            output.push_str("\r\n");
            output.push_str(&self.mime_base64_encode(&msg_data.body));
            output.push_str("\r\n");

            // Attachments
            for attachment in &msg_data.attachments {
                output.push_str(&format!("--{}\r\n", boundary));
                output.push_str(&format!(
                    "Content-Type: {}; name=\"{}\"\r\n",
                    attachment.content_type, attachment.filename
                ));
                output.push_str(&format!(
                    "Content-Disposition: attachment; filename=\"{}\"\r\n",
                    attachment.filename
                ));
                output.push_str("Content-Transfer-Encoding: base64\r\n");
                output.push_str("\r\n");
                // Base64 encode with line wrapping at 76 chars
                let encoded = self.mime_base64_encode(&attachment.data);
                for chunk in encoded.as_bytes().chunks(76) {
                    output.push_str(&String::from_utf8_lossy(chunk));
                    output.push_str("\r\n");
                }
            }

            // Final boundary
            output.push_str(&format!("--{}--\r\n", boundary));
        } else {
            // Simple message without attachments
            for (name, value) in &msg_data.headers {
                output.push_str(&format!("{}: {}\r\n", name, value));
            }
            output.push_str("MIME-Version: 1.0\r\n");
            output.push_str(&format!("Content-Type: {}\r\n", msg_data.content_type));

            // Use base64 for non-ASCII content, otherwise 7bit
            let needs_encoding = msg_data.body.iter().any(|&b| b > 127);
            if needs_encoding {
                output.push_str("Content-Transfer-Encoding: base64\r\n");
                output.push_str("\r\n");
                let encoded = self.mime_base64_encode(&msg_data.body);
                for chunk in encoded.as_bytes().chunks(76) {
                    output.push_str(&String::from_utf8_lossy(chunk));
                    output.push_str("\r\n");
                }
            } else {
                output.push_str("Content-Transfer-Encoding: 7bit\r\n");
                output.push_str("\r\n");
                output.push_str(&String::from_utf8_lossy(&msg_data.body));
                if !msg_data.body.ends_with(b"\n") {
                    output.push_str("\r\n");
                }
            }
        }

        Ok(output.into_bytes())
    }

    fn mime_message_parse(&self, data: &[u8]) -> Result<MimeMessageHandle, MailError> {
        let content = String::from_utf8_lossy(data);
        let handle = self.alloc_handle();
        let mut msg_data = MimeMessageData {
            headers: HashMap::new(),
            body: Vec::new(),
            content_type: "text/plain".to_string(),
            attachments: Vec::new(),
        };

        // Split headers and body at first blank line
        let mut in_headers = true;
        let mut body_start = 0;
        let lines: Vec<&str> = content.lines().collect();

        for (i, line) in lines.iter().enumerate() {
            if line.is_empty() && in_headers {
                in_headers = false;
                body_start = i + 1;
                continue;
            }

            if in_headers {
                // Parse header: Name: Value
                if let Some(colon_pos) = line.find(':') {
                    let name = line[..colon_pos].trim().to_string();
                    let value = line[colon_pos + 1..].trim().to_string();
                    if name.to_lowercase() == "content-type" {
                        msg_data.content_type = value.clone();
                    }
                    msg_data.headers.insert(name, value);
                }
            }
        }

        // Collect body (everything after the blank line)
        if body_start < lines.len() {
            let body_text = lines[body_start..].join("\r\n");

            // Check if we need to decode
            let transfer_encoding = msg_data
                .headers
                .get("Content-Transfer-Encoding")
                .or_else(|| msg_data.headers.get("content-transfer-encoding"))
                .map(|s| s.to_lowercase());

            match transfer_encoding.as_deref() {
                Some("base64") => {
                    // Remove whitespace and decode
                    let clean: String = body_text.chars().filter(|c| !c.is_whitespace()).collect();
                    msg_data.body = self
                        .mime_base64_decode(&clean)
                        .unwrap_or_else(|_| body_text.into_bytes());
                }
                Some("quoted-printable") => {
                    msg_data.body = self
                        .mime_quoted_printable_decode(&body_text)
                        .unwrap_or_else(|_| body_text.into_bytes());
                }
                _ => {
                    msg_data.body = body_text.into_bytes();
                }
            }
        }

        self.mime_messages.lock().unwrap().insert(handle, msg_data);
        Ok(MimeMessageHandle(handle))
    }

    fn mime_message_get_header(
        &self,
        msg: MimeMessageHandle,
        name: &str,
    ) -> Result<Option<String>, MailError> {
        let msgs = self.mime_messages.lock().unwrap();
        let msg_data = msgs
            .get(&msg.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        Ok(msg_data.headers.get(name).cloned())
    }

    fn mime_message_get_body(&self, msg: MimeMessageHandle) -> Result<Vec<u8>, MailError> {
        let msgs = self.mime_messages.lock().unwrap();
        let msg_data = msgs
            .get(&msg.0)
            .ok_or_else(|| MailError::invalid_handle())?;

        Ok(msg_data.body.clone())
    }

    fn mime_message_free(&self, msg: MimeMessageHandle) {
        self.mime_messages.lock().unwrap().remove(&msg.0);
    }
    fn tls_context_new(&self, _verify_certs: bool) -> Result<TlsContextHandle, MailError> {
        Err(MailError::new(
            MailErrorKind::NotSupported,
            "TLS not yet implemented",
        ))
    }
    fn tls_upgrade(
        &self,
        _ctx: TlsContextHandle,
        _hostname: &str,
    ) -> Result<TlsStreamHandle, MailError> {
        Err(MailError::new(
            MailErrorKind::NotSupported,
            "TLS not yet implemented",
        ))
    }
    fn tls_close(&self, _stream: TlsStreamHandle) -> Result<(), MailError> {
        Err(MailError::new(
            MailErrorKind::NotSupported,
            "TLS not yet implemented",
        ))
    }
}

// ============================================================================
// StdHostTime Implementation
// ============================================================================

/// Standard host time implementation using arth-rt C FFI layer.
///
/// This implementation uses the arth_rt_* functions which wrap libc directly,
/// making the same code usable for both VM and native compilation.
pub struct StdHostTime;

impl StdHostTime {
    pub fn new() -> Self {
        Self
    }

    /// Convert an arth_rt error code to TimeError
    fn error_from_code(code: i32) -> TimeError {
        use arth_rt::error::ErrorCode;

        let kind = match code {
            c if c == ErrorCode::InvalidArgument.as_i32() => TimeErrorKind::ParseError,
            c if c == ErrorCode::InvalidHandle.as_i32() => TimeErrorKind::InvalidHandle,
            c if c == ErrorCode::BufferTooSmall.as_i32() => TimeErrorKind::FormatError,
            _ => TimeErrorKind::Other,
        };

        let msg = arth_rt::error::get_last_error()
            .unwrap_or_else(|| format!("Time error (code {})", code));

        TimeError::new(kind, msg)
    }
}

impl Default for StdHostTime {
    fn default() -> Self {
        Self::new()
    }
}

impl HostTime for StdHostTime {
    fn now_realtime(&self) -> i64 {
        arth_rt::time::arth_rt_time_now()
    }

    fn parse(&self, format: &str, input: &str) -> Result<i64, TimeError> {
        // For ISO8601 with 'T' separator, use our custom parser since strptime
        // doesn't handle the 'T' separator consistently across platforms
        if format == "%Y-%m-%dT%H:%M:%S" || format == "ISO8601" {
            return parse_iso8601(input);
        }

        // Use arth_rt_time_parse (strptime) for other formats
        let result = arth_rt::time::arth_rt_time_parse(
            input.as_ptr(),
            input.len(),
            format.as_ptr(),
            format.len(),
        );

        if result < 0 {
            Err(Self::error_from_code(result as i32))
        } else {
            Ok(result)
        }
    }

    fn format(&self, millis: i64, format: &str) -> Result<String, TimeError> {
        let mut buf = vec![0u8; 256];

        let result = arth_rt::time::arth_rt_time_format(
            millis,
            format.as_ptr(),
            format.len(),
            buf.as_mut_ptr(),
            buf.len(),
        );

        if result < 0 {
            Err(Self::error_from_code(result))
        } else {
            buf.truncate(result as usize);
            String::from_utf8(buf).map_err(|_| {
                TimeError::new(
                    TimeErrorKind::FormatError,
                    "invalid UTF-8 in formatted output",
                )
            })
        }
    }

    fn instant_now(&self) -> InstantHandle {
        let handle = arth_rt::time::arth_rt_instant_now();
        InstantHandle(handle)
    }

    fn instant_elapsed(&self, instant: InstantHandle) -> Result<i64, TimeError> {
        let elapsed = arth_rt::time::arth_rt_instant_elapsed(instant.0);
        if elapsed < 0 {
            Err(Self::error_from_code(elapsed as i32))
        } else {
            Ok(elapsed)
        }
    }

    fn sleep(&self, millis: i64) {
        if millis > 0 {
            arth_rt::time::arth_rt_sleep(millis);
        }
    }
}

/// Simplified ISO 8601 parser: YYYY-MM-DDTHH:MM:SS
/// Used by both StdHostTime and MockHostTime.
fn parse_iso8601(input: &str) -> Result<i64, TimeError> {
    let parts: Vec<&str> = input.split('T').collect();
    if parts.len() != 2 {
        return Err(TimeError::parse_error(input, "ISO8601"));
    }

    let date_parts: Vec<&str> = parts[0].split('-').collect();
    let time_parts: Vec<&str> = parts[1].split(':').collect();

    if date_parts.len() != 3 || time_parts.len() < 2 {
        return Err(TimeError::parse_error(input, "ISO8601"));
    }

    let year: i32 = date_parts[0]
        .parse()
        .map_err(|_| TimeError::parse_error(input, "ISO8601"))?;
    let month: u32 = date_parts[1]
        .parse()
        .map_err(|_| TimeError::parse_error(input, "ISO8601"))?;
    let day: u32 = date_parts[2]
        .parse()
        .map_err(|_| TimeError::parse_error(input, "ISO8601"))?;
    let hour: u32 = time_parts[0]
        .parse()
        .map_err(|_| TimeError::parse_error(input, "ISO8601"))?;
    let minute: u32 = time_parts[1]
        .parse()
        .map_err(|_| TimeError::parse_error(input, "ISO8601"))?;
    let second: u32 = if time_parts.len() > 2 {
        // Handle seconds, possibly with fractional part
        let sec_str = time_parts[2].split('.').next().unwrap_or("0");
        // Also handle timezone suffix like 'Z' or '+00:00'
        let sec_str = sec_str
            .trim_end_matches('Z')
            .split('+')
            .next()
            .unwrap_or(sec_str)
            .split('-')
            .next()
            .unwrap_or(sec_str);
        sec_str
            .parse()
            .map_err(|_| TimeError::parse_error(input, "ISO8601"))?
    } else {
        0
    };

    // Calculate days since Unix epoch (1970-01-01)
    // This is a simplified calculation that doesn't handle all edge cases
    let mut days: i64 = 0;

    // Add days for years
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }
    for y in year..1970 {
        days -= if is_leap_year(y) { 366 } else { 365 };
    }

    // Add days for months
    let days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += days_in_month[(m - 1) as usize] as i64;
        if m == 2 && is_leap_year(year) {
            days += 1;
        }
    }

    // Add days
    days += (day - 1) as i64;

    // Convert to milliseconds
    let millis = days * 24 * 60 * 60 * 1000
        + (hour as i64) * 60 * 60 * 1000
        + (minute as i64) * 60 * 1000
        + (second as i64) * 1000;

    Ok(millis)
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

// ============================================================================
// MockHostTime Implementation (for testing)
// ============================================================================

/// Mock host time implementation for deterministic testing.
pub struct MockHostTime {
    current_time: std::sync::atomic::AtomicI64,
    instants: std::sync::Mutex<HashMap<i64, i64>>,
    next_handle: std::sync::atomic::AtomicI64,
}

impl MockHostTime {
    /// Create a new mock time starting at the given milliseconds since epoch.
    pub fn new(start_time_millis: i64) -> Self {
        Self {
            current_time: std::sync::atomic::AtomicI64::new(start_time_millis),
            instants: std::sync::Mutex::new(HashMap::new()),
            next_handle: std::sync::atomic::AtomicI64::new(1),
        }
    }

    /// Advance the mock time by the given milliseconds.
    pub fn advance(&self, millis: i64) {
        self.current_time
            .fetch_add(millis, std::sync::atomic::Ordering::SeqCst);
    }

    /// Set the mock time to a specific value.
    pub fn set_time(&self, millis: i64) {
        self.current_time
            .store(millis, std::sync::atomic::Ordering::SeqCst);
    }

    fn alloc_handle(&self) -> InstantHandle {
        let h = self
            .next_handle
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        InstantHandle(h)
    }
}

impl Default for MockHostTime {
    fn default() -> Self {
        // Default to Unix epoch
        Self::new(0)
    }
}

impl HostTime for MockHostTime {
    fn now_realtime(&self) -> i64 {
        self.current_time.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn parse(&self, format: &str, input: &str) -> Result<i64, TimeError> {
        // Delegate to StdHostTime's parser
        if format == "%Y-%m-%dT%H:%M:%S" || format == "ISO8601" {
            parse_iso8601(input)
        } else {
            Err(TimeError::parse_error(input, format))
        }
    }

    fn format(&self, millis: i64, _format: &str) -> Result<String, TimeError> {
        // Simple ISO 8601 formatting
        let secs = millis / 1000;
        let days = secs / 86400;
        let remaining = secs % 86400;
        let hours = remaining / 3600;
        let minutes = (remaining % 3600) / 60;
        let seconds = remaining % 60;

        // Calculate year, month, day from days since epoch
        let mut year = 1970;
        let mut remaining_days = days;

        loop {
            let days_in_year = if is_leap_year(year) { 366 } else { 365 };
            if remaining_days < days_in_year {
                break;
            }
            remaining_days -= days_in_year;
            year += 1;
        }

        let days_in_month = if is_leap_year(year) {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };

        let mut month = 1;
        for &dim in &days_in_month {
            if remaining_days < dim {
                break;
            }
            remaining_days -= dim;
            month += 1;
        }

        let day = remaining_days + 1;

        Ok(format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            year, month, day, hours, minutes, seconds
        ))
    }

    fn instant_now(&self) -> InstantHandle {
        let current = self.current_time.load(std::sync::atomic::Ordering::SeqCst);
        let handle = self.alloc_handle();
        self.instants.lock().unwrap().insert(handle.0, current);
        handle
    }

    fn instant_elapsed(&self, instant: InstantHandle) -> Result<i64, TimeError> {
        let instants = self.instants.lock().unwrap();
        let start = instants
            .get(&instant.0)
            .ok_or_else(TimeError::invalid_handle)?;
        let current = self.current_time.load(std::sync::atomic::Ordering::SeqCst);
        Ok(current - start)
    }

    fn sleep(&self, millis: i64) {
        // In mock mode, sleep just advances the clock
        if millis > 0 {
            self.advance(millis);
        }
    }
}

// ============================================================================
// StdHostDb Implementation
// ============================================================================

use std::sync::Mutex;

/// Stored value type for prepared statement parameters.
#[derive(Clone, Debug)]
enum SqlValue {
    Null,
    Int(i32),
    Int64(i64),
    Double(f64),
    Text(String),
    Blob(Vec<u8>),
}

/// A prepared statement entry storing SQL and bound parameters.
struct PreparedStatement {
    conn_handle: i64,
    sql: String,
    params: Vec<SqlValue>,
    /// Cached result rows (column values per row)
    result_rows: Vec<Vec<SqlValue>>,
    /// Column names
    column_names: Vec<String>,
    /// Current row index for iteration
    current_row: usize,
    /// Whether we've executed and have results ready
    executed: bool,
}

/// Transaction state for a connection.
/// Tracks nesting depth and savepoint names for automatic rollback.
#[derive(Clone, Debug, Default)]
struct TxState {
    /// Current transaction nesting depth.
    /// 0 = no transaction, 1 = top-level, 2+ = nested (savepoints)
    depth: u32,
    /// Savepoint names for nested transactions.
    /// Index 0 corresponds to depth 2, index 1 to depth 3, etc.
    savepoints: Vec<String>,
    /// Scope IDs for each depth level, to validate scope_end calls.
    scope_ids: Vec<i64>,
}

/// Standard host database implementation using arth_rt C FFI for SQLite and PostgreSQL.
pub struct StdHostDb {
    // SQLite storage - connections are managed by arth_rt, we just cache statements
    statements: Mutex<HashMap<i64, PreparedStatement>>,
    next_stmt_handle: std::sync::atomic::AtomicI64,
    // PostgreSQL storage (sync) - connections and results are managed by arth_rt
    // We only track prepared statement name mappings
    pg_prepared: Mutex<HashMap<i64, PgPreparedInfo>>,
    next_pg_stmt_handle: std::sync::atomic::AtomicI64,
    // PostgreSQL sync results storage
    pg_results: Mutex<HashMap<i64, PgStoredResult>>,
    next_pg_result_handle: std::sync::atomic::AtomicI64,
    // Async PostgreSQL storage (using arth_rt C FFI with non-blocking libpq)
    pg_async_connections: Mutex<HashMap<i64, PgAsyncConnection>>,
    pg_async_queries: Mutex<HashMap<i64, PgAsyncQueryState>>,
    next_pg_async_conn_handle: std::sync::atomic::AtomicI64,
    next_pg_async_query_handle: std::sync::atomic::AtomicI64,
    // Connection pool storage
    sqlite_pools: Mutex<HashMap<i64, SqlitePool>>,
    pg_pools: Mutex<HashMap<i64, PgPool>>,
    next_sqlite_pool_handle: std::sync::atomic::AtomicI64,
    next_pg_pool_handle: std::sync::atomic::AtomicI64,
    // Transaction state tracking
    sqlite_tx_state: Mutex<HashMap<i64, TxState>>,
    pg_tx_state: Mutex<HashMap<i64, TxState>>,
    next_tx_scope_id: std::sync::atomic::AtomicI64,
}

/// SQLite connection pool.
struct SqlitePool {
    /// Connection string for creating new connections.
    connection_string: String,
    /// Pool configuration.
    config: PoolConfig,
    /// Available connections in the pool.
    available: Vec<PooledSqliteConnection>,
    /// Handles of connections currently in use (borrowed).
    in_use: std::collections::HashSet<i64>,
    /// Condition variable for waiting when pool is exhausted.
    condvar: std::sync::Condvar,
    /// Lock for pool state.
    lock: std::sync::Mutex<()>,
}

/// A pooled SQLite connection with metadata.
struct PooledSqliteConnection {
    /// The connection handle.
    handle: i64,
    /// When this connection was created.
    created_at: std::time::Instant,
    /// When this connection was last used.
    last_used: std::time::Instant,
}

/// PostgreSQL connection pool.
struct PgPool {
    /// Connection string for creating new connections.
    connection_string: String,
    /// Pool configuration.
    config: PoolConfig,
    /// Available connections in the pool.
    available: Vec<PooledPgConnection>,
    /// Handles of connections currently in use (borrowed).
    in_use: std::collections::HashSet<i64>,
    /// Condition variable for waiting when pool is exhausted.
    condvar: std::sync::Condvar,
    /// Lock for pool state.
    lock: std::sync::Mutex<()>,
}

/// A pooled PostgreSQL connection with metadata.
struct PooledPgConnection {
    /// The connection handle.
    handle: i64,
    /// When this connection was created.
    created_at: std::time::Instant,
    /// When this connection was last used.
    last_used: std::time::Instant,
}

/// Async PostgreSQL connection wrapper using arth_rt C FFI.
///
/// This uses the non-blocking libpq API through arth_rt for true async operations.
struct PgAsyncConnection {
    /// The arth_rt connection handle.
    conn_handle: i64,
    /// Prepared statement names tracked by this connection.
    prepared_stmts: std::collections::HashSet<String>,
    /// Socket file descriptor for poll/select.
    socket_fd: i32,
}

/// State of an async PostgreSQL query.
enum PgAsyncQueryState {
    /// Query is in progress (non-blocking)
    Pending {
        /// The async connection handle.
        async_conn_handle: i64,
        /// Query type for proper result handling.
        query_type: PgAsyncQueryType,
        /// Socket fd for polling readability.
        socket_fd: i32,
    },
    /// Query completed successfully with result handle
    Completed {
        /// The arth_rt result handle.
        result_handle: i64,
        /// Column names (cached).
        column_names: Vec<String>,
        /// Column types (as OIDs).
        column_oids: Vec<u32>,
        /// Number of rows.
        row_count: i32,
    },
    /// Query completed with affected rows (for DML)
    CompletedRows(u64),
    /// Prepare completed
    PrepareCompleted(String),
    /// Transaction control completed
    TransactionCompleted,
    /// Query failed
    Failed(String),
}

/// Type of async query for proper result handling.
#[derive(Clone, Debug)]
enum PgAsyncQueryType {
    Query,
    Execute,
    Prepare { name: String },
    ExecutePrepared,
    Begin,
    Commit,
    Rollback,
}

/// Stored PostgreSQL result for random access.
/// Note: Row data is accessed directly via arth_rt functions, not stored here.
struct PgStoredResult {
    column_names: Vec<String>,
    /// Column type OIDs from PostgreSQL (e.g., 23 = INT4, 25 = TEXT)
    column_type_oids: Vec<u32>,
    affected_rows: u64,
}

/// Information about a prepared PostgreSQL statement.
#[derive(Clone)]
struct PgPreparedInfo {
    conn_handle: i64,
    name: String,
}

impl StdHostDb {
    pub fn new() -> Self {
        Self {
            // SQLite - connections managed by arth_rt, we just cache statements
            statements: Mutex::new(HashMap::new()),
            next_stmt_handle: std::sync::atomic::AtomicI64::new(1),
            // PostgreSQL (sync) - connections and results managed by arth_rt
            pg_prepared: Mutex::new(HashMap::new()),
            next_pg_stmt_handle: std::sync::atomic::AtomicI64::new(300_000),
            // PostgreSQL sync results
            pg_results: Mutex::new(HashMap::new()),
            next_pg_result_handle: std::sync::atomic::AtomicI64::new(200_000),
            // Async PostgreSQL (using arth_rt C FFI with non-blocking libpq)
            pg_async_connections: Mutex::new(HashMap::new()),
            pg_async_queries: Mutex::new(HashMap::new()),
            next_pg_async_conn_handle: std::sync::atomic::AtomicI64::new(400_000),
            next_pg_async_query_handle: std::sync::atomic::AtomicI64::new(500_000),
            // Connection pools
            sqlite_pools: Mutex::new(HashMap::new()),
            pg_pools: Mutex::new(HashMap::new()),
            next_sqlite_pool_handle: std::sync::atomic::AtomicI64::new(600_000),
            next_pg_pool_handle: std::sync::atomic::AtomicI64::new(700_000),
            // Transaction state
            sqlite_tx_state: Mutex::new(HashMap::new()),
            pg_tx_state: Mutex::new(HashMap::new()),
            next_tx_scope_id: std::sync::atomic::AtomicI64::new(1),
        }
    }

    /// Allocate a new transaction scope ID.
    fn alloc_tx_scope_id(&self) -> i64 {
        self.next_tx_scope_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    fn alloc_pg_async_conn_handle(&self) -> PgAsyncConnectionHandle {
        let h = self
            .next_pg_async_conn_handle
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        PgAsyncConnectionHandle(h)
    }

    fn alloc_pg_async_query_handle(&self) -> PgAsyncQueryHandle {
        let h = self
            .next_pg_async_query_handle
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        PgAsyncQueryHandle(h)
    }

    /// Convert PgValue slice to C string format for arth_rt parameterized queries.
    /// Returns (param_values, param_lengths, param_formats, cstrings) where cstrings must be kept alive.
    fn pg_values_to_c_params(
        params: &[PgValue],
    ) -> (
        Vec<*const libc::c_char>,
        Vec<i32>,
        Vec<i32>,
        Vec<std::ffi::CString>,
    ) {
        let mut cstrings = Vec::with_capacity(params.len());
        let mut values = Vec::with_capacity(params.len());
        let mut lengths = Vec::with_capacity(params.len());
        let mut formats = Vec::with_capacity(params.len());

        for param in params {
            let (cstr, len, fmt) = match param {
                PgValue::Null => {
                    // NULL is represented as a null pointer
                    cstrings.push(std::ffi::CString::new("").unwrap());
                    (std::ptr::null(), 0, 0)
                }
                PgValue::Bool(b) => {
                    let s = if *b { "t" } else { "f" };
                    let cstr = std::ffi::CString::new(s).unwrap();
                    let len = cstr.as_bytes().len() as i32;
                    cstrings.push(cstr);
                    (cstrings.last().unwrap().as_ptr(), len, 0)
                }
                PgValue::Int(i) => {
                    let s = i.to_string();
                    let cstr = std::ffi::CString::new(s).unwrap();
                    let len = cstr.as_bytes().len() as i32;
                    cstrings.push(cstr);
                    (cstrings.last().unwrap().as_ptr(), len, 0)
                }
                PgValue::Int64(i) => {
                    let s = i.to_string();
                    let cstr = std::ffi::CString::new(s).unwrap();
                    let len = cstr.as_bytes().len() as i32;
                    cstrings.push(cstr);
                    (cstrings.last().unwrap().as_ptr(), len, 0)
                }
                PgValue::Float(f) => {
                    let s = f.to_string();
                    let cstr = std::ffi::CString::new(s).unwrap();
                    let len = cstr.as_bytes().len() as i32;
                    cstrings.push(cstr);
                    (cstrings.last().unwrap().as_ptr(), len, 0)
                }
                PgValue::Double(d) => {
                    let s = d.to_string();
                    let cstr = std::ffi::CString::new(s).unwrap();
                    let len = cstr.as_bytes().len() as i32;
                    cstrings.push(cstr);
                    (cstrings.last().unwrap().as_ptr(), len, 0)
                }
                PgValue::Text(s) => {
                    let cstr = std::ffi::CString::new(s.as_str()).unwrap_or_else(|_| {
                        // Handle embedded NULs by replacing with space
                        std::ffi::CString::new(s.replace('\0', " ")).unwrap()
                    });
                    let len = cstr.as_bytes().len() as i32;
                    cstrings.push(cstr);
                    (cstrings.last().unwrap().as_ptr(), len, 0)
                }
                PgValue::Bytes(b) => {
                    // Binary format
                    let cstr = std::ffi::CString::new(b.clone()).unwrap_or_else(|_| {
                        // If bytes contain NUL, use text format with hex encoding
                        let hex: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
                        std::ffi::CString::new(format!("\\x{}", hex)).unwrap()
                    });
                    let len = cstr.as_bytes().len() as i32;
                    cstrings.push(cstr);
                    (cstrings.last().unwrap().as_ptr(), len, 1) // Binary format
                }
            };
            values.push(cstr);
            lengths.push(len);
            formats.push(fmt);
        }

        // Handle nulls properly - null params have null pointer
        for (i, param) in params.iter().enumerate() {
            if matches!(param, PgValue::Null) {
                values[i] = std::ptr::null();
            }
        }

        (values, lengths, formats, cstrings)
    }

    fn alloc_stmt_handle(&self) -> SqliteStatementHandle {
        let h = self
            .next_stmt_handle
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        SqliteStatementHandle(h)
    }

    fn alloc_pg_stmt_handle(&self) -> PgStatementHandle {
        let h = self
            .next_pg_stmt_handle
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        PgStatementHandle(h)
    }

    fn alloc_pg_result_handle(&self) -> PgResultHandle {
        let h = self
            .next_pg_result_handle
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        PgResultHandle(h)
    }

    /// Convert arth_rt error code to DbError for PostgreSQL
    fn pg_error_from_code(code: i32) -> DbError {
        use arth_rt::error::ErrorCode;

        let kind = match code {
            c if c == ErrorCode::InvalidHandle.as_i32() => DbErrorKind::InvalidHandle,
            c if c == ErrorCode::InvalidArgument.as_i32() => DbErrorKind::PrepareError,
            c if c == ErrorCode::ConnectionRefused.as_i32() => DbErrorKind::ConnectionError,
            c if c == ErrorCode::DbError.as_i32() => DbErrorKind::QueryError,
            c if c == ErrorCode::BufferTooSmall.as_i32() => DbErrorKind::Other,
            _ => DbErrorKind::QueryError,
        };

        let msg = arth_rt::error::get_last_error()
            .unwrap_or_else(|| format!("PostgreSQL error (code {})", code));

        DbError::new(kind, msg)
    }

    /// Convert arth_rt error code to DbError for SQLite
    fn sqlite_error_from_code(code: i32) -> DbError {
        use arth_rt::error::ErrorCode;

        let kind = match code {
            c if c == ErrorCode::InvalidHandle.as_i32() => DbErrorKind::InvalidHandle,
            c if c == ErrorCode::InvalidArgument.as_i32() => DbErrorKind::PrepareError,
            c if c == ErrorCode::NotFound.as_i32() => DbErrorKind::ConnectionError,
            c if c == ErrorCode::Busy.as_i32() => DbErrorKind::TransactionError,
            c if c == ErrorCode::BufferTooSmall.as_i32() => DbErrorKind::Other,
            c if c == ErrorCode::DbError.as_i32() => DbErrorKind::QueryError, // Constraint violations etc.
            _ => DbErrorKind::QueryError,
        };

        let msg = arth_rt::error::get_last_error()
            .unwrap_or_else(|| format!("SQLite error (code {})", code));

        DbError::new(kind, msg)
    }

    /// Execute a PostgreSQL query using arth_rt and return result handle (helper)
    fn pg_exec_helper(&self, conn: i64, sql: &str) -> Result<i64, DbError> {
        let handle = arth_rt::postgres::arth_rt_pg_exec(conn, sql.as_ptr(), sql.len());
        if handle < 0 {
            return Err(Self::pg_error_from_code(handle as i32));
        }
        Ok(handle)
    }

    /// Get a value from a PostgreSQL result as a string (helper)
    fn pg_get_value_str(&self, result: i64, row: i32, col: i32) -> Result<String, DbError> {
        // First check if null
        let is_null = arth_rt::postgres::arth_rt_pg_getisnull(result, row, col);
        if is_null < 0 {
            return Err(Self::pg_error_from_code(is_null));
        }
        if is_null == 1 {
            return Ok(String::new());
        }

        // Get value length
        let len = arth_rt::postgres::arth_rt_pg_getlength(result, row, col);
        if len < 0 {
            return Err(Self::pg_error_from_code(len));
        }

        if len == 0 {
            return Ok(String::new());
        }

        // Get the value
        let mut buf = vec![0u8; (len + 1) as usize];
        let rc =
            arth_rt::postgres::arth_rt_pg_getvalue(result, row, col, buf.as_mut_ptr(), buf.len());
        if rc < 0 {
            return Err(Self::pg_error_from_code(rc));
        }

        // Convert to string (rc is the length written)
        buf.truncate(rc as usize);
        String::from_utf8(buf).map_err(|_| DbError::type_mismatch("Invalid UTF-8"))
    }
}

impl Default for StdHostDb {
    fn default() -> Self {
        Self::new()
    }
}

impl HostDb for StdHostDb {
    fn sqlite_open(&self, path: &str) -> Result<SqliteConnectionHandle, DbError> {
        let handle = arth_rt::sqlite::arth_rt_sqlite_open(path.as_ptr(), path.len(), 0);
        if handle < 0 {
            return Err(Self::sqlite_error_from_code(handle as i32));
        }
        Ok(SqliteConnectionHandle(handle))
    }

    fn sqlite_close(&self, conn: SqliteConnectionHandle) -> Result<(), DbError> {
        // First, remove all cached statements associated with this connection
        {
            let mut stmts = self.statements.lock().unwrap();
            stmts.retain(|_, entry| entry.conn_handle != conn.0);
        }

        // Then close the connection (arth_rt auto-finalizes statements)
        let rc = arth_rt::sqlite::arth_rt_sqlite_close(conn.0);
        if rc < 0 {
            return Err(Self::sqlite_error_from_code(rc));
        }
        Ok(())
    }

    fn sqlite_prepare(
        &self,
        conn: SqliteConnectionHandle,
        sql: &str,
    ) -> Result<SqliteStatementHandle, DbError> {
        // Get column metadata immediately by preparing (but not executing) the statement
        let column_names = {
            // Prepare a temporary statement just to get column metadata
            let temp_stmt =
                arth_rt::sqlite::arth_rt_sqlite_prepare(conn.0, sql.as_ptr(), sql.len());
            if temp_stmt < 0 {
                return Err(Self::sqlite_error_from_code(temp_stmt as i32));
            }

            let col_count = arth_rt::sqlite::arth_rt_sqlite_column_count(temp_stmt);
            let mut names = Vec::with_capacity(col_count.max(0) as usize);
            for i in 0..col_count {
                let mut buf = [0u8; 256];
                let len = arth_rt::sqlite::arth_rt_sqlite_column_name(
                    temp_stmt,
                    i,
                    buf.as_mut_ptr(),
                    buf.len(),
                );
                if len >= 0 {
                    let name = String::from_utf8_lossy(&buf[..len as usize]).to_string();
                    names.push(name);
                } else {
                    names.push(format!("column_{}", i));
                }
            }
            arth_rt::sqlite::arth_rt_sqlite_finalize(temp_stmt);
            names
        };

        let handle = self.alloc_stmt_handle();
        let stmt = PreparedStatement {
            conn_handle: conn.0,
            sql: sql.to_string(),
            params: Vec::new(),
            result_rows: Vec::new(),
            column_names,
            current_row: 0,
            executed: false,
        };
        self.statements.lock().unwrap().insert(handle.0, stmt);
        Ok(handle)
    }

    fn sqlite_step(&self, stmt_handle: SqliteStatementHandle) -> Result<bool, DbError> {
        let mut stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get_mut(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        // If not yet executed, execute the statement and fetch all results
        if !stmt.executed {
            // Prepare the statement with arth_rt
            let arth_stmt = arth_rt::sqlite::arth_rt_sqlite_prepare(
                stmt.conn_handle,
                stmt.sql.as_ptr(),
                stmt.sql.len(),
            );
            if arth_stmt < 0 {
                return Err(Self::sqlite_error_from_code(arth_stmt as i32));
            }

            // Bind parameters
            for (i, param) in stmt.params.iter().enumerate() {
                let idx = (i + 1) as i32; // SQLite uses 1-based indexing
                let rc = match param {
                    SqlValue::Null => arth_rt::sqlite::arth_rt_sqlite_bind_null(arth_stmt, idx),
                    SqlValue::Int(v) => {
                        arth_rt::sqlite::arth_rt_sqlite_bind_int(arth_stmt, idx, *v)
                    }
                    SqlValue::Int64(v) => {
                        arth_rt::sqlite::arth_rt_sqlite_bind_int64(arth_stmt, idx, *v)
                    }
                    SqlValue::Double(v) => {
                        arth_rt::sqlite::arth_rt_sqlite_bind_double(arth_stmt, idx, *v)
                    }
                    SqlValue::Text(v) => arth_rt::sqlite::arth_rt_sqlite_bind_text(
                        arth_stmt,
                        idx,
                        v.as_ptr(),
                        v.len() as i32,
                    ),
                    SqlValue::Blob(v) => arth_rt::sqlite::arth_rt_sqlite_bind_blob(
                        arth_stmt,
                        idx,
                        v.as_ptr(),
                        v.len() as i32,
                    ),
                };
                if rc < 0 {
                    arth_rt::sqlite::arth_rt_sqlite_finalize(arth_stmt);
                    return Err(DbError::bind_error(format!(
                        "Failed to bind parameter {}: code {}",
                        idx, rc
                    )));
                }
            }

            // Get column count and update column names if needed
            let col_count = arth_rt::sqlite::arth_rt_sqlite_column_count(arth_stmt);
            if stmt.column_names.is_empty() && col_count > 0 {
                for i in 0..col_count {
                    let mut buf = [0u8; 256];
                    let len = arth_rt::sqlite::arth_rt_sqlite_column_name(
                        arth_stmt,
                        i,
                        buf.as_mut_ptr(),
                        buf.len(),
                    );
                    if len >= 0 {
                        let name = String::from_utf8_lossy(&buf[..len as usize]).to_string();
                        stmt.column_names.push(name);
                    } else {
                        stmt.column_names.push(format!("column_{}", i));
                    }
                }
            }

            // Execute and collect all rows
            stmt.result_rows.clear();
            loop {
                let step_result = arth_rt::sqlite::arth_rt_sqlite_step(arth_stmt);
                if step_result == 1 {
                    // SQLITE_ROW - there's a row
                    let mut row_values = Vec::new();
                    for i in 0..col_count {
                        let col_type = arth_rt::sqlite::arth_rt_sqlite_column_type(arth_stmt, i);
                        let sql_value = match col_type {
                            arth_rt::sqlite::SQLITE_NULL => SqlValue::Null,
                            arth_rt::sqlite::SQLITE_INTEGER => {
                                let v = arth_rt::sqlite::arth_rt_sqlite_column_int64(arth_stmt, i);
                                SqlValue::Int64(v)
                            }
                            arth_rt::sqlite::SQLITE_FLOAT => {
                                let v = arth_rt::sqlite::arth_rt_sqlite_column_double(arth_stmt, i);
                                SqlValue::Double(v)
                            }
                            arth_rt::sqlite::SQLITE_TEXT => {
                                // Get the actual size first
                                let size =
                                    arth_rt::sqlite::arth_rt_sqlite_column_bytes(arth_stmt, i);
                                if size < 0 {
                                    SqlValue::Null
                                } else {
                                    let mut buf = vec![0u8; (size as usize) + 1];
                                    let len = arth_rt::sqlite::arth_rt_sqlite_column_text(
                                        arth_stmt,
                                        i,
                                        buf.as_mut_ptr(),
                                        buf.len(),
                                    );
                                    if len >= 0 {
                                        buf.truncate(len as usize);
                                        SqlValue::Text(String::from_utf8_lossy(&buf).to_string())
                                    } else {
                                        SqlValue::Null
                                    }
                                }
                            }
                            arth_rt::sqlite::SQLITE_BLOB => {
                                // Get the actual size first
                                let size =
                                    arth_rt::sqlite::arth_rt_sqlite_column_bytes(arth_stmt, i);
                                if size < 0 {
                                    SqlValue::Null
                                } else if size == 0 {
                                    SqlValue::Blob(Vec::new())
                                } else {
                                    let mut buf = vec![0u8; size as usize];
                                    let len = arth_rt::sqlite::arth_rt_sqlite_column_blob(
                                        arth_stmt,
                                        i,
                                        buf.as_mut_ptr(),
                                        buf.len(),
                                    );
                                    if len >= 0 {
                                        buf.truncate(len as usize);
                                        SqlValue::Blob(buf)
                                    } else {
                                        SqlValue::Null
                                    }
                                }
                            }
                            _ => SqlValue::Null,
                        };
                        row_values.push(sql_value);
                    }
                    stmt.result_rows.push(row_values);
                } else if step_result == 0 {
                    // SQLITE_DONE - finished
                    break;
                } else {
                    // Error
                    arth_rt::sqlite::arth_rt_sqlite_finalize(arth_stmt);
                    return Err(Self::sqlite_error_from_code(step_result));
                }
            }

            arth_rt::sqlite::arth_rt_sqlite_finalize(arth_stmt);
            stmt.executed = true;
            stmt.current_row = 0;
        }

        // Return whether there's a current row
        if stmt.current_row < stmt.result_rows.len() {
            stmt.current_row += 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn sqlite_finalize(&self, stmt: SqliteStatementHandle) -> Result<(), DbError> {
        self.statements
            .lock()
            .unwrap()
            .remove(&stmt.0)
            .ok_or_else(DbError::invalid_handle)?;
        Ok(())
    }

    fn sqlite_reset(&self, stmt_handle: SqliteStatementHandle) -> Result<(), DbError> {
        let mut stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get_mut(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        stmt.executed = false;
        stmt.result_rows.clear();
        stmt.column_names.clear();
        stmt.current_row = 0;
        // Note: we keep params for rebinding
        Ok(())
    }

    fn sqlite_bind_int(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
        val: i32,
    ) -> Result<(), DbError> {
        let mut stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get_mut(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        let idx = idx as usize;
        while stmt.params.len() < idx {
            stmt.params.push(SqlValue::Null);
        }
        if idx > 0 && idx <= stmt.params.len() {
            stmt.params[idx - 1] = SqlValue::Int(val);
        } else if idx > stmt.params.len() {
            stmt.params.push(SqlValue::Int(val));
        }
        Ok(())
    }

    fn sqlite_bind_int64(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
        val: i64,
    ) -> Result<(), DbError> {
        let mut stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get_mut(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        let idx = idx as usize;
        while stmt.params.len() < idx {
            stmt.params.push(SqlValue::Null);
        }
        if idx > 0 && idx <= stmt.params.len() {
            stmt.params[idx - 1] = SqlValue::Int64(val);
        } else if idx > stmt.params.len() {
            stmt.params.push(SqlValue::Int64(val));
        }
        Ok(())
    }

    fn sqlite_bind_double(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
        val: f64,
    ) -> Result<(), DbError> {
        let mut stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get_mut(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        let idx = idx as usize;
        while stmt.params.len() < idx {
            stmt.params.push(SqlValue::Null);
        }
        if idx > 0 && idx <= stmt.params.len() {
            stmt.params[idx - 1] = SqlValue::Double(val);
        } else if idx > stmt.params.len() {
            stmt.params.push(SqlValue::Double(val));
        }
        Ok(())
    }

    fn sqlite_bind_text(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
        val: &str,
    ) -> Result<(), DbError> {
        let mut stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get_mut(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        let idx = idx as usize;
        while stmt.params.len() < idx {
            stmt.params.push(SqlValue::Null);
        }
        if idx > 0 && idx <= stmt.params.len() {
            stmt.params[idx - 1] = SqlValue::Text(val.to_string());
        } else if idx > stmt.params.len() {
            stmt.params.push(SqlValue::Text(val.to_string()));
        }
        Ok(())
    }

    fn sqlite_bind_blob(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
        val: &[u8],
    ) -> Result<(), DbError> {
        let mut stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get_mut(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        let idx = idx as usize;
        while stmt.params.len() < idx {
            stmt.params.push(SqlValue::Null);
        }
        if idx > 0 && idx <= stmt.params.len() {
            stmt.params[idx - 1] = SqlValue::Blob(val.to_vec());
        } else if idx > stmt.params.len() {
            stmt.params.push(SqlValue::Blob(val.to_vec()));
        }
        Ok(())
    }

    fn sqlite_bind_null(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
    ) -> Result<(), DbError> {
        let mut stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get_mut(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        let idx = idx as usize;
        while stmt.params.len() < idx {
            stmt.params.push(SqlValue::Null);
        }
        if idx > 0 && idx <= stmt.params.len() {
            stmt.params[idx - 1] = SqlValue::Null;
        } else if idx > stmt.params.len() {
            stmt.params.push(SqlValue::Null);
        }
        Ok(())
    }

    fn sqlite_column_int(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
    ) -> Result<i32, DbError> {
        let stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        if stmt.current_row == 0 || stmt.current_row > stmt.result_rows.len() {
            return Err(DbError::query_error("No current row"));
        }

        let row = &stmt.result_rows[stmt.current_row - 1];
        let col_idx = idx as usize;
        if col_idx >= row.len() {
            return Err(DbError::query_error("Column index out of range"));
        }

        match &row[col_idx] {
            SqlValue::Int(v) => Ok(*v),
            SqlValue::Int64(v) => Ok(*v as i32),
            SqlValue::Null => Ok(0),
            _ => Err(DbError::new(DbErrorKind::TypeMismatch, "Expected integer")),
        }
    }

    fn sqlite_column_int64(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
    ) -> Result<i64, DbError> {
        let stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        if stmt.current_row == 0 || stmt.current_row > stmt.result_rows.len() {
            return Err(DbError::query_error("No current row"));
        }

        let row = &stmt.result_rows[stmt.current_row - 1];
        let col_idx = idx as usize;
        if col_idx >= row.len() {
            return Err(DbError::query_error("Column index out of range"));
        }

        match &row[col_idx] {
            SqlValue::Int(v) => Ok(*v as i64),
            SqlValue::Int64(v) => Ok(*v),
            SqlValue::Null => Ok(0),
            _ => Err(DbError::new(DbErrorKind::TypeMismatch, "Expected integer")),
        }
    }

    fn sqlite_column_double(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
    ) -> Result<f64, DbError> {
        let stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        if stmt.current_row == 0 || stmt.current_row > stmt.result_rows.len() {
            return Err(DbError::query_error("No current row"));
        }

        let row = &stmt.result_rows[stmt.current_row - 1];
        let col_idx = idx as usize;
        if col_idx >= row.len() {
            return Err(DbError::query_error("Column index out of range"));
        }

        match &row[col_idx] {
            SqlValue::Double(v) => Ok(*v),
            SqlValue::Int(v) => Ok(*v as f64),
            SqlValue::Int64(v) => Ok(*v as f64),
            SqlValue::Null => Ok(0.0),
            _ => Err(DbError::new(DbErrorKind::TypeMismatch, "Expected real")),
        }
    }

    fn sqlite_column_text(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
    ) -> Result<String, DbError> {
        let stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        if stmt.current_row == 0 || stmt.current_row > stmt.result_rows.len() {
            return Err(DbError::query_error("No current row"));
        }

        let row = &stmt.result_rows[stmt.current_row - 1];
        let col_idx = idx as usize;
        if col_idx >= row.len() {
            return Err(DbError::query_error("Column index out of range"));
        }

        match &row[col_idx] {
            SqlValue::Text(v) => Ok(v.clone()),
            SqlValue::Null => Ok(String::new()),
            _ => Err(DbError::new(DbErrorKind::TypeMismatch, "Expected text")),
        }
    }

    fn sqlite_column_blob(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
    ) -> Result<Vec<u8>, DbError> {
        let stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        if stmt.current_row == 0 || stmt.current_row > stmt.result_rows.len() {
            return Err(DbError::query_error("No current row"));
        }

        let row = &stmt.result_rows[stmt.current_row - 1];
        let col_idx = idx as usize;
        if col_idx >= row.len() {
            return Err(DbError::query_error("Column index out of range"));
        }

        match &row[col_idx] {
            SqlValue::Blob(v) => Ok(v.clone()),
            SqlValue::Null => Ok(Vec::new()),
            _ => Err(DbError::new(DbErrorKind::TypeMismatch, "Expected blob")),
        }
    }

    fn sqlite_column_type(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
    ) -> Result<i32, DbError> {
        let stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        if stmt.current_row == 0 || stmt.current_row > stmt.result_rows.len() {
            return Err(DbError::query_error("No current row"));
        }

        let row = &stmt.result_rows[stmt.current_row - 1];
        let col_idx = idx as usize;
        if col_idx >= row.len() {
            return Err(DbError::query_error("Column index out of range"));
        }

        // SQLite type codes: 1=INTEGER, 2=FLOAT, 3=TEXT, 4=BLOB, 5=NULL
        Ok(match &row[col_idx] {
            SqlValue::Int(_) | SqlValue::Int64(_) => 1, // SQLITE_INTEGER
            SqlValue::Double(_) => 2,                   // SQLITE_FLOAT
            SqlValue::Text(_) => 3,                     // SQLITE_TEXT
            SqlValue::Blob(_) => 4,                     // SQLITE_BLOB
            SqlValue::Null => 5,                        // SQLITE_NULL
        })
    }

    fn sqlite_column_count(&self, stmt_handle: SqliteStatementHandle) -> Result<i32, DbError> {
        let stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        Ok(stmt.column_names.len() as i32)
    }

    fn sqlite_column_name(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
    ) -> Result<String, DbError> {
        let stmts = self.statements.lock().unwrap();
        let stmt = stmts
            .get(&stmt_handle.0)
            .ok_or_else(DbError::invalid_handle)?;

        let col_idx = idx as usize;
        if col_idx >= stmt.column_names.len() {
            return Err(DbError::query_error("Column index out of range"));
        }

        Ok(stmt.column_names[col_idx].clone())
    }

    fn sqlite_is_null(
        &self,
        stmt_handle: SqliteStatementHandle,
        idx: i32,
    ) -> Result<bool, DbError> {
        let col_type = self.sqlite_column_type(stmt_handle, idx)?;
        Ok(col_type == 5) // SQLITE_NULL
    }

    fn sqlite_changes(&self, conn: SqliteConnectionHandle) -> Result<i32, DbError> {
        let changes = arth_rt::sqlite::arth_rt_sqlite_changes(conn.0);
        if changes < 0 {
            return Err(Self::sqlite_error_from_code(changes as i32));
        }
        Ok(changes as i32)
    }

    fn sqlite_last_insert_rowid(&self, conn: SqliteConnectionHandle) -> Result<i64, DbError> {
        let rowid = arth_rt::sqlite::arth_rt_sqlite_last_insert_rowid(conn.0);
        if rowid < 0 {
            // Check if it's actually an error (negative error codes)
            // Note: rowid 0 is valid (no insert), negative rowid is also technically valid
            // but very unlikely. We treat very negative values as errors.
            if rowid < -1000 {
                return Err(Self::sqlite_error_from_code(rowid as i32));
            }
        }
        Ok(rowid)
    }

    fn sqlite_errmsg(&self, conn: SqliteConnectionHandle) -> Result<String, DbError> {
        let mut buf = [0u8; 1024];
        let len = arth_rt::sqlite::arth_rt_sqlite_errmsg(conn.0, buf.as_mut_ptr(), buf.len());
        if len < 0 {
            return Err(Self::sqlite_error_from_code(len));
        }
        Ok(String::from_utf8_lossy(&buf[..len as usize]).to_string())
    }

    fn sqlite_execute(&self, conn: SqliteConnectionHandle, sql: &str) -> Result<(), DbError> {
        // Execute: prepare, step until done, finalize
        let stmt = self.sqlite_prepare(conn, sql)?;
        // Step until no more rows (execute doesn't need to return results)
        while self.sqlite_step(stmt)? {}
        self.sqlite_finalize(stmt)?;
        Ok(())
    }

    fn sqlite_query(
        &self,
        conn: SqliteConnectionHandle,
        sql: &str,
    ) -> Result<SqliteStatementHandle, DbError> {
        // Query is just an alias for prepare
        self.sqlite_prepare(conn, sql)
    }

    fn sqlite_begin(&self, conn: SqliteConnectionHandle) -> Result<(), DbError> {
        self.sqlite_execute(conn, "BEGIN TRANSACTION")
    }

    fn sqlite_commit(&self, conn: SqliteConnectionHandle) -> Result<(), DbError> {
        self.sqlite_execute(conn, "COMMIT")
    }

    fn sqlite_rollback(&self, conn: SqliteConnectionHandle) -> Result<(), DbError> {
        self.sqlite_execute(conn, "ROLLBACK")
    }

    fn sqlite_savepoint(&self, conn: SqliteConnectionHandle, name: &str) -> Result<(), DbError> {
        self.sqlite_execute(conn, &format!("SAVEPOINT {}", name))
    }

    fn sqlite_release_savepoint(
        &self,
        conn: SqliteConnectionHandle,
        name: &str,
    ) -> Result<(), DbError> {
        self.sqlite_execute(conn, &format!("RELEASE SAVEPOINT {}", name))
    }

    fn sqlite_rollback_to_savepoint(
        &self,
        conn: SqliteConnectionHandle,
        name: &str,
    ) -> Result<(), DbError> {
        self.sqlite_execute(conn, &format!("ROLLBACK TO SAVEPOINT {}", name))
    }

    // =========================================================================
    // PostgreSQL Implementation (using arth_rt C FFI)
    // =========================================================================

    fn pg_connect(&self, connection_string: &str) -> Result<PgConnectionHandle, DbError> {
        let handle = arth_rt::postgres::arth_rt_pg_connect(
            connection_string.as_ptr(),
            connection_string.len(),
        );
        if handle < 0 {
            return Err(Self::pg_error_from_code(handle as i32));
        }
        Ok(PgConnectionHandle(handle))
    }

    fn pg_disconnect(&self, conn: PgConnectionHandle) -> Result<(), DbError> {
        // Remove any prepared statements associated with this connection
        {
            let mut prepared = self.pg_prepared.lock().unwrap();
            prepared.retain(|_, info| info.conn_handle != conn.0);
        }

        // Close the connection using arth_rt
        let rc = arth_rt::postgres::arth_rt_pg_finish(conn.0);
        if rc < 0 {
            return Err(Self::pg_error_from_code(rc));
        }
        Ok(())
    }

    fn pg_status(&self, conn: PgConnectionHandle) -> Result<bool, DbError> {
        let status = arth_rt::postgres::arth_rt_pg_status(conn.0);
        if status < 0 {
            return Err(Self::pg_error_from_code(status));
        }
        // CONNECTION_OK is 0, CONNECTION_BAD is 1
        Ok(status == 0)
    }

    fn pg_query(
        &self,
        conn: PgConnectionHandle,
        sql: &str,
        params: &[PgValue],
    ) -> Result<PgResultHandle, DbError> {
        let result_handle = if params.is_empty() {
            // Simple query without parameters
            arth_rt::postgres::arth_rt_pg_exec(conn.0, sql.as_ptr(), sql.len())
        } else {
            // Parameterized query - convert PgValue to CStrings
            let param_cstrings: Vec<std::ffi::CString> = params
                .iter()
                .map(|p| {
                    let s = Self::format_param_value(p);
                    std::ffi::CString::new(s).unwrap()
                })
                .collect();

            let param_ptrs: Vec<*const libc::c_char> =
                param_cstrings.iter().map(|cstr| cstr.as_ptr()).collect();

            arth_rt::postgres::arth_rt_pg_exec_params(
                conn.0,
                sql.as_ptr(),
                sql.len(),
                params.len() as i32,
                param_ptrs.as_ptr(),
                std::ptr::null(), // param_lengths - null for text format
                std::ptr::null(), // param_formats - null for text format
            )
        };

        if result_handle < 0 {
            return Err(Self::pg_error_from_code(result_handle as i32));
        }

        Ok(PgResultHandle(result_handle))
    }

    fn pg_execute(
        &self,
        conn: PgConnectionHandle,
        sql: &str,
        params: &[PgValue],
    ) -> Result<u64, DbError> {
        // Execute the query
        let result = self.pg_query(conn, sql, params)?;

        // Get affected rows
        let affected = arth_rt::postgres::arth_rt_pg_cmd_tuples(result.0);

        // Free the result
        arth_rt::postgres::arth_rt_pg_clear(result.0);

        if affected < 0 {
            return Err(Self::pg_error_from_code(affected as i32));
        }

        Ok(affected as u64)
    }

    fn pg_prepare(
        &self,
        conn: PgConnectionHandle,
        name: &str,
        sql: &str,
    ) -> Result<PgStatementHandle, DbError> {
        // Use arth_rt_pg_prepare to prepare the statement
        let result_handle = arth_rt::postgres::arth_rt_pg_prepare(
            conn.0,
            name.as_ptr(),
            name.len(),
            sql.as_ptr(),
            sql.len(),
            0, // nparams - let server infer
        );

        if result_handle < 0 {
            return Err(Self::pg_error_from_code(result_handle as i32));
        }

        // Free the prepare result (it's just for status)
        arth_rt::postgres::arth_rt_pg_clear(result_handle);

        // Track the prepared statement
        let handle = self.alloc_pg_stmt_handle();
        let info = PgPreparedInfo {
            conn_handle: conn.0,
            name: name.to_string(),
        };
        self.pg_prepared.lock().unwrap().insert(handle.0, info);
        Ok(handle)
    }

    fn pg_execute_prepared(
        &self,
        conn: PgConnectionHandle,
        stmt: PgStatementHandle,
        params: &[PgValue],
    ) -> Result<PgResultHandle, DbError> {
        let info = {
            let prepared = self.pg_prepared.lock().unwrap();
            prepared
                .get(&stmt.0)
                .ok_or_else(DbError::invalid_handle)?
                .clone()
        };

        if info.conn_handle != conn.0 {
            return Err(DbError::invalid_handle());
        }

        // Convert params to CStrings
        let param_cstrings: Vec<std::ffi::CString> = params
            .iter()
            .map(|p| {
                let s = Self::format_param_value(p);
                std::ffi::CString::new(s).unwrap()
            })
            .collect();

        let param_ptrs: Vec<*const libc::c_char> =
            param_cstrings.iter().map(|cstr| cstr.as_ptr()).collect();

        let result_handle = arth_rt::postgres::arth_rt_pg_exec_prepared(
            conn.0,
            info.name.as_ptr(),
            info.name.len(),
            params.len() as i32,
            if params.is_empty() {
                std::ptr::null()
            } else {
                param_ptrs.as_ptr()
            },
            std::ptr::null(), // param_lengths
            std::ptr::null(), // param_formats
        );

        if result_handle < 0 {
            return Err(Self::pg_error_from_code(result_handle as i32));
        }

        Ok(PgResultHandle(result_handle))
    }

    fn pg_row_count(&self, result: PgResultHandle) -> Result<i64, DbError> {
        let count = arth_rt::postgres::arth_rt_pg_ntuples(result.0);
        if count < 0 {
            return Err(Self::pg_error_from_code(count));
        }
        Ok(count as i64)
    }

    fn pg_column_count(&self, result: PgResultHandle) -> Result<i32, DbError> {
        let count = arth_rt::postgres::arth_rt_pg_nfields(result.0);
        if count < 0 {
            return Err(Self::pg_error_from_code(count));
        }
        Ok(count)
    }

    fn pg_column_name(&self, result: PgResultHandle, col: i32) -> Result<String, DbError> {
        let mut buf = [0u8; 256];
        let len = arth_rt::postgres::arth_rt_pg_fname(result.0, col, buf.as_mut_ptr(), buf.len());
        if len < 0 {
            return Err(Self::pg_error_from_code(len));
        }
        Ok(String::from_utf8_lossy(&buf[..len as usize]).to_string())
    }

    fn pg_column_type(&self, result: PgResultHandle, col: i32) -> Result<i32, DbError> {
        let oid = arth_rt::postgres::arth_rt_pg_ftype(result.0, col);
        if oid < 0 {
            return Err(Self::pg_error_from_code(oid));
        }
        Ok(oid)
    }

    fn pg_get_value(&self, result: PgResultHandle, row: i64, col: i32) -> Result<String, DbError> {
        self.pg_get_value_str(result.0, row as i32, col)
    }

    fn pg_get_int(&self, result: PgResultHandle, row: i64, col: i32) -> Result<i32, DbError> {
        let val = self.pg_get_value_str(result.0, row as i32, col)?;
        if val.is_empty() {
            return Err(DbError::type_mismatch("NULL value"));
        }
        val.parse::<i32>()
            .map_err(|e| DbError::type_mismatch(e.to_string()))
    }

    fn pg_get_int64(&self, result: PgResultHandle, row: i64, col: i32) -> Result<i64, DbError> {
        let val = self.pg_get_value_str(result.0, row as i32, col)?;
        if val.is_empty() {
            return Err(DbError::type_mismatch("NULL value"));
        }
        val.parse::<i64>()
            .map_err(|e| DbError::type_mismatch(e.to_string()))
    }

    fn pg_get_double(&self, result: PgResultHandle, row: i64, col: i32) -> Result<f64, DbError> {
        let val = self.pg_get_value_str(result.0, row as i32, col)?;
        if val.is_empty() {
            return Err(DbError::type_mismatch("NULL value"));
        }
        val.parse::<f64>()
            .map_err(|e| DbError::type_mismatch(e.to_string()))
    }

    fn pg_get_text(&self, result: PgResultHandle, row: i64, col: i32) -> Result<String, DbError> {
        self.pg_get_value_str(result.0, row as i32, col)
    }

    fn pg_get_bytes(&self, result: PgResultHandle, row: i64, col: i32) -> Result<Vec<u8>, DbError> {
        let val = self.pg_get_value_str(result.0, row as i32, col)?;
        // PostgreSQL bytea format: \x followed by hex digits
        if val.starts_with("\\x") {
            let hex = &val[2..];
            let mut bytes = Vec::with_capacity(hex.len() / 2);
            for chunk in hex.as_bytes().chunks(2) {
                if chunk.len() == 2 {
                    let byte = u8::from_str_radix(std::str::from_utf8(chunk).unwrap_or("00"), 16)
                        .unwrap_or(0);
                    bytes.push(byte);
                }
            }
            Ok(bytes)
        } else {
            // Return raw bytes
            Ok(val.into_bytes())
        }
    }

    fn pg_get_bool(&self, result: PgResultHandle, row: i64, col: i32) -> Result<bool, DbError> {
        let val = self.pg_get_value_str(result.0, row as i32, col)?;
        match val.as_str() {
            "t" | "true" | "TRUE" | "1" | "yes" | "on" => Ok(true),
            "f" | "false" | "FALSE" | "0" | "no" | "off" => Ok(false),
            "" => Err(DbError::type_mismatch("NULL value")),
            _ => Err(DbError::type_mismatch(format!("Invalid boolean: {}", val))),
        }
    }

    fn pg_is_null(&self, result: PgResultHandle, row: i64, col: i32) -> Result<bool, DbError> {
        let is_null = arth_rt::postgres::arth_rt_pg_getisnull(result.0, row as i32, col);
        if is_null < 0 {
            return Err(Self::pg_error_from_code(is_null));
        }
        Ok(is_null == 1)
    }

    fn pg_affected_rows(&self, result: PgResultHandle) -> Result<u64, DbError> {
        let count = arth_rt::postgres::arth_rt_pg_cmd_tuples(result.0);
        if count < 0 {
            return Err(Self::pg_error_from_code(count as i32));
        }
        Ok(count as u64)
    }

    fn pg_free_result(&self, result: PgResultHandle) -> Result<(), DbError> {
        let rc = arth_rt::postgres::arth_rt_pg_clear(result.0);
        if rc < 0 {
            return Err(Self::pg_error_from_code(rc));
        }
        Ok(())
    }

    fn pg_begin(&self, conn: PgConnectionHandle) -> Result<(), DbError> {
        let result = arth_rt::postgres::arth_rt_pg_begin(conn.0);
        if result < 0 {
            return Err(Self::pg_error_from_code(result as i32));
        }
        // Free the result
        arth_rt::postgres::arth_rt_pg_clear(result);
        Ok(())
    }

    fn pg_commit(&self, conn: PgConnectionHandle) -> Result<(), DbError> {
        let result = arth_rt::postgres::arth_rt_pg_commit(conn.0);
        if result < 0 {
            return Err(Self::pg_error_from_code(result as i32));
        }
        // Free the result
        arth_rt::postgres::arth_rt_pg_clear(result);
        Ok(())
    }

    fn pg_rollback(&self, conn: PgConnectionHandle) -> Result<(), DbError> {
        let result = arth_rt::postgres::arth_rt_pg_rollback(conn.0);
        if result < 0 {
            return Err(Self::pg_error_from_code(result as i32));
        }
        // Free the result
        arth_rt::postgres::arth_rt_pg_clear(result);
        Ok(())
    }

    fn pg_savepoint(&self, conn: PgConnectionHandle, name: &str) -> Result<(), DbError> {
        let sql = format!("SAVEPOINT {}", name);
        let result = self.pg_exec_helper(conn.0, &sql)?;
        arth_rt::postgres::arth_rt_pg_clear(result);
        Ok(())
    }

    fn pg_release_savepoint(&self, conn: PgConnectionHandle, name: &str) -> Result<(), DbError> {
        let sql = format!("RELEASE SAVEPOINT {}", name);
        let result = self.pg_exec_helper(conn.0, &sql)?;
        arth_rt::postgres::arth_rt_pg_clear(result);
        Ok(())
    }

    fn pg_rollback_to_savepoint(
        &self,
        conn: PgConnectionHandle,
        name: &str,
    ) -> Result<(), DbError> {
        let sql = format!("ROLLBACK TO SAVEPOINT {}", name);
        let result = self.pg_exec_helper(conn.0, &sql)?;
        arth_rt::postgres::arth_rt_pg_clear(result);
        Ok(())
    }

    fn pg_errmsg(&self, conn: PgConnectionHandle) -> Result<String, DbError> {
        let mut buf = [0u8; 1024];
        let len = arth_rt::postgres::arth_rt_pg_error_message(conn.0, buf.as_mut_ptr(), buf.len());
        if len < 0 {
            return Err(Self::pg_error_from_code(len));
        }
        Ok(String::from_utf8_lossy(&buf[..len as usize]).to_string())
    }

    fn pg_escape(&self, _conn: PgConnectionHandle, s: &str) -> Result<String, DbError> {
        // Basic SQL escaping - double single quotes
        Ok(s.replace('\'', "''"))
    }

    // =========================================================================
    // Async PostgreSQL Operations (using arth_rt C FFI with non-blocking libpq)
    // =========================================================================

    fn pg_connect_async(
        &self,
        connection_string: &str,
    ) -> Result<PgAsyncConnectionHandle, DbError> {
        // Connect using arth_rt C FFI
        let conn_handle = arth_rt::postgres::arth_rt_pg_connect(
            connection_string.as_ptr(),
            connection_string.len(),
        );

        if conn_handle < 0 {
            return Err(Self::pg_error_from_code(conn_handle as i32));
        }

        // Set non-blocking mode
        let result = arth_rt::postgres::arth_rt_pg_set_nonblocking(conn_handle, 1);
        if result != 0 {
            arth_rt::postgres::arth_rt_pg_finish(conn_handle);
            return Err(DbError::connection_error(
                "Failed to set non-blocking mode on connection",
            ));
        }

        // Get the socket fd for polling
        let socket_fd = arth_rt::postgres::arth_rt_pg_socket(conn_handle);
        if socket_fd < 0 {
            arth_rt::postgres::arth_rt_pg_finish(conn_handle);
            return Err(DbError::connection_error("Failed to get connection socket"));
        }

        let handle = self.alloc_pg_async_conn_handle();
        let conn = PgAsyncConnection {
            conn_handle,
            prepared_stmts: std::collections::HashSet::new(),
            socket_fd,
        };
        self.pg_async_connections
            .lock()
            .unwrap()
            .insert(handle.0, conn);
        Ok(handle)
    }

    fn pg_disconnect_async(&self, conn: PgAsyncConnectionHandle) -> Result<(), DbError> {
        // Remove the connection and close it
        let async_conn = self
            .pg_async_connections
            .lock()
            .unwrap()
            .remove(&conn.0)
            .ok_or_else(|| DbError::connection_error("Invalid async connection handle"))?;

        // Close the underlying connection
        arth_rt::postgres::arth_rt_pg_finish(async_conn.conn_handle);
        Ok(())
    }

    fn pg_status_async(&self, conn: PgAsyncConnectionHandle) -> Result<bool, DbError> {
        let conns = self.pg_async_connections.lock().unwrap();
        let async_conn = conns
            .get(&conn.0)
            .ok_or_else(|| DbError::connection_error("Invalid async connection handle"))?;

        // Check connection status via arth_rt
        let status = arth_rt::postgres::arth_rt_pg_status(async_conn.conn_handle);
        Ok(status == arth_rt::postgres::CONNECTION_OK)
    }

    fn pg_query_async(
        &self,
        conn: PgAsyncConnectionHandle,
        sql: &str,
        params: &[PgValue],
    ) -> Result<PgAsyncQueryHandle, DbError> {
        let (conn_handle, socket_fd) = {
            let conns = self.pg_async_connections.lock().unwrap();
            let async_conn = conns
                .get(&conn.0)
                .ok_or_else(|| DbError::connection_error("Invalid async connection handle"))?;
            (async_conn.conn_handle, async_conn.socket_fd)
        };

        // Send the query asynchronously
        let result = if params.is_empty() {
            arth_rt::postgres::arth_rt_pg_send_query(conn_handle, sql.as_ptr(), sql.len())
        } else {
            let (param_values, param_lengths, param_formats, _cstrings) =
                Self::pg_values_to_c_params(params);
            arth_rt::postgres::arth_rt_pg_send_query_params(
                conn_handle,
                sql.as_ptr(),
                sql.len(),
                params.len() as i32,
                param_values.as_ptr(),
                param_lengths.as_ptr(),
                param_formats.as_ptr(),
            )
        };

        if result != 1 {
            return Err(DbError::query_error("Failed to send async query"));
        }

        // Flush output
        let flush_result = arth_rt::postgres::arth_rt_pg_flush(conn_handle);
        if flush_result < 0 {
            return Err(DbError::query_error("Failed to flush query to server"));
        }

        let query_handle = self.alloc_pg_async_query_handle();

        // Store the pending query state
        self.pg_async_queries.lock().unwrap().insert(
            query_handle.0,
            PgAsyncQueryState::Pending {
                async_conn_handle: conn.0,
                query_type: PgAsyncQueryType::Query,
                socket_fd,
            },
        );

        Ok(query_handle)
    }

    fn pg_execute_async(
        &self,
        conn: PgAsyncConnectionHandle,
        sql: &str,
        params: &[PgValue],
    ) -> Result<PgAsyncQueryHandle, DbError> {
        let (conn_handle, socket_fd) = {
            let conns = self.pg_async_connections.lock().unwrap();
            let async_conn = conns
                .get(&conn.0)
                .ok_or_else(|| DbError::connection_error("Invalid async connection handle"))?;
            (async_conn.conn_handle, async_conn.socket_fd)
        };

        // Send the execute query asynchronously (same as query for DML)
        let result = if params.is_empty() {
            arth_rt::postgres::arth_rt_pg_send_query(conn_handle, sql.as_ptr(), sql.len())
        } else {
            let (param_values, param_lengths, param_formats, _cstrings) =
                Self::pg_values_to_c_params(params);
            arth_rt::postgres::arth_rt_pg_send_query_params(
                conn_handle,
                sql.as_ptr(),
                sql.len(),
                params.len() as i32,
                param_values.as_ptr(),
                param_lengths.as_ptr(),
                param_formats.as_ptr(),
            )
        };

        if result != 1 {
            return Err(DbError::query_error("Failed to send async execute"));
        }

        // Flush output
        let flush_result = arth_rt::postgres::arth_rt_pg_flush(conn_handle);
        if flush_result < 0 {
            return Err(DbError::query_error("Failed to flush execute to server"));
        }

        let query_handle = self.alloc_pg_async_query_handle();

        // Store the pending query state
        self.pg_async_queries.lock().unwrap().insert(
            query_handle.0,
            PgAsyncQueryState::Pending {
                async_conn_handle: conn.0,
                query_type: PgAsyncQueryType::Execute,
                socket_fd,
            },
        );

        Ok(query_handle)
    }

    fn pg_prepare_async(
        &self,
        conn: PgAsyncConnectionHandle,
        name: &str,
        sql: &str,
    ) -> Result<PgAsyncQueryHandle, DbError> {
        let (conn_handle, socket_fd) = {
            let conns = self.pg_async_connections.lock().unwrap();
            let async_conn = conns
                .get(&conn.0)
                .ok_or_else(|| DbError::connection_error("Invalid async connection handle"))?;
            (async_conn.conn_handle, async_conn.socket_fd)
        };

        // Send prepare asynchronously
        let result = arth_rt::postgres::arth_rt_pg_send_prepare(
            conn_handle,
            name.as_ptr(),
            name.len(),
            sql.as_ptr(),
            sql.len(),
        );

        if result != 1 {
            return Err(DbError::prepare_error("Failed to send async prepare"));
        }

        // Flush output
        let flush_result = arth_rt::postgres::arth_rt_pg_flush(conn_handle);
        if flush_result < 0 {
            return Err(DbError::prepare_error("Failed to flush prepare to server"));
        }

        // Track the prepared statement
        {
            let mut conns = self.pg_async_connections.lock().unwrap();
            if let Some(conn_entry) = conns.get_mut(&conn.0) {
                conn_entry.prepared_stmts.insert(name.to_string());
            }
        }

        let query_handle = self.alloc_pg_async_query_handle();
        self.pg_async_queries.lock().unwrap().insert(
            query_handle.0,
            PgAsyncQueryState::Pending {
                async_conn_handle: conn.0,
                query_type: PgAsyncQueryType::Prepare {
                    name: name.to_string(),
                },
                socket_fd,
            },
        );

        Ok(query_handle)
    }

    fn pg_execute_prepared_async(
        &self,
        conn: PgAsyncConnectionHandle,
        stmt_name: &str,
        params: &[PgValue],
    ) -> Result<PgAsyncQueryHandle, DbError> {
        let (conn_handle, socket_fd) = {
            let conns = self.pg_async_connections.lock().unwrap();
            let async_conn = conns
                .get(&conn.0)
                .ok_or_else(|| DbError::connection_error("Invalid async connection handle"))?;
            (async_conn.conn_handle, async_conn.socket_fd)
        };

        // Send prepared query execution asynchronously
        let result = if params.is_empty() {
            arth_rt::postgres::arth_rt_pg_send_query_prepared(
                conn_handle,
                stmt_name.as_ptr(),
                stmt_name.len(),
                0,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        } else {
            let (param_values, param_lengths, param_formats, _cstrings) =
                Self::pg_values_to_c_params(params);
            arth_rt::postgres::arth_rt_pg_send_query_prepared(
                conn_handle,
                stmt_name.as_ptr(),
                stmt_name.len(),
                params.len() as i32,
                param_values.as_ptr(),
                param_lengths.as_ptr(),
                param_formats.as_ptr(),
            )
        };

        if result != 1 {
            return Err(DbError::query_error(
                "Failed to send async execute prepared",
            ));
        }

        // Flush output
        let flush_result = arth_rt::postgres::arth_rt_pg_flush(conn_handle);
        if flush_result < 0 {
            return Err(DbError::query_error(
                "Failed to flush execute prepared to server",
            ));
        }

        let query_handle = self.alloc_pg_async_query_handle();
        self.pg_async_queries.lock().unwrap().insert(
            query_handle.0,
            PgAsyncQueryState::Pending {
                async_conn_handle: conn.0,
                query_type: PgAsyncQueryType::ExecutePrepared,
                socket_fd,
            },
        );

        Ok(query_handle)
    }

    fn pg_is_ready(&self, query: PgAsyncQueryHandle) -> Result<bool, DbError> {
        let queries = self.pg_async_queries.lock().unwrap();
        match queries.get(&query.0) {
            Some(PgAsyncQueryState::Pending {
                async_conn_handle,
                query_type,
                socket_fd: _,
            }) => {
                // Get the underlying connection handle
                let conn_handle = {
                    let conns = self.pg_async_connections.lock().unwrap();
                    match conns.get(async_conn_handle) {
                        Some(c) => c.conn_handle,
                        None => return Err(DbError::connection_error("Connection closed")),
                    }
                };

                // Consume any available input
                let consume_result = arth_rt::postgres::arth_rt_pg_consume_input(conn_handle);
                if consume_result == 0 {
                    return Err(DbError::connection_error(
                        "Failed to consume input from server",
                    ));
                }

                // Check if still busy
                let is_busy = arth_rt::postgres::arth_rt_pg_is_busy(conn_handle);
                if is_busy == 1 {
                    return Ok(false); // Still waiting for data
                }

                // Query is ready - get the result and update state
                let result_handle = arth_rt::postgres::arth_rt_pg_get_result(conn_handle);

                if result_handle == 0 {
                    // No more results - query complete
                    let query_type_clone = query_type.clone();
                    drop(queries);
                    Self::complete_async_query_helper(
                        &self.pg_async_queries,
                        query.0,
                        query_type_clone,
                        0,
                    )?;
                    return Ok(true);
                } else if result_handle < 0 {
                    // Error
                    let err_msg = Self::get_pg_error_string_helper(conn_handle);
                    drop(queries);
                    Self::fail_async_query_helper(&self.pg_async_queries, query.0, err_msg);
                    return Ok(true);
                }

                // We have a valid result
                let query_type_clone = query_type.clone();
                drop(queries);
                Self::complete_async_query_helper(
                    &self.pg_async_queries,
                    query.0,
                    query_type_clone,
                    result_handle,
                )?;

                // Consume remaining results (there might be more)
                loop {
                    let next_result = arth_rt::postgres::arth_rt_pg_get_result(conn_handle);
                    if next_result == 0 {
                        break;
                    }
                    if next_result > 0 {
                        arth_rt::postgres::arth_rt_pg_clear(next_result);
                    }
                }

                Ok(true)
            }
            Some(_) => Ok(true), // Already completed
            None => Err(DbError::query_error("Invalid async query handle")),
        }
    }

    fn pg_get_async_result(&self, query: PgAsyncQueryHandle) -> Result<PgResultHandle, DbError> {
        // First ensure query is ready
        if !self.pg_is_ready(query)? {
            return Err(DbError::query_error("Async query not yet complete"));
        }

        let mut queries = self.pg_async_queries.lock().unwrap();
        match queries.remove(&query.0) {
            Some(PgAsyncQueryState::Completed {
                result_handle,
                column_names,
                column_oids,
                row_count,
            }) => {
                // Store the result in pg_results for later access
                let vm_result_handle = self.alloc_pg_result_handle();

                self.pg_results.lock().unwrap().insert(
                    vm_result_handle.0,
                    PgStoredResult {
                        column_names,
                        column_type_oids: column_oids,
                        affected_rows: row_count as u64,
                    },
                );

                // Clear the arth_rt result handle
                if result_handle > 0 {
                    arth_rt::postgres::arth_rt_pg_clear(result_handle);
                }

                Ok(vm_result_handle)
            }
            Some(PgAsyncQueryState::CompletedRows(affected)) => {
                let result_handle = self.alloc_pg_result_handle();
                self.pg_results.lock().unwrap().insert(
                    result_handle.0,
                    PgStoredResult {
                        column_names: Vec::new(),
                        column_type_oids: Vec::new(),
                        affected_rows: affected,
                    },
                );
                Ok(result_handle)
            }
            Some(PgAsyncQueryState::PrepareCompleted(_)) => {
                let result_handle = self.alloc_pg_result_handle();
                self.pg_results.lock().unwrap().insert(
                    result_handle.0,
                    PgStoredResult {
                        column_names: Vec::new(),
                        column_type_oids: Vec::new(),
                        affected_rows: 0,
                    },
                );
                Ok(result_handle)
            }
            Some(PgAsyncQueryState::TransactionCompleted) => {
                let result_handle = self.alloc_pg_result_handle();
                self.pg_results.lock().unwrap().insert(
                    result_handle.0,
                    PgStoredResult {
                        column_names: Vec::new(),
                        column_type_oids: Vec::new(),
                        affected_rows: 0,
                    },
                );
                Ok(result_handle)
            }
            Some(PgAsyncQueryState::Failed(msg)) => Err(DbError::query_error(msg)),
            Some(PgAsyncQueryState::Pending { .. }) => {
                Err(DbError::query_error("Async query not yet complete"))
            }
            None => Err(DbError::query_error("Invalid async query handle")),
        }
    }

    fn pg_cancel_async(&self, query: PgAsyncQueryHandle) -> Result<(), DbError> {
        let mut queries = self.pg_async_queries.lock().unwrap();
        if queries.remove(&query.0).is_some() {
            Ok(())
        } else {
            Err(DbError::query_error("Invalid async query handle"))
        }
    }

    fn pg_begin_async(&self, conn: PgAsyncConnectionHandle) -> Result<PgAsyncQueryHandle, DbError> {
        let (conn_handle, socket_fd) = {
            let conns = self.pg_async_connections.lock().unwrap();
            let async_conn = conns
                .get(&conn.0)
                .ok_or_else(|| DbError::connection_error("Invalid async connection handle"))?;
            (async_conn.conn_handle, async_conn.socket_fd)
        };

        // Send BEGIN asynchronously
        let sql = b"BEGIN";
        let result = arth_rt::postgres::arth_rt_pg_send_query(conn_handle, sql.as_ptr(), sql.len());
        if result != 1 {
            return Err(DbError::transaction_error("Failed to send BEGIN"));
        }

        let flush_result = arth_rt::postgres::arth_rt_pg_flush(conn_handle);
        if flush_result < 0 {
            return Err(DbError::transaction_error("Failed to flush BEGIN"));
        }

        let query_handle = self.alloc_pg_async_query_handle();
        self.pg_async_queries.lock().unwrap().insert(
            query_handle.0,
            PgAsyncQueryState::Pending {
                async_conn_handle: conn.0,
                query_type: PgAsyncQueryType::Begin,
                socket_fd,
            },
        );

        Ok(query_handle)
    }

    fn pg_commit_async(
        &self,
        conn: PgAsyncConnectionHandle,
    ) -> Result<PgAsyncQueryHandle, DbError> {
        let (conn_handle, socket_fd) = {
            let conns = self.pg_async_connections.lock().unwrap();
            let async_conn = conns
                .get(&conn.0)
                .ok_or_else(|| DbError::connection_error("Invalid async connection handle"))?;
            (async_conn.conn_handle, async_conn.socket_fd)
        };

        // Send COMMIT asynchronously
        let sql = b"COMMIT";
        let result = arth_rt::postgres::arth_rt_pg_send_query(conn_handle, sql.as_ptr(), sql.len());
        if result != 1 {
            return Err(DbError::transaction_error("Failed to send COMMIT"));
        }

        let flush_result = arth_rt::postgres::arth_rt_pg_flush(conn_handle);
        if flush_result < 0 {
            return Err(DbError::transaction_error("Failed to flush COMMIT"));
        }

        let query_handle = self.alloc_pg_async_query_handle();
        self.pg_async_queries.lock().unwrap().insert(
            query_handle.0,
            PgAsyncQueryState::Pending {
                async_conn_handle: conn.0,
                query_type: PgAsyncQueryType::Commit,
                socket_fd,
            },
        );

        Ok(query_handle)
    }

    fn pg_rollback_async(
        &self,
        conn: PgAsyncConnectionHandle,
    ) -> Result<PgAsyncQueryHandle, DbError> {
        let (conn_handle, socket_fd) = {
            let conns = self.pg_async_connections.lock().unwrap();
            let async_conn = conns
                .get(&conn.0)
                .ok_or_else(|| DbError::connection_error("Invalid async connection handle"))?;
            (async_conn.conn_handle, async_conn.socket_fd)
        };

        // Send ROLLBACK asynchronously
        let sql = b"ROLLBACK";
        let result = arth_rt::postgres::arth_rt_pg_send_query(conn_handle, sql.as_ptr(), sql.len());
        if result != 1 {
            return Err(DbError::transaction_error("Failed to send ROLLBACK"));
        }

        let flush_result = arth_rt::postgres::arth_rt_pg_flush(conn_handle);
        if flush_result < 0 {
            return Err(DbError::transaction_error("Failed to flush ROLLBACK"));
        }

        let query_handle = self.alloc_pg_async_query_handle();
        self.pg_async_queries.lock().unwrap().insert(
            query_handle.0,
            PgAsyncQueryState::Pending {
                async_conn_handle: conn.0,
                query_type: PgAsyncQueryType::Rollback,
                socket_fd,
            },
        );

        Ok(query_handle)
    }

    // ========================================================================
    // SQLite Connection Pool Operations
    // ========================================================================

    fn sqlite_pool_create(
        &self,
        connection_string: &str,
        config: &PoolConfig,
    ) -> Result<SqlitePoolHandle, DbError> {
        let handle = self
            .next_sqlite_pool_handle
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Create initial minimum connections
        let mut available = Vec::with_capacity(config.min_connections as usize);
        for _ in 0..config.min_connections {
            let conn_handle = self.sqlite_open(connection_string)?;
            available.push(PooledSqliteConnection {
                handle: conn_handle.0,
                created_at: std::time::Instant::now(),
                last_used: std::time::Instant::now(),
            });
        }

        let pool = SqlitePool {
            connection_string: connection_string.to_string(),
            config: config.clone(),
            available,
            in_use: std::collections::HashSet::new(),
            condvar: std::sync::Condvar::new(),
            lock: std::sync::Mutex::new(()),
        };

        self.sqlite_pools.lock().unwrap().insert(handle, pool);
        Ok(SqlitePoolHandle(handle))
    }

    fn sqlite_pool_close(&self, pool: SqlitePoolHandle) -> Result<(), DbError> {
        let mut pools = self.sqlite_pools.lock().unwrap();
        let pool_data = pools
            .remove(&pool.0)
            .ok_or_else(|| DbError::connection_error("Invalid pool handle"))?;

        // Close all available connections
        for conn in pool_data.available {
            let _ = self.sqlite_close(SqliteConnectionHandle(conn.handle));
        }

        // Close all in-use connections (they shouldn't be used after pool close)
        for handle in pool_data.in_use {
            let _ = self.sqlite_close(SqliteConnectionHandle(handle));
        }

        Ok(())
    }

    fn sqlite_pool_acquire(
        &self,
        pool: SqlitePoolHandle,
    ) -> Result<SqliteConnectionHandle, DbError> {
        let start = std::time::Instant::now();

        loop {
            {
                let mut pools = self.sqlite_pools.lock().unwrap();
                let pool_data = pools
                    .get_mut(&pool.0)
                    .ok_or_else(|| DbError::connection_error("Invalid pool handle"))?;

                // Try to get an available connection
                if let Some(mut pooled_conn) = pool_data.available.pop() {
                    // Check if connection is still healthy
                    let conn_handle = SqliteConnectionHandle(pooled_conn.handle);
                    let healthy = if pool_data.config.test_on_acquire {
                        // Simple health check: try to execute a simple query
                        self.sqlite_execute(conn_handle, "SELECT 1").is_ok()
                    } else {
                        true
                    };

                    if healthy {
                        // Check max lifetime
                        let lifetime_ok = pool_data.config.max_lifetime_ms == 0
                            || pooled_conn.created_at.elapsed().as_millis()
                                < pool_data.config.max_lifetime_ms as u128;

                        if lifetime_ok {
                            pooled_conn.last_used = std::time::Instant::now();
                            pool_data.in_use.insert(pooled_conn.handle);
                            return Ok(conn_handle);
                        }
                    }

                    // Connection unhealthy or expired, close it and try to create new
                    let _ = self.sqlite_close(conn_handle);
                }

                // Check if we can create a new connection
                let total = pool_data.available.len() + pool_data.in_use.len();
                if total < pool_data.config.max_connections as usize {
                    drop(pools); // Release lock before creating connection

                    // Get connection string from pool
                    let pools = self.sqlite_pools.lock().unwrap();
                    let pool_data = pools.get(&pool.0).unwrap();
                    let conn_string = pool_data.connection_string.clone();
                    drop(pools);

                    let conn_handle = self.sqlite_open(&conn_string)?;

                    let mut pools = self.sqlite_pools.lock().unwrap();
                    let pool_data = pools.get_mut(&pool.0).unwrap();
                    pool_data.in_use.insert(conn_handle.0);
                    return Ok(conn_handle);
                }
            }

            // Pool exhausted, check timeout
            let elapsed = start.elapsed().as_millis() as u64;
            let timeout = {
                let pools = self.sqlite_pools.lock().unwrap();
                pools
                    .get(&pool.0)
                    .map(|p| p.config.acquire_timeout_ms)
                    .unwrap_or(0)
            };

            if timeout > 0 && elapsed >= timeout {
                return Err(DbError::pool_exhausted("SQLite pool acquire timeout"));
            }

            // Wait a bit before retrying
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn sqlite_pool_release(
        &self,
        pool: SqlitePoolHandle,
        conn: SqliteConnectionHandle,
    ) -> Result<(), DbError> {
        let mut pools = self.sqlite_pools.lock().unwrap();
        let pool_data = pools
            .get_mut(&pool.0)
            .ok_or_else(|| DbError::connection_error("Invalid pool handle"))?;

        // Remove from in-use set
        if !pool_data.in_use.remove(&conn.0) {
            return Err(DbError::connection_error(
                "Connection not from this pool or already released",
            ));
        }

        // Rollback any uncommitted transaction
        let _ = self.sqlite_rollback(conn);

        // Check if connection is still healthy
        let healthy = self.sqlite_execute(conn, "SELECT 1").is_ok();

        // Check idle timeout and max lifetime
        let now = std::time::Instant::now();
        let should_return = healthy && {
            // Only return to pool if not exceeding max connections
            pool_data.available.len() + pool_data.in_use.len()
                < pool_data.config.max_connections as usize
        };

        if should_return {
            pool_data.available.push(PooledSqliteConnection {
                handle: conn.0,
                created_at: now, // We don't track original creation, reset it
                last_used: now,
            });
            pool_data.condvar.notify_one();
        } else {
            drop(pools); // Release lock before closing
            let _ = self.sqlite_close(conn);
        }

        Ok(())
    }

    fn sqlite_pool_stats(&self, pool: SqlitePoolHandle) -> Result<PoolStats, DbError> {
        let pools = self.sqlite_pools.lock().unwrap();
        let pool_data = pools
            .get(&pool.0)
            .ok_or_else(|| DbError::connection_error("Invalid pool handle"))?;

        Ok(PoolStats {
            available: pool_data.available.len() as u32,
            in_use: pool_data.in_use.len() as u32,
            total: (pool_data.available.len() + pool_data.in_use.len()) as u32,
            waiters: 0, // We don't track waiters in this simple implementation
        })
    }

    // ========================================================================
    // PostgreSQL Connection Pool Operations
    // ========================================================================

    fn pg_pool_create(
        &self,
        connection_string: &str,
        config: &PoolConfig,
    ) -> Result<PgPoolHandle, DbError> {
        let handle = self
            .next_pg_pool_handle
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Create initial minimum connections
        let mut available = Vec::with_capacity(config.min_connections as usize);
        for _ in 0..config.min_connections {
            let conn_handle = self.pg_connect(connection_string)?;
            available.push(PooledPgConnection {
                handle: conn_handle.0,
                created_at: std::time::Instant::now(),
                last_used: std::time::Instant::now(),
            });
        }

        let pool = PgPool {
            connection_string: connection_string.to_string(),
            config: config.clone(),
            available,
            in_use: std::collections::HashSet::new(),
            condvar: std::sync::Condvar::new(),
            lock: std::sync::Mutex::new(()),
        };

        self.pg_pools.lock().unwrap().insert(handle, pool);
        Ok(PgPoolHandle(handle))
    }

    fn pg_pool_close(&self, pool: PgPoolHandle) -> Result<(), DbError> {
        let mut pools = self.pg_pools.lock().unwrap();
        let pool_data = pools
            .remove(&pool.0)
            .ok_or_else(|| DbError::connection_error("Invalid pool handle"))?;

        // Close all available connections
        for conn in pool_data.available {
            let _ = self.pg_disconnect(PgConnectionHandle(conn.handle));
        }

        // Close all in-use connections
        for handle in pool_data.in_use {
            let _ = self.pg_disconnect(PgConnectionHandle(handle));
        }

        Ok(())
    }

    fn pg_pool_acquire(&self, pool: PgPoolHandle) -> Result<PgConnectionHandle, DbError> {
        let start = std::time::Instant::now();

        loop {
            {
                let mut pools = self.pg_pools.lock().unwrap();
                let pool_data = pools
                    .get_mut(&pool.0)
                    .ok_or_else(|| DbError::connection_error("Invalid pool handle"))?;

                // Try to get an available connection
                if let Some(mut pooled_conn) = pool_data.available.pop() {
                    let conn_handle = PgConnectionHandle(pooled_conn.handle);

                    // Check if connection is still healthy
                    let healthy = if pool_data.config.test_on_acquire {
                        self.pg_execute(conn_handle, "SELECT 1", &[]).is_ok()
                    } else {
                        true
                    };

                    if healthy {
                        // Check max lifetime
                        let lifetime_ok = pool_data.config.max_lifetime_ms == 0
                            || pooled_conn.created_at.elapsed().as_millis()
                                < pool_data.config.max_lifetime_ms as u128;

                        if lifetime_ok {
                            pooled_conn.last_used = std::time::Instant::now();
                            pool_data.in_use.insert(pooled_conn.handle);
                            return Ok(conn_handle);
                        }
                    }

                    // Connection unhealthy or expired, close it
                    let _ = self.pg_disconnect(conn_handle);
                }

                // Check if we can create a new connection
                let total = pool_data.available.len() + pool_data.in_use.len();
                if total < pool_data.config.max_connections as usize {
                    let conn_string = pool_data.connection_string.clone();
                    drop(pools); // Release lock before creating connection

                    let conn_handle = self.pg_connect(&conn_string)?;

                    let mut pools = self.pg_pools.lock().unwrap();
                    let pool_data = pools.get_mut(&pool.0).unwrap();
                    pool_data.in_use.insert(conn_handle.0);
                    return Ok(conn_handle);
                }
            }

            // Pool exhausted, check timeout
            let elapsed = start.elapsed().as_millis() as u64;
            let timeout = {
                let pools = self.pg_pools.lock().unwrap();
                pools
                    .get(&pool.0)
                    .map(|p| p.config.acquire_timeout_ms)
                    .unwrap_or(0)
            };

            if timeout > 0 && elapsed >= timeout {
                return Err(DbError::pool_exhausted("PostgreSQL pool acquire timeout"));
            }

            // Wait a bit before retrying
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn pg_pool_release(&self, pool: PgPoolHandle, conn: PgConnectionHandle) -> Result<(), DbError> {
        let mut pools = self.pg_pools.lock().unwrap();
        let pool_data = pools
            .get_mut(&pool.0)
            .ok_or_else(|| DbError::connection_error("Invalid pool handle"))?;

        // Remove from in-use set
        if !pool_data.in_use.remove(&conn.0) {
            return Err(DbError::connection_error(
                "Connection not from this pool or already released",
            ));
        }

        // Rollback any uncommitted transaction
        let _ = self.pg_rollback(conn);

        // Check if connection is still healthy
        let healthy = self.pg_execute(conn, "SELECT 1", &[]).is_ok();

        // Check if we should return connection to pool
        let should_return = healthy && {
            pool_data.available.len() + pool_data.in_use.len()
                < pool_data.config.max_connections as usize
        };

        if should_return {
            let now = std::time::Instant::now();
            pool_data.available.push(PooledPgConnection {
                handle: conn.0,
                created_at: now,
                last_used: now,
            });
            pool_data.condvar.notify_one();
        } else {
            drop(pools); // Release lock before closing
            let _ = self.pg_disconnect(conn);
        }

        Ok(())
    }

    fn pg_pool_stats(&self, pool: PgPoolHandle) -> Result<PoolStats, DbError> {
        let pools = self.pg_pools.lock().unwrap();
        let pool_data = pools
            .get(&pool.0)
            .ok_or_else(|| DbError::connection_error("Invalid pool handle"))?;

        Ok(PoolStats {
            available: pool_data.available.len() as u32,
            in_use: pool_data.in_use.len() as u32,
            total: (pool_data.available.len() + pool_data.in_use.len()) as u32,
            waiters: 0,
        })
    }

    // ========================================================================
    // SQLite Transaction Helper Operations
    // ========================================================================

    fn sqlite_tx_scope_begin(&self, conn: SqliteConnectionHandle) -> Result<i64, DbError> {
        let scope_id = self.alloc_tx_scope_id();

        let mut tx_states = self.sqlite_tx_state.lock().unwrap();
        let tx_state = tx_states.entry(conn.0).or_insert_with(TxState::default);

        if tx_state.depth == 0 {
            // No active transaction - begin a new one
            self.sqlite_begin(conn)?;
            tx_state.depth = 1;
            tx_state.scope_ids.push(scope_id);
        } else {
            // Already in a transaction - create a savepoint for nesting
            let savepoint_name = format!("sp_{}", scope_id);
            self.sqlite_savepoint(conn, &savepoint_name)?;
            tx_state.depth += 1;
            tx_state.savepoints.push(savepoint_name);
            tx_state.scope_ids.push(scope_id);
        }

        Ok(scope_id)
    }

    fn sqlite_tx_scope_end(
        &self,
        conn: SqliteConnectionHandle,
        scope_id: i64,
        success: bool,
    ) -> Result<(), DbError> {
        let mut tx_states = self.sqlite_tx_state.lock().unwrap();
        let tx_state = tx_states
            .get_mut(&conn.0)
            .ok_or_else(|| DbError::transaction_error("No active transaction scope"))?;

        if tx_state.depth == 0 {
            return Err(DbError::transaction_error("No active transaction scope"));
        }

        // Verify the scope_id matches the current scope
        let expected_scope_id = tx_state.scope_ids.last().copied().unwrap_or(-1);
        if expected_scope_id != scope_id {
            return Err(DbError::transaction_error(format!(
                "Scope ID mismatch: expected {}, got {}",
                expected_scope_id, scope_id
            )));
        }

        tx_state.scope_ids.pop();

        if tx_state.depth == 1 {
            // Top-level transaction
            tx_state.depth = 0;
            drop(tx_states); // Release lock before commit/rollback

            if success {
                self.sqlite_commit(conn)
            } else {
                self.sqlite_rollback(conn)
            }
        } else {
            // Nested transaction (savepoint)
            let savepoint_name = tx_state.savepoints.pop().ok_or_else(|| {
                DbError::transaction_error("Savepoint stack is empty for nested transaction")
            })?;
            tx_state.depth -= 1;
            drop(tx_states); // Release lock before savepoint operations

            if success {
                self.sqlite_release_savepoint(conn, &savepoint_name)
            } else {
                self.sqlite_rollback_to_savepoint(conn, &savepoint_name)
            }
        }
    }

    fn sqlite_tx_depth(&self, conn: SqliteConnectionHandle) -> Result<u32, DbError> {
        let tx_states = self.sqlite_tx_state.lock().unwrap();
        Ok(tx_states.get(&conn.0).map(|s| s.depth).unwrap_or(0))
    }

    fn sqlite_tx_active(&self, conn: SqliteConnectionHandle) -> Result<bool, DbError> {
        let tx_states = self.sqlite_tx_state.lock().unwrap();
        Ok(tx_states.get(&conn.0).map(|s| s.depth > 0).unwrap_or(false))
    }

    // ========================================================================
    // PostgreSQL Transaction Helper Operations
    // ========================================================================

    fn pg_tx_scope_begin(&self, conn: PgConnectionHandle) -> Result<i64, DbError> {
        let scope_id = self.alloc_tx_scope_id();

        let mut tx_states = self.pg_tx_state.lock().unwrap();
        let tx_state = tx_states.entry(conn.0).or_insert_with(TxState::default);

        if tx_state.depth == 0 {
            // No active transaction - begin a new one
            self.pg_begin(conn)?;
            tx_state.depth = 1;
            tx_state.scope_ids.push(scope_id);
        } else {
            // Already in a transaction - create a savepoint for nesting
            let savepoint_name = format!("sp_{}", scope_id);
            self.pg_savepoint(conn, &savepoint_name)?;
            tx_state.depth += 1;
            tx_state.savepoints.push(savepoint_name);
            tx_state.scope_ids.push(scope_id);
        }

        Ok(scope_id)
    }

    fn pg_tx_scope_end(
        &self,
        conn: PgConnectionHandle,
        scope_id: i64,
        success: bool,
    ) -> Result<(), DbError> {
        let mut tx_states = self.pg_tx_state.lock().unwrap();
        let tx_state = tx_states
            .get_mut(&conn.0)
            .ok_or_else(|| DbError::transaction_error("No active transaction scope"))?;

        if tx_state.depth == 0 {
            return Err(DbError::transaction_error("No active transaction scope"));
        }

        // Verify the scope_id matches the current scope
        let expected_scope_id = tx_state.scope_ids.last().copied().unwrap_or(-1);
        if expected_scope_id != scope_id {
            return Err(DbError::transaction_error(format!(
                "Scope ID mismatch: expected {}, got {}",
                expected_scope_id, scope_id
            )));
        }

        tx_state.scope_ids.pop();

        if tx_state.depth == 1 {
            // Top-level transaction
            tx_state.depth = 0;
            drop(tx_states); // Release lock before commit/rollback

            if success {
                self.pg_commit(conn)
            } else {
                self.pg_rollback(conn)
            }
        } else {
            // Nested transaction (savepoint)
            let savepoint_name = tx_state.savepoints.pop().ok_or_else(|| {
                DbError::transaction_error("Savepoint stack is empty for nested transaction")
            })?;
            tx_state.depth -= 1;
            drop(tx_states); // Release lock before savepoint operations

            if success {
                self.pg_release_savepoint(conn, &savepoint_name)
            } else {
                self.pg_rollback_to_savepoint(conn, &savepoint_name)
            }
        }
    }

    fn pg_tx_depth(&self, conn: PgConnectionHandle) -> Result<u32, DbError> {
        let tx_states = self.pg_tx_state.lock().unwrap();
        Ok(tx_states.get(&conn.0).map(|s| s.depth).unwrap_or(0))
    }

    fn pg_tx_active(&self, conn: PgConnectionHandle) -> Result<bool, DbError> {
        let tx_states = self.pg_tx_state.lock().unwrap();
        Ok(tx_states.get(&conn.0).map(|s| s.depth > 0).unwrap_or(false))
    }
}

impl StdHostDb {
    /// Format a PgValue as a SQL literal for EXECUTE statements.
    fn format_param_value(param: &PgValue) -> String {
        match param {
            PgValue::Null => "NULL".to_string(),
            PgValue::Bool(v) => if *v { "TRUE" } else { "FALSE" }.to_string(),
            PgValue::Int(v) => v.to_string(),
            PgValue::Int64(v) => v.to_string(),
            PgValue::Float(v) => v.to_string(),
            PgValue::Double(v) => v.to_string(),
            PgValue::Text(v) => format!("'{}'", v.replace('\'', "''")),
            PgValue::Bytes(v) => {
                let hex: String = v.iter().map(|b| format!("{:02x}", b)).collect();
                format!("'\\x{}'", hex)
            }
        }
    }

    // =========================================================================
    // Async PostgreSQL Helpers (standalone functions to avoid trait issues)
    // =========================================================================

    /// Get PostgreSQL error message as a String
    fn get_pg_error_string_helper(conn_handle: i64) -> String {
        let mut buf = [0u8; 1024];
        let len =
            arth_rt::postgres::arth_rt_pg_error_message(conn_handle, buf.as_mut_ptr(), buf.len());
        if len > 0 {
            String::from_utf8_lossy(&buf[..len as usize]).to_string()
        } else {
            "Unknown error".to_string()
        }
    }

    /// Complete an async query with the given result
    fn complete_async_query_helper(
        pg_async_queries: &Mutex<HashMap<i64, PgAsyncQueryState>>,
        query_handle: i64,
        query_type: PgAsyncQueryType,
        result_handle: i64,
    ) -> Result<(), DbError> {
        let new_state = match query_type {
            PgAsyncQueryType::Query | PgAsyncQueryType::ExecutePrepared => {
                if result_handle == 0 {
                    // No result rows
                    PgAsyncQueryState::Completed {
                        result_handle: 0,
                        column_names: Vec::new(),
                        column_oids: Vec::new(),
                        row_count: 0,
                    }
                } else {
                    // Get column info
                    let nfields = arth_rt::postgres::arth_rt_pg_nfields(result_handle);
                    let ntuples = arth_rt::postgres::arth_rt_pg_ntuples(result_handle);

                    let mut column_names = Vec::with_capacity(nfields as usize);
                    let mut column_oids = Vec::with_capacity(nfields as usize);

                    for i in 0..nfields {
                        let mut name_buf = [0u8; 256];
                        let name_len = arth_rt::postgres::arth_rt_pg_fname(
                            result_handle,
                            i,
                            name_buf.as_mut_ptr(),
                            name_buf.len(),
                        );
                        if name_len > 0 {
                            column_names.push(
                                String::from_utf8_lossy(&name_buf[..name_len as usize]).to_string(),
                            );
                        } else {
                            column_names.push(format!("column{}", i));
                        }

                        let oid = arth_rt::postgres::arth_rt_pg_ftype(result_handle, i);
                        column_oids.push(oid as u32);
                    }

                    PgAsyncQueryState::Completed {
                        result_handle,
                        column_names,
                        column_oids,
                        row_count: ntuples,
                    }
                }
            }
            PgAsyncQueryType::Execute => {
                let affected = if result_handle > 0 {
                    // Get affected rows from cmd_tuples (returns the count directly)
                    let count = arth_rt::postgres::arth_rt_pg_cmd_tuples(result_handle);
                    if count >= 0 { count as u64 } else { 0 }
                } else {
                    0
                };
                if result_handle > 0 {
                    arth_rt::postgres::arth_rt_pg_clear(result_handle);
                }
                PgAsyncQueryState::CompletedRows(affected)
            }
            PgAsyncQueryType::Prepare { name } => {
                if result_handle > 0 {
                    arth_rt::postgres::arth_rt_pg_clear(result_handle);
                }
                PgAsyncQueryState::PrepareCompleted(name)
            }
            PgAsyncQueryType::Begin | PgAsyncQueryType::Commit | PgAsyncQueryType::Rollback => {
                if result_handle > 0 {
                    arth_rt::postgres::arth_rt_pg_clear(result_handle);
                }
                PgAsyncQueryState::TransactionCompleted
            }
        };

        pg_async_queries
            .lock()
            .unwrap()
            .insert(query_handle, new_state);
        Ok(())
    }

    /// Mark an async query as failed
    fn fail_async_query_helper(
        pg_async_queries: &Mutex<HashMap<i64, PgAsyncQueryState>>,
        query_handle: i64,
        error_msg: String,
    ) {
        pg_async_queries
            .lock()
            .unwrap()
            .insert(query_handle, PgAsyncQueryState::Failed(error_msg));
    }
}

// ============================================================================
// HostContext
// ============================================================================

/// Container for all host capabilities.
///
/// This is passed to the VM runtime to provide pluggable host functions.
/// The `config` field controls which capability domains are enabled.
pub struct HostContext {
    pub io: Arc<dyn HostIo>,
    pub net: Arc<dyn HostNet>,
    pub time: Arc<dyn HostTime>,
    pub db: Arc<dyn HostDb>,
    pub mail: Arc<dyn HostMail>,
    pub generic: Arc<dyn HostGenericCall>,
    pub config: HostConfig,
    /// Local state storage for HostCallGeneric operations.
    /// Uses interior mutability since HostContext is passed as immutable reference.
    pub local_state: std::sync::RwLock<HashMap<String, Vec<u8>>>,
}

impl HostContext {
    /// Create a new host context with the given implementations and full capabilities.
    pub fn new(
        io: Arc<dyn HostIo>,
        net: Arc<dyn HostNet>,
        time: Arc<dyn HostTime>,
        db: Arc<dyn HostDb>,
    ) -> Self {
        Self {
            io,
            net,
            time,
            db,
            mail: Arc::new(StdHostMail::new()),
            generic: Arc::new(StdHostGenericCall::new()),
            config: HostConfig::full(),
            local_state: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Create a new host context with custom config.
    pub fn with_config(
        io: Arc<dyn HostIo>,
        net: Arc<dyn HostNet>,
        time: Arc<dyn HostTime>,
        db: Arc<dyn HostDb>,
        config: HostConfig,
    ) -> Self {
        Self {
            io,
            net,
            time,
            db,
            mail: Arc::new(StdHostMail::new()),
            generic: Arc::new(StdHostGenericCall::new()),
            config,
            local_state: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Create a host context with standard (real) implementations and full capabilities.
    pub fn std() -> Self {
        Self {
            io: Arc::new(StdHostIo::new()),
            net: Arc::new(StdHostNet::new()),
            time: Arc::new(StdHostTime::new()),
            db: Arc::new(StdHostDb::new()),
            mail: Arc::new(StdHostMail::new()),
            generic: Arc::new(StdHostGenericCall::new()),
            config: HostConfig::full(),
            local_state: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Create a host context that denies all host operations (sandboxed).
    ///
    /// Uses `NoHostIo`, `NoHostNet`, `NoHostDb`, and `NoHostMail` implementations that return
    /// capability-denied errors, and sets the config to sandboxed (all capabilities disabled).
    pub fn sandboxed() -> Self {
        Self {
            io: Arc::new(NoHostIo::new()),
            net: Arc::new(NoHostNet::new()),
            time: Arc::new(StdHostTime::new()), // Time is usually safe to expose
            db: Arc::new(NoHostDb::new()),
            mail: Arc::new(NoHostMail::new()),
            generic: Arc::new(StdHostGenericCall::new()),
            config: HostConfig::sandboxed(),
            local_state: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Create a host context for TS guest execution with specific capabilities.
    ///
    /// Uses standard implementations but restricts access based on the provided
    /// capability list (e.g., `["io", "time", "db", "mail", "net"]`).
    pub fn for_guest(capabilities: &[String]) -> Self {
        let config = HostConfig::from_capabilities(capabilities);
        Self {
            io: Arc::new(StdHostIo::new()),
            net: Arc::new(StdHostNet::new()),
            time: Arc::new(StdHostTime::new()),
            db: Arc::new(StdHostDb::new()),
            mail: Arc::new(StdHostMail::new()),
            generic: Arc::new(StdHostGenericCall::new()),
            config,
            local_state: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Create a host context for testing with mock time.
    pub fn testing(start_time_millis: i64) -> Self {
        Self {
            io: Arc::new(StdHostIo::new()),
            net: Arc::new(StdHostNet::new()),
            time: Arc::new(MockHostTime::new(start_time_millis)),
            db: Arc::new(StdHostDb::new()),
            mail: Arc::new(StdHostMail::new()),
            generic: Arc::new(StdHostGenericCall::new()),
            config: HostConfig::full(),
            local_state: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Set a custom generic call handler.
    ///
    /// This allows embedding hosts to provide their own implementation
    /// of generic host calls (e.g., WAID-specific set_data, get_current_event).
    pub fn with_generic(mut self, generic: Arc<dyn HostGenericCall>) -> Self {
        self.generic = generic;
        self
    }
}

impl Default for HostContext {
    fn default() -> Self {
        Self::std()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_std_host_io_file_operations() {
        let io = StdHostIo::new();
        let test_path = "/tmp/arth_host_test.txt";

        // Write
        let handle = io.file_open(test_path, FileMode::Write).unwrap();
        io.file_write(handle, b"hello world").unwrap();
        io.file_close(handle).unwrap();

        // Read back
        let handle = io.file_open(test_path, FileMode::Read).unwrap();
        let data = io.file_read(handle, 1024).unwrap();
        assert_eq!(data, b"hello world");
        io.file_close(handle).unwrap();

        // Cleanup
        io.file_delete(test_path).unwrap();
    }

    #[test]
    fn test_no_host_io_denies_operations() {
        let io = NoHostIo::new();
        let result = io.file_open("/tmp/test", FileMode::Read);
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, IoErrorKind::CapabilityDenied);
        }
    }

    #[test]
    fn test_std_host_time() {
        let time = StdHostTime::new();
        let now = time.now_realtime();
        assert!(now > 0); // Should be after Unix epoch

        let instant = time.instant_now();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let elapsed = time.instant_elapsed(instant).unwrap();
        assert!(elapsed >= 10);
    }

    #[test]
    fn test_mock_host_time() {
        let time = MockHostTime::new(1000);
        assert_eq!(time.now_realtime(), 1000);

        let instant = time.instant_now();
        time.advance(500);
        assert_eq!(time.instant_elapsed(instant).unwrap(), 500);

        time.set_time(5000);
        assert_eq!(time.now_realtime(), 5000);
    }

    #[test]
    fn test_mock_time_sleep_advances_clock() {
        let time = MockHostTime::new(0);
        time.sleep(100);
        assert_eq!(time.now_realtime(), 100);
    }

    #[test]
    fn test_host_context_std() {
        let ctx = HostContext::std();
        let now = ctx.time.now_realtime();
        assert!(now > 0);
    }

    #[test]
    fn test_host_context_sandboxed() {
        let ctx = HostContext::sandboxed();
        let result = ctx.io.file_open("/tmp/test", FileMode::Read);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_iso8601() {
        let time = StdHostTime::new();

        // Test basic ISO 8601 parsing
        let millis = time.parse("ISO8601", "1970-01-01T00:00:00").unwrap();
        assert_eq!(millis, 0);

        let millis = time.parse("ISO8601", "1970-01-01T00:00:01").unwrap();
        assert_eq!(millis, 1000);

        let millis = time.parse("ISO8601", "1970-01-01T01:00:00").unwrap();
        assert_eq!(millis, 3600 * 1000);
    }

    #[test]
    fn test_mock_time_format() {
        let time = MockHostTime::new(0);

        let formatted = time.format(0, "ISO8601").unwrap();
        assert_eq!(formatted, "1970-01-01T00:00:00");

        let formatted = time.format(86400 * 1000, "ISO8601").unwrap();
        assert_eq!(formatted, "1970-01-02T00:00:00");
    }

    // ========================================================================
    // HostConfig tests
    // ========================================================================

    #[test]
    fn test_host_config_full() {
        let config = HostConfig::full();
        assert!(config.allow_io);
        assert!(config.allow_net);
        assert!(config.allow_time);
        assert!(config.allow_db);
        assert!(config.allow_mail);
        assert!(config.allow_crypto);
    }

    #[test]
    fn test_host_config_sandboxed() {
        let config = HostConfig::sandboxed();
        assert!(!config.allow_io);
        assert!(!config.allow_net);
        assert!(!config.allow_time);
        assert!(!config.allow_db);
        assert!(!config.allow_mail);
        assert!(!config.allow_crypto);
    }

    #[test]
    fn test_host_config_from_capabilities() {
        // Empty list = fully sandboxed
        let config = HostConfig::from_capabilities(&[]);
        assert!(!config.allow_io);
        assert!(!config.allow_net);
        assert!(!config.allow_time);
        assert!(!config.allow_db);
        assert!(!config.allow_mail);
        assert!(!config.allow_crypto);

        // Single capability
        let config = HostConfig::from_capabilities(&["io".to_string()]);
        assert!(config.allow_io);
        assert!(!config.allow_net);
        assert!(!config.allow_time);
        assert!(!config.allow_crypto);

        // Crypto capability
        let config = HostConfig::from_capabilities(&["crypto".to_string()]);
        assert!(!config.allow_io);
        assert!(!config.allow_net);
        assert!(config.allow_crypto);

        // Multiple capabilities
        let config = HostConfig::from_capabilities(&["io".to_string(), "time".to_string()]);
        assert!(config.allow_io);
        assert!(!config.allow_net);
        assert!(config.allow_time);

        // All capabilities
        let config = HostConfig::from_capabilities(&[
            "io".to_string(),
            "net".to_string(),
            "time".to_string(),
            "db".to_string(),
            "mail".to_string(),
            "crypto".to_string(),
        ]);
        assert!(config.allow_io);
        assert!(config.allow_net);
        assert!(config.allow_time);
        assert!(config.allow_db);
        assert!(config.allow_mail);
        assert!(config.allow_crypto);

        // Unknown capabilities are ignored
        let config = HostConfig::from_capabilities(&["unknown".to_string(), "io".to_string()]);
        assert!(config.allow_io);
        assert!(!config.allow_net);
        assert!(!config.allow_time);
        assert!(!config.allow_crypto);
    }

    #[test]
    fn test_host_config_from_capability_strs() {
        let config = HostConfig::from_capability_strs(&["net", "time"]);
        assert!(!config.allow_io);
        assert!(config.allow_net);
        assert!(config.allow_time);
        assert!(!config.allow_crypto);

        // Test with crypto capability
        let config = HostConfig::from_capability_strs(&["crypto", "db"]);
        assert!(!config.allow_io);
        assert!(!config.allow_net);
        assert!(config.allow_db);
        assert!(config.allow_crypto);
    }

    #[test]
    fn test_host_config_default() {
        // Default is full capabilities
        let config = HostConfig::default();
        assert_eq!(config, HostConfig::full());
    }

    #[test]
    fn test_host_context_for_guest() {
        // Guest context with limited capabilities
        let ctx = HostContext::for_guest(&["time".to_string()]);
        assert!(!ctx.config.allow_io);
        assert!(!ctx.config.allow_net);
        assert!(ctx.config.allow_time);

        // Time operations should work
        let now = ctx.time.now_realtime();
        assert!(now > 0);
    }

    // ========================================================================
    // Capability denial integration tests
    // ========================================================================

    #[test]
    fn test_sandboxed_context_denies_all_io() {
        let ctx = HostContext::sandboxed();
        assert!(ctx.io.file_open("/tmp/test", FileMode::Read).is_err());
        assert!(ctx.io.file_open("/tmp/test", FileMode::Write).is_err());
        assert!(ctx.io.dir_create("/tmp/sandbox_test_dir").is_err());
        assert!(ctx.io.dir_list("/tmp").is_err());
        assert!(ctx.io.file_exists("/tmp/test").is_err());
        assert!(ctx.io.file_delete("/tmp/test").is_err());
        assert!(ctx.io.console_read_line().is_err());
    }

    #[test]
    fn test_sandboxed_context_denies_all_net() {
        let ctx = HostContext::sandboxed();
        assert!(
            ctx.net
                .http_fetch("http://example.com", "GET", &HashMap::new(), &[])
                .is_err()
        );
        assert!(ctx.net.http_serve(8080).is_err());
        assert!(ctx.net.ws_serve(9090, "/ws").is_err());
        assert!(ctx.net.sse_serve(9091, "/events").is_err());
    }

    #[test]
    fn test_sandboxed_context_denies_all_db() {
        let ctx = HostContext::sandboxed();
        assert!(ctx.db.sqlite_open(":memory:").is_err());
    }

    #[test]
    fn test_sandboxed_context_denies_all_mail() {
        let ctx = HostContext::sandboxed();
        assert!(ctx.mail.smtp_connect("localhost", 25, false, 5000).is_err());
    }

    #[test]
    fn test_no_host_io_returns_capability_denied_kind() {
        let io = NoHostIo::new();
        let err = io.file_open("/tmp/x", FileMode::Read).unwrap_err();
        assert_eq!(err.kind, IoErrorKind::CapabilityDenied);
        let err = io.dir_create("/tmp/x").unwrap_err();
        assert_eq!(err.kind, IoErrorKind::CapabilityDenied);
    }

    #[test]
    fn test_no_host_net_returns_capability_denied_kind() {
        let net = NoHostNet::new();
        let err = net
            .http_fetch("http://x", "GET", &HashMap::new(), &[])
            .unwrap_err();
        assert_eq!(err.kind, NetErrorKind::CapabilityDenied);
        let err = net.http_serve(80).unwrap_err();
        assert_eq!(err.kind, NetErrorKind::CapabilityDenied);
        let err = net.ws_serve(80, "/ws").unwrap_err();
        assert_eq!(err.kind, NetErrorKind::CapabilityDenied);
    }

    #[test]
    fn test_no_host_db_returns_capability_denied_kind() {
        let db = NoHostDb::new();
        let err = db.sqlite_open(":memory:").unwrap_err();
        assert_eq!(err.kind, DbErrorKind::CapabilityDenied);
    }

    #[test]
    fn test_no_host_mail_returns_capability_denied_kind() {
        let mail = NoHostMail::new();
        let err = mail.smtp_connect("x", 25, false, 5000).unwrap_err();
        assert_eq!(err.kind, MailErrorKind::CapabilityDenied);
    }

    #[test]
    fn test_for_guest_selective_io_only() {
        let ctx = HostContext::for_guest(&["io".to_string()]);
        assert!(ctx.config.allow_io);
        assert!(!ctx.config.allow_net);
        assert!(!ctx.config.allow_db);
        assert!(!ctx.config.allow_mail);
        // IO operations should work (real impl)
        let handle = ctx.io.file_open("/tmp/arth_guest_io_test", FileMode::Write);
        if let Ok(h) = handle {
            let _ = ctx.io.file_close(h);
            let _ = ctx.io.file_delete("/tmp/arth_guest_io_test");
        }
    }

    #[test]
    fn test_host_config_wisp_capabilities() {
        let config = HostConfig::from_capability_strs(&["wisp.calendar", "wisp.doc"]);
        assert!(!config.allow_io);
        assert!(config.allow_wisp_calendar);
        assert!(!config.allow_wisp_sheet);
        assert!(config.allow_wisp_doc);
        assert!(!config.allow_wisp_pres);
    }

    // ========================================================================
    // StdHostIo directory operation tests
    // ========================================================================

    #[test]
    fn test_std_host_io_dir_create_and_delete() {
        let io = StdHostIo::new();
        let test_dir = "/tmp/arth_host_test_dir";

        // Create directory
        let _ = io.dir_delete(test_dir); // Clean up if exists
        io.dir_create(test_dir).unwrap();
        assert!(io.dir_exists(test_dir).unwrap());
        assert!(io.is_dir(test_dir).unwrap());
        assert!(!io.is_file(test_dir).unwrap());

        // Delete directory
        io.dir_delete(test_dir).unwrap();
        assert!(!io.dir_exists(test_dir).unwrap());
    }

    #[test]
    fn test_std_host_io_dir_create_all() {
        let io = StdHostIo::new();
        let nested_dir = "/tmp/arth_host_test_nested/a/b/c";
        let parent_dir = "/tmp/arth_host_test_nested";

        // Clean up if exists
        let _ = std::fs::remove_dir_all(parent_dir);

        // Create nested directories
        io.dir_create_all(nested_dir).unwrap();
        assert!(io.dir_exists(nested_dir).unwrap());

        // Cleanup
        std::fs::remove_dir_all(parent_dir).unwrap();
    }

    #[test]
    fn test_std_host_io_dir_list() {
        let io = StdHostIo::new();
        let test_dir = "/tmp/arth_host_test_list_dir";

        // Setup: create directory with files
        let _ = std::fs::remove_dir_all(test_dir);
        std::fs::create_dir(test_dir).unwrap();
        std::fs::write(format!("{}/file1.txt", test_dir), "a").unwrap();
        std::fs::write(format!("{}/file2.txt", test_dir), "b").unwrap();

        // List directory
        let entries = io.dir_list(test_dir).unwrap();
        assert!(entries.contains(&"file1.txt".to_string()));
        assert!(entries.contains(&"file2.txt".to_string()));
        assert_eq!(entries.len(), 2);

        // Cleanup
        std::fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn test_std_host_io_file_copy_and_move() {
        let io = StdHostIo::new();
        let src = "/tmp/arth_host_test_src.txt";
        let dst = "/tmp/arth_host_test_dst.txt";
        let moved = "/tmp/arth_host_test_moved.txt";

        // Setup
        std::fs::write(src, "copy me").unwrap();

        // Test copy
        io.file_copy(src, dst).unwrap();
        assert!(io.file_exists(src).unwrap());
        assert!(io.file_exists(dst).unwrap());
        assert_eq!(std::fs::read_to_string(dst).unwrap(), "copy me");

        // Test move
        io.file_move(dst, moved).unwrap();
        assert!(!io.file_exists(dst).unwrap());
        assert!(io.file_exists(moved).unwrap());
        assert_eq!(std::fs::read_to_string(moved).unwrap(), "copy me");

        // Cleanup
        let _ = io.file_delete(src);
        let _ = io.file_delete(moved);
    }

    #[test]
    fn test_std_host_io_file_seek_and_size() {
        let io = StdHostIo::new();
        let path = "/tmp/arth_host_test_seek.txt";

        // Setup: create file with content
        std::fs::write(path, "0123456789").unwrap();

        // Open and check size
        let handle = io.file_open(path, FileMode::Read).unwrap();
        assert_eq!(io.file_size(handle).unwrap(), 10);

        // Seek to position 5 and read
        let new_pos = io.file_seek(handle, SeekPosition::Start(5)).unwrap();
        assert_eq!(new_pos, 5);
        let data = io.file_read(handle, 5).unwrap();
        assert_eq!(data, b"56789");

        // Seek back from current position
        io.file_seek(handle, SeekPosition::Current(-3)).unwrap();
        let data = io.file_read(handle, 2).unwrap();
        assert_eq!(data, b"78");

        io.file_close(handle).unwrap();

        // Cleanup
        io.file_delete(path).unwrap();
    }

    #[test]
    fn test_std_host_io_path_absolute() {
        let io = StdHostIo::new();

        // Test with existing path (current directory)
        let abs = io.path_absolute(".").unwrap();
        assert!(abs.starts_with('/') || abs.contains(':')); // Unix or Windows

        // Test with non-existent path should error
        let result = io.path_absolute("/nonexistent/path/abc123");
        assert!(result.is_err());
    }

    // ========================================================================
    // StdHostIo console operation tests
    // Note: Console operations are difficult to test in unit tests since they
    // interact with stdin/stdout. We test what we can.
    // ========================================================================

    #[test]
    fn test_std_host_io_console_write() {
        // Console write should not panic
        let io = StdHostIo::new();
        io.console_write("test output\n");
        io.console_write_err("test error\n");
        // No assertions - just ensure it doesn't panic
    }

    #[test]
    fn test_no_host_io_console_silent() {
        // NoHostIo console writes should be silent (not panic or error)
        let io = NoHostIo::new();
        io.console_write("ignored");
        io.console_write_err("ignored");
        // No assertions - just ensure it doesn't panic
    }

    #[test]
    fn test_no_host_io_console_read_denied() {
        let io = NoHostIo::new();
        let result = io.console_read_line();
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, IoErrorKind::CapabilityDenied);
        }
    }

    // ========================================================================
    // NoHostNet tests (network operations denied in sandboxed mode)
    // ========================================================================

    #[test]
    fn test_no_host_net_http_denied() {
        let net = NoHostNet::new();

        // HTTP fetch denied
        let result = net.http_fetch("http://example.com", "GET", &HashMap::new(), &[]);
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }

        // HTTP serve denied
        let result = net.http_serve(8080);
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }

        // HTTP accept denied
        let result = net.http_accept(HttpServerHandle(1));
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }

        // HTTP respond denied
        let result = net.http_respond(HttpRequestHandle(1), 200, &HashMap::new(), &[]);
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }
    }

    #[test]
    fn test_no_host_net_websocket_denied() {
        let net = NoHostNet::new();

        // WS serve denied
        let result = net.ws_serve(8080, "/ws");
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }

        // WS accept denied
        let result = net.ws_accept(WsServerHandle(1));
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }

        // WS send text denied
        let result = net.ws_send_text(WsConnectionHandle(1), "hello");
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }

        // WS send binary denied
        let result = net.ws_send_binary(WsConnectionHandle(1), &[1, 2, 3]);
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }

        // WS recv denied
        let result = net.ws_recv(WsConnectionHandle(1));
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }

        // WS close denied
        let result = net.ws_close(WsConnectionHandle(1), 1000, "bye");
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }

        // WS is_open denied
        let result = net.ws_is_open(WsConnectionHandle(1));
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }
    }

    #[test]
    fn test_no_host_net_sse_denied() {
        let net = NoHostNet::new();

        // SSE serve denied
        let result = net.sse_serve(8080, "/events");
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }

        // SSE accept denied
        let result = net.sse_accept(SseServerHandle(1));
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }

        // SSE send denied
        let result = net.sse_send(SseEmitterHandle(1), "message", "data", "1");
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }

        // SSE close denied
        let result = net.sse_close(SseEmitterHandle(1));
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }

        // SSE is_open denied
        let result = net.sse_is_open(SseEmitterHandle(1));
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, NetErrorKind::CapabilityDenied);
        }
    }

    // ========================================================================
    // Additional time tests
    // ========================================================================

    #[test]
    fn test_std_host_time_invalid_instant_handle() {
        let time = StdHostTime::new();
        let result = time.instant_elapsed(InstantHandle(999));
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, TimeErrorKind::InvalidHandle);
        }
    }

    #[test]
    fn test_std_host_time_parse_error() {
        let time = StdHostTime::new();
        let result = time.parse("ISO8601", "not-a-date");
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, TimeErrorKind::ParseError);
        }
    }

    #[test]
    fn test_std_host_time_format_strftime() {
        let time = StdHostTime::new();
        // With arth_rt implementation, strftime formats like %Y-%m-%d are supported
        let result = time.format(0, "%Y-%m-%d");
        assert!(result.is_ok(), "Expected success, got: {:?}", result);
        let formatted = result.unwrap();
        // Epoch 0 is 1970-01-01 (in UTC) or Dec 31 1969 in some timezones
        assert!(
            formatted.contains("1970") || formatted.contains("1969"),
            "Expected year 1969 or 1970, got: {}",
            formatted
        );
    }

    #[test]
    fn test_std_host_time_format_empty_returns_error() {
        let time = StdHostTime::new();
        // Empty format produces empty output, which triggers BufferTooSmall error
        let result = time.format(0, "");
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, TimeErrorKind::FormatError);
        }
    }

    #[test]
    fn test_mock_host_time_invalid_instant_handle() {
        let time = MockHostTime::new(0);
        let result = time.instant_elapsed(InstantHandle(999));
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.kind, TimeErrorKind::InvalidHandle);
        }
    }

    #[test]
    fn test_std_host_time_sleep_zero() {
        let time = StdHostTime::new();
        // Sleep with 0 should not block
        time.sleep(0);
        // Sleep with negative should be treated as 0
        time.sleep(-100);
    }

    // ========================================================================
    // StdHostNet Tests
    // ========================================================================

    #[test]
    fn test_parse_http_url_http() {
        let (host, port, path, use_tls) = parse_http_url("http://example.com/test").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/test");
        assert!(!use_tls);
    }

    #[test]
    fn test_parse_http_url_https() {
        let (host, port, path, use_tls) = parse_http_url("https://example.com/api/v1").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
        assert_eq!(path, "/api/v1");
        assert!(use_tls);
    }

    #[test]
    fn test_parse_http_url_custom_port() {
        let (host, port, path, use_tls) = parse_http_url("http://localhost:8080/").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
        assert_eq!(path, "/");
        assert!(!use_tls);
    }

    #[test]
    fn test_parse_http_url_no_path() {
        let (host, port, path, use_tls) = parse_http_url("https://api.example.com").unwrap();
        assert_eq!(host, "api.example.com");
        assert_eq!(port, 443);
        assert_eq!(path, "/");
        assert!(use_tls);
    }

    #[test]
    fn test_parse_http_url_invalid() {
        assert!(parse_http_url("ftp://example.com").is_err());
        assert!(parse_http_url("example.com").is_err());
    }

    #[test]
    fn test_std_host_net_server_invalid_handles() {
        let net = StdHostNet::new();

        // Invalid HTTP handles should error
        assert!(net.http_accept(HttpServerHandle(99999)).is_err());
        assert!(
            net.http_respond(HttpRequestHandle(99999), 200, &HashMap::new(), &[])
                .is_err()
        );

        // Invalid WebSocket handles should error
        assert!(net.ws_accept(WsServerHandle(99999)).is_err());
        assert!(net.ws_send_text(WsConnectionHandle(99999), "test").is_err());
        assert!(net.ws_send_binary(WsConnectionHandle(99999), &[]).is_err());
        assert!(net.ws_recv(WsConnectionHandle(99999)).is_err());
        // ws_close and ws_is_open with invalid handle
        assert!(net.ws_close(WsConnectionHandle(99999), 1000, "").is_ok()); // no-op for missing
        assert!(net.ws_is_open(WsConnectionHandle(99999)).is_err());

        // Invalid SSE handles should error
        assert!(net.sse_accept(SseServerHandle(99999)).is_err());
        assert!(
            net.sse_send(SseEmitterHandle(99999), "event", "data", "")
                .is_err()
        );
        // sse_close with invalid handle is a no-op
        assert!(net.sse_close(SseEmitterHandle(99999)).is_ok());
        assert!(net.sse_is_open(SseEmitterHandle(99999)).is_err());
    }
}
