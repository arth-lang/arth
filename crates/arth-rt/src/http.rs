//! HTTP/1.1 client implementation
//!
//! This module provides C FFI wrappers for HTTP/1.1 client operations,
//! built on top of sockets and TLS.
//!
//! Features:
//! - HTTP/1.1 request/response handling
//! - Chunked transfer encoding
//! - Keep-alive connections
//! - TLS (HTTPS) support

use crate::error::{ErrorCode, set_last_error};
use crate::new_handle;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Mutex;

// -----------------------------------------------------------------------------
// HTTP Request/Response Structures
// -----------------------------------------------------------------------------

/// HTTP request builder (reserved for future use)
#[allow(dead_code)]
struct HttpRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// HTTP response data
struct HttpResponse {
    status_code: i32,
    #[allow(dead_code)]
    status_text: String, // Kept for debugging and future status text access
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// Active HTTP connection
enum HttpConnection {
    Plain(BufReader<TcpStream>),
    #[cfg(feature = "tls")]
    Tls(BufReader<native_tls::TlsStream<TcpStream>>),
}

/// HTTP client session (supports keep-alive)
struct HttpSession {
    connection: HttpConnection,
    host: String,
    #[allow(dead_code)]
    port: u16, // Kept for potential reconnection logic
    #[allow(dead_code)]
    is_tls: bool, // Kept for potential reconnection logic
}

lazy_static::lazy_static! {
    static ref HTTP_SESSIONS: Mutex<HashMap<i64, HttpSession>> = Mutex::new(HashMap::new());
    static ref HTTP_RESPONSES: Mutex<HashMap<i64, HttpResponse>> = Mutex::new(HashMap::new());
}

// -----------------------------------------------------------------------------
// Connection Management
// -----------------------------------------------------------------------------

/// Connect to an HTTP server
///
/// # Arguments
/// * `host` - Server hostname (UTF-8)
/// * `host_len` - Length of hostname
/// * `port` - Port number (80 for HTTP, 443 for HTTPS)
/// * `use_tls` - 1 for HTTPS, 0 for HTTP
///
/// # Returns
/// * Positive session handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_http_connect(
    host: *const u8,
    host_len: usize,
    port: u16,
    use_tls: i32,
) -> i64 {
    if host.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let host_slice = unsafe { std::slice::from_raw_parts(host, host_len) };
    let host_str = match std::str::from_utf8(host_slice) {
        Ok(s) => s.to_string(),
        Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
    };

    // Connect to server
    let addr = format!("{}:{}", host_str, port);
    let tcp_stream = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => {
            set_last_error(&format!("Failed to connect: {}", e));
            return ErrorCode::ConnectionRefused.as_i32() as i64;
        }
    };

    let connection = if use_tls != 0 {
        #[cfg(feature = "tls")]
        {
            let connector = match native_tls::TlsConnector::new() {
                Ok(c) => c,
                Err(e) => {
                    set_last_error(&format!("Failed to create TLS connector: {}", e));
                    return ErrorCode::Error.as_i32() as i64;
                }
            };

            match connector.connect(&host_str, tcp_stream) {
                Ok(tls_stream) => HttpConnection::Tls(BufReader::new(tls_stream)),
                Err(e) => {
                    set_last_error(&format!("TLS handshake failed: {}", e));
                    return ErrorCode::Error.as_i32() as i64;
                }
            }
        }
        #[cfg(not(feature = "tls"))]
        {
            set_last_error("TLS support not compiled in");
            return ErrorCode::NotSupported.as_i32() as i64;
        }
    } else {
        HttpConnection::Plain(BufReader::new(tcp_stream))
    };

    let handle = new_handle();
    HTTP_SESSIONS.lock().unwrap().insert(
        handle,
        HttpSession {
            connection,
            host: host_str,
            port,
            is_tls: use_tls != 0,
        },
    );
    handle
}

