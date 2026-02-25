//! Network operations using BSD sockets
//!
//! This module provides C FFI wrappers for BSD socket operations and DNS resolution.
//! All functions use opaque i64 handles for sockets.

use crate::error::{ErrorCode, set_last_error};
use crate::new_handle;

use std::collections::HashMap;
use std::sync::Mutex;

// Extern C declaration for inet_pton (not always exposed in libc crate)
unsafe extern "C" {
    fn inet_pton(af: libc::c_int, src: *const libc::c_char, dst: *mut libc::c_void) -> libc::c_int;
}

// -----------------------------------------------------------------------------
// Socket Handle Management
// -----------------------------------------------------------------------------

/// Socket handle data
pub(crate) struct SocketData {
    pub(crate) fd: libc::c_int,
    pub(crate) is_connected: bool,
    pub(crate) is_listening: bool,
}

lazy_static::lazy_static! {
    /// Active sockets (accessible from tls module for TLS wrapping)
    pub(crate) static ref SOCKETS: Mutex<HashMap<i64, SocketData>> = Mutex::new(HashMap::new());
    static ref ADDRINFO_HANDLES: Mutex<HashMap<i64, AddrInfoIter>> = Mutex::new(HashMap::new());
}

/// Iterator over addrinfo results
struct AddrInfoIter {
    head: *mut libc::addrinfo,
    current: *mut libc::addrinfo,
}

// SAFETY: addrinfo pointers are thread-safe for read access
unsafe impl Send for AddrInfoIter {}

impl Drop for AddrInfoIter {
    fn drop(&mut self) {
        if !self.head.is_null() {
            unsafe { libc::freeaddrinfo(self.head) };
        }
    }
}

// -----------------------------------------------------------------------------
// Socket Constants
// -----------------------------------------------------------------------------

// Address families
pub const AF_INET: i32 = libc::AF_INET;
pub const AF_INET6: i32 = libc::AF_INET6;
pub const AF_UNSPEC: i32 = libc::AF_UNSPEC;

// Socket types
pub const SOCK_STREAM: i32 = libc::SOCK_STREAM;
pub const SOCK_DGRAM: i32 = libc::SOCK_DGRAM;

// Protocols
pub const IPPROTO_TCP: i32 = libc::IPPROTO_TCP;
pub const IPPROTO_UDP: i32 = libc::IPPROTO_UDP;

// Socket options
pub const SOL_SOCKET: i32 = libc::SOL_SOCKET;
pub const SO_REUSEADDR: i32 = libc::SO_REUSEADDR;
pub const SO_KEEPALIVE: i32 = libc::SO_KEEPALIVE;
pub const SO_RCVTIMEO: i32 = libc::SO_RCVTIMEO;
pub const SO_SNDTIMEO: i32 = libc::SO_SNDTIMEO;
pub const TCP_NODELAY: i32 = libc::TCP_NODELAY;

// Send/recv flags
pub const MSG_DONTWAIT: i32 = libc::MSG_DONTWAIT;
pub const MSG_PEEK: i32 = libc::MSG_PEEK;

// -----------------------------------------------------------------------------
// Socket Operations
// -----------------------------------------------------------------------------

/// Create a new socket
///
/// # Arguments
/// * `domain` - Address family (AF_INET, AF_INET6)
/// * `socket_type` - Socket type (SOCK_STREAM, SOCK_DGRAM)
/// * `protocol` - Protocol (IPPROTO_TCP, IPPROTO_UDP, or 0)
///
/// # Returns
/// * Positive socket handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_create(domain: i32, socket_type: i32, protocol: i32) -> i64 {
    let fd = unsafe { libc::socket(domain, socket_type, protocol) };
    if fd < 0 {
        set_last_error("Failed to create socket");
        return ErrorCode::Error.as_i32() as i64;
    }

    let handle = new_handle();
    SOCKETS.lock().unwrap().insert(
        handle,
        SocketData {
            fd,
            is_connected: false,
            is_listening: false,
        },
    );
    handle
}

