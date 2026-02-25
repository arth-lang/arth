//! TLS/SSL operations using native-tls
//!
//! This module provides C FFI wrappers for TLS/SSL operations using the
//! native TLS library for each platform:
//! - macOS: Security.framework
//! - Linux: OpenSSL
//! - Windows: SChannel
//!
//! All functions use opaque i64 handles for TLS streams.

use crate::error::{ErrorCode, set_last_error};
use crate::net::SOCKETS;
use crate::new_handle;

use native_tls::{HandshakeError, Identity, TlsAcceptor, TlsConnector, TlsStream};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::fd::FromRawFd;
use std::sync::Mutex;

// -----------------------------------------------------------------------------
// TLS Handle Management
// -----------------------------------------------------------------------------

/// TLS stream wrapper that owns the TcpStream
struct TlsStreamData {
    stream: TlsStream<TcpStream>,
    #[allow(dead_code)]
    original_handle: i64, // The socket handle that was wrapped (for debugging)
}

/// TLS acceptor (server-side)
struct TlsAcceptorData {
    acceptor: TlsAcceptor,
}

lazy_static::lazy_static! {
    /// Active TLS streams
    static ref TLS_STREAMS: Mutex<HashMap<i64, TlsStreamData>> = Mutex::new(HashMap::new());

    /// TLS connectors (client-side)
    static ref TLS_CONNECTORS: Mutex<HashMap<i64, TlsConnector>> = Mutex::new(HashMap::new());

    /// TLS acceptors (server-side)
    static ref TLS_ACCEPTORS: Mutex<HashMap<i64, TlsAcceptorData>> = Mutex::new(HashMap::new());
}

// -----------------------------------------------------------------------------
// TLS Connector (Client-side)
// -----------------------------------------------------------------------------

/// Create a default TLS connector for client connections
///
/// # Returns
/// * Positive handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_connector_new() -> i64 {
    match TlsConnector::new() {
        Ok(connector) => {
            let handle = new_handle();
            TLS_CONNECTORS.lock().unwrap().insert(handle, connector);
            handle
        }
        Err(e) => {
            set_last_error(&format!("Failed to create TLS connector: {}", e));
            ErrorCode::Error.as_i32() as i64
        }
    }
}

/// Create a TLS connector that skips certificate verification (INSECURE)
///
/// WARNING: This should only be used for testing or when connecting to
/// servers with self-signed certificates. It is NOT secure for production.
///
/// # Returns
/// * Positive handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_connector_insecure() -> i64 {
    let mut builder = native_tls::TlsConnector::builder();
    builder
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true);

    match builder.build() {
        Ok(connector) => {
            let handle = new_handle();
            TLS_CONNECTORS.lock().unwrap().insert(handle, connector);
            handle
        }
        Err(e) => {
            set_last_error(&format!("Failed to create insecure TLS connector: {}", e));
            ErrorCode::Error.as_i32() as i64
        }
    }
}