/// Close an HTTP session
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_http_close(session: i64) -> i32 {
    match HTTP_SESSIONS.lock().unwrap().remove(&session) {
        Some(_) => 0,
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

// -----------------------------------------------------------------------------
// HTTP Request Execution
// -----------------------------------------------------------------------------

/// Send an HTTP request and receive the response
///
/// # Arguments
/// * `session` - HTTP session handle
/// * `method` - HTTP method (GET, POST, etc.)
/// * `method_len` - Length of method
/// * `path` - Request path (e.g., "/api/users")
/// * `path_len` - Length of path
/// * `headers` - Headers as "Key: Value\r\n" format
/// * `headers_len` - Length of headers
/// * `body` - Request body (can be NULL for GET)
/// * `body_len` - Length of body
///
/// # Returns
/// * Positive response handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_http_request(
    session: i64,
    method: *const u8,
    method_len: usize,
    path: *const u8,
    path_len: usize,
    headers: *const u8,
    headers_len: usize,
    body: *const u8,
    body_len: usize,
) -> i64 {
    // Parse parameters
    if method.is_null() || path.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let method_str = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(method, method_len)) {
            Ok(s) => s,
            Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
        }
    };

    let path_str = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(path, path_len)) {
            Ok(s) => s,
            Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
        }
    };

    let headers_str = if !headers.is_null() && headers_len > 0 {
        unsafe {
            match std::str::from_utf8(std::slice::from_raw_parts(headers, headers_len)) {
                Ok(s) => s,
                Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
            }
        }
    } else {
        ""
    };

    let body_slice = if !body.is_null() && body_len > 0 {
        unsafe { std::slice::from_raw_parts(body, body_len) }
    } else {
        &[]
    };

    // Get session
    let mut sessions = HTTP_SESSIONS.lock().unwrap();
    let session_data = match sessions.get_mut(&session) {
        Some(s) => s,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    // Build request
    let mut request = format!("{} {} HTTP/1.1\r\n", method_str, path_str);
    request.push_str(&format!("Host: {}\r\n", session_data.host));

    // Add Content-Length if body present
    if !body_slice.is_empty() {
        request.push_str(&format!("Content-Length: {}\r\n", body_slice.len()));
    }

    // Add custom headers
    if !headers_str.is_empty() {
        request.push_str(headers_str);
    }

    request.push_str("\r\n"); // End of headers

    // Send request
    let write_result = match &mut session_data.connection {
        HttpConnection::Plain(reader) => {
            let stream = reader.get_mut();
            stream
                .write_all(request.as_bytes())
                .and_then(|_| stream.write_all(body_slice))
                .and_then(|_| stream.flush())
        }
        #[cfg(feature = "tls")]
        HttpConnection::Tls(reader) => {
            let stream = reader.get_mut();
            stream
                .write_all(request.as_bytes())
                .and_then(|_| stream.write_all(body_slice))
                .and_then(|_| stream.flush())
        }
    };

    if let Err(e) = write_result {
        set_last_error(&format!("Failed to send request: {}", e));
        return ErrorCode::Error.as_i32() as i64;
    }

    // Read response
    let response = match read_http_response(&mut session_data.connection) {
        Ok(r) => r,
        Err(e) => {
            set_last_error(&format!("Failed to read response: {}", e));
            return ErrorCode::Error.as_i32() as i64;
        }
    };

    let handle = new_handle();
    HTTP_RESPONSES.lock().unwrap().insert(handle, response);
    handle
}