/// Close a socket
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_close(sock: i64) -> i32 {
    let mut sockets = SOCKETS.lock().unwrap();
    match sockets.remove(&sock) {
        Some(data) => {
            let rc = unsafe { libc::close(data.fd) };
            if rc < 0 { ErrorCode::Error.as_i32() } else { 0 }
        }
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Connect a socket to a remote address
///
/// # Arguments
/// * `sock` - Socket handle
/// * `addr` - Pointer to sockaddr structure
/// * `addr_len` - Length of sockaddr structure
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_connect(
    sock: i64,
    addr: *const libc::sockaddr,
    addr_len: libc::socklen_t,
) -> i32 {
    if addr.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let mut sockets = SOCKETS.lock().unwrap();
    let data = match sockets.get_mut(&sock) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let rc = unsafe { libc::connect(data.fd, addr, addr_len) };
    if rc < 0 {
        set_last_error("Failed to connect");
        return ErrorCode::ConnectionRefused.as_i32();
    }

    data.is_connected = true;
    0
}

/// Connect a socket to a host:port (convenience function)
///
/// # Arguments
/// * `sock` - Socket handle
/// * `host` - Hostname or IP address (UTF-8)
/// * `host_len` - Length of hostname
/// * `port` - Port number
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_connect_host(
    sock: i64,
    host: *const u8,
    host_len: usize,
    port: u16,
) -> i32 {
    if host.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    // Get the socket fd
    let fd = {
        let sockets = SOCKETS.lock().unwrap();
        match sockets.get(&sock) {
            Some(d) => d.fd,
            None => return ErrorCode::InvalidHandle.as_i32(),
        }
    };

    // Resolve the hostname
    let host_slice = unsafe { std::slice::from_raw_parts(host, host_len) };
    let host_str = match std::str::from_utf8(host_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };

    let host_cstr = match std::ffi::CString::new(host_str) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };

    let port_str = format!("{}", port);
    let port_cstr = std::ffi::CString::new(port_str).unwrap();

    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = libc::AF_UNSPEC;
    hints.ai_socktype = libc::SOCK_STREAM;

    let mut result: *mut libc::addrinfo = std::ptr::null_mut();
    let rc =
        unsafe { libc::getaddrinfo(host_cstr.as_ptr(), port_cstr.as_ptr(), &hints, &mut result) };

    if rc != 0 || result.is_null() {
        set_last_error("Failed to resolve hostname");
        return ErrorCode::NotFound.as_i32();
    }

    // Try connecting to each address
    let mut connected = false;
    let mut current = result;
    while !current.is_null() {
        let ai = unsafe { &*current };
        let connect_rc = unsafe { libc::connect(fd, ai.ai_addr, ai.ai_addrlen) };
        if connect_rc == 0 {
            connected = true;
            break;
        }
        current = ai.ai_next;
    }

    unsafe { libc::freeaddrinfo(result) };

    if !connected {
        set_last_error("Failed to connect to any address");
        return ErrorCode::ConnectionRefused.as_i32();
    }

    // Mark as connected
    {
        let mut sockets = SOCKETS.lock().unwrap();
        if let Some(data) = sockets.get_mut(&sock) {
            data.is_connected = true;
        }
    }

    0
}

/// Bind a socket to a local address
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_bind(
    sock: i64,
    addr: *const libc::sockaddr,
    addr_len: libc::socklen_t,
) -> i32 {
    if addr.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let sockets = SOCKETS.lock().unwrap();
    let data = match sockets.get(&sock) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let rc = unsafe { libc::bind(data.fd, addr, addr_len) };
    if rc < 0 {
        set_last_error("Failed to bind socket");
        return ErrorCode::Error.as_i32();
    }

    0
}

/// Bind a socket to a port on all interfaces
///
/// # Arguments
/// * `sock` - Socket handle
/// * `port` - Port number to bind to
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_bind_port(sock: i64, port: u16) -> i32 {
    let sockets = SOCKETS.lock().unwrap();
    let data = match sockets.get(&sock) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let addr = libc::sockaddr_in {
        sin_len: std::mem::size_of::<libc::sockaddr_in>() as u8,
        sin_family: libc::AF_INET as u8,
        sin_port: port.to_be(),
        sin_addr: libc::in_addr { s_addr: 0 }, // INADDR_ANY
        sin_zero: [0; 8],
    };

    let rc = unsafe {
        libc::bind(
            data.fd,
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        )
    };

    if rc < 0 {
        set_last_error("Failed to bind socket");
        return ErrorCode::Error.as_i32();
    }

    0
}

/// Start listening for connections
///
/// # Arguments
/// * `sock` - Socket handle
/// * `backlog` - Maximum pending connections
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_listen(sock: i64, backlog: i32) -> i32 {
    let mut sockets = SOCKETS.lock().unwrap();
    let data = match sockets.get_mut(&sock) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let rc = unsafe { libc::listen(data.fd, backlog) };
    if rc < 0 {
        set_last_error("Failed to listen on socket");
        return ErrorCode::Error.as_i32();
    }

    data.is_listening = true;
    0
}

/// Accept a connection on a listening socket
///
/// # Returns
/// * Positive socket handle for the new connection
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_accept(sock: i64) -> i64 {
    let fd = {
        let sockets = SOCKETS.lock().unwrap();
        match sockets.get(&sock) {
            Some(d) => {
                if !d.is_listening {
                    return ErrorCode::InvalidArgument.as_i32() as i64;
                }
                d.fd
            }
            None => return ErrorCode::InvalidHandle.as_i32() as i64,
        }
    };

    let mut client_addr: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut addr_len: libc::socklen_t =
        std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;

    let client_fd = unsafe {
        libc::accept(
            fd,
            &mut client_addr as *mut _ as *mut libc::sockaddr,
            &mut addr_len,
        )
    };

    if client_fd < 0 {
        set_last_error("Failed to accept connection");
        return ErrorCode::Error.as_i32() as i64;
    }

    let handle = new_handle();
    SOCKETS.lock().unwrap().insert(
        handle,
        SocketData {
            fd: client_fd,
            is_connected: true,
            is_listening: false,
        },
    );
    handle
}

/// Send data on a socket
///
/// # Arguments
/// * `sock` - Socket handle
/// * `buf` - Data buffer
/// * `len` - Length of data
/// * `flags` - Send flags (e.g., MSG_DONTWAIT)
///
/// # Returns
/// * Number of bytes sent on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_send(sock: i64, buf: *const u8, len: usize, flags: i32) -> i64 {
    if buf.is_null() && len > 0 {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let sockets = SOCKETS.lock().unwrap();
    let data = match sockets.get(&sock) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    let sent = unsafe { libc::send(data.fd, buf as *const libc::c_void, len, flags) };

    if sent < 0 {
        set_last_error("Failed to send data");
        return ErrorCode::Error.as_i32() as i64;
    }

    sent as i64
}

/// Receive data from a socket
///
/// # Arguments
/// * `sock` - Socket handle
/// * `buf` - Buffer to receive data
/// * `len` - Buffer length
/// * `flags` - Receive flags (e.g., MSG_PEEK)
///
/// # Returns
/// * Number of bytes received on success (0 = connection closed)
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_recv(sock: i64, buf: *mut u8, len: usize, flags: i32) -> i64 {
    if buf.is_null() && len > 0 {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let sockets = SOCKETS.lock().unwrap();
    let data = match sockets.get(&sock) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    let received = unsafe { libc::recv(data.fd, buf as *mut libc::c_void, len, flags) };

    if received < 0 {
        set_last_error("Failed to receive data");
        return ErrorCode::Error.as_i32() as i64;
    }

    received as i64
}

/// Set a socket option
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_setsockopt(
    sock: i64,
    level: i32,
    optname: i32,
    optval: *const libc::c_void,
    optlen: libc::socklen_t,
) -> i32 {
    let sockets = SOCKETS.lock().unwrap();
    let data = match sockets.get(&sock) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let rc = unsafe { libc::setsockopt(data.fd, level, optname, optval, optlen) };
    if rc < 0 {
        set_last_error("Failed to set socket option");
        return ErrorCode::Error.as_i32();
    }

    0
}

/// Set socket option (integer value convenience)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_setsockopt_int(
    sock: i64,
    level: i32,
    optname: i32,
    optval: i32,
) -> i32 {
    arth_rt_socket_setsockopt(
        sock,
        level,
        optname,
        &optval as *const i32 as *const libc::c_void,
        std::mem::size_of::<i32>() as libc::socklen_t,
    )
}

/// Set socket to non-blocking mode
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_set_nonblocking(sock: i64, nonblocking: i32) -> i32 {
    let sockets = SOCKETS.lock().unwrap();
    let data = match sockets.get(&sock) {
        Some(d) => d,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    let flags = unsafe { libc::fcntl(data.fd, libc::F_GETFL, 0) };
    if flags < 0 {
        return ErrorCode::Error.as_i32();
    }

    let new_flags = if nonblocking != 0 {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };

    let rc = unsafe { libc::fcntl(data.fd, libc::F_SETFL, new_flags) };
    if rc < 0 {
        return ErrorCode::Error.as_i32();
    }

    0
}

/// Get the underlying file descriptor for a socket
/// Useful for poll/select operations
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_socket_fd(sock: i64) -> i32 {
    let sockets = SOCKETS.lock().unwrap();
    match sockets.get(&sock) {
        Some(d) => d.fd,
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

// -----------------------------------------------------------------------------
// DNS Resolution
// -----------------------------------------------------------------------------

/// Resolve a hostname to addresses
///
/// # Arguments
/// * `host` - Hostname (UTF-8)
/// * `host_len` - Length of hostname
/// * `port` - Port number (as string), or 0 for NULL
///
/// # Returns
/// * Positive handle for iterating results
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_getaddrinfo(host: *const u8, host_len: usize, port: u16) -> i64 {
    if host.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let host_slice = unsafe { std::slice::from_raw_parts(host, host_len) };
    let host_str = match std::str::from_utf8(host_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
    };

    let host_cstr = match std::ffi::CString::new(host_str) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
    };

    let port_str = if port > 0 {
        Some(std::ffi::CString::new(format!("{}", port)).unwrap())
    } else {
        None
    };

    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = libc::AF_UNSPEC;
    hints.ai_socktype = libc::SOCK_STREAM;

    let mut result: *mut libc::addrinfo = std::ptr::null_mut();
    let rc = unsafe {
        libc::getaddrinfo(
            host_cstr.as_ptr(),
            port_str
                .as_ref()
                .map(|s| s.as_ptr())
                .unwrap_or(std::ptr::null()),
            &hints,
            &mut result,
        )
    };

    if rc != 0 || result.is_null() {
        set_last_error("Failed to resolve hostname");
        return ErrorCode::NotFound.as_i32() as i64;
    }

    let handle = new_handle();
    ADDRINFO_HANDLES.lock().unwrap().insert(
        handle,
        AddrInfoIter {
            head: result,
            current: result,
        },
    );
    handle
}

/// Get the next address from DNS resolution results
///
/// # Arguments
/// * `handle` - Handle from arth_rt_getaddrinfo
/// * `addr` - Buffer to receive sockaddr
/// * `addr_len` - On input: buffer size. On output: actual size written.
///
/// # Returns
/// * 0 if an address was copied
/// * 1 if no more addresses
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_addrinfo_next(
    handle: i64,
    addr: *mut libc::sockaddr_storage,
    addr_len: *mut libc::socklen_t,
) -> i32 {
    if addr.is_null() || addr_len.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let mut handles = ADDRINFO_HANDLES.lock().unwrap();
    let iter = match handles.get_mut(&handle) {
        Some(i) => i,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    if iter.current.is_null() {
        return 1; // No more addresses
    }

    let ai = unsafe { &*iter.current };
    let required_len = ai.ai_addrlen as usize;
    let buf_len = unsafe { *addr_len } as usize;

    if buf_len < required_len {
        return ErrorCode::BufferTooSmall.as_i32();
    }

    unsafe {
        std::ptr::copy_nonoverlapping(ai.ai_addr as *const u8, addr as *mut u8, required_len);
        *addr_len = required_len as libc::socklen_t;
    }

    // Move to next
    iter.current = ai.ai_next;
    0
}

/// Free DNS resolution results
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_freeaddrinfo(handle: i64) -> i32 {
    let mut handles = ADDRINFO_HANDLES.lock().unwrap();
    match handles.remove(&handle) {
        Some(_) => 0, // AddrInfoIter's Drop will call freeaddrinfo
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

// -----------------------------------------------------------------------------
// Address Helper Functions
// -----------------------------------------------------------------------------

/// Create an IPv4 address structure
///
/// # Arguments
/// * `ip` - IP address as 4 bytes (network byte order)
/// * `port` - Port number
/// * `addr` - Output sockaddr_in buffer
///
/// # Returns
/// * 0 on success
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_addr_ipv4(ip: *const u8, port: u16, addr: *mut libc::sockaddr_in) -> i32 {
    if ip.is_null() || addr.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    unsafe {
        (*addr).sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
        (*addr).sin_family = libc::AF_INET as u8;
        (*addr).sin_port = port.to_be();
        std::ptr::copy_nonoverlapping(ip, &mut (*addr).sin_addr.s_addr as *mut _ as *mut u8, 4);
        (*addr).sin_zero = [0; 8];
    }

    0
}

/// Parse an IP address string to binary form
///
/// # Arguments
/// * `ip_str` - IP address string (UTF-8)
/// * `ip_len` - Length of IP string
/// * `addr` - Output buffer for sockaddr_in or sockaddr_in6
/// * `addr_len` - On input: buffer size. On output: actual size written.
///
/// # Returns
/// * 0 on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_addr_parse(
    ip_str: *const u8,
    ip_len: usize,
    port: u16,
    addr: *mut libc::sockaddr_storage,
    addr_len: *mut libc::socklen_t,
) -> i32 {
    if ip_str.is_null() || addr.is_null() || addr_len.is_null() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let ip_slice = unsafe { std::slice::from_raw_parts(ip_str, ip_len) };
    let ip_string = match std::str::from_utf8(ip_slice) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };

    let ip_cstr = match std::ffi::CString::new(ip_string) {
        Ok(s) => s,
        Err(_) => return ErrorCode::InvalidArgument.as_i32(),
    };

    // Try IPv4 first
    let mut addr4: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    if unsafe {
        inet_pton(
            libc::AF_INET,
            ip_cstr.as_ptr(),
            &mut addr4.sin_addr as *mut _ as *mut libc::c_void,
        )
    } == 1
    {
        let size = std::mem::size_of::<libc::sockaddr_in>();
        if (unsafe { *addr_len } as usize) < size {
            return ErrorCode::BufferTooSmall.as_i32();
        }
        addr4.sin_len = size as u8;
        addr4.sin_family = libc::AF_INET as u8;
        addr4.sin_port = port.to_be();
        unsafe {
            std::ptr::copy_nonoverlapping(&addr4 as *const _ as *const u8, addr as *mut u8, size);
            *addr_len = size as libc::socklen_t;
        }
        return 0;
    }

    // Try IPv6
    let mut addr6: libc::sockaddr_in6 = unsafe { std::mem::zeroed() };
    if unsafe {
        inet_pton(
            libc::AF_INET6,
            ip_cstr.as_ptr(),
            &mut addr6.sin6_addr as *mut _ as *mut libc::c_void,
        )
    } == 1
    {
        let size = std::mem::size_of::<libc::sockaddr_in6>();
        if (unsafe { *addr_len } as usize) < size {
            return ErrorCode::BufferTooSmall.as_i32();
        }
        addr6.sin6_len = size as u8;
        addr6.sin6_family = libc::AF_INET6 as u8;
        addr6.sin6_port = port.to_be();
        unsafe {
            std::ptr::copy_nonoverlapping(&addr6 as *const _ as *const u8, addr as *mut u8, size);
            *addr_len = size as libc::socklen_t;
        }
        return 0;
    }

    set_last_error("Invalid IP address");
    ErrorCode::InvalidArgument.as_i32()
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_socket_create_close() {
        let sock = arth_rt_socket_create(AF_INET, SOCK_STREAM, 0);
        assert!(sock > 0, "Failed to create socket");

        let rc = arth_rt_socket_close(sock);
        assert_eq!(rc, 0);
    }

    #[test]
    fn test_socket_invalid_handle() {
        let rc = arth_rt_socket_close(99999);
        assert!(rc < 0);
    }

    #[test]
    fn test_socket_set_nonblocking() {
        let sock = arth_rt_socket_create(AF_INET, SOCK_STREAM, 0);
        assert!(sock > 0);

        let rc = arth_rt_socket_set_nonblocking(sock, 1);
        assert_eq!(rc, 0);

        let rc = arth_rt_socket_set_nonblocking(sock, 0);
        assert_eq!(rc, 0);

        arth_rt_socket_close(sock);
    }

    #[test]
    fn test_getaddrinfo_localhost() {
        let host = b"localhost";
        let handle = arth_rt_getaddrinfo(host.as_ptr(), host.len(), 80);
        assert!(handle > 0, "Failed to resolve localhost");

        let mut addr: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut addr_len: libc::socklen_t =
            std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;

        let rc = arth_rt_addrinfo_next(handle, &mut addr, &mut addr_len);
        assert_eq!(rc, 0, "Failed to get first address");

        arth_rt_freeaddrinfo(handle);
    }
}