/// Free a TLS connector
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_connector_free(handle: i64) -> i32 {
    match TLS_CONNECTORS.lock().unwrap().remove(&handle) {
        Some(_) => 0,
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Perform TLS handshake on a socket (client-side)
///
/// This consumes the socket handle - after a successful handshake,
/// use the returned TLS handle for all I/O operations.
///
/// # Arguments
/// * `connector` - TLS connector handle
/// * `socket` - Socket handle to wrap
/// * `hostname` - Server hostname for SNI (UTF-8)
/// * `hostname_len` - Length of hostname
///
/// # Returns
/// * Positive TLS stream handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_connect(
    connector: i64,
    socket: i64,
    hostname: *const u8,
    hostname_len: usize,
) -> i64 {
    if hostname.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    // Get the connector
    let connectors = TLS_CONNECTORS.lock().unwrap();
    let tls_connector = match connectors.get(&connector) {
        Some(c) => c,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    // Get the socket fd and remove from socket table
    let fd = {
        let mut sockets = SOCKETS.lock().unwrap();
        match sockets.remove(&socket) {
            Some(data) => data.fd,
            None => return ErrorCode::InvalidHandle.as_i32() as i64,
        }
    };

    // Parse hostname
    let host_slice = unsafe { std::slice::from_raw_parts(hostname, hostname_len) };
    let host_str = match std::str::from_utf8(host_slice) {
        Ok(s) => s,
        Err(_) => {
            // Re-add socket on error (before TcpStream takes ownership)
            let mut sockets = SOCKETS.lock().unwrap();
            sockets.insert(
                socket,
                crate::net::SocketData {
                    fd,
                    is_connected: true,
                    is_listening: false,
                },
            );
            return ErrorCode::InvalidArgument.as_i32() as i64;
        }
    };

    // Create TcpStream from fd
    let tcp_stream = unsafe { TcpStream::from_raw_fd(fd) };

    // Perform handshake
    match tls_connector.connect(host_str, tcp_stream) {
        Ok(tls_stream) => {
            let handle = new_handle();
            TLS_STREAMS.lock().unwrap().insert(
                handle,
                TlsStreamData {
                    stream: tls_stream,
                    original_handle: socket,
                },
            );
            handle
        }
        Err(e) => {
            match e {
                HandshakeError::Failure(e) => {
                    set_last_error(&format!("TLS handshake failed: {}", e));
                    ErrorCode::Error.as_i32() as i64
                }
                HandshakeError::WouldBlock(_mid) => {
                    // For non-blocking, we'd need to handle mid-handshake state
                    // We can't easily restore the socket here because native-tls
                    // doesn't expose into_inner on MidHandshakeTlsStream
                    set_last_error("TLS handshake would block (non-blocking not supported)");
                    ErrorCode::WouldBlock.as_i32() as i64
                }
            }
        }
    }
}

// -----------------------------------------------------------------------------
// TLS Acceptor (Server-side)
// -----------------------------------------------------------------------------

/// Create a TLS acceptor from a PKCS#12 identity
///
/// # Arguments
/// * `pkcs12_data` - PKCS#12 data (certificate + private key)
/// * `pkcs12_len` - Length of PKCS#12 data
/// * `password` - Password for the PKCS#12 file (UTF-8)
/// * `password_len` - Length of password
///
/// # Returns
/// * Positive acceptor handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_acceptor_new(
    pkcs12_data: *const u8,
    pkcs12_len: usize,
    password: *const u8,
    password_len: usize,
) -> i64 {
    if pkcs12_data.is_null() || password.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let pkcs12_slice = unsafe { std::slice::from_raw_parts(pkcs12_data, pkcs12_len) };
    let pwd_slice = unsafe { std::slice::from_raw_parts(password, password_len) };

    let pwd_str = match std::str::from_utf8(pwd_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
    };

    // Parse PKCS#12 identity
    let identity = match Identity::from_pkcs12(pkcs12_slice, pwd_str) {
        Ok(id) => id,
        Err(e) => {
            set_last_error(&format!("Failed to parse PKCS#12: {}", e));
            return ErrorCode::InvalidArgument.as_i32() as i64;
        }
    };

    // Create acceptor
    match TlsAcceptor::new(identity) {
        Ok(acceptor) => {
            let handle = new_handle();
            TLS_ACCEPTORS
                .lock()
                .unwrap()
                .insert(handle, TlsAcceptorData { acceptor });
            handle
        }
        Err(e) => {
            set_last_error(&format!("Failed to create TLS acceptor: {}", e));
            ErrorCode::Error.as_i32() as i64
        }
    }
}

/// Free a TLS acceptor
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_acceptor_free(handle: i64) -> i32 {
    match TLS_ACCEPTORS.lock().unwrap().remove(&handle) {
        Some(_) => 0,
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Accept a TLS connection on a socket (server-side)
///
/// This consumes the socket handle - after a successful handshake,
/// use the returned TLS handle for all I/O operations.
///
/// # Arguments
/// * `acceptor` - TLS acceptor handle
/// * `socket` - Socket handle from accept()
///
/// # Returns
/// * Positive TLS stream handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_accept(acceptor: i64, socket: i64) -> i64 {
    // Get the acceptor
    let acceptors = TLS_ACCEPTORS.lock().unwrap();
    let tls_acceptor = match acceptors.get(&acceptor) {
        Some(a) => &a.acceptor,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    // Get the socket fd and remove from socket table
    let fd = {
        let mut sockets = SOCKETS.lock().unwrap();
        match sockets.remove(&socket) {
            Some(data) => data.fd,
            None => return ErrorCode::InvalidHandle.as_i32() as i64,
        }
    };

    // Create TcpStream from fd
    let tcp_stream = unsafe { TcpStream::from_raw_fd(fd) };

    // Perform handshake
    match tls_acceptor.accept(tcp_stream) {
        Ok(tls_stream) => {
            let handle = new_handle();
            TLS_STREAMS.lock().unwrap().insert(
                handle,
                TlsStreamData {
                    stream: tls_stream,
                    original_handle: socket,
                },
            );
            handle
        }
        Err(e) => {
            match e {
                HandshakeError::Failure(e) => {
                    set_last_error(&format!("TLS accept handshake failed: {}", e));
                    ErrorCode::Error.as_i32() as i64
                }
                HandshakeError::WouldBlock(_mid) => {
                    // We can't easily restore the socket here because native-tls
                    // doesn't expose into_inner on MidHandshakeTlsStream
                    set_last_error("TLS handshake would block (non-blocking not supported)");
                    ErrorCode::WouldBlock.as_i32() as i64
                }
            }
        }
    }
}

// -----------------------------------------------------------------------------
// TLS Stream Operations
// -----------------------------------------------------------------------------

/// Read data from a TLS stream
///
/// # Arguments
/// * `handle` - TLS stream handle
/// * `buf` - Buffer to receive data
/// * `len` - Buffer length
///
/// # Returns
/// * Number of bytes read (0 = connection closed)
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_read(handle: i64, buf: *mut u8, len: usize) -> i64 {
    if buf.is_null() && len > 0 {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let mut streams = TLS_STREAMS.lock().unwrap();
    let data = match streams.get_mut(&handle) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    let buf_slice = unsafe { std::slice::from_raw_parts_mut(buf, len) };

    match data.stream.read(buf_slice) {
        Ok(n) => n as i64,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                return ErrorCode::WouldBlock.as_i32() as i64;
            }
            set_last_error(&format!("TLS read error: {}", e));
            ErrorCode::Error.as_i32() as i64
        }
    }
}

/// Write data to a TLS stream
///
/// # Arguments
/// * `handle` - TLS stream handle
/// * `buf` - Data to write
/// * `len` - Data length
///
/// # Returns
/// * Number of bytes written
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_write(handle: i64, buf: *const u8, len: usize) -> i64 {
    if buf.is_null() && len > 0 {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let mut streams = TLS_STREAMS.lock().unwrap();
    let data = match streams.get_mut(&handle) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    let buf_slice = unsafe { std::slice::from_raw_parts(buf, len) };

    match data.stream.write(buf_slice) {
        Ok(n) => n as i64,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                return ErrorCode::WouldBlock.as_i32() as i64;
            }
            set_last_error(&format!("TLS write error: {}", e));
            ErrorCode::Error.as_i32() as i64
        }
    }
}

/// Flush pending TLS data
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_flush(handle: i64) -> i32 {
    let mut streams = TLS_STREAMS.lock().unwrap();
    let data = match streams.get_mut(&handle) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    match data.stream.flush() {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(&format!("TLS flush error: {}", e));
            ErrorCode::Error.as_i32()
        }
    }
}