/// Read HTTP response from connection
fn read_http_response(conn: &mut HttpConnection) -> Result<HttpResponse, String> {
    // Read status line
    let mut status_line = String::new();
    match conn {
        HttpConnection::Plain(reader) => {
            reader
                .read_line(&mut status_line)
                .map_err(|e| e.to_string())?;
        }
        #[cfg(feature = "tls")]
        HttpConnection::Tls(reader) => {
            reader
                .read_line(&mut status_line)
                .map_err(|e| e.to_string())?;
        }
    }

    // Parse status line: "HTTP/1.1 200 OK\r\n"
    let status_line = status_line.trim();
    let parts: Vec<&str> = status_line.splitn(3, ' ').collect();
    if parts.len() < 2 {
        return Err(format!("Invalid status line: {}", status_line));
    }

    let status_code: i32 = parts[1].parse().map_err(|_| "Invalid status code")?;
    let status_text = if parts.len() > 2 {
        parts[2].to_string()
    } else {
        String::new()
    };

    // Read headers
    let mut headers = Vec::new();
    let mut content_length: Option<usize> = None;
    let mut chunked = false;

    loop {
        let mut line = String::new();
        match conn {
            HttpConnection::Plain(reader) => {
                reader.read_line(&mut line).map_err(|e| e.to_string())?;
            }
            #[cfg(feature = "tls")]
            HttpConnection::Tls(reader) => {
                reader.read_line(&mut line).map_err(|e| e.to_string())?;
            }
        }

        let line = line.trim();
        if line.is_empty() {
            break; // End of headers
        }

        if let Some(pos) = line.find(':') {
            let name = line[..pos].trim().to_string();
            let value = line[pos + 1..].trim().to_string();

            // Check for content-length and transfer-encoding
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().ok();
            } else if name.eq_ignore_ascii_case("transfer-encoding")
                && value.eq_ignore_ascii_case("chunked")
            {
                chunked = true;
            }

            headers.push((name, value));
        }
    }

    // Read body
    let body = if chunked {
        read_chunked_body(conn)?
    } else if let Some(len) = content_length {
        read_fixed_body(conn, len)?
    } else {
        // No content-length and not chunked - read until EOF or assume empty for 204/304
        if status_code == 204 || status_code == 304 {
            Vec::new()
        } else {
            // Try to read until EOF for HTTP/1.0 style
            let mut body = Vec::new();
            match conn {
                HttpConnection::Plain(reader) => {
                    let _ = reader.read_to_end(&mut body);
                }
                #[cfg(feature = "tls")]
                HttpConnection::Tls(reader) => {
                    let _ = reader.read_to_end(&mut body);
                }
            }
            body
        }
    };

    Ok(HttpResponse {
        status_code,
        status_text,
        headers,
        body,
    })
}

/// Read fixed-length body
fn read_fixed_body(conn: &mut HttpConnection, len: usize) -> Result<Vec<u8>, String> {
    let mut body = vec![0u8; len];
    match conn {
        HttpConnection::Plain(reader) => {
            reader.read_exact(&mut body).map_err(|e| e.to_string())?;
        }
        #[cfg(feature = "tls")]
        HttpConnection::Tls(reader) => {
            reader.read_exact(&mut body).map_err(|e| e.to_string())?;
        }
    }
    Ok(body)
}

/// Read chunked transfer-encoded body
fn read_chunked_body(conn: &mut HttpConnection) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();

    loop {
        // Read chunk size line
        let mut size_line = String::new();
        match conn {
            HttpConnection::Plain(reader) => {
                reader
                    .read_line(&mut size_line)
                    .map_err(|e| e.to_string())?;
            }
            #[cfg(feature = "tls")]
            HttpConnection::Tls(reader) => {
                reader
                    .read_line(&mut size_line)
                    .map_err(|e| e.to_string())?;
            }
        }

        let size_line = size_line.trim();
        let chunk_size = usize::from_str_radix(size_line, 16).map_err(|_| "Invalid chunk size")?;

        if chunk_size == 0 {
            // Final chunk - read trailing CRLF
            let mut trailing = String::new();
            match conn {
                HttpConnection::Plain(reader) => {
                    let _ = reader.read_line(&mut trailing);
                }
                #[cfg(feature = "tls")]
                HttpConnection::Tls(reader) => {
                    let _ = reader.read_line(&mut trailing);
                }
            }
            break;
        }

        // Read chunk data
        let mut chunk = vec![0u8; chunk_size];
        match conn {
            HttpConnection::Plain(reader) => {
                reader.read_exact(&mut chunk).map_err(|e| e.to_string())?;
            }
            #[cfg(feature = "tls")]
            HttpConnection::Tls(reader) => {
                reader.read_exact(&mut chunk).map_err(|e| e.to_string())?;
            }
        }
        body.extend_from_slice(&chunk);

        // Read trailing CRLF after chunk data
        let mut crlf = [0u8; 2];
        match conn {
            HttpConnection::Plain(reader) => {
                let _ = reader.read_exact(&mut crlf);
            }
            #[cfg(feature = "tls")]
            HttpConnection::Tls(reader) => {
                let _ = reader.read_exact(&mut crlf);
            }
        }
    }

    Ok(body)
}

