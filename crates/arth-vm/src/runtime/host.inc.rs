// Host Call Dispatch Functions
// ============================================================================

/// Dispatch a HostIoOp call using the provided HostContext.
///
/// Stack effects vary by operation; see HostIoOp documentation.
fn dispatch_host_io(
    op: HostIoOp,
    stack: &mut Vec<Value>,
    strings: &[String],
    ctx: &HostContext,
) {
    match op {
        HostIoOp::FileOpen => {
            // Stack: [path_idx, mode] -> [handle]
            let mode = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(n),
                _ => None,
            }).unwrap_or(0);
            let path_idx = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(n as usize),
                _ => None,
            }).unwrap_or(0);
            let path = strings.get(path_idx).map(|s| s.as_str()).unwrap_or("");
            let file_mode = match mode {
                0 => FileMode::Read,
                1 => FileMode::Write,
                2 => FileMode::Append,
                3 => FileMode::ReadWrite,
                _ => FileMode::Read,
            };
            match ctx.io.file_open(path, file_mode) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostIoOp::FileClose => {
            // Stack: [handle] -> [result]
            let handle = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(crate::host::FileHandle(n)),
                _ => None,
            });
            if let Some(h) = handle {
                match ctx.io.file_close(h) {
                    Ok(()) => stack.push(Value::I64(0)),
                    Err(_) => stack.push(Value::I64(-1)),
                }
            } else {
                stack.push(Value::I64(-1));
            }
        }
        HostIoOp::FileRead => {
            // Stack: [handle, max_bytes] -> [string]
            let max_bytes = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(n as usize),
                _ => None,
            }).unwrap_or(4096);
            let handle = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(crate::host::FileHandle(n)),
                _ => None,
            });
            if let Some(h) = handle {
                match ctx.io.file_read(h, max_bytes) {
                    Ok(data) => {
                        let s = String::from_utf8_lossy(&data).to_string();
                        stack.push(Value::Str(s));
                    }
                    Err(_) => stack.push(Value::Str(String::new())),
                }
            } else {
                stack.push(Value::Str(String::new()));
            }
        }
        HostIoOp::FileWrite => {
            // Stack: [handle, data_bytes_list] -> [bytes_written]
            // For simplicity, we expect data as a string on stack
            let data = stack.pop().and_then(|v| match v {
                Value::Str(s) => Some(s),
                Value::I64(n) => Some(n.to_string()),
                _ => None,
            }).unwrap_or_default();
            let handle = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(crate::host::FileHandle(n)),
                _ => None,
            });
            if let Some(h) = handle {
                match ctx.io.file_write(h, data.as_bytes()) {
                    Ok(n) => stack.push(Value::I64(n as i64)),
                    Err(_) => stack.push(Value::I64(-1)),
                }
            } else {
                stack.push(Value::I64(-1));
            }
        }
        HostIoOp::FileWriteStr => {
            // Stack: [handle, str_idx] -> [bytes_written]
            let str_idx = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(n as usize),
                Value::Str(s) => {
                    // If it's already a string, use it directly
                    stack.push(Value::Str(s));
                    return None;
                }
                _ => None,
            });
            // Check if we pushed back a string
            if let Some(Value::Str(s)) = stack.last() {
                let data = s.clone();
                stack.pop();
                let handle = stack.pop().and_then(|v| match v {
                    Value::I64(n) => Some(crate::host::FileHandle(n)),
                    _ => None,
                });
                if let Some(h) = handle {
                    match ctx.io.file_write(h, data.as_bytes()) {
                        Ok(n) => stack.push(Value::I64(n as i64)),
                        Err(_) => stack.push(Value::I64(-1)),
                    }
                } else {
                    stack.push(Value::I64(-1));
                }
            } else if let Some(idx) = str_idx {
                let data = strings.get(idx).map(|s| s.as_str()).unwrap_or("");
                let handle = stack.pop().and_then(|v| match v {
                    Value::I64(n) => Some(crate::host::FileHandle(n)),
                    _ => None,
                });
                if let Some(h) = handle {
                    match ctx.io.file_write(h, data.as_bytes()) {
                        Ok(n) => stack.push(Value::I64(n as i64)),
                        Err(_) => stack.push(Value::I64(-1)),
                    }
                } else {
                    stack.push(Value::I64(-1));
                }
            } else {
                stack.push(Value::I64(-1));
            }
        }
        HostIoOp::FileFlush => {
            // Stack: [handle] -> [result]
            let handle = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(crate::host::FileHandle(n)),
                _ => None,
            });
            if let Some(h) = handle {
                match ctx.io.file_flush(h) {
                    Ok(()) => stack.push(Value::I64(0)),
                    Err(_) => stack.push(Value::I64(-1)),
                }
            } else {
                stack.push(Value::I64(-1));
            }
        }
        HostIoOp::FileSeek => {
            // Stack: [handle, offset, whence] -> [new_position]
            let whence = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(n),
                _ => None,
            }).unwrap_or(0);
            let offset = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(n),
                _ => None,
            }).unwrap_or(0);
            let handle = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(crate::host::FileHandle(n)),
                _ => None,
            });
            if let Some(h) = handle {
                let seek_pos = match whence {
                    0 => SeekPosition::Start(offset.max(0) as u64),
                    1 => SeekPosition::Current(offset),
                    2 => SeekPosition::End(offset),
                    _ => SeekPosition::Start(offset.max(0) as u64),
                };
                match ctx.io.file_seek(h, seek_pos) {
                    Ok(new_pos) => stack.push(Value::I64(new_pos)),
                    Err(_) => stack.push(Value::I64(-1)),
                }
            } else {
                stack.push(Value::I64(-1));
            }
        }
        HostIoOp::FileSize => {
            // Stack: [handle] -> [size]
            let handle = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(crate::host::FileHandle(n)),
                _ => None,
            });
            if let Some(h) = handle {
                match ctx.io.file_size(h) {
                    Ok(size) => stack.push(Value::I64(size)),
                    Err(_) => stack.push(Value::I64(-1)),
                }
            } else {
                stack.push(Value::I64(-1));
            }
        }
        HostIoOp::FileExists => {
            // Stack: [path_idx] -> [exists]
            let path = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            match ctx.io.file_exists(&path) {
                Ok(exists) => stack.push(Value::I64(if exists { 1 } else { 0 })),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostIoOp::FileDelete => {
            // Stack: [path_idx] -> [result]
            let path = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            match ctx.io.file_delete(&path) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostIoOp::FileCopy => {
            // Stack: [src_path_idx, dst_path_idx] -> [result]
            let dst = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            let src = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            match ctx.io.file_copy(&src, &dst) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostIoOp::FileMove => {
            // Stack: [src_path_idx, dst_path_idx] -> [result]
            let dst = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            let src = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            match ctx.io.file_move(&src, &dst) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostIoOp::DirCreate => {
            // Stack: [path_idx] -> [result]
            let path = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            match ctx.io.dir_create(&path) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostIoOp::DirCreateAll => {
            // Stack: [path_idx] -> [result]
            let path = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            match ctx.io.dir_create_all(&path) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostIoOp::DirDelete => {
            // Stack: [path_idx] -> [result]
            let path = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            match ctx.io.dir_delete(&path) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostIoOp::DirList => {
            // Stack: [path_idx] -> [list_handle]
            // Returns a list handle containing directory entries
            let path = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            match ctx.io.dir_list(&path) {
                Ok(entries) => {
                    // Create a list and push entries
                    let h = list_new();
                    for entry in entries {
                        list_push(h, Value::Str(entry));
                    }
                    stack.push(Value::I64(h));
                }
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostIoOp::DirExists => {
            // Stack: [path_idx] -> [exists]
            let path = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            match ctx.io.dir_exists(&path) {
                Ok(exists) => stack.push(Value::I64(if exists { 1 } else { 0 })),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostIoOp::IsDir => {
            // Stack: [path_idx] -> [is_dir]
            let path = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            match ctx.io.is_dir(&path) {
                Ok(is_dir) => stack.push(Value::I64(if is_dir { 1 } else { 0 })),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostIoOp::IsFile => {
            // Stack: [path_idx] -> [is_file]
            let path = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            match ctx.io.is_file(&path) {
                Ok(is_file) => stack.push(Value::I64(if is_file { 1 } else { 0 })),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostIoOp::PathAbsolute => {
            // Stack: [path_idx] -> [absolute_path_str]
            let path = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            match ctx.io.path_absolute(&path) {
                Ok(abs) => stack.push(Value::Str(abs)),
                Err(_) => stack.push(Value::Str(path)),
            }
        }
        HostIoOp::ConsoleReadLine => {
            // Stack: [] -> [line_str]
            match ctx.io.console_read_line() {
                Ok(line) => stack.push(Value::Str(line)),
                Err(_) => stack.push(Value::Str(String::new())),
            }
        }
        HostIoOp::ConsoleWrite => {
            // Stack: [str] -> []
            let s = stack.pop().and_then(|v| match v {
                Value::Str(s) => Some(s),
                Value::I64(n) => Some(n.to_string()),
                Value::F64(f) => Some(f.to_string()),
                Value::Bool(b) => Some(if b { "true" } else { "false" }.to_string()),
            }).unwrap_or_default();
            let _ = ctx.io.console_write(&s);
        }
        HostIoOp::ConsoleWriteErr => {
            // Stack: [str] -> []
            let s = stack.pop().and_then(|v| match v {
                Value::Str(s) => Some(s),
                Value::I64(n) => Some(n.to_string()),
                Value::F64(f) => Some(f.to_string()),
                Value::Bool(b) => Some(if b { "true" } else { "false" }.to_string()),
            }).unwrap_or_default();
            let _ = ctx.io.console_write_err(&s);
        }
    }
}

/// Dispatch a HostTimeOp call using the provided HostContext.
fn dispatch_host_time(
    op: HostTimeOp,
    stack: &mut Vec<Value>,
    strings: &[String],
    ctx: &HostContext,
) {
    match op {
        HostTimeOp::DateTimeNow => {
            // Stack: [] -> [millis]
            let millis = ctx.time.now_realtime();
            stack.push(Value::I64(millis));
        }
        HostTimeOp::DateTimeParse => {
            // Stack: [format_idx, input_str] -> [millis]
            let input = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            let format = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            match ctx.time.parse(&format, &input) {
                Ok(millis) => stack.push(Value::I64(millis)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostTimeOp::DateTimeFormat => {
            // Stack: [millis, format_idx] -> [formatted_str]
            let format = stack.pop().and_then(|v| match v {
                Value::I64(n) => strings.get(n as usize).cloned(),
                Value::Str(s) => Some(s),
                _ => None,
            }).unwrap_or_default();
            let millis = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(n),
                _ => None,
            }).unwrap_or(0);
            match ctx.time.format(millis, &format) {
                Ok(formatted) => stack.push(Value::Str(formatted)),
                Err(_) => stack.push(Value::Str(String::new())),
            }
        }
        HostTimeOp::InstantNow => {
            // Stack: [] -> [handle]
            let handle = ctx.time.instant_now();
            stack.push(Value::I64(handle.0));
        }
        HostTimeOp::InstantElapsed => {
            // Stack: [handle] -> [elapsed_millis]
            let handle = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(InstantHandle(n)),
                _ => None,
            });
            if let Some(h) = handle {
                match ctx.time.instant_elapsed(h) {
                    Ok(elapsed) => stack.push(Value::I64(elapsed)),
                    Err(_) => stack.push(Value::I64(0)),
                }
            } else {
                stack.push(Value::I64(0));
            }
        }
        HostTimeOp::Sleep => {
            // Stack: [millis] -> []
            let millis = stack.pop().and_then(|v| match v {
                Value::I64(n) => Some(n),
                _ => None,
            }).unwrap_or(0);
            ctx.time.sleep(millis);
        }
    }
}

/// Dispatch a HostNetOp call using the provided HostContext.
///
/// Network operations dispatch to FFI functions for HTTP, WebSocket, and SSE.
fn dispatch_host_net(
    op: HostNetOp,
    stack: &mut Vec<Value>,
    strings: &[String],
    _ctx: &HostContext,
) {
    match op {
        HostNetOp::HttpFetch => {
            // Stack: [url_idx] -> [response_handle or -1]
            // Performs HTTP GET request and returns response handle
            let url_idx = match stack.pop() {
                Some(Value::I64(n)) => n as usize,
                Some(Value::Str(s)) => {
                    // Direct URL string - make the request
                    let handle = __arth_http_fetch_url(s.as_ptr(), s.len() as i64, 30000);
                    stack.push(Value::I64(handle));
                    return;
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let url = strings.get(url_idx).map(|s| s.as_str()).unwrap_or("");
            let handle = __arth_http_fetch_url(url.as_ptr(), url.len() as i64, 30000);
            stack.push(Value::I64(handle));
        }
        HostNetOp::HttpServe => {
            // Stack: [port] -> [server_handle or -1]
            let port = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let handle = __arth_http_server_create(port);
            stack.push(Value::I64(handle));
        }
        HostNetOp::HttpAccept => {
            // Stack: [server_handle] -> [request_handle or -1]
            let handle = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let conn = __arth_http_server_accept(handle);
            stack.push(Value::I64(conn));
        }
        HostNetOp::HttpRespond => {
            // Stack: [request_handle, status, body_idx] -> []
            let _ = stack.pop(); // body
            let _ = stack.pop(); // status
            let _ = stack.pop(); // request handle
            // Response handled via HttpWriter ops
        }
        HostNetOp::WsServe => {
            // Stack: [port, path_idx] -> [server_handle or -1]
            let path_idx = match stack.pop() {
                Some(Value::I64(n)) => n as usize,
                _ => 0,
            };
            let port = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let path = strings.get(path_idx).map(|s| s.as_str()).unwrap_or("/");
            let handle = __arth_ws_serve(port, path.as_ptr(), path.len() as i64);
            stack.push(Value::I64(handle));
        }
        HostNetOp::WsAccept => {
            // Stack: [server_handle] -> [connection_handle or -1]
            let server = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let conn = __arth_ws_accept(server);
            stack.push(Value::I64(conn));
        }
        HostNetOp::WsSendText => {
            // Stack: [conn_handle, message_idx] -> [status]
            let msg_idx = match stack.pop() {
                Some(Value::I64(n)) => n as usize,
                Some(Value::Str(s)) => {
                    // Direct string value
                    let conn = match stack.pop() {
                        Some(Value::I64(h)) => h,
                        _ => -1,
                    };
                    let status = __arth_ws_send_text(conn, s.as_ptr(), s.len() as i64);
                    stack.push(Value::I64(status));
                    return;
                }
                _ => 0,
            };
            let conn = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let msg = strings.get(msg_idx).map(|s| s.as_str()).unwrap_or("");
            let status = __arth_ws_send_text(conn, msg.as_ptr(), msg.len() as i64);
            stack.push(Value::I64(status));
        }
        HostNetOp::WsSendBinary => {
            // Stack: [conn_handle, data_handle] -> [status]
            let _ = stack.pop(); // data handle
            let conn = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let status = __arth_ws_send_binary(conn, std::ptr::null(), 0);
            stack.push(Value::I64(status));
        }
        HostNetOp::WsRecv => {
            // Stack: [conn_handle] -> [message_type or -1]
            let conn = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let msg_type = __arth_ws_recv(conn);
            stack.push(Value::I64(msg_type));
        }
        HostNetOp::WsClose => {
            // Stack: [conn_handle, code, reason_idx] -> [status]
            let reason_idx = match stack.pop() {
                Some(Value::I64(n)) => n as usize,
                _ => 0,
            };
            let code = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => 1000,
            };
            let conn = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let reason = strings.get(reason_idx).map(|s| s.as_str()).unwrap_or("");
            let status = __arth_ws_close(conn, code, reason.as_ptr(), reason.len() as i64);
            stack.push(Value::I64(status));
        }
        HostNetOp::WsIsOpen => {
            // Stack: [conn_handle] -> [0 or 1]
            let conn = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let is_open = __arth_ws_is_open(conn);
            stack.push(Value::I64(is_open));
        }
        HostNetOp::SseServe => {
            // Stack: [port, path_idx] -> [server_handle or -1]
            let path_idx = match stack.pop() {
                Some(Value::I64(n)) => n as usize,
                _ => 0,
            };
            let port = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let path = strings.get(path_idx).map(|s| s.as_str()).unwrap_or("/");
            let handle = __arth_sse_serve(port, path.as_ptr(), path.len() as i64);
            stack.push(Value::I64(handle));
        }
        HostNetOp::SseAccept => {
            // Stack: [server_handle] -> [emitter_handle or -1]
            let server = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let emitter = __arth_sse_accept(server);
            stack.push(Value::I64(emitter));
        }
        HostNetOp::SseSend => {
            // Stack: [emitter_handle, event_idx, data_idx, id_idx] -> [status]
            let id_idx = match stack.pop() {
                Some(Value::I64(n)) => n as usize,
                _ => 0,
            };
            let data_idx = match stack.pop() {
                Some(Value::I64(n)) => n as usize,
                _ => 0,
            };
            let event_idx = match stack.pop() {
                Some(Value::I64(n)) => n as usize,
                _ => 0,
            };
            let emitter = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let event_type = strings.get(event_idx).map(|s| s.as_str()).unwrap_or("");
            let data = strings.get(data_idx).map(|s| s.as_str()).unwrap_or("");
            let id = strings.get(id_idx).map(|s| s.as_str()).unwrap_or("");
            let status = __arth_sse_send(
                emitter,
                event_type.as_ptr(), event_type.len() as i64,
                data.as_ptr(), data.len() as i64,
                id.as_ptr(), id.len() as i64,
            );
            stack.push(Value::I64(status));
        }
        HostNetOp::SseClose => {
            // Stack: [emitter_handle] -> [status]
            let emitter = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let status = __arth_sse_close(emitter);
            stack.push(Value::I64(status));
        }
        HostNetOp::SseIsOpen => {
            // Stack: [emitter_handle] -> [0 or 1]
            let emitter = match stack.pop() {
                Some(Value::I64(n)) => n,
                _ => -1,
            };
            let is_open = __arth_sse_is_open(emitter);
            stack.push(Value::I64(is_open));
        }
    }
}

/// Dispatch a HostDbOp call using the provided HostContext.
///
/// Stack effects vary by operation; see HostDbOp documentation.
fn dispatch_host_db(
    op: HostDbOp,
    stack: &mut Vec<Value>,
    strings: &[String],
    ctx: &HostContext,
) {
    use crate::host::{SqliteConnectionHandle, SqliteStatementHandle};

    match op {
        HostDbOp::SqliteOpen => {
            // Stack: [path_str or path_idx] -> [conn_handle or -1]
            let path = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_open(&path) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteClose => {
            // Stack: [conn_handle] -> [0 or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_close(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqlitePrepare => {
            // Stack: [conn_handle, sql_str or sql_idx] -> [stmt_handle or -1]
            let sql = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_prepare(conn_handle, &sql) {
                Ok(stmt) => stack.push(Value::I64(stmt.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteStep => {
            // Stack: [stmt_handle] -> [has_row: 1 or 0, or -1 on error]
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_step(stmt_handle) {
                Ok(has_row) => stack.push(Value::I64(if has_row { 1 } else { 0 })),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteFinalize => {
            // Stack: [stmt_handle] -> [0 or -1]
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_finalize(stmt_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteReset => {
            // Stack: [stmt_handle] -> [0 or -1]
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_reset(stmt_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteBindInt => {
            // Stack: [stmt_handle, param_idx, value] -> [0 or -1]
            let value = match stack.pop() {
                Some(Value::I64(v)) => v as i32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let param_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_bind_int(stmt_handle, param_idx, value) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteBindInt64 => {
            // Stack: [stmt_handle, param_idx, value] -> [0 or -1]
            let value = match stack.pop() {
                Some(Value::I64(v)) => v,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let param_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_bind_int64(stmt_handle, param_idx, value) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteBindDouble => {
            // Stack: [stmt_handle, param_idx, value] -> [0 or -1]
            let value = match stack.pop() {
                Some(Value::F64(v)) => v,
                Some(Value::I64(v)) => v as f64,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let param_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_bind_double(stmt_handle, param_idx, value) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteBindText => {
            // Stack: [stmt_handle, param_idx, text_str or text_idx] -> [0 or -1]
            let text = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let param_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_bind_text(stmt_handle, param_idx, &text) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteBindBlob => {
            // Stack: [stmt_handle, param_idx, blob_data_str] -> [0 or -1]
            // Blob is passed as a string with raw bytes
            let blob = match stack.pop() {
                Some(Value::Str(s)) => s.into_bytes(),
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).map(|s| s.as_bytes().to_vec()).unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let param_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_bind_blob(stmt_handle, param_idx, &blob) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteBindNull => {
            // Stack: [stmt_handle, param_idx] -> [0 or -1]
            let param_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_bind_null(stmt_handle, param_idx) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteColumnInt => {
            // Stack: [stmt_handle, col_idx] -> [value]
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            match ctx.db.sqlite_column_int(stmt_handle, col_idx) {
                Ok(value) => stack.push(Value::I64(value as i64)),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostDbOp::SqliteColumnInt64 => {
            // Stack: [stmt_handle, col_idx] -> [value]
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            match ctx.db.sqlite_column_int64(stmt_handle, col_idx) {
                Ok(value) => stack.push(Value::I64(value)),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostDbOp::SqliteColumnDouble => {
            // Stack: [stmt_handle, col_idx] -> [value]
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::F64(0.0));
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::F64(0.0));
                    return;
                }
            };
            match ctx.db.sqlite_column_double(stmt_handle, col_idx) {
                Ok(value) => stack.push(Value::F64(value)),
                Err(_) => stack.push(Value::F64(0.0)),
            }
        }
        HostDbOp::SqliteColumnText => {
            // Stack: [stmt_handle, col_idx] -> [text_str]
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            match ctx.db.sqlite_column_text(stmt_handle, col_idx) {
                Ok(value) => stack.push(Value::Str(value)),
                Err(_) => stack.push(Value::Str(String::new())),
            }
        }
        HostDbOp::SqliteColumnBlob => {
            // Stack: [stmt_handle, col_idx] -> [blob_str]
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            match ctx.db.sqlite_column_blob(stmt_handle, col_idx) {
                Ok(blob) => {
                    // Convert blob to string (lossy for non-UTF8)
                    let text = String::from_utf8_lossy(&blob).to_string();
                    stack.push(Value::Str(text));
                }
                Err(_) => stack.push(Value::Str(String::new())),
            }
        }
        HostDbOp::SqliteColumnType => {
            // Stack: [stmt_handle, col_idx] -> [type_code]
            // 1=INTEGER, 2=REAL, 3=TEXT, 4=BLOB, 5=NULL
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(5)); // NULL
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(5));
                    return;
                }
            };
            match ctx.db.sqlite_column_type(stmt_handle, col_idx) {
                Ok(type_code) => stack.push(Value::I64(type_code as i64)),
                Err(_) => stack.push(Value::I64(5)), // NULL on error
            }
        }
        HostDbOp::SqliteColumnCount => {
            // Stack: [stmt_handle] -> [count]
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            match ctx.db.sqlite_column_count(stmt_handle) {
                Ok(count) => stack.push(Value::I64(count as i64)),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostDbOp::SqliteColumnName => {
            // Stack: [stmt_handle, col_idx] -> [name_str]
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            match ctx.db.sqlite_column_name(stmt_handle, col_idx) {
                Ok(name) => stack.push(Value::Str(name)),
                Err(_) => stack.push(Value::Str(String::new())),
            }
        }
        HostDbOp::SqliteIsNull => {
            // Stack: [stmt_handle, col_idx] -> [1 if null, 0 otherwise]
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(1)); // Treat as null on error
                    return;
                }
            };
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteStatementHandle(h),
                _ => {
                    stack.push(Value::I64(1));
                    return;
                }
            };
            match ctx.db.sqlite_is_null(stmt_handle, col_idx) {
                Ok(is_null) => stack.push(Value::I64(if is_null { 1 } else { 0 })),
                Err(_) => stack.push(Value::I64(1)),
            }
        }
        HostDbOp::SqliteChanges => {
            // Stack: [conn_handle] -> [changes_count]
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            match ctx.db.sqlite_changes(conn_handle) {
                Ok(changes) => stack.push(Value::I64(changes as i64)),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostDbOp::SqliteLastInsertRowid => {
            // Stack: [conn_handle] -> [rowid]
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            match ctx.db.sqlite_last_insert_rowid(conn_handle) {
                Ok(rowid) => stack.push(Value::I64(rowid)),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostDbOp::SqliteErrmsg => {
            // Stack: [conn_handle] -> [error_msg_str]
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            match ctx.db.sqlite_errmsg(conn_handle) {
                Ok(msg) => stack.push(Value::Str(msg)),
                Err(_) => stack.push(Value::Str(String::new())),
            }
        }
        HostDbOp::SqliteBegin => {
            // Stack: [conn_handle] -> [0 or -1]
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_begin(conn_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteCommit => {
            // Stack: [conn_handle] -> [0 or -1]
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_commit(conn_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteRollback => {
            // Stack: [conn_handle] -> [0 or -1]
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_rollback(conn_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteSavepoint => {
            // Stack: [conn_handle, name_str or name_idx] -> [0 or -1]
            let name = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_savepoint(conn_handle, &name) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteReleaseSavepoint => {
            // Stack: [conn_handle, name_str or name_idx] -> [0 or -1]
            let name = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_release_savepoint(conn_handle, &name) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteRollbackToSavepoint => {
            // Stack: [conn_handle, name_str or name_idx] -> [0 or -1]
            let name = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_rollback_to_savepoint(conn_handle, &name) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteQuery => {
            // Stack: [conn_handle, sql_str or sql_idx] -> [stmt_handle or -1]
            let sql = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_query(conn_handle, &sql) {
                Ok(stmt) => stack.push(Value::I64(stmt.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::SqliteExecute => {
            // Stack: [conn_handle, sql_str or sql_idx] -> [0 or -1]
            let sql = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_execute(conn_handle, &sql) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        // =====================================================================
        // PostgreSQL Operations
        // =====================================================================

        HostDbOp::PgConnect => {
            // Stack: [conn_str or conn_idx] -> [conn_handle or -1]
            let conn_str = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_connect(&conn_str) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::PgDisconnect => {
            // Stack: [conn_handle] -> [0 or -1]
            use crate::host::PgConnectionHandle;
            let handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_disconnect(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::PgStatus => {
            // Stack: [conn_handle] -> [1 if ok, 0 if not]
            use crate::host::PgConnectionHandle;
            let handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            match ctx.db.pg_status(handle) {
                Ok(ok) => stack.push(Value::I64(if ok { 1 } else { 0 })),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostDbOp::PgQuery => {
            // Stack: [conn_handle, sql_str or sql_idx] -> [result_handle or -1]
            use crate::host::PgConnectionHandle;
            let sql = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_query(conn_handle, &sql, &[]) {
                Ok(result) => stack.push(Value::I64(result.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::PgExecute => {
            // Stack: [conn_handle, sql_str or sql_idx] -> [affected_rows or -1]
            use crate::host::PgConnectionHandle;
            let sql = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_execute(conn_handle, &sql, &[]) {
                Ok(affected) => stack.push(Value::I64(affected as i64)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::PgPrepare => {
            // Stack: [conn_handle, name_str, sql_str] -> [stmt_handle or -1]
            use crate::host::PgConnectionHandle;
            let sql = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let name = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_prepare(conn_handle, &name, &sql) {
                Ok(stmt) => stack.push(Value::I64(stmt.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::PgExecutePrepared => {
            // Stack: [conn_handle, stmt_handle] -> [result_handle or -1]
            // Note: params must be bound separately or passed differently
            use crate::host::{PgConnectionHandle, PgStatementHandle};
            let stmt_handle = match stack.pop() {
                Some(Value::I64(h)) => PgStatementHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_execute_prepared(conn_handle, stmt_handle, &[]) {
                Ok(result) => stack.push(Value::I64(result.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::PgRowCount => {
            // Stack: [result_handle] -> [row_count]
            use crate::host::PgResultHandle;
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            match ctx.db.pg_row_count(result_handle) {
                Ok(count) => stack.push(Value::I64(count as i64)),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostDbOp::PgColumnCount => {
            // Stack: [result_handle] -> [column_count]
            use crate::host::PgResultHandle;
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            match ctx.db.pg_column_count(result_handle) {
                Ok(count) => stack.push(Value::I64(count as i64)),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostDbOp::PgColumnName => {
            // Stack: [result_handle, col_idx] -> [name_str]
            use crate::host::PgResultHandle;
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            match ctx.db.pg_column_name(result_handle, col_idx) {
                Ok(name) => stack.push(Value::Str(name)),
                Err(_) => stack.push(Value::Str(String::new())),
            }
        }
        HostDbOp::PgColumnType => {
            // Stack: [result_handle, col_idx] -> [type_oid]
            use crate::host::PgResultHandle;
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            match ctx.db.pg_column_type(result_handle, col_idx) {
                Ok(oid) => stack.push(Value::I64(oid as i64)),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostDbOp::PgGetValue => {
            // Stack: [result_handle, row_idx, col_idx] -> [value_str]
            use crate::host::PgResultHandle;
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            let row_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx,
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            match ctx.db.pg_get_value(result_handle, row_idx, col_idx) {
                Ok(val) => stack.push(Value::Str(val)),
                Err(_) => stack.push(Value::Str(String::new())),
            }
        }
        HostDbOp::PgGetInt => {
            // Stack: [result_handle, row_idx, col_idx] -> [value]
            use crate::host::PgResultHandle;
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            let row_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx,
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            match ctx.db.pg_get_int(result_handle, row_idx, col_idx) {
                Ok(val) => stack.push(Value::I64(val as i64)),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostDbOp::PgGetInt64 => {
            // Stack: [result_handle, row_idx, col_idx] -> [value]
            use crate::host::PgResultHandle;
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            let row_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx,
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            match ctx.db.pg_get_int64(result_handle, row_idx, col_idx) {
                Ok(val) => stack.push(Value::I64(val)),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostDbOp::PgGetDouble => {
            // Stack: [result_handle, row_idx, col_idx] -> [value]
            use crate::host::PgResultHandle;
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::F64(0.0));
                    return;
                }
            };
            let row_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx,
                _ => {
                    stack.push(Value::F64(0.0));
                    return;
                }
            };
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::F64(0.0));
                    return;
                }
            };
            match ctx.db.pg_get_double(result_handle, row_idx, col_idx) {
                Ok(val) => stack.push(Value::F64(val)),
                Err(_) => stack.push(Value::F64(0.0)),
            }
        }
        HostDbOp::PgGetText => {
            // Stack: [result_handle, row_idx, col_idx] -> [text_str]
            use crate::host::PgResultHandle;
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            let row_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx,
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            match ctx.db.pg_get_text(result_handle, row_idx, col_idx) {
                Ok(val) => stack.push(Value::Str(val)),
                Err(_) => stack.push(Value::Str(String::new())),
            }
        }
        HostDbOp::PgGetBytes => {
            // Stack: [result_handle, row_idx, col_idx] -> [bytes_str]
            use crate::host::PgResultHandle;
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            let row_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx,
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            match ctx.db.pg_get_bytes(result_handle, row_idx, col_idx) {
                Ok(bytes) => {
                    // Convert bytes to string (lossy for non-UTF8)
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    stack.push(Value::Str(text));
                }
                Err(_) => stack.push(Value::Str(String::new())),
            }
        }
        HostDbOp::PgGetBool => {
            // Stack: [result_handle, row_idx, col_idx] -> [1 or 0]
            use crate::host::PgResultHandle;
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            let row_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx,
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            match ctx.db.pg_get_bool(result_handle, row_idx, col_idx) {
                Ok(val) => stack.push(Value::I64(if val { 1 } else { 0 })),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostDbOp::PgIsNull => {
            // Stack: [result_handle, row_idx, col_idx] -> [1 if null, 0 otherwise]
            use crate::host::PgResultHandle;
            let col_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx as i32,
                _ => {
                    stack.push(Value::I64(1));
                    return;
                }
            };
            let row_idx = match stack.pop() {
                Some(Value::I64(idx)) => idx,
                _ => {
                    stack.push(Value::I64(1));
                    return;
                }
            };
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::I64(1));
                    return;
                }
            };
            match ctx.db.pg_is_null(result_handle, row_idx, col_idx) {
                Ok(is_null) => stack.push(Value::I64(if is_null { 1 } else { 0 })),
                Err(_) => stack.push(Value::I64(1)),
            }
        }
        HostDbOp::PgAffectedRows => {
            // Stack: [result_handle] -> [affected_rows]
            use crate::host::PgResultHandle;
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            match ctx.db.pg_affected_rows(result_handle) {
                Ok(count) => stack.push(Value::I64(count as i64)),
                Err(_) => stack.push(Value::I64(0)),
            }
        }
        HostDbOp::PgBegin => {
            // Stack: [conn_handle] -> [0 or -1]
            use crate::host::PgConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_begin(conn_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::PgCommit => {
            // Stack: [conn_handle] -> [0 or -1]
            use crate::host::PgConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_commit(conn_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::PgRollback => {
            // Stack: [conn_handle] -> [0 or -1]
            use crate::host::PgConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_rollback(conn_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::PgSavepoint => {
            // Stack: [conn_handle, name_str or name_idx] -> [0 or -1]
            use crate::host::PgConnectionHandle;
            let name = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_savepoint(conn_handle, &name) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::PgReleaseSavepoint => {
            // Stack: [conn_handle, name_str or name_idx] -> [0 or -1]
            use crate::host::PgConnectionHandle;
            let name = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_release_savepoint(conn_handle, &name) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::PgRollbackToSavepoint => {
            // Stack: [conn_handle, name_str or name_idx] -> [0 or -1]
            use crate::host::PgConnectionHandle;
            let name = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_rollback_to_savepoint(conn_handle, &name) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostDbOp::PgErrmsg => {
            // Stack: [conn_handle] -> [error_msg_str]
            use crate::host::PgConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            match ctx.db.pg_errmsg(conn_handle) {
                Ok(msg) => stack.push(Value::Str(msg)),
                Err(_) => stack.push(Value::Str(String::new())),
            }
        }
        HostDbOp::PgEscape => {
            // Stack: [conn_handle, input_str or input_idx] -> [escaped_str]
            use crate::host::PgConnectionHandle;
            let input = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::Str(String::new()));
                    return;
                }
            };
            match ctx.db.pg_escape(conn_handle, &input) {
                Ok(escaped) => stack.push(Value::Str(escaped)),
                Err(_) => stack.push(Value::Str(String::new())),
            }
        }
        HostDbOp::PgFreeResult => {
            // Stack: [result_handle] -> [0 or -1]
            use crate::host::PgResultHandle;
            let result_handle = match stack.pop() {
                Some(Value::I64(h)) => PgResultHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_free_result(result_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        // =====================================================================
        // Async PostgreSQL Operations
        // =====================================================================

        HostDbOp::PgConnectAsync => {
            // Stack: [conn_str] -> [handle or -1]
            let conn_str = match stack.pop() {
                Some(Value::Str(s)) => s,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_connect_async(&conn_str) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgDisconnectAsync => {
            // Stack: [conn_handle] -> [0 or -1]
            use crate::host::PgAsyncConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgAsyncConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_disconnect_async(conn_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgStatusAsync => {
            // Stack: [conn_handle] -> [1 if connected, 0 otherwise]
            use crate::host::PgAsyncConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgAsyncConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_status_async(conn_handle) {
                Ok(true) => stack.push(Value::I64(1)),
                Ok(false) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgQueryAsync => {
            // Stack: [conn_handle, sql] -> [query_handle or -1]
            // Note: params are simplified for now - no params supported
            use crate::host::{PgAsyncConnectionHandle, PgValue};
            let sql = match stack.pop() {
                Some(Value::Str(s)) => s,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgAsyncConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let params: Vec<PgValue> = vec![];
            match ctx.db.pg_query_async(conn_handle, &sql, &params) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgExecuteAsync => {
            // Stack: [conn_handle, sql] -> [query_handle or -1]
            use crate::host::{PgAsyncConnectionHandle, PgValue};
            let sql = match stack.pop() {
                Some(Value::Str(s)) => s,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgAsyncConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let params: Vec<PgValue> = vec![];
            match ctx.db.pg_execute_async(conn_handle, &sql, &params) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgPrepareAsync => {
            // Stack: [conn_handle, name, sql] -> [query_handle or -1]
            use crate::host::PgAsyncConnectionHandle;
            let sql = match stack.pop() {
                Some(Value::Str(s)) => s,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let name = match stack.pop() {
                Some(Value::Str(s)) => s,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgAsyncConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_prepare_async(conn_handle, &name, &sql) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgExecutePreparedAsync => {
            // Stack: [conn_handle, stmt_name] -> [query_handle or -1]
            use crate::host::{PgAsyncConnectionHandle, PgValue};
            let stmt_name = match stack.pop() {
                Some(Value::Str(s)) => s,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgAsyncConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let params: Vec<PgValue> = vec![];
            match ctx.db.pg_execute_prepared_async(conn_handle, &stmt_name, &params) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgIsReady => {
            // Stack: [query_handle] -> [1 if ready, 0 if pending, -1 on error]
            use crate::host::PgAsyncQueryHandle;
            let query_handle = match stack.pop() {
                Some(Value::I64(h)) => PgAsyncQueryHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_is_ready(query_handle) {
                Ok(true) => stack.push(Value::I64(1)),
                Ok(false) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgGetAsyncResult => {
            // Stack: [query_handle] -> [result_handle or -1]
            use crate::host::PgAsyncQueryHandle;
            let query_handle = match stack.pop() {
                Some(Value::I64(h)) => PgAsyncQueryHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_get_async_result(query_handle) {
                Ok(result_handle) => stack.push(Value::I64(result_handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgCancelAsync => {
            // Stack: [query_handle] -> [0 or -1]
            use crate::host::PgAsyncQueryHandle;
            let query_handle = match stack.pop() {
                Some(Value::I64(h)) => PgAsyncQueryHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_cancel_async(query_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgBeginAsync => {
            // Stack: [conn_handle] -> [query_handle or -1]
            use crate::host::PgAsyncConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgAsyncConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_begin_async(conn_handle) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgCommitAsync => {
            // Stack: [conn_handle] -> [query_handle or -1]
            use crate::host::PgAsyncConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgAsyncConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_commit_async(conn_handle) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgRollbackAsync => {
            // Stack: [conn_handle] -> [query_handle or -1]
            use crate::host::PgAsyncConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgAsyncConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_rollback_async(conn_handle) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        // ====================================================================
        // SQLite Connection Pool Operations
        // ====================================================================

        HostDbOp::SqlitePoolCreate => {
            // Stack: [conn_str_idx, min, max, acquire_timeout_ms, idle_timeout_ms, max_lifetime_ms, test_on_acquire] -> [pool_handle or -1]
            use crate::host::PoolConfig;
            let test_on_acquire = match stack.pop() {
                Some(Value::I64(v)) => v != 0,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let max_lifetime_ms = match stack.pop() {
                Some(Value::I64(v)) => v as u64,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let idle_timeout_ms = match stack.pop() {
                Some(Value::I64(v)) => v as u64,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let acquire_timeout_ms = match stack.pop() {
                Some(Value::I64(v)) => v as u64,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let max_connections = match stack.pop() {
                Some(Value::I64(v)) => v as u32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let min_connections = match stack.pop() {
                Some(Value::I64(v)) => v as u32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_str_idx = match stack.pop() {
                Some(Value::I64(i)) => i as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_str = match strings.get(conn_str_idx) {
                Some(s) => s,
                None => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let config = PoolConfig {
                min_connections,
                max_connections,
                acquire_timeout_ms,
                idle_timeout_ms,
                max_lifetime_ms,
                test_on_acquire,
            };
            match ctx.db.sqlite_pool_create(conn_str, &config) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::SqlitePoolClose => {
            // Stack: [pool_handle] -> [0 on success, -1 on error]
            use crate::host::SqlitePoolHandle;
            let pool_handle = match stack.pop() {
                Some(Value::I64(h)) => SqlitePoolHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_pool_close(pool_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::SqlitePoolAcquire => {
            // Stack: [pool_handle] -> [conn_handle or -1]
            use crate::host::SqlitePoolHandle;
            let pool_handle = match stack.pop() {
                Some(Value::I64(h)) => SqlitePoolHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_pool_acquire(pool_handle) {
                Ok(conn) => stack.push(Value::I64(conn.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::SqlitePoolRelease => {
            // Stack: [pool_handle, conn_handle] -> [0 on success, -1 on error]
            use crate::host::{SqliteConnectionHandle, SqlitePoolHandle};
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let pool_handle = match stack.pop() {
                Some(Value::I64(h)) => SqlitePoolHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_pool_release(pool_handle, conn_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::SqlitePoolStats => {
            // Stack: [pool_handle] -> [available, in_use, total, waiters] or [-1] on error
            use crate::host::SqlitePoolHandle;
            let pool_handle = match stack.pop() {
                Some(Value::I64(h)) => SqlitePoolHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_pool_stats(pool_handle) {
                Ok(stats) => {
                    stack.push(Value::I64(stats.available as i64));
                    stack.push(Value::I64(stats.in_use as i64));
                    stack.push(Value::I64(stats.total as i64));
                    stack.push(Value::I64(stats.waiters as i64));
                }
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        // ====================================================================
        // PostgreSQL Connection Pool Operations
        // ====================================================================

        HostDbOp::PgPoolCreate => {
            // Stack: [conn_str_idx, min, max, acquire_timeout_ms, idle_timeout_ms, max_lifetime_ms, test_on_acquire] -> [pool_handle or -1]
            use crate::host::PoolConfig;
            let test_on_acquire = match stack.pop() {
                Some(Value::I64(v)) => v != 0,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let max_lifetime_ms = match stack.pop() {
                Some(Value::I64(v)) => v as u64,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let idle_timeout_ms = match stack.pop() {
                Some(Value::I64(v)) => v as u64,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let acquire_timeout_ms = match stack.pop() {
                Some(Value::I64(v)) => v as u64,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let max_connections = match stack.pop() {
                Some(Value::I64(v)) => v as u32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let min_connections = match stack.pop() {
                Some(Value::I64(v)) => v as u32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_str_idx = match stack.pop() {
                Some(Value::I64(i)) => i as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_str = match strings.get(conn_str_idx) {
                Some(s) => s,
                None => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let config = PoolConfig {
                min_connections,
                max_connections,
                acquire_timeout_ms,
                idle_timeout_ms,
                max_lifetime_ms,
                test_on_acquire,
            };
            match ctx.db.pg_pool_create(conn_str, &config) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgPoolClose => {
            // Stack: [pool_handle] -> [0 on success, -1 on error]
            use crate::host::PgPoolHandle;
            let pool_handle = match stack.pop() {
                Some(Value::I64(h)) => PgPoolHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_pool_close(pool_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgPoolAcquire => {
            // Stack: [pool_handle] -> [conn_handle or -1]
            use crate::host::PgPoolHandle;
            let pool_handle = match stack.pop() {
                Some(Value::I64(h)) => PgPoolHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_pool_acquire(pool_handle) {
                Ok(conn) => stack.push(Value::I64(conn.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgPoolRelease => {
            // Stack: [pool_handle, conn_handle] -> [0 on success, -1 on error]
            use crate::host::{PgConnectionHandle, PgPoolHandle};
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let pool_handle = match stack.pop() {
                Some(Value::I64(h)) => PgPoolHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_pool_release(pool_handle, conn_handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgPoolStats => {
            // Stack: [pool_handle] -> [available, in_use, total, waiters] or [-1] on error
            use crate::host::PgPoolHandle;
            let pool_handle = match stack.pop() {
                Some(Value::I64(h)) => PgPoolHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_pool_stats(pool_handle) {
                Ok(stats) => {
                    stack.push(Value::I64(stats.available as i64));
                    stack.push(Value::I64(stats.in_use as i64));
                    stack.push(Value::I64(stats.total as i64));
                    stack.push(Value::I64(stats.waiters as i64));
                }
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        // ====================================================================
        // SQLite Transaction Helper Operations
        // ====================================================================

        HostDbOp::SqliteTxScopeBegin => {
            // Stack: [conn_handle] -> [scope_id or -1]
            use crate::host::SqliteConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_tx_scope_begin(conn_handle) {
                Ok(scope_id) => stack.push(Value::I64(scope_id)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::SqliteTxScopeEnd => {
            // Stack: [conn_handle, scope_id, success] -> [0 on success, -1 on error]
            use crate::host::SqliteConnectionHandle;
            let success = match stack.pop() {
                Some(Value::I64(v)) => v != 0,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let scope_id = match stack.pop() {
                Some(Value::I64(id)) => id,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_tx_scope_end(conn_handle, scope_id, success) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::SqliteTxDepth => {
            // Stack: [conn_handle] -> [depth]
            use crate::host::SqliteConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_tx_depth(conn_handle) {
                Ok(depth) => stack.push(Value::I64(depth as i64)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::SqliteTxActive => {
            // Stack: [conn_handle] -> [1 if active, 0 if not, -1 on error]
            use crate::host::SqliteConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => SqliteConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.sqlite_tx_active(conn_handle) {
                Ok(active) => stack.push(Value::I64(if active { 1 } else { 0 })),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        // ====================================================================
        // PostgreSQL Transaction Helper Operations
        // ====================================================================

        HostDbOp::PgTxScopeBegin => {
            // Stack: [conn_handle] -> [scope_id or -1]
            use crate::host::PgConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_tx_scope_begin(conn_handle) {
                Ok(scope_id) => stack.push(Value::I64(scope_id)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgTxScopeEnd => {
            // Stack: [conn_handle, scope_id, success] -> [0 on success, -1 on error]
            use crate::host::PgConnectionHandle;
            let success = match stack.pop() {
                Some(Value::I64(v)) => v != 0,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let scope_id = match stack.pop() {
                Some(Value::I64(id)) => id,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_tx_scope_end(conn_handle, scope_id, success) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgTxDepth => {
            // Stack: [conn_handle] -> [depth]
            use crate::host::PgConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_tx_depth(conn_handle) {
                Ok(depth) => stack.push(Value::I64(depth as i64)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }

        HostDbOp::PgTxActive => {
            // Stack: [conn_handle] -> [1 if active, 0 if not, -1 on error]
            use crate::host::PgConnectionHandle;
            let conn_handle = match stack.pop() {
                Some(Value::I64(h)) => PgConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.db.pg_tx_active(conn_handle) {
                Ok(active) => stack.push(Value::I64(if active { 1 } else { 0 })),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
    }
}

/// Dispatch a mail operation to the HostMail implementation.
///
/// Stack effects vary by operation; see HostMailOp documentation.
fn dispatch_host_mail(
    op: HostMailOp,
    stack: &mut Vec<Value>,
    strings: &[String],
    ctx: &HostContext,
) {
    use crate::host::{ImapConnectionHandle, MimeMessageHandle, Pop3ConnectionHandle, SmtpConnectionHandle};

    match op {
        // =========================================================================
        // SMTP Operations
        // =========================================================================
        HostMailOp::SmtpConnect => {
            // Stack: [host_str, port, use_tls, timeout_ms] -> [conn_handle or -1]
            let timeout_ms = match stack.pop() {
                Some(Value::I64(t)) => t as u64,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let use_tls = match stack.pop() {
                Some(Value::I64(t)) => t != 0,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let port = match stack.pop() {
                Some(Value::I64(p)) => p as u16,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let host = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_connect(&host, port, use_tls, timeout_ms) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpStartTls => {
            // Stack: [conn_handle] -> [0 or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_start_tls(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpAuth => {
            // Stack: [conn_handle, mechanism_str, username_str, password_str] -> [0 or -1]
            let password = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let username = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let mechanism = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_auth(handle, &mechanism, &username, &password) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpEhlo => {
            // Stack: [conn_handle, hostname_str] -> [capabilities_list_handle or -1]
            let hostname = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_ehlo(handle, &hostname) {
                Ok(_caps) => {
                    // Capabilities cached in connection; retrieve via SmtpGetCapabilities
                    stack.push(Value::I64(0))
                }
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpQuit => {
            // Stack: [conn_handle] -> [0 or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_quit(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpNoop => {
            // Stack: [conn_handle] -> [0 or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_noop(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpReset => {
            // Stack: [conn_handle] -> [0 or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_reset(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpMailFrom => {
            // Stack: [conn_handle, sender_str] -> [0 or -1]
            let sender = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_mail_from(handle, &sender) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpRcptTo => {
            // Stack: [conn_handle, recipient_str] -> [0 or -1]
            let recipient = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_rcpt_to(handle, &recipient) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpData => {
            // Stack: [conn_handle] -> [0 or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_data(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpSendData => {
            // Stack: [conn_handle, data_str] -> [0 or -1]
            let data = match stack.pop() {
                Some(Value::Str(s)) => s.into_bytes(),
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default().into_bytes()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_send_data(handle, &data) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpEndData => {
            // Stack: [conn_handle] -> [0 or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_end_data(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpReadResponse => {
            // Stack: [conn_handle] -> [response_code or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_read_response(handle) {
                Ok(resp) => stack.push(Value::I64(resp.code as i64)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpGetResponseCode => {
            // Stack: [conn_handle] -> [code or -1]
            // Note: This reads the next response from the server
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_read_response(handle) {
                Ok(resp) => stack.push(Value::I64(resp.code as i64)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpGetResponseMessage => {
            // Stack: [conn_handle] -> [message_str or -1]
            // Note: This reads the next response from the server
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_read_response(handle) {
                Ok(resp) => stack.push(Value::Str(resp.message)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpGetCapabilities => {
            // Stack: [conn_handle] -> [0 or -1]
            // Note: Returns success if capabilities exist, caps stored in connection
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.smtp_get_capabilities(handle) {
                Ok(_caps) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpSendMessage => {
            // Stack: [conn_handle, from_str, to_str, message_data_str] -> [0 or -1]
            // Note: to_str can be comma or semicolon separated for multiple recipients
            let message_data = match stack.pop() {
                Some(Value::Str(s)) => s.into_bytes(),
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default().into_bytes()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let to_str = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let from = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => SmtpConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            // Parse recipients (comma or semicolon separated)
            let recipients: Vec<&str> = to_str
                .split([',', ';'])
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            match ctx.mail.smtp_send_message(handle, &from, &recipients, &message_data) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::SmtpVerify => {
            // Stack: [conn_handle, address_str] -> [-1]
            // Note: VRFY command not implemented in HostMail trait
            stack.pop();
            stack.pop();
            stack.push(Value::I64(-1));
        }
        HostMailOp::SmtpExpand => {
            // Stack: [conn_handle, list_str] -> [-1]
            // Note: EXPN command not implemented in HostMail trait
            stack.pop();
            stack.pop();
            stack.push(Value::I64(-1));
        }

        // =========================================================================
        // IMAP Operations
        // =========================================================================
        HostMailOp::ImapConnect => {
            // Stack: [host_str, port, use_tls, timeout_ms] -> [conn_handle or -1]
            let timeout_ms = match stack.pop() {
                Some(Value::I64(t)) => t as u64,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let use_tls = match stack.pop() {
                Some(Value::I64(t)) => t != 0,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let port = match stack.pop() {
                Some(Value::I64(p)) => p as u16,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let host = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_connect(&host, port, use_tls, timeout_ms) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapStartTls => {
            // Stack: [conn_handle] -> [0 or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_start_tls(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapAuth => {
            // Stack: [conn_handle, username_str, password_str] -> [0 or -1]
            let password = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let username = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_auth(handle, &username, &password) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapAuthOAuth => {
            // Stack: [conn_handle, username_str, access_token_str] -> [0 or -1]
            let access_token = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let username = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_auth_oauth(handle, &username, &access_token) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapLogout => {
            // Stack: [conn_handle] -> [0 or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_logout(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapNoop => {
            // Stack: [conn_handle] -> [0 or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_noop(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapCapability => {
            // Stack: [conn_handle] -> [0 or -1]
            // Note: Capabilities are returned but not serialized to list handle yet
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_capability(handle) {
                Ok(_caps) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapSelect => {
            // Stack: [conn_handle, mailbox_str] -> [folder_handle or -1]
            let mailbox = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_select(handle, &mailbox) {
                Ok(folder_handle) => stack.push(Value::I64(folder_handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapExamine => {
            // Stack: [conn_handle, mailbox_str] -> [folder_handle or -1]
            let mailbox = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_examine(handle, &mailbox) {
                Ok(folder_handle) => stack.push(Value::I64(folder_handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapCreate | HostMailOp::ImapDelete | HostMailOp::ImapRename
        | HostMailOp::ImapSubscribe | HostMailOp::ImapUnsubscribe => {
            // These mailbox management operations are not yet implemented in HostMail trait
            stack.pop();
            stack.pop();
            stack.push(Value::I64(-1));
        }
        HostMailOp::ImapList => {
            // Stack: [conn_handle, reference_str, pattern_str] -> [0 or -1]
            // Note: Returns list of mailboxes but not serialized to list handle yet
            let pattern = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let reference = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_list(handle, &reference, &pattern) {
                Ok(_mailboxes) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapLsub | HostMailOp::ImapStatus => {
            // These operations are not yet implemented in HostMail trait
            stack.pop();
            stack.pop();
            stack.push(Value::I64(-1));
        }
        HostMailOp::ImapFetch => {
            // Stack: [conn_handle, sequence_str, items_str] -> [0 or -1]
            // Note: Returns fetch results but not serialized to list handle yet
            let items = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let sequence = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_fetch(handle, &sequence, &items) {
                Ok(_results) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapStore => {
            // Stack: [conn_handle, sequence_str, flags_str, action_str] -> [0 or -1]
            let action = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let flags = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let sequence = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_store(handle, &sequence, &flags, &action) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapCopy | HostMailOp::ImapMove | HostMailOp::ImapAppend => {
            // These operations are not yet implemented in HostMail trait
            stack.pop();
            stack.pop();
            stack.pop();
            stack.push(Value::I64(-1));
        }
        HostMailOp::ImapExpunge => {
            // Stack: [conn_handle] -> [0 or -1]
            // Note: Returns list of expunged sequence numbers but not serialized yet
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_expunge(handle) {
                Ok(_expunged) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapSearch => {
            // Stack: [conn_handle, criteria_str] -> [0 or -1]
            // Note: Returns list of message sequence numbers but not serialized yet
            let criteria = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_search(handle, &criteria) {
                Ok(_results) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapIdle => {
            // Stack: [conn_handle, timeout_ms] -> [event_code or -1]
            // Event codes: 0=timeout, positive=EXISTS count, negative=EXPUNGE seq
            let timeout_ms = match stack.pop() {
                Some(Value::I64(t)) => t as u64,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => ImapConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.imap_idle(handle, timeout_ms) {
                Ok(event) => {
                    use crate::host::ImapIdleEvent;
                    let code = match event {
                        ImapIdleEvent::Timeout => 0,
                        ImapIdleEvent::Exists(count) => count as i64,
                        ImapIdleEvent::Expunge(seq) => -(seq as i64),
                        ImapIdleEvent::Fetch { seq_num, .. } => seq_num as i64 + 1_000_000,
                    };
                    stack.push(Value::I64(code));
                }
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::ImapIdleDone | HostMailOp::ImapIdlePoll => {
            // These are handled internally by imap_idle
            stack.pop();
            stack.push(Value::I64(0));
        }

        // =========================================================================
        // POP3 Operations
        // =========================================================================
        HostMailOp::Pop3Connect => {
            // Stack: [host_str, port, use_tls, timeout_ms] -> [conn_handle or -1]
            let timeout_ms = match stack.pop() {
                Some(Value::I64(t)) => t as u64,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let use_tls = match stack.pop() {
                Some(Value::I64(t)) => t != 0,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let port = match stack.pop() {
                Some(Value::I64(p)) => p as u16,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let host = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.pop3_connect(&host, port, use_tls, timeout_ms) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::Pop3StartTls => {
            // Stack: [conn_handle] -> [0 or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => Pop3ConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.pop3_start_tls(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::Pop3Auth => {
            // Stack: [conn_handle, username_str, password_str] -> [0 or -1]
            let password = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let username = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => Pop3ConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.pop3_auth(handle, &username, &password) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::Pop3AuthApop => {
            // Stack: [conn_handle, username_str, password_str] -> [0 or -1]
            let password = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let username = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => Pop3ConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.pop3_auth_apop(handle, &username, &password) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::Pop3Quit => {
            // Stack: [conn_handle] -> [0 or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => Pop3ConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.pop3_quit(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::Pop3Noop => {
            // Stack: [conn_handle] -> [0 or -1]
            // Note: POP3 doesn't have NOOP in HostMail trait, return success
            let _handle = stack.pop();
            stack.push(Value::I64(0));
        }
        HostMailOp::Pop3Stat => {
            // Stack: [conn_handle] -> [message_count, total_size or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => Pop3ConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.pop3_stat(handle) {
                Ok(stat) => {
                    // Push total_size first, then message_count (so count is on top)
                    stack.push(Value::I64(stat.total_size as i64));
                    stack.push(Value::I64(stat.message_count as i64));
                }
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::Pop3List => {
            // Stack: [conn_handle] -> [0 or -1]
            // Note: Returns list of messages but not serialized to list handle yet
            let handle = match stack.pop() {
                Some(Value::I64(h)) => Pop3ConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.pop3_list(handle) {
                Ok(_messages) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::Pop3Uidl => {
            // Stack: [conn_handle] -> [0 or -1]
            // Note: Returns list of UIDs but not serialized to list handle yet
            let handle = match stack.pop() {
                Some(Value::I64(h)) => Pop3ConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.pop3_uidl(handle) {
                Ok(_uids) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::Pop3Retr => {
            // Stack: [conn_handle, msg_num] -> [data_str or -1]
            let msg_num = match stack.pop() {
                Some(Value::I64(n)) => n as u32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => Pop3ConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.pop3_retr(handle, msg_num) {
                Ok(data) => {
                    // Return as string (may contain binary data)
                    match String::from_utf8(data) {
                        Ok(s) => stack.push(Value::Str(s)),
                        Err(e) => {
                            // Return as lossy string for non-UTF8 data
                            stack.push(Value::Str(String::from_utf8_lossy(e.as_bytes()).into_owned()));
                        }
                    }
                }
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::Pop3Dele => {
            // Stack: [conn_handle, msg_num] -> [0 or -1]
            let msg_num = match stack.pop() {
                Some(Value::I64(n)) => n as u32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => Pop3ConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.pop3_dele(handle, msg_num) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::Pop3Reset => {
            // Stack: [conn_handle] -> [0 or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => Pop3ConnectionHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.pop3_reset(handle) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::Pop3Top => {
            // Stack: [conn_handle, msg_num, lines] -> [-1]
            // Note: TOP is not implemented in HostMail trait
            stack.pop();
            stack.pop();
            stack.pop();
            stack.push(Value::I64(-1));
        }

        // =========================================================================
        // MIME Operations
        // =========================================================================
        HostMailOp::MimeBase64Encode => {
            // Stack: [data_str] -> [encoded_str_idx]
            let data = match stack.pop() {
                Some(Value::Str(s)) => s.into_bytes(),
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default().into_bytes()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let encoded = ctx.mail.mime_base64_encode(&data);
            stack.push(Value::Str(encoded));
        }
        HostMailOp::MimeBase64Decode => {
            // Stack: [encoded_str] -> [decoded_str or -1]
            let encoded = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.mime_base64_decode(&encoded) {
                Ok(decoded) => {
                    match String::from_utf8(decoded) {
                        Ok(s) => stack.push(Value::Str(s)),
                        Err(_) => stack.push(Value::I64(-1)),
                    }
                }
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::MimeQuotedPrintableEncode => {
            // Stack: [data_str] -> [encoded_str]
            let data = match stack.pop() {
                Some(Value::Str(s)) => s.into_bytes(),
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default().into_bytes()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let encoded = ctx.mail.mime_quoted_printable_encode(&data);
            stack.push(Value::Str(encoded));
        }
        HostMailOp::MimeQuotedPrintableDecode => {
            // Stack: [encoded_str] -> [decoded_str or -1]
            let encoded = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.mime_quoted_printable_decode(&encoded) {
                Ok(decoded) => {
                    match String::from_utf8(decoded) {
                        Ok(s) => stack.push(Value::Str(s)),
                        Err(_) => stack.push(Value::I64(-1)),
                    }
                }
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::MimeEncodeHeader => {
            // Stack: [value_str, charset_str] -> [encoded_str]
            let charset = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let value = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let encoded = ctx.mail.mime_encode_header(&value, &charset);
            stack.push(Value::Str(encoded));
        }
        HostMailOp::MimeDecodeHeader => {
            // Stack: [encoded_str] -> [decoded_str or -1]
            let encoded = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.mime_decode_header(&encoded) {
                Ok(decoded) => stack.push(Value::Str(decoded)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::MimeMessageNew => {
            // Stack: [] -> [msg_handle]
            let handle = ctx.mail.mime_message_new();
            stack.push(Value::I64(handle.0));
        }
        HostMailOp::MimeMessageSetHeader => {
            // Stack: [msg_handle, name_str, value_str] -> [0 or -1]
            let value = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let name = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => MimeMessageHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.mime_message_set_header(handle, &name, &value) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::MimeMessageSetBody => {
            // Stack: [msg_handle, content_type_str, body_str] -> [0 or -1]
            let body = match stack.pop() {
                Some(Value::Str(s)) => s.into_bytes(),
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default().into_bytes()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let content_type = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => MimeMessageHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.mime_message_set_body(handle, &content_type, &body) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::MimeMessageAddAttachment => {
            // Stack: [msg_handle, filename_str, content_type_str, data_str] -> [0 or -1]
            let data = match stack.pop() {
                Some(Value::Str(s)) => s.into_bytes(),
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default().into_bytes()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let content_type = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let filename = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => MimeMessageHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.mime_message_add_attachment(handle, &filename, &content_type, &data) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::MimeMessageAddInline => {
            // Stack: [msg_handle, content_id_str, content_type_str, data_str] -> [0 or -1]
            // Note: AddInline not in HostMail trait, use AddAttachment with content-id
            let data = match stack.pop() {
                Some(Value::Str(s)) => s.into_bytes(),
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default().into_bytes()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let content_type = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let content_id = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => MimeMessageHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            // Use content_id as filename for inline attachments
            match ctx.mail.mime_message_add_attachment(handle, &content_id, &content_type, &data) {
                Ok(()) => stack.push(Value::I64(0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::MimeMessageBuildMultipart => {
            // Stack: [msg_handle, subtype_str] -> [0 or -1]
            // Note: BuildMultipart not in HostMail trait, multipart is built automatically
            stack.pop();
            stack.pop();
            stack.push(Value::I64(0));
        }
        HostMailOp::MimeMessageSerialize => {
            // Stack: [msg_handle] -> [data_str or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => MimeMessageHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.mime_message_serialize(handle) {
                Ok(data) => {
                    match String::from_utf8(data) {
                        Ok(s) => stack.push(Value::Str(s)),
                        Err(e) => {
                            // Return as lossy string for non-UTF8 data
                            stack.push(Value::Str(String::from_utf8_lossy(e.as_bytes()).into_owned()));
                        }
                    }
                }
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::MimeMessageParse => {
            // Stack: [data_str] -> [msg_handle or -1]
            let data = match stack.pop() {
                Some(Value::Str(s)) => s.into_bytes(),
                Some(Value::I64(idx)) => {
                    strings.get(idx as usize).cloned().unwrap_or_default().into_bytes()
                }
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.mime_message_parse(&data) {
                Ok(handle) => stack.push(Value::I64(handle.0)),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::MimeMessageGetHeader => {
            // Stack: [msg_handle, name_str] -> [value_str or -1]
            let name = match stack.pop() {
                Some(Value::Str(s)) => s,
                Some(Value::I64(idx)) => strings.get(idx as usize).cloned().unwrap_or_default(),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => MimeMessageHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.mime_message_get_header(handle, &name) {
                Ok(Some(value)) => stack.push(Value::Str(value)),
                Ok(None) => stack.push(Value::Str(String::new())),
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::MimeMessageGetBody => {
            // Stack: [msg_handle] -> [body_str or -1]
            let handle = match stack.pop() {
                Some(Value::I64(h)) => MimeMessageHandle(h),
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            match ctx.mail.mime_message_get_body(handle) {
                Ok(body) => {
                    match String::from_utf8(body) {
                        Ok(s) => stack.push(Value::Str(s)),
                        Err(e) => {
                            // Return as lossy string for non-UTF8 data
                            stack.push(Value::Str(String::from_utf8_lossy(e.as_bytes()).into_owned()));
                        }
                    }
                }
                Err(_) => stack.push(Value::I64(-1)),
            }
        }
        HostMailOp::MimeMessageGetAttachments => {
            // Stack: [msg_handle] -> [0 or -1]
            // Note: GetAttachments not in HostMail trait, returns success for now
            let _handle = stack.pop();
            stack.push(Value::I64(0));
        }
        HostMailOp::MimeMessageGetAllHeaders => {
            // Stack: [msg_handle] -> [0 or -1]
            // Note: GetAllHeaders not in HostMail trait, returns success for now
            let _handle = stack.pop();
            stack.push(Value::I64(0));
        }

        // =========================================================================
        // TLS Operations
        // =========================================================================
        HostMailOp::TlsContextNew
        | HostMailOp::TlsUpgrade
        | HostMailOp::TlsHandshake
        | HostMailOp::TlsRead
        | HostMailOp::TlsWrite
        | HostMailOp::TlsClose
        | HostMailOp::TlsSetCert
        | HostMailOp::TlsSetVerify => {
            // TLS operations - used internally, stub for now
            stack.pop();
            stack.push(Value::I64(-1));
        }
    }
}

// ============================================================================
// Crypto Operations Dispatch
// ============================================================================

fn dispatch_host_crypto(op: HostCryptoOp, stack: &mut Vec<Value>, _ctx: &HostContext) {
    match op {
        // =====================================================================
        // Secure Memory Operations
        // =====================================================================
        HostCryptoOp::SecureAlloc => {
            let size = match stack.pop() {
                Some(Value::I64(s)) if s > 0 => s as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = arth_rt::crypto::arth_rt_secure_alloc(size);
            stack.push(Value::I64(handle));
        }
        HostCryptoOp::SecureFree => {
            let handle = match stack.pop() {
                Some(Value::I64(h)) => h,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            arth_rt::crypto::arth_rt_secure_free(handle);
            stack.push(Value::I64(0));
        }
        HostCryptoOp::SecurePtr => {
            let handle = match stack.pop() {
                Some(Value::I64(h)) => h,
                _ => {
                    stack.push(Value::I64(0));
                    return;
                }
            };
            let ptr = arth_rt::crypto::arth_rt_secure_ptr(handle);
            stack.push(Value::I64(ptr as i64));
        }
        HostCryptoOp::SecureLen => {
            let handle = match stack.pop() {
                Some(Value::I64(h)) => h,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let len = arth_rt::crypto::arth_rt_secure_len(handle);
            stack.push(Value::I64(len as i64));
        }
        HostCryptoOp::SecureWrite => {
            // Stack: [handle, data_ptr, data_len] -> [result]
            let data_len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let data_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *const u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => h,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let result = arth_rt::crypto::arth_rt_secure_write(handle, data_ptr, data_len);
            stack.push(Value::I64(result));
        }
        HostCryptoOp::SecureRead => {
            // Stack: [handle, out_ptr, out_len] -> [result]
            let out_len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let out_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *mut u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => h,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let result = arth_rt::crypto::arth_rt_secure_read(handle, out_ptr, out_len);
            stack.push(Value::I64(result));
        }
        HostCryptoOp::SecureZero => {
            // Stack: [ptr, len] -> [0]
            // Zeros memory at ptr for len bytes
            let len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *mut u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            arth_rt::crypto::arth_rt_secure_zero(ptr, len);
            stack.push(Value::I64(0));
        }
        HostCryptoOp::SecureCompare => {
            // Stack: [a_ptr, b_ptr, len] -> [result]
            // Constant-time comparison of two equal-length buffers
            let len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let b_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *const u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let a_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *const u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let result = arth_rt::crypto::arth_rt_secure_compare(a_ptr, b_ptr, len);
            stack.push(Value::I64(result as i64));
        }

        // =====================================================================
        // Hash Operations
        // =====================================================================
        HostCryptoOp::Hash => {
            let out_len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let out_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *mut u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let data_len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let data_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *const u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let algorithm = match stack.pop() {
                Some(Value::I64(a)) => a as i32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let result = arth_rt::crypto::arth_rt_hash(algorithm, data_ptr, data_len, out_ptr, out_len);
            stack.push(Value::I64(result));
        }
        HostCryptoOp::HasherNew => {
            let algorithm = match stack.pop() {
                Some(Value::I64(a)) => a as i32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = arth_rt::crypto::arth_rt_hasher_new(algorithm);
            stack.push(Value::I64(handle));
        }
        HostCryptoOp::HasherUpdate => {
            let data_len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let data_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *const u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => h,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let result = arth_rt::crypto::arth_rt_hasher_update(handle, data_ptr, data_len);
            stack.push(Value::I64(result as i64));
        }
        HostCryptoOp::HasherFinalize => {
            let out_len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let out_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *mut u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let handle = match stack.pop() {
                Some(Value::I64(h)) => h,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let result = arth_rt::crypto::arth_rt_hasher_finalize(handle, out_ptr, out_len);
            stack.push(Value::I64(result));
        }

        // =====================================================================
        // HMAC Operations
        // =====================================================================
        HostCryptoOp::Hmac => {
            let out_len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let out_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *mut u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let data_len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let data_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *const u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let key_len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let key_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *const u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let algorithm = match stack.pop() {
                Some(Value::I64(a)) => a as i32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let result = arth_rt::crypto::arth_rt_hmac(
                algorithm, key_ptr, key_len, data_ptr, data_len, out_ptr, out_len,
            );
            stack.push(Value::I64(result));
        }
        HostCryptoOp::HmacNew
        | HostCryptoOp::HmacUpdate
        | HostCryptoOp::HmacFinalize
        | HostCryptoOp::HmacVerify => {
            // Streaming HMAC - stub for now, pop args and return error
            while stack.pop().is_some() {}
            stack.push(Value::I64(-1));
        }

        // =====================================================================
        // AEAD Operations
        // =====================================================================
        HostCryptoOp::AeadGenerateNonce => {
            let out_len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let out_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *mut u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let algorithm = match stack.pop() {
                Some(Value::I64(a)) => a as i32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let result = arth_rt::crypto::arth_rt_aead_generate_nonce(algorithm, out_ptr, out_len);
            stack.push(Value::I64(result.into()));
        }
        HostCryptoOp::AeadEncrypt => {
            // Pop all 11 arguments in reverse order
            let out_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let out_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let aad_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let aad_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let plaintext_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let plaintext_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let nonce_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let nonce_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let key_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let key_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let algorithm = match stack.pop() { Some(Value::I64(a)) => a as i32, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_aead_encrypt(
                algorithm, key_ptr, key_len, nonce_ptr, nonce_len,
                plaintext_ptr, plaintext_len, aad_ptr, aad_len, out_ptr, out_len,
            );
            stack.push(Value::I64(result));
        }
        HostCryptoOp::AeadDecrypt => {
            // Pop all 11 arguments in reverse order
            let out_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let out_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let aad_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let aad_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let ciphertext_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let ciphertext_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let nonce_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let nonce_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let key_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let key_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let algorithm = match stack.pop() { Some(Value::I64(a)) => a as i32, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_aead_decrypt(
                algorithm, key_ptr, key_len, nonce_ptr, nonce_len,
                ciphertext_ptr, ciphertext_len, aad_ptr, aad_len, out_ptr, out_len,
            );
            stack.push(Value::I64(result));
        }

        // =====================================================================
        // Signature Operations
        // =====================================================================
        HostCryptoOp::SignatureGenerateKeypair => {
            // Stack: [algorithm, priv_ptr, priv_len, pub_ptr, pub_len] -> [result]
            let pub_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let pub_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let priv_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let priv_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let algorithm = match stack.pop() { Some(Value::I64(a)) => a as i32, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_signature_generate_keypair(algorithm, priv_ptr, priv_len, pub_ptr, pub_len);
            stack.push(Value::I64(result.into()));
        }
        HostCryptoOp::SignatureDerivePublicKey => {
            // Stack: [algorithm, priv_ptr, priv_len, pub_ptr, pub_len] -> [result]
            let pub_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let pub_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let priv_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let priv_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let algorithm = match stack.pop() { Some(Value::I64(a)) => a as i32, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_signature_derive_public_key(algorithm, priv_ptr, priv_len, pub_ptr, pub_len);
            stack.push(Value::I64(result.into()));
        }
        HostCryptoOp::SignatureSign | HostCryptoOp::SignatureSignHash => {
            // Stack: [algorithm, priv_ptr, priv_len, msg_ptr, msg_len, sig_ptr, sig_len] -> [result]
            let sig_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let sig_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let msg_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let msg_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let priv_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let priv_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let algorithm = match stack.pop() { Some(Value::I64(a)) => a as i32, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_signature_sign(algorithm, priv_ptr, priv_len, msg_ptr, msg_len, sig_ptr, sig_len);
            stack.push(Value::I64(result));
        }
        HostCryptoOp::SignatureVerify | HostCryptoOp::SignatureVerifyHash => {
            // Stack: [algorithm, pub_ptr, pub_len, msg_ptr, msg_len, sig_ptr, sig_len] -> [result]
            let sig_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let sig_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let msg_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let msg_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let pub_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let pub_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let algorithm = match stack.pop() { Some(Value::I64(a)) => a as i32, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_signature_verify(algorithm, pub_ptr, pub_len, msg_ptr, msg_len, sig_ptr, sig_len);
            stack.push(Value::I64(result.into()));
        }

        // =====================================================================
        // Key Exchange Operations
        // =====================================================================
        HostCryptoOp::KexGenerateKeypair => {
            // Stack: [algorithm, priv_ptr, priv_len, pub_ptr, pub_len] -> [result]
            let pub_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let pub_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let priv_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let priv_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let algorithm = match stack.pop() { Some(Value::I64(a)) => a as i32, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_kex_generate_keypair(algorithm, priv_ptr, priv_len, pub_ptr, pub_len);
            stack.push(Value::I64(result.into()));
        }
        HostCryptoOp::KexAgree => {
            // Stack: [algorithm, priv_ptr, priv_len, peer_pub_ptr, peer_pub_len, secret_ptr, secret_len] -> [result]
            let secret_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let secret_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let peer_pub_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let peer_pub_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let priv_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let priv_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let algorithm = match stack.pop() { Some(Value::I64(a)) => a as i32, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_kex_agree(algorithm, priv_ptr, priv_len, peer_pub_ptr, peer_pub_len, secret_ptr, secret_len);
            stack.push(Value::I64(result));
        }
        HostCryptoOp::KexAgreeWithKdf => {
            // Stack: [algorithm, kdf_algo, priv_ptr, priv_len, peer_pub_ptr, peer_pub_len, salt_ptr, salt_len, info_ptr, info_len, out_ptr, out_len] -> [result]
            let out_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let out_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let info_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let info_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let salt_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let salt_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let peer_pub_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let peer_pub_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let priv_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let priv_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let kdf_algo = match stack.pop() { Some(Value::I64(a)) => a as i32, _ => { stack.push(Value::I64(-1)); return; } };
            let algorithm = match stack.pop() { Some(Value::I64(a)) => a as i32, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_kex_agree_with_kdf(
                algorithm, kdf_algo, priv_ptr, priv_len, peer_pub_ptr, peer_pub_len,
                salt_ptr, salt_len, info_ptr, info_len, out_ptr, out_len
            );
            stack.push(Value::I64(result));
        }

        // =====================================================================
        // KDF Operations
        // =====================================================================
        HostCryptoOp::KdfHkdf => {
            // Stack: [algorithm, ikm_ptr, ikm_len, salt_ptr, salt_len, info_ptr, info_len, okm_ptr, okm_len] -> [result]
            let okm_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let okm_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let info_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let info_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let salt_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let salt_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let ikm_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let ikm_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let algorithm = match stack.pop() { Some(Value::I64(a)) => a as i32, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_hkdf_derive(
                algorithm, ikm_ptr, ikm_len, salt_ptr, salt_len, info_ptr, info_len, okm_ptr, okm_len
            );
            stack.push(Value::I64(result));
        }
        HostCryptoOp::KdfPbkdf2 => {
            // Stack: [algorithm, password_ptr, password_len, salt_ptr, salt_len, iterations, okm_ptr, okm_len] -> [result]
            let okm_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let okm_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let iterations = match stack.pop() { Some(Value::I64(i)) if i > 0 => i as u32, _ => { stack.push(Value::I64(-1)); return; } };
            let salt_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let salt_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let password_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let password_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let algorithm = match stack.pop() { Some(Value::I64(a)) => a as i32, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_pbkdf2_derive(
                algorithm, password_ptr, password_len, salt_ptr, salt_len, iterations, okm_ptr, okm_len
            );
            stack.push(Value::I64(result));
        }
        HostCryptoOp::KdfArgon2 => {
            // Stack: [password_ptr, password_len, salt_ptr, salt_len, memory_kib, iterations, parallelism, okm_ptr, okm_len] -> [result]
            let okm_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let okm_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let parallelism = match stack.pop() { Some(Value::I64(p)) if p > 0 => p as u32, _ => { stack.push(Value::I64(-1)); return; } };
            let iterations = match stack.pop() { Some(Value::I64(i)) if i > 0 => i as u32, _ => { stack.push(Value::I64(-1)); return; } };
            let memory_kib = match stack.pop() { Some(Value::I64(m)) if m > 0 => m as u32, _ => { stack.push(Value::I64(-1)); return; } };
            let salt_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let salt_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let password_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let password_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_argon2_derive(
                password_ptr, password_len, salt_ptr, salt_len, memory_kib, iterations, parallelism, okm_ptr, okm_len
            );
            stack.push(Value::I64(result));
        }

        // =====================================================================
        // Password Hashing Operations
        // =====================================================================
        HostCryptoOp::PasswordHashArgon2 => {
            // Stack: [password_ptr, password_len, memory_kib, iterations, parallelism, output_ptr, output_len] -> [result]
            let output_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let output_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let parallelism = match stack.pop() { Some(Value::I64(p)) if p > 0 => p as u32, _ => { stack.push(Value::I64(-1)); return; } };
            let iterations = match stack.pop() { Some(Value::I64(i)) if i > 0 => i as u32, _ => { stack.push(Value::I64(-1)); return; } };
            let memory_kib = match stack.pop() { Some(Value::I64(m)) if m > 0 => m as u32, _ => { stack.push(Value::I64(-1)); return; } };
            let password_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let password_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_password_hash_argon2id(
                password_ptr, password_len, memory_kib, iterations, parallelism, output_ptr, output_len
            );
            stack.push(Value::I64(result));
        }
        HostCryptoOp::PasswordHashBcrypt => {
            // Stack: [password_ptr, password_len, cost, output_ptr, output_len] -> [result]
            let output_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let output_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let cost = match stack.pop() { Some(Value::I64(c)) if c >= 4 && c <= 31 => c as u32, _ => { stack.push(Value::I64(-1)); return; } };
            let password_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let password_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_password_hash_bcrypt(
                password_ptr, password_len, cost, output_ptr, output_len
            );
            stack.push(Value::I64(result));
        }
        HostCryptoOp::PasswordVerify => {
            // Stack: [password_ptr, password_len, hash_ptr] -> [result]
            // hash_ptr points to a null-terminated hash string
            let hash_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const std::ffi::c_char, _ => { stack.push(Value::I64(-1)); return; } };
            let password_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let password_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_password_verify(password_ptr, password_len, hash_ptr);
            stack.push(Value::I64(result.into()));
        }

        // =====================================================================
        // Random Operations
        // =====================================================================
        HostCryptoOp::RandomBytes => {
            let out_len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let out_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *mut u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let result = arth_rt::crypto::arth_rt_random_bytes(out_ptr, out_len);
            stack.push(Value::I64(result));
        }
        HostCryptoOp::RandomSalt => {
            let out_len = match stack.pop() {
                Some(Value::I64(l)) if l >= 0 => l as usize,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let out_ptr = match stack.pop() {
                Some(Value::I64(p)) => p as *mut u8,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            // Salt is just random bytes
            let result = arth_rt::crypto::arth_rt_random_bytes(out_ptr, out_len);
            stack.push(Value::I64(result));
        }

        // =====================================================================
        // Encoding Operations
        // =====================================================================
        HostCryptoOp::EncodingToHex => {
            let out_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let out_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let data_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let data_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::encoding::arth_rt_hex_encode(data_ptr, data_len, out_ptr, out_len);
            stack.push(Value::I64(result));
        }
        HostCryptoOp::EncodingFromHex => {
            let out_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let out_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let hex_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let hex_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::encoding::arth_rt_hex_decode(hex_ptr, hex_len, out_ptr, out_len);
            stack.push(Value::I64(result));
        }
        HostCryptoOp::EncodingToBase64 => {
            let out_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let out_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let data_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let data_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::encoding::arth_rt_base64_encode(data_ptr, data_len, out_ptr, out_len);
            stack.push(Value::I64(result));
        }
        HostCryptoOp::EncodingFromBase64 => {
            let out_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let out_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let b64_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let b64_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::encoding::arth_rt_base64_decode(b64_ptr, b64_len, out_ptr, out_len);
            stack.push(Value::I64(result));
        }
        HostCryptoOp::EncodingToBase64Url => {
            let out_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let out_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let data_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let data_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::encoding::arth_rt_base64url_encode(data_ptr, data_len, out_ptr, out_len);
            stack.push(Value::I64(result));
        }
        HostCryptoOp::EncodingFromBase64Url => {
            let out_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let out_ptr = match stack.pop() { Some(Value::I64(p)) => p as *mut u8, _ => { stack.push(Value::I64(-1)); return; } };
            let b64_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let b64_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::encoding::arth_rt_base64url_decode(b64_ptr, b64_len, out_ptr, out_len);
            stack.push(Value::I64(result));
        }

        // =====================================================================
        // Nonce Tracking Operations
        // =====================================================================
        HostCryptoOp::NonceTrackingEnable => {
            let enable = match stack.pop() {
                Some(Value::I64(e)) => e as i32,
                _ => {
                    stack.push(Value::I64(-1));
                    return;
                }
            };
            let result = arth_rt::crypto::arth_rt_aead_nonce_tracking_enable(enable);
            stack.push(Value::I64(result as i64));
        }
        HostCryptoOp::NonceCheck => {
            let nonce_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let nonce_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let key_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let key_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_aead_nonce_check(key_ptr, key_len, nonce_ptr, nonce_len);
            stack.push(Value::I64(result as i64));
        }
        HostCryptoOp::NonceMarkUsed => {
            let nonce_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let nonce_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let key_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let key_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_aead_nonce_mark_used(key_ptr, key_len, nonce_ptr, nonce_len);
            stack.push(Value::I64(result as i64));
        }
        HostCryptoOp::NonceCheckAndMark => {
            let nonce_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let nonce_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };
            let key_len = match stack.pop() { Some(Value::I64(l)) if l >= 0 => l as usize, _ => { stack.push(Value::I64(-1)); return; } };
            let key_ptr = match stack.pop() { Some(Value::I64(p)) => p as *const u8, _ => { stack.push(Value::I64(-1)); return; } };

            let result = arth_rt::crypto::arth_rt_aead_nonce_check_and_mark(key_ptr, key_len, nonce_ptr, nonce_len);
            stack.push(Value::I64(result as i64));
        }
        HostCryptoOp::NonceClear => {
            let result = arth_rt::crypto::arth_rt_aead_nonce_tracking_clear();
            stack.push(Value::I64(result));
        }
    }
}

/// Run a program with a custom host context.
///
/// This is the primary entry point for running programs with capability-based
/// sandboxing. Use `HostContext::for_guest(caps)` to create a context with
/// specific capabilities enabled.
///
/// All opcodes are handled with full capability enforcement. Host calls
/// (IO, Net, Time, DB, Mail, Crypto) are checked against the context's
/// configuration and denied if the capability is not allowed.
pub fn run_program_with_host(p: &Program, ctx: &HostContext) -> i32 {
    match run_program_internal(p, Some(ctx), None) {
        InterpreterResult::ExitCode(code) => code,
        InterpreterResult::Failed(code) => code,
        InterpreterResult::ReturnValue(_) => 0, // Shouldn't happen for run_program
    }
}