/// Initiate TLS shutdown
///
/// This sends a TLS close_notify alert and shuts down the write side
/// of the connection. After calling this, you should continue reading
/// until you receive 0 bytes (peer's close_notify).
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_shutdown(handle: i64) -> i32 {
    let mut streams = TLS_STREAMS.lock().unwrap();
    let data = match streams.get_mut(&handle) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    match data.stream.shutdown() {
        Ok(()) => 0,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                return ErrorCode::WouldBlock.as_i32();
            }
            set_last_error(&format!("TLS shutdown error: {}", e));
            ErrorCode::Error.as_i32()
        }
    }
}

/// Close and free a TLS stream
///
/// This closes the underlying socket as well.
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_close(handle: i64) -> i32 {
    let mut streams = TLS_STREAMS.lock().unwrap();
    match streams.remove(&handle) {
        Some(data) => {
            // Try to shutdown gracefully, ignore errors
            drop(data.stream);
            0
        }
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

// -----------------------------------------------------------------------------
// TLS Stream Information
// -----------------------------------------------------------------------------

/// Get the negotiated TLS protocol version
///
/// # Arguments
/// * `handle` - TLS stream handle
/// * `buf` - Buffer to receive version string
/// * `buf_len` - Buffer length
///
/// # Returns
/// * Number of bytes written (excluding null terminator)
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_version(handle: i64, buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let streams = TLS_STREAMS.lock().unwrap();
    let data = match streams.get(&handle) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let version = match data.stream.tls_server_end_point() {
        // native-tls doesn't expose version directly, so we use a generic string
        _ => "TLS",
    };

    let bytes = version.as_bytes();
    if bytes.len() >= buf_len {
        return ErrorCode::BufferTooSmall.as_i32();
    }

    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
        *buf.add(bytes.len()) = 0;
    }

    bytes.len() as i32
}

/// Get the negotiated cipher suite
///
/// # Arguments
/// * `handle` - TLS stream handle
/// * `buf` - Buffer to receive cipher string
/// * `buf_len` - Buffer length
///
/// # Returns
/// * Number of bytes written (excluding null terminator)
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_cipher(handle: i64, buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let streams = TLS_STREAMS.lock().unwrap();
    let _data = match streams.get(&handle) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    // native-tls doesn't expose the cipher suite directly
    // Return a placeholder
    let cipher = "unknown";
    let bytes = cipher.as_bytes();
    if bytes.len() >= buf_len {
        return ErrorCode::BufferTooSmall.as_i32();
    }

    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
        *buf.add(bytes.len()) = 0;
    }

    bytes.len() as i32
}