// -----------------------------------------------------------------------------
// Response Access
// -----------------------------------------------------------------------------

/// Get HTTP response status code
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_http_response_status(response: i64) -> i32 {
    match HTTP_RESPONSES.lock().unwrap().get(&response) {
        Some(r) => r.status_code,
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Get HTTP response header count
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_http_response_header_count(response: i64) -> i32 {
    match HTTP_RESPONSES.lock().unwrap().get(&response) {
        Some(r) => r.headers.len() as i32,
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

/// Get HTTP response header name by index
///
/// # Returns
/// * Number of bytes written
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_http_response_header_name(
    response: i64,
    index: i32,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let responses = HTTP_RESPONSES.lock().unwrap();
    let resp = match responses.get(&response) {
        Some(r) => r,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    if index < 0 || (index as usize) >= resp.headers.len() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let name = &resp.headers[index as usize].0;
    if name.len() >= buf_len {
        return ErrorCode::BufferTooSmall.as_i32();
    }

    unsafe {
        std::ptr::copy_nonoverlapping(name.as_ptr(), buf, name.len());
        *buf.add(name.len()) = 0;
    }

    name.len() as i32
}

/// Get HTTP response header value by index
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_http_response_header_value(
    response: i64,
    index: i32,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let responses = HTTP_RESPONSES.lock().unwrap();
    let resp = match responses.get(&response) {
        Some(r) => r,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    if index < 0 || (index as usize) >= resp.headers.len() {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let value = &resp.headers[index as usize].1;
    if value.len() >= buf_len {
        return ErrorCode::BufferTooSmall.as_i32();
    }

    unsafe {
        std::ptr::copy_nonoverlapping(value.as_ptr(), buf, value.len());
        *buf.add(value.len()) = 0;
    }

    value.len() as i32
}

/// Get HTTP response header by name
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_http_response_header(
    response: i64,
    name: *const u8,
    name_len: usize,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    if name.is_null() || buf.is_null() || buf_len == 0 {
        return ErrorCode::InvalidArgument.as_i32();
    }

    let name_str = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(name, name_len)) {
            Ok(s) => s,
            Err(_) => return ErrorCode::InvalidArgument.as_i32(),
        }
    };

    let responses = HTTP_RESPONSES.lock().unwrap();
    let resp = match responses.get(&response) {
        Some(r) => r,
        None => return ErrorCode::InvalidHandle.as_i32(),
    };

    for (key, value) in &resp.headers {
        if key.eq_ignore_ascii_case(name_str) {
            if value.len() >= buf_len {
                return ErrorCode::BufferTooSmall.as_i32();
            }
            unsafe {
                std::ptr::copy_nonoverlapping(value.as_ptr(), buf, value.len());
                *buf.add(value.len()) = 0;
            }
            return value.len() as i32;
        }
    }

    ErrorCode::NotFound.as_i32()
}

/// Get HTTP response body length
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_http_response_body_len(response: i64) -> i64 {
    match HTTP_RESPONSES.lock().unwrap().get(&response) {
        Some(r) => r.body.len() as i64,
        None => ErrorCode::InvalidHandle.as_i32() as i64,
    }
}

/// Get HTTP response body
///
/// # Returns
/// * Number of bytes written
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_http_response_body(response: i64, buf: *mut u8, buf_len: usize) -> i64 {
    if buf.is_null() && buf_len > 0 {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let responses = HTTP_RESPONSES.lock().unwrap();
    let resp = match responses.get(&response) {
        Some(r) => r,
        None => return ErrorCode::InvalidHandle.as_i32() as i64,
    };

    let copy_len = std::cmp::min(buf_len, resp.body.len());
    if copy_len > 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(resp.body.as_ptr(), buf, copy_len);
        }
    }

    copy_len as i64
}

/// Free HTTP response
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_http_response_free(response: i64) -> i32 {
    match HTTP_RESPONSES.lock().unwrap().remove(&response) {
        Some(_) => 0,
        None => ErrorCode::InvalidHandle.as_i32(),
    }
}

// -----------------------------------------------------------------------------
// Simple Request Functions
// -----------------------------------------------------------------------------

/// Perform a simple GET request
///
/// This is a convenience function that connects, sends a GET request,
/// and returns the response handle. The caller must free the response.
///
/// # Returns
/// * Positive response handle on success
/// * Negative error code on failure
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_http_get(
    url: *const u8,
    url_len: usize,
    headers: *const u8,
    headers_len: usize,
) -> i64 {
    if url.is_null() {
        return ErrorCode::InvalidArgument.as_i32() as i64;
    }

    let url_str = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(url, url_len)) {
            Ok(s) => s,
            Err(_) => return ErrorCode::InvalidArgument.as_i32() as i64,
        }
    };

    // Parse URL
    let (host, port, path, use_tls) = match parse_url(url_str) {
        Ok(p) => p,
        Err(e) => {
            set_last_error(&e);
            return ErrorCode::InvalidArgument.as_i32() as i64;
        }
    };

    // Connect
    let session =
        arth_rt_http_connect(host.as_ptr(), host.len(), port, if use_tls { 1 } else { 0 });
    if session < 0 {
        return session;
    }

    // Send request
    let method = b"GET";
    let response = arth_rt_http_request(
        session,
        method.as_ptr(),
        method.len(),
        path.as_ptr(),
        path.len(),
        headers,
        headers_len,
        std::ptr::null(),
        0,
    );

    // Close session (we could keep it for connection reuse, but for simplicity close it)
    arth_rt_http_close(session);

    response
}

/// Parse URL into components
fn parse_url(url: &str) -> Result<(String, u16, String, bool), String> {
    let (scheme, rest) = if url.starts_with("https://") {
        ("https", &url[8..])
    } else if url.starts_with("http://") {
        ("http", &url[7..])
    } else {
        return Err("URL must start with http:// or https://".to_string());
    };

    let use_tls = scheme == "https";
    let default_port: u16 = if use_tls { 443 } else { 80 };

    // Split host and path
    let (host_port, path) = match rest.find('/') {
        Some(pos) => (&rest[..pos], &rest[pos..]),
        None => (rest, "/"),
    };

    // Split host and port
    let (host, port) = match host_port.rfind(':') {
        Some(pos) => {
            let port_str = &host_port[pos + 1..];
            let port: u16 = port_str.parse().map_err(|_| "Invalid port")?;
            (&host_port[..pos], port)
        }
        None => (host_port, default_port),
    };

    Ok((host.to_string(), port, path.to_string(), use_tls))
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_url_http() {
        let (host, port, path, use_tls) = parse_url("http://example.com/test").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/test");
        assert!(!use_tls);
    }

    #[test]
    fn test_parse_url_https() {
        let (host, port, path, use_tls) = parse_url("https://example.com/api/v1").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
        assert_eq!(path, "/api/v1");
        assert!(use_tls);
    }

    #[test]
    fn test_parse_url_custom_port() {
        let (host, port, path, use_tls) = parse_url("http://localhost:8080/").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
        assert_eq!(path, "/");
        assert!(!use_tls);
    }

    #[test]
    fn test_parse_url_no_path() {
        let (host, port, path, use_tls) = parse_url("https://api.example.com").unwrap();
        assert_eq!(host, "api.example.com");
        assert_eq!(port, 443);
        assert_eq!(path, "/");
        assert!(use_tls);
    }

    #[test]
    fn test_session_invalid_handle() {
        let rc = arth_rt_http_close(99999);
        assert!(rc < 0);
    }

    #[test]
    fn test_response_invalid_handle() {
        let rc = arth_rt_http_response_free(99999);
        assert!(rc < 0);
    }

    #[test]
    fn test_response_status_invalid() {
        let status = arth_rt_http_response_status(99999);
        assert!(status < 0);
    }
}