/// Get the peer's certificate (DER encoded)
///
/// # Arguments
/// * `handle` - TLS stream handle
/// * `buf` - Buffer to receive certificate
/// * `buf_len` - Buffer length
///
/// # Returns
/// * Number of bytes written
/// * 0 if no peer certificate
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_tls_peer_cert(handle: i64, buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() && buf_len > 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let streams = TLS_STREAMS.lock().unwrap();
    let data = match streams.get(&handle) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    match data.stream.peer_certificate() {
        Ok(Some(cert)) => {
            let der = match cert.to_der() {
                Ok(d) => d,
                Err(e) => {
                    set_last_error(&format!("Failed to encode certificate: {}", e));
                    return ErrorCode::Error.as_i32();
                }
            };
            if der.len() > buf_len {
                return ErrorCode::BufferTooSmall.as_i32();
            }
            unsafe {
                std::ptr::copy_nonoverlapping(der.as_ptr(), buf, der.len());
            }
            der.len() as i32
        }
        Ok(None) => 0, // No peer certificate
        Err(e) => {
            set_last_error(&format!("Failed to get peer certificate: {}", e));
            ErrorCode::Error.as_i32()
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connector_create_free() {
        let handle = arth_rt_tls_connector_new();
        assert!(handle > 0, "Failed to create TLS connector");

        let rc = arth_rt_tls_connector_free(handle);
        assert_eq!(rc, 0);
    }

    #[test]
    fn test_connector_insecure() {
        let handle = arth_rt_tls_connector_insecure();
        assert!(handle > 0, "Failed to create insecure TLS connector");

        let rc = arth_rt_tls_connector_free(handle);
        assert_eq!(rc, 0);
    }

    #[test]
    fn test_connector_invalid_handle() {
        let rc = arth_rt_tls_connector_free(99999);
        assert!(rc < 0);
    }

    #[test]
    fn test_stream_invalid_handle() {
        let rc = arth_rt_tls_close(99999);
        assert!(rc < 0);
    }

    #[test]
    fn test_read_invalid_handle() {
        let mut buf = [0u8; 16];
        let rc = arth_rt_tls_read(99999, buf.as_mut_ptr(), buf.len());
        assert!(rc < 0);
    }

    #[test]
    fn test_write_invalid_handle() {
        let buf = b"hello";
        let rc = arth_rt_tls_write(99999, buf.as_ptr(), buf.len());
        assert!(rc < 0);
    }
}
