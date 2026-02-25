// --- Async runtime implementation for IR backends ---
// This module provides a simple single-threaded async executor for Arth.
// Tasks are spawned with a function ID and arguments, and can be awaited.
// The executor runs tasks cooperatively at await points.

use std::collections::VecDeque;
static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

/// Task state in the executor
#[derive(Debug, Clone)]
enum TaskState {
    /// Task is pending execution
    Pending,
    /// Task is currently running
    Running,
    /// Task completed successfully with a result
    Completed(i64),
    /// Task was cancelled
    Cancelled,
    /// Task panicked with a message
    Panicked(String),
    /// Task threw an exception (exception handle stored)
    /// The awaiting task should catch or rethrow this exception
    Error(i64),
}

/// Information about a spawned task
#[derive(Debug)]
struct TaskInfo {
    /// Unique function identifier (hash of async body name)
    fn_id: i64,
    /// Number of arguments passed (reserved for future multi-threaded executor)
    #[allow(dead_code)]
    argc: i64,
    /// Arguments passed to the function (stored for later execution)
    args: Vec<i64>,
    /// Current state of the task
    state: TaskState,
    /// Whether this task has been detached (reserved for future detach support)
    #[allow(dead_code)]
    detached: bool,
}

fn task_store() -> &'static Mutex<HashMap<i64, TaskInfo>> {
    static MAP: OnceLock<Mutex<HashMap<i64, TaskInfo>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cancel_map() -> &'static Mutex<HashMap<i64, bool>> {
    static MAP: OnceLock<Mutex<HashMap<i64, bool>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Spawn a new async task with the given function ID and argument count.
/// Arguments are passed separately via __arth_task_push_arg.
///
/// Returns: task handle (i64)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_spawn_fn(fn_id: i64, argc: i64) -> i64 {
    let h = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);

    let info = TaskInfo {
        fn_id,
        argc,
        args: Vec::with_capacity(argc as usize),
        state: TaskState::Pending,
        detached: false,
    };

    if let Ok(mut m) = task_store().lock() {
        m.insert(h, info);
    }
    if let Ok(mut m) = cancel_map().lock() {
        m.insert(h, false);
    }
    h
}

/// Push an argument to a pending task.
/// Arguments should be pushed in order before the task is executed.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_push_arg(handle: i64, arg: i64) -> i32 {
    if let Ok(mut m) = task_store().lock() {
        if let Some(info) = m.get_mut(&handle) {
            info.args.push(arg);
            return 0;
        }
    }
    1
}

/// Get an argument from a task by index.
/// Used by the async body function to retrieve its parameters.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_get_arg(handle: i64, index: i64) -> i64 {
    if let Ok(m) = task_store().lock() {
        if let Some(info) = m.get(&handle) {
            if let Some(&arg) = info.args.get(index as usize) {
                return arg;
            }
        }
    }
    0
}

/// Get the function ID for a spawned task.
/// This is used by the VM to dispatch to the correct async body function.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_get_fn_id(handle: i64) -> i64 {
    if let Ok(m) = task_store().lock() {
        if let Some(info) = m.get(&handle) {
            return info.fn_id;
        }
    }
    0
}

/// Mark a task as running.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_mark_running(handle: i64) -> i32 {
    if let Ok(mut m) = task_store().lock() {
        if let Some(info) = m.get_mut(&handle) {
            info.state = TaskState::Running;
            return 0;
        }
    }
    1
}

/// Complete a task with the given result value.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_complete(handle: i64, result: i64) -> i32 {
    if let Ok(mut m) = task_store().lock() {
        if let Some(info) = m.get_mut(&handle) {
            info.state = TaskState::Completed(result);
            return 0;
        }
    }
    1
}

/// Check if a task is completed (successfully, cancelled, or panicked).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_is_completed(handle: i64) -> i32 {
    if let Ok(m) = task_store().lock() {
        if let Some(info) = m.get(&handle) {
            return match info.state {
                TaskState::Completed(_) => 1,
                TaskState::Cancelled => 1,
                TaskState::Panicked(_) => 1,
                _ => 0,
            };
        }
    }
    0
}

/// Mark a task as panicked with a message.
/// This is called when a panic occurs within a task.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_set_panicked(handle: i64, msg_ptr: *const i8) -> i32 {
    let msg = if msg_ptr.is_null() {
        "panic".to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(msg_ptr) }
            .to_string_lossy()
            .into_owned()
    };
    if let Ok(mut m) = task_store().lock() {
        if let Some(info) = m.get_mut(&handle) {
            info.state = TaskState::Panicked(msg);
            return 0;
        }
    }
    1
}

/// Await a task and return its result.
/// Returns -1 if the task was cancelled (triggers exception in IR path).
/// Returns -2 if the task panicked (triggers re-panic in calling task).
/// For pending/running tasks, this is currently a no-op that returns 0
/// (the VM runs single-threaded, so tasks complete synchronously).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_await(handle: i64) -> i64 {
    // Check cancellation first
    if let Ok(m) = cancel_map().lock() {
        if m.get(&handle).copied().unwrap_or(false) {
            return -1; // Cancelled sentinel
        }
    }

    // Check task state and return result if completed
    if let Ok(m) = task_store().lock() {
        if let Some(info) = m.get(&handle) {
            match &info.state {
                TaskState::Completed(result) => return *result,
                TaskState::Cancelled => return -1,
                TaskState::Panicked(msg) => {
                    // Propagate panic to the calling task
                    // Store the panic message for retrieval
                    panic_with_message(msg.clone());
                    return -2; // Panicked sentinel
                }
                _ => {}
            }
        }
    }

    // For pending/running tasks, return 0 as default
    // In a real async runtime, this would suspend the current task
    0
}

/// Cancel a task.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_cancel(handle: i64) -> i32 {
    if let Ok(mut m) = cancel_map().lock() {
        m.insert(handle, true);
    }
    if let Ok(mut m) = task_store().lock() {
        if let Some(info) = m.get_mut(&handle) {
            info.state = TaskState::Cancelled;
        }
    }
    0
}

/// Explicitly await a task (same as __arth_await).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_await(handle: i64) -> i64 {
    __arth_await(handle)
}

/// Await a channel receive operation.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_recv_await(chan: i64) -> i64 {
    __arth_chan_recv(chan)
}

// Logging intrinsic used by IR lowering
// Signature: i64 __arth_log_emit(level, event_hash, message_hash)
// Returns 0. For now prints a formatted line using hashes.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_log_emit(level: i64, event: i64, message: i64) -> i64 {
    let lvl = match level {
        0 => "TRACE",
        1 => "DEBUG",
        2 => "INFO",
        3 => "WARN",
        _ => "ERROR",
    };
    // Render hashes as hex to make them compact and stable-looking
    println!(
        "{} ev=0x{:016x} msg=0x{:016x}",
        lvl, event as u64, message as u64
    );
    0
}

// String variant for native backends: pointers to NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_log_emit_str(
    level: i64,
    event: *const i8,
    message: *const i8,
    fields: *const i8,
) -> i64 {
    // Safety: best-effort read of NUL-terminated bytes; guard with a max length.
    unsafe fn cstr_to_string(ptr: *const i8) -> String {
        if ptr.is_null() {
            return String::new();
        }
        let mut out = Vec::new();
        let mut off: usize = 0;
        // cap to 4KB to avoid runaway
        while off < 4096 {
            let b = unsafe { *ptr.add(off) as u8 };
            if b == 0 {
                break;
            }
            out.push(b);
            off += 1;
        }
        String::from_utf8_lossy(&out).to_string()
    }
    let lvl = match level {
        0 => "TRACE",
        1 => "DEBUG",
        2 => "INFO",
        3 => "WARN",
        _ => "ERROR",
    };
    let ev = unsafe { cstr_to_string(event) };
    let msg = unsafe { cstr_to_string(message) };
    let fld = unsafe { cstr_to_string(fields) };
    let mut line = String::new();
    line.push_str(lvl);
    if !ev.is_empty() {
        line.push(' ');
        line.push_str(&ev);
    }
    if !msg.is_empty() {
        line.push_str(": ");
        line.push_str(&msg);
    }
    if !fld.is_empty() {
        line.push(' ');
        line.push_str(&fld);
    }
    println!("{}", line);
    0
}

// --- Minimal executor/task/chan/actor/cancellation stubs for the concurrent namespace ---

// Task registry stores completion and detachment flags for bookkeeping.
#[derive(Clone, Copy, Debug, Default)]
struct TaskEntry {
    completed: bool,
    detached: bool,
}

fn task_map() -> &'static Mutex<HashMap<i64, TaskEntry>> {
    static MAP: OnceLock<Mutex<HashMap<i64, TaskEntry>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Join a task and return its result.
/// Returns -1 if the task was cancelled.
/// Returns -2 if the task panicked (and re-panics the calling context).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_join(handle: i64) -> i64 {
    // Cancellation takes priority
    if let Ok(m) = cancel_map().lock() {
        if m.get(&handle).copied().unwrap_or(false) {
            return -1; // cancelled sentinel
        }
    }

    // Check task state for panic
    if let Ok(m) = task_store().lock() {
        if let Some(info) = m.get(&handle) {
            match &info.state {
                TaskState::Completed(result) => {
                    // Mark as joined in task_map
                    if let Ok(mut tm) = task_map().lock() {
                        if let Some(e) = tm.get_mut(&handle) {
                            e.completed = true;
                        }
                    }
                    return *result;
                }
                TaskState::Cancelled => return -1,
                TaskState::Panicked(msg) => {
                    // Propagate panic to the calling task
                    panic_with_message(msg.clone());
                    return -2; // Panicked sentinel
                }
                _ => {}
            }
        }
    }

    // Mark as joined for pending/running tasks
    if let Ok(mut tm) = task_map().lock() {
        if let Some(e) = tm.get_mut(&handle) {
            e.completed = true;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_detach(handle: i64) -> i32 {
    if let Ok(mut tm) = task_map().lock() {
        if let Some(e) = tm.get_mut(&handle) {
            e.detached = true;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_current() -> i64 {
    // Single-threaded deterministic stub: return 0 (no ambient task context).
    0
}

/// Cooperatively yield execution to the scheduler.
///
/// Returns 0 on success.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_yield() -> i64 {
    std::thread::yield_now();
    0
}

/// Check whether the current task has been cancelled.
///
/// Returns 1 if cancelled, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_check_cancelled() -> i64 {
    let current = __arth_task_current();
    if current <= 0 {
        return 0;
    }
    __arth_task_is_cancelled(current) as i64
}

// ============================================================================
// Poll-Based Task Functions (CPS Async Support)
// ============================================================================

/// Async frame storage for poll-based tasks
#[derive(Debug)]
struct PollTaskInfo {
    /// Pointer to the async frame (passed as i64)
    frame_ptr: i64,
    /// Function ID of the poll function
    poll_fn_id: i64,
    /// Current state of the task
    state: TaskState,
    /// Whether the task is detached
    detached: bool,
    /// Result value (when completed)
    result: Option<i64>,
}

fn poll_task_store() -> &'static Mutex<HashMap<i64, PollTaskInfo>> {
    static MAP: OnceLock<Mutex<HashMap<i64, PollTaskInfo>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_POLL_HANDLE: AtomicI64 = AtomicI64::new(100_000);

/// Spawn a task with poll-based execution.
/// This is used by CPS-transformed async functions.
///
/// Arguments:
/// - frame_ptr: Pointer to the allocated async frame
/// - poll_fn_id: Hash of the poll function name for dispatch
///
/// Returns: task handle (i64)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_spawn_with_poll(frame_ptr: i64, poll_fn_id: i64) -> i64 {
    let h = NEXT_POLL_HANDLE.fetch_add(1, Ordering::Relaxed);

    let info = PollTaskInfo {
        frame_ptr,
        poll_fn_id,
        state: TaskState::Pending,
        detached: false,
        result: None,
    };

    if let Ok(mut m) = poll_task_store().lock() {
        m.insert(h, info);
    }
    if let Ok(mut m) = cancel_map().lock() {
        m.insert(h, false);
    }
    h
}

/// Get the frame pointer for a poll-based task.
/// Used by the executor to pass to the poll function.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_get_frame(handle: i64) -> i64 {
    if let Ok(m) = poll_task_store().lock() {
        if let Some(info) = m.get(&handle) {
            return info.frame_ptr;
        }
    }
    0
}

/// Get the poll function ID for a poll-based task.
/// Used by the executor to dispatch to the correct poll function.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_get_poll_fn_id(handle: i64) -> i64 {
    if let Ok(m) = poll_task_store().lock() {
        if let Some(info) = m.get(&handle) {
            return info.poll_fn_id;
        }
    }
    0
}

/// Get the result from a completed task.
/// Used after awaiting a task that has returned Ready.
/// Returns 0 if the task is not completed.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_get_result(handle: i64) -> i64 {
    // First check poll-based tasks
    if let Ok(m) = poll_task_store().lock() {
        if let Some(info) = m.get(&handle) {
            if let Some(result) = info.result {
                return result;
            }
        }
    }

    // Fall back to regular task store
    if let Ok(m) = task_store().lock() {
        if let Some(info) = m.get(&handle) {
            if let TaskState::Completed(result) = info.state {
                return result;
            }
        }
    }
    0
}

/// Set the result for a poll-based task (called when poll returns Ready).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_set_result(handle: i64, result: i64) -> i32 {
    if let Ok(mut m) = poll_task_store().lock() {
        if let Some(info) = m.get_mut(&handle) {
            info.result = Some(result);
            info.state = TaskState::Completed(result);
            return 0;
        }
    }
    1
}

/// Check if a task has been cancelled.
/// Returns 1 if cancelled, 0 otherwise.
/// Poll functions should call this at the start of each poll.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_is_cancelled(handle: i64) -> i32 {
    if let Ok(m) = cancel_map().lock() {
        if m.get(&handle).copied().unwrap_or(false) {
            return 1;
        }
    }
    0
}

/// Mark a poll-based task as pending (needs another poll).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_mark_pending(handle: i64) -> i32 {
    if let Ok(mut m) = poll_task_store().lock() {
        if let Some(info) = m.get_mut(&handle) {
            info.state = TaskState::Pending;
            return 0;
        }
    }
    1
}

/// Check if a task is a poll-based task.
/// Returns 1 if it's a poll-based task, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_is_poll_based(handle: i64) -> i32 {
    if let Ok(m) = poll_task_store().lock() {
        if m.contains_key(&handle) {
            return 1;
        }
    }
    0
}

// ============================================================================
// Task Exception Handling Functions (Async Error Propagation)
// ============================================================================

/// Complete a task with an exception (error state).
/// Called when throw occurs inside an async function.
/// The exception handle is stored so it can be rethrown when awaited.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_set_error(handle: i64, exception: i64) -> i32 {
    // Try poll-based tasks first
    if let Ok(mut m) = poll_task_store().lock() {
        if let Some(info) = m.get_mut(&handle) {
            info.state = TaskState::Error(exception);
            return 0;
        }
    }

    // Fall back to regular task store
    if let Ok(mut m) = task_store().lock() {
        if let Some(info) = m.get_mut(&handle) {
            info.state = TaskState::Error(exception);
            return 0;
        }
    }
    1
}

/// Get the exception from a task that completed with an error.
/// Returns the exception handle if task is in error state, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_get_error(handle: i64) -> i64 {
    // Try poll-based tasks first
    if let Ok(m) = poll_task_store().lock() {
        if let Some(info) = m.get(&handle) {
            if let TaskState::Error(exc) = info.state {
                return exc;
            }
        }
    }

    // Fall back to regular task store
    if let Ok(m) = task_store().lock() {
        if let Some(info) = m.get(&handle) {
            if let TaskState::Error(exc) = info.state {
                return exc;
            }
        }
    }
    0
}

/// Check if a task completed with an error.
/// Returns 1 if task has error, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_has_error(handle: i64) -> i32 {
    // Try poll-based tasks first
    if let Ok(m) = poll_task_store().lock() {
        if let Some(info) = m.get(&handle) {
            if matches!(info.state, TaskState::Error(_)) {
                return 1;
            }
        }
    }

    // Fall back to regular task store
    if let Ok(m) = task_store().lock() {
        if let Some(info) = m.get(&handle) {
            if matches!(info.state, TaskState::Error(_)) {
                return 1;
            }
        }
    }
    0
}

/// Get the state of a task as an integer.
/// Returns: 0 = Pending, 1 = Running, 2 = Completed, 3 = Cancelled, 4 = Panicked, 5 = Error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_get_state(handle: i64) -> i32 {
    // Try poll-based tasks first
    if let Ok(m) = poll_task_store().lock() {
        if let Some(info) = m.get(&handle) {
            return match info.state {
                TaskState::Pending => 0,
                TaskState::Running => 1,
                TaskState::Completed(_) => 2,
                TaskState::Cancelled => 3,
                TaskState::Panicked(_) => 4,
                TaskState::Error(_) => 5,
            };
        }
    }

    // Fall back to regular task store
    if let Ok(m) = task_store().lock() {
        if let Some(info) = m.get(&handle) {
            return match info.state {
                TaskState::Pending => 0,
                TaskState::Running => 1,
                TaskState::Completed(_) => 2,
                TaskState::Cancelled => 3,
                TaskState::Panicked(_) => 4,
                TaskState::Error(_) => 5,
            };
        }
    }
    -1 // Unknown task
}

// ============================================================================
// CancelledError Support
// ============================================================================

/// Type name constant for CancelledError struct
const CANCELLED_ERROR_TYPE_NAME: &str = "concurrent.CancelledError";

/// Create a CancelledError struct for a cancelled task.
/// This is called when a task is cancelled and we need to throw CancelledError.
///
/// CancelledError has the following fields:
///   - taskHandle: int (the handle of the cancelled task)
///   - message: String (default "Task was cancelled")
///
/// Returns: struct handle (i64) for the CancelledError instance
#[unsafe(no_mangle)]
pub extern "C" fn __arth_create_cancelled_error(task_handle: i64) -> i64 {
    // Create the struct with 2 fields
    let h = struct_new(CANCELLED_ERROR_TYPE_NAME.to_string(), 2);

    // Set field 0: taskHandle (int)
    struct_set(h, 0, Value::I64(task_handle), "taskHandle".to_string());

    // Set field 1: message (String)
    struct_set(
        h,
        1,
        Value::Str("Task was cancelled".to_string()),
        "message".to_string(),
    );

    h
}

/// Create a CancelledError struct with a custom message.
///
/// Arguments:
///   - task_handle: The handle of the cancelled task
///   - message: Pointer to a NUL-terminated C string with the message
///
/// Returns: struct handle (i64) for the CancelledError instance
#[unsafe(no_mangle)]
pub extern "C" fn __arth_create_cancelled_error_with_message(
    task_handle: i64,
    message: *const i8,
) -> i64 {
    let msg = if message.is_null() {
        "Task was cancelled".to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };

    // Create the struct with 2 fields
    let h = struct_new(CANCELLED_ERROR_TYPE_NAME.to_string(), 2);

    // Set field 0: taskHandle (int)
    struct_set(h, 0, Value::I64(task_handle), "taskHandle".to_string());

    // Set field 1: message (String)
    struct_set(h, 1, Value::Str(msg), "message".to_string());

    h
}

/// Check if an exception is a CancelledError by examining its type name.
/// Returns 1 if it's a CancelledError, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_is_cancelled_error(exception_handle: i64) -> i32 {
    if let Ok(m) = struct_store().lock() {
        if let Some(s) = m.get(&exception_handle) {
            if s.type_name == CANCELLED_ERROR_TYPE_NAME {
                return 1;
            }
        }
    }
    0
}

// ============================================================================
// End Poll-Based Task Functions
// ============================================================================

// Bounded channel of i64 payloads (opaque value from the IR demo path).
#[derive(Debug)]
struct Channel {
    cap: usize,
    q: VecDeque<i64>,
    closed: bool,
}

fn chan_map() -> &'static Mutex<HashMap<i64, Channel>> {
    static MAP: OnceLock<Mutex<HashMap<i64, Channel>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_CHAN: AtomicI64 = AtomicI64::new(10_000);

#[unsafe(no_mangle)]
pub extern "C" fn __arth_chan_create(cap: i64) -> i64 {
    let h = NEXT_CHAN.fetch_add(1, Ordering::Relaxed);
    let ch = Channel {
        cap: cap.max(0) as usize,
        q: VecDeque::new(),
        closed: false,
    };
    if let Ok(mut m) = chan_map().lock() {
        m.insert(h, ch);
    }
    h
}

// Returns: 0 = ok, 1 = full, 2 = closed
#[unsafe(no_mangle)]
pub extern "C" fn __arth_chan_send(handle: i64, val: i64) -> i32 {
    if let Ok(mut m) = chan_map().lock() {
        if let Some(ch) = m.get_mut(&handle) {
            if ch.closed {
                return 2;
            }
            if ch.q.len() >= ch.cap && ch.cap > 0 {
                return 1;
            }
            ch.q.push_back(val);
            return 0;
        }
    }
    2
}

// Returns next value or sentinel:
//  -2 = empty (would block), -3 = closed and empty
#[unsafe(no_mangle)]
pub extern "C" fn __arth_chan_recv(handle: i64) -> i64 {
    if let Ok(mut m) = chan_map().lock() {
        if let Some(ch) = m.get_mut(&handle) {
            if let Some(v) = ch.q.pop_front() {
                return v;
            }
            return if ch.closed { -3 } else { -2 };
        }
    }
    -3
}

#[unsafe(no_mangle)]
pub extern "C" fn __arth_chan_close(handle: i64) -> i32 {
    if let Ok(mut m) = chan_map().lock() {
        if let Some(ch) = m.get_mut(&handle) {
            ch.closed = true;
            return 0;
        }
    }
    0
}

// ============================================================================
// MPMC Channels - Thread-safe Multi-Producer Multi-Consumer channels
// ============================================================================
// Uses crossbeam-channel for lock-free MPMC communication.
// These channels can be safely shared across multiple threads.

use crossbeam_channel::{bounded, unbounded, Receiver, Sender, TryRecvError, TrySendError};
use parking_lot::Mutex as ParkingMutex;

/// MPMC channel that can be shared across threads
/// Enhanced with waiting task queues for executor integration (C07/C08)
struct MpmcChannel {
    /// Sender end (cloneable for multiple producers)
    sender: Sender<i64>,
    /// Receiver end (cloneable for multiple consumers)
    receiver: Receiver<i64>,
    /// Capacity (0 for unbounded)
    capacity: usize,
    /// Whether the channel has been explicitly closed
    closed: AtomicBool,
    /// Number of senders (for tracking)
    tx_count: AtomicUsize,
    /// Number of receivers (for tracking)
    rx_count: AtomicUsize,
    /// Task IDs waiting to send (blocked on full channel)
    /// Each entry is (task_id, value_to_send)
    waiting_senders: ParkingMutex<Vec<(u64, i64)>>,
    /// Task IDs waiting to receive (blocked on empty channel)
    waiting_receivers: ParkingMutex<Vec<u64>>,
}

impl MpmcChannel {
    /// Create a new bounded MPMC channel
    fn bounded(capacity: usize) -> Self {
        let (sender, receiver) = bounded(capacity);
        Self {
            sender,
            receiver,
            capacity,
            closed: AtomicBool::new(false),
            tx_count: AtomicUsize::new(1),
            rx_count: AtomicUsize::new(1),
            waiting_senders: ParkingMutex::new(Vec::new()),
            waiting_receivers: ParkingMutex::new(Vec::new()),
        }
    }

    /// Create a new unbounded MPMC channel
    fn unbounded() -> Self {
        let (sender, receiver) = unbounded();
        Self {
            sender,
            receiver,
            capacity: 0,
            closed: AtomicBool::new(false),
            tx_count: AtomicUsize::new(1),
            rx_count: AtomicUsize::new(1),
            waiting_senders: ParkingMutex::new(Vec::new()),
            waiting_receivers: ParkingMutex::new(Vec::new()),
        }
    }

    /// Add a task to the waiting senders queue
    fn add_waiting_sender(&self, task_id: u64, value: i64) {
        self.waiting_senders.lock().push((task_id, value));
    }

    /// Pop a waiting sender (returns task_id, value) or None if empty
    fn pop_waiting_sender(&self) -> Option<(u64, i64)> {
        let mut senders = self.waiting_senders.lock();
        if senders.is_empty() {
            None
        } else {
            Some(senders.remove(0)) // FIFO order
        }
    }

    /// Get count of waiting senders
    fn waiting_sender_count(&self) -> usize {
        self.waiting_senders.lock().len()
    }

    /// Add a task to the waiting receivers queue
    fn add_waiting_receiver(&self, task_id: u64) {
        self.waiting_receivers.lock().push(task_id);
    }

    /// Pop a waiting receiver (returns task_id) or None if empty
    fn pop_waiting_receiver(&self) -> Option<u64> {
        let mut receivers = self.waiting_receivers.lock();
        if receivers.is_empty() {
            None
        } else {
            Some(receivers.remove(0)) // FIFO order
        }
    }

    /// Get count of waiting receivers
    fn waiting_receiver_count(&self) -> usize {
        self.waiting_receivers.lock().len()
    }

    /// Check if the channel is closed
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Mark the channel as closed
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// Get current length (number of messages in the channel)
    fn len(&self) -> usize {
        self.receiver.len()
    }

    /// Check if the channel is empty
    fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    /// Check if the channel is full (always false for unbounded)
    fn is_full(&self) -> bool {
        if self.capacity == 0 {
            false
        } else {
            self.receiver.len() >= self.capacity
        }
    }
}

fn mpmc_chan_map() -> &'static Mutex<HashMap<i64, MpmcChannel>> {
    static MAP: OnceLock<Mutex<HashMap<i64, MpmcChannel>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_MPMC_CHAN: AtomicI64 = AtomicI64::new(50_000);

/// Create a new MPMC channel with the specified capacity.
/// If capacity <= 0, creates an unbounded channel.
/// Returns: channel handle (i64)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_create(capacity: i64) -> i64 {
    let h = NEXT_MPMC_CHAN.fetch_add(1, Ordering::Relaxed);
    let ch = if capacity <= 0 {
        MpmcChannel::unbounded()
    } else {
        MpmcChannel::bounded(capacity as usize)
    };
    if let Ok(mut m) = mpmc_chan_map().lock() {
        m.insert(h, ch);
    }
    h
}

/// Non-blocking send to an MPMC channel.
/// Returns:
///   0 = success
///   1 = channel full (would block)
///   2 = channel closed
///   3 = channel not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_send(handle: i64, val: i64) -> i32 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            if ch.is_closed() {
                return 2; // closed
            }
            match ch.sender.try_send(val) {
                Ok(()) => return 0, // success
                Err(TrySendError::Full(_)) => return 1, // full
                Err(TrySendError::Disconnected(_)) => return 2, // disconnected
            }
        }
    }
    3 // channel not found
}

/// Blocking send to an MPMC channel.
/// Blocks until the message is sent or the channel is closed.
/// Returns:
///   0 = success
///   2 = channel closed/disconnected
///   3 = channel not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_send_blocking(handle: i64, val: i64) -> i32 {
    // We need to get the sender clone outside the lock to avoid deadlock
    let sender_opt = {
        if let Ok(m) = mpmc_chan_map().lock() {
            m.get(&handle).map(|ch| {
                if ch.is_closed() {
                    None
                } else {
                    Some(ch.sender.clone())
                }
            }).flatten()
        } else {
            None
        }
    };

    match sender_opt {
        Some(sender) => {
            match sender.send(val) {
                Ok(()) => 0, // success
                Err(_) => 2, // disconnected
            }
        }
        None => {
            // Check if channel exists but is closed, or doesn't exist
            if let Ok(m) = mpmc_chan_map().lock() {
                if m.contains_key(&handle) {
                    return 2; // closed
                }
            }
            3 // not found
        }
    }
}

/// Non-blocking receive from an MPMC channel.
/// Returns:
///   The received value if successful
///   -2 = channel empty (would block)
///   -3 = channel closed and empty
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_recv(handle: i64) -> i64 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            match ch.receiver.try_recv() {
                Ok(val) => return val,
                Err(TryRecvError::Empty) => {
                    return if ch.is_closed() { -3 } else { -2 };
                }
                Err(TryRecvError::Disconnected) => return -3, // disconnected = closed
            }
        }
    }
    -3 // channel not found = treat as closed
}

/// Blocking receive from an MPMC channel.
/// Blocks until a message is received or the channel is closed.
/// Returns:
///   The received value if successful
///   -3 = channel closed and empty
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_recv_blocking(handle: i64) -> i64 {
    // We need to get the receiver clone outside the lock to avoid deadlock
    let receiver_opt = {
        if let Ok(m) = mpmc_chan_map().lock() {
            m.get(&handle).map(|ch| ch.receiver.clone())
        } else {
            None
        }
    };

    match receiver_opt {
        Some(receiver) => {
            match receiver.recv() {
                Ok(val) => val,
                Err(_) => -3, // disconnected = closed
            }
        }
        None => -3 // not found = closed
    }
}

/// Close an MPMC channel.
/// After closing, sends will fail and receives will drain remaining messages.
/// Returns:
///   0 = success
///   1 = channel not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_close(handle: i64) -> i32 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            ch.close();
            return 0;
        }
    }
    1 // not found
}

/// Get the current length of an MPMC channel (number of queued messages).
/// Returns:
///   The number of messages in the channel
///   -1 = channel not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_len(handle: i64) -> i64 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            return ch.len() as i64;
        }
    }
    -1 // not found
}

/// Check if an MPMC channel is empty.
/// Returns:
///   1 = empty
///   0 = not empty
///   -1 = channel not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_is_empty(handle: i64) -> i32 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            return if ch.is_empty() { 1 } else { 0 };
        }
    }
    -1 // not found
}

/// Check if an MPMC channel is full.
/// Returns:
///   1 = full
///   0 = not full
///   -1 = channel not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_is_full(handle: i64) -> i32 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            return if ch.is_full() { 1 } else { 0 };
        }
    }
    -1 // not found
}

/// Check if an MPMC channel is closed.
/// Returns:
///   1 = closed
///   0 = open
///   -1 = channel not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_is_closed(handle: i64) -> i32 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            return if ch.is_closed() { 1 } else { 0 };
        }
    }
    -1 // not found
}

/// Get the capacity of an MPMC channel.
/// Returns:
///   The capacity (0 for unbounded)
///   -1 = channel not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_capacity(handle: i64) -> i64 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            return ch.capacity as i64;
        }
    }
    -1 // not found
}

// ============================================================================
// C07: Executor-Integrated Channel Operations
// ============================================================================
// These functions integrate with the executor's task suspension mechanism.
// Instead of blocking the worker thread, they return status codes that tell
// the executor to suspend the task and re-queue it when space/data is available.

/// Try to send a value to an MPMC channel, with task suspension support.
/// If the channel is full, registers the task as waiting and returns a suspend status.
///
/// Parameters:
///   handle: channel handle
///   value: value to send
///   task_id: ID of the sending task (for suspension tracking)
///
/// Returns:
///   0 = success (value sent)
///   1 = channel full, task registered as waiting (suspend task)
///   2 = channel closed
///   3 = channel not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_send_with_task(handle: i64, value: i64, task_id: u64) -> i32 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            if ch.is_closed() {
                return 2; // closed
            }
            match ch.sender.try_send(value) {
                Ok(()) => return 0, // success
                Err(TrySendError::Full(_)) => {
                    // Channel is full - register task as waiting sender
                    ch.add_waiting_sender(task_id, value);
                    return 1; // suspend
                }
                Err(TrySendError::Disconnected(_)) => return 2, // disconnected
            }
        }
    }
    3 // channel not found
}

/// Try to receive a value from an MPMC channel, with task suspension support.
/// If the channel is empty, registers the task as waiting and returns a suspend status.
///
/// Parameters:
///   handle: channel handle
///   task_id: ID of the receiving task (for suspension tracking)
///
/// Returns packed result:
///   High 32 bits: status (0=success, 1=empty/suspend, 2=closed)
///   Low 32 bits: value (if success) or 0
///
/// For better ergonomics, use the separate status and value functions.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_recv_with_task(handle: i64, task_id: u64) -> i64 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            match ch.receiver.try_recv() {
                Ok(val) => {
                    // Got a value - also check if any senders are waiting to wake
                    return val; // success, return the value directly
                }
                Err(TryRecvError::Empty) => {
                    if ch.is_closed() {
                        return -3; // closed and empty
                    }
                    // Channel is empty - register task as waiting receiver
                    ch.add_waiting_receiver(task_id);
                    return -2; // empty, should suspend
                }
                Err(TryRecvError::Disconnected) => return -3, // disconnected = closed
            }
        }
    }
    -3 // channel not found = closed
}

/// Receive a value and wake any waiting senders.
/// This is the key function for C07: after receiving, wake up a blocked sender.
///
/// Parameters:
///   handle: channel handle
///
/// Returns:
///   The received value, or -2 (empty) or -3 (closed)
///   Also returns the task_id of a woken sender via the out parameter.
///
/// Note: Woken sender task ID is stored and can be retrieved via
/// __arth_mpmc_chan_get_woken_sender().
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_recv_and_wake(handle: i64) -> i64 {
    // First, try to receive
    let result = __arth_mpmc_chan_recv(handle);

    // If we successfully received, check for waiting senders
    if result >= 0 || (result != -2 && result != -3) {
        // We got a value - now there's space, so wake a waiting sender
        if let Ok(m) = mpmc_chan_map().lock() {
            if let Some(ch) = m.get(&handle) {
                if let Some((sender_task_id, value)) = ch.pop_waiting_sender() {
                    // Send the waiting sender's value
                    let _ = ch.sender.try_send(value);
                    // Store the woken task ID for the executor to retrieve
                    store_woken_sender(sender_task_id);
                }
            }
        }
    }

    result
}

/// Pop a waiting sender from the channel (used when space becomes available).
/// Returns the task ID and value, or (0, 0) if no waiting senders.
///
/// Parameters:
///   handle: channel handle
///
/// Returns: task_id (0 if none waiting)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_pop_waiting_sender(handle: i64) -> u64 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            if let Some((task_id, value)) = ch.pop_waiting_sender() {
                // Store the value for later retrieval
                store_waiting_sender_value(value);
                return task_id;
            }
        }
    }
    0 // no waiting senders
}

/// Get the value associated with the last popped waiting sender.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_get_waiting_sender_value() -> i64 {
    get_waiting_sender_value()
}

/// Pop a waiting receiver from the channel (used when data becomes available).
/// Returns the task ID, or 0 if no waiting receivers.
///
/// Parameters:
///   handle: channel handle
///
/// Returns: task_id (0 if none waiting)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_pop_waiting_receiver(handle: i64) -> u64 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            if let Some(task_id) = ch.pop_waiting_receiver() {
                return task_id;
            }
        }
    }
    0 // no waiting receivers
}

/// Get count of waiting senders on a channel.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_waiting_sender_count(handle: i64) -> i64 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            return ch.waiting_sender_count() as i64;
        }
    }
    -1
}

/// Get count of waiting receivers on a channel.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_waiting_receiver_count(handle: i64) -> i64 {
    if let Ok(m) = mpmc_chan_map().lock() {
        if let Some(ch) = m.get(&handle) {
            return ch.waiting_receiver_count() as i64;
        }
    }
    -1
}

// Thread-local storage for woken task communication
thread_local! {
    static WOKEN_SENDER_TASK_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static WOKEN_RECEIVER_TASK_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static WAITING_SENDER_VALUE: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };
}

fn store_woken_sender(task_id: u64) {
    WOKEN_SENDER_TASK_ID.with(|cell| cell.set(task_id));
}

fn store_woken_receiver(task_id: u64) {
    WOKEN_RECEIVER_TASK_ID.with(|cell| cell.set(task_id));
}

/// Get the task ID of the sender that was woken by the last recv_and_wake call.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_get_woken_sender() -> u64 {
    WOKEN_SENDER_TASK_ID.with(|cell| {
        let id = cell.get();
        cell.set(0); // clear after reading
        id
    })
}

/// Get the task ID of the receiver that was woken by the last send_and_wake call.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_get_woken_receiver() -> u64 {
    WOKEN_RECEIVER_TASK_ID.with(|cell| {
        let id = cell.get();
        cell.set(0); // clear after reading
        id
    })
}

fn store_waiting_sender_value(value: i64) {
    WAITING_SENDER_VALUE.with(|cell| cell.set(value));
}

fn get_waiting_sender_value() -> i64 {
    WAITING_SENDER_VALUE.with(|cell| {
        let val = cell.get();
        cell.set(0); // clear after reading
        val
    })
}

// ============================================================================
// C08: Send and Wake Waiting Receivers
// ============================================================================

/// Send a value and wake any waiting receivers.
/// This is the key function for C08: after sending, wake up a blocked receiver.
///
/// Parameters:
///   handle: channel handle
///   value: value to send
///
/// Returns:
///   0 = success (value sent)
///   1 = channel full (would block)
///   2 = channel closed
///   3 = channel not found
///
/// Side effect: The woken receiver's task ID can be retrieved via
/// __arth_mpmc_chan_get_woken_receiver().
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_send_and_wake(handle: i64, value: i64) -> i32 {
    // First, try to send
    let send_result = __arth_mpmc_chan_send(handle, value);

    // If we successfully sent, check for waiting receivers to wake
    if send_result == 0 {
        // We sent a value - now there's data, so wake a waiting receiver
        if let Ok(m) = mpmc_chan_map().lock() {
            if let Some(ch) = m.get(&handle) {
                if let Some(receiver_task_id) = ch.pop_waiting_receiver() {
                    // Store the woken receiver task ID for the executor to retrieve
                    store_woken_receiver(receiver_task_id);
                }
            }
        }
    }

    send_result
}

// ============================================================================
// C09: Channel Select - Wait on Multiple Channels
// ============================================================================
// Select allows waiting on multiple channels simultaneously, receiving from
// whichever channel becomes ready first. This is essential for multiplexing
// I/O or handling multiple message sources.

use std::cell::RefCell;

thread_local! {
    /// Channel handles registered for the current select operation
    static SELECT_CHANNELS: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
    /// Index of the channel that was ready (or -1 if none)
    static SELECT_RESULT_INDEX: std::cell::Cell<i64> = const { std::cell::Cell::new(-1) };
    /// Value received from the ready channel
    static SELECT_RESULT_VALUE: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };
}

/// Clear the select channel set for a new select operation.
/// Must be called before adding channels with select_add.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_select_clear() {
    SELECT_CHANNELS.with(|channels| {
        channels.borrow_mut().clear();
    });
    SELECT_RESULT_INDEX.with(|cell| cell.set(-1));
    SELECT_RESULT_VALUE.with(|cell| cell.set(0));
}

/// Add a channel handle to the select set.
/// Returns the index of the channel in the select set (0-based).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_select_add(handle: i64) -> i64 {
    SELECT_CHANNELS.with(|channels| {
        let mut ch = channels.borrow_mut();
        let index = ch.len() as i64;
        ch.push(handle);
        index
    })
}

/// Get the number of channels in the current select set.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_select_count() -> i64 {
    SELECT_CHANNELS.with(|channels| channels.borrow().len() as i64)
}

/// Non-blocking try to receive from any channel in the select set.
/// Checks channels in order (index 0, 1, 2, ...) and returns as soon as
/// one has data available.
///
/// Returns:
///   0 = received a value (use select_get_ready_index and select_get_value)
///   1 = no channel ready (all empty)
///   2 = all channels closed
///   3 = no channels in select set
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_try_select_recv() -> i32 {
    let handles: Vec<i64> = SELECT_CHANNELS.with(|channels| channels.borrow().clone());

    if handles.is_empty() {
        return 3; // no channels
    }

    let mut all_closed = true;

    // Try each channel in order
    for (index, &handle) in handles.iter().enumerate() {
        if let Ok(m) = mpmc_chan_map().lock() {
            if let Some(ch) = m.get(&handle) {
                if !ch.is_closed() {
                    all_closed = false;
                }
                match ch.receiver.try_recv() {
                    Ok(val) => {
                        // Got a value from this channel
                        SELECT_RESULT_INDEX.with(|cell| cell.set(index as i64));
                        SELECT_RESULT_VALUE.with(|cell| cell.set(val));
                        return 0; // success
                    }
                    Err(TryRecvError::Empty) => {
                        // This channel is empty, try next
                        continue;
                    }
                    Err(TryRecvError::Disconnected) => {
                        // This channel is disconnected, try next
                        continue;
                    }
                }
            }
        }
    }

    if all_closed {
        2 // all closed
    } else {
        1 // none ready
    }
}

/// Blocking select receive using crossbeam's Select API.
/// Blocks until one of the channels has data available.
///
/// Returns:
///   0 = received a value (use select_get_ready_index and select_get_value)
///   2 = all channels closed
///   3 = no channels in select set
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_select_recv_blocking() -> i32 {
    use crossbeam_channel::Select;

    let handles: Vec<i64> = SELECT_CHANNELS.with(|channels| channels.borrow().clone());

    if handles.is_empty() {
        return 3; // no channels
    }

    // Collect receivers outside the lock to avoid deadlock
    let receivers: Vec<Option<Receiver<i64>>> = {
        if let Ok(m) = mpmc_chan_map().lock() {
            handles
                .iter()
                .map(|&h| m.get(&h).map(|ch| ch.receiver.clone()))
                .collect()
        } else {
            return 3; // lock failed
        }
    };

    // Filter out None entries and build the select
    let valid_receivers: Vec<(usize, Receiver<i64>)> = receivers
        .into_iter()
        .enumerate()
        .filter_map(|(i, opt)| opt.map(|r| (i, r)))
        .collect();

    if valid_receivers.is_empty() {
        return 2; // all channels not found or closed
    }

    let mut sel = Select::new();
    for (_, receiver) in &valid_receivers {
        sel.recv(receiver);
    }

    // Block until one is ready
    let oper = sel.select();
    let select_index = oper.index();

    // Get the original index and receive the value
    if select_index < valid_receivers.len() {
        let (original_index, ref receiver) = valid_receivers[select_index];
        match oper.recv(receiver) {
            Ok(val) => {
                SELECT_RESULT_INDEX.with(|cell| cell.set(original_index as i64));
                SELECT_RESULT_VALUE.with(|cell| cell.set(val));
                return 0; // success
            }
            Err(_) => {
                // Channel disconnected during select
                return 2;
            }
        }
    }

    2 // unexpected
}

/// Try select receive with task registration.
/// If no channel has data, registers the task as a waiting receiver on ALL channels.
///
/// Parameters:
///   task_id: ID of the task to register if blocked
///
/// Returns:
///   0 = received a value (use select_get_ready_index and select_get_value)
///   1 = no channel ready, task registered on all channels (suspend task)
///   2 = all channels closed
///   3 = no channels in select set
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_select_recv_with_task(task_id: u64) -> i32 {
    let handles: Vec<i64> = SELECT_CHANNELS.with(|channels| channels.borrow().clone());

    if handles.is_empty() {
        return 3; // no channels
    }

    let mut all_closed = true;

    // First pass: try to receive from any channel
    for (index, &handle) in handles.iter().enumerate() {
        if let Ok(m) = mpmc_chan_map().lock() {
            if let Some(ch) = m.get(&handle) {
                if !ch.is_closed() {
                    all_closed = false;
                }
                match ch.receiver.try_recv() {
                    Ok(val) => {
                        // Got a value from this channel
                        SELECT_RESULT_INDEX.with(|cell| cell.set(index as i64));
                        SELECT_RESULT_VALUE.with(|cell| cell.set(val));
                        return 0; // success
                    }
                    Err(TryRecvError::Empty) => continue,
                    Err(TryRecvError::Disconnected) => continue,
                }
            }
        }
    }

    if all_closed {
        return 2; // all closed
    }

    // Second pass: register as waiter on all non-closed channels
    if let Ok(m) = mpmc_chan_map().lock() {
        for &handle in &handles {
            if let Some(ch) = m.get(&handle) {
                if !ch.is_closed() {
                    ch.add_waiting_receiver(task_id);
                }
            }
        }
    }

    1 // suspended, waiting on all channels
}

/// Get the index of the channel that was ready in the last select operation.
/// Returns -1 if no channel was ready.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_select_get_ready_index() -> i64 {
    SELECT_RESULT_INDEX.with(|cell| cell.get())
}

/// Get the value received in the last select operation.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_select_get_value() -> i64 {
    SELECT_RESULT_VALUE.with(|cell| cell.get())
}

/// Deregister a task from all channels in the select set except one.
/// Called when a select completes to clean up the other waiting registrations.
///
/// Parameters:
///   task_id: ID of the task to deregister
///   except_index: Index of the channel that was ready (don't deregister from this one)
///                 Use -1 to deregister from all channels.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_select_deregister(task_id: u64, except_index: i64) {
    let handles: Vec<i64> = SELECT_CHANNELS.with(|channels| channels.borrow().clone());

    if let Ok(m) = mpmc_chan_map().lock() {
        for (index, &handle) in handles.iter().enumerate() {
            if index as i64 == except_index {
                continue; // Skip the channel that was ready
            }
            if let Some(ch) = m.get(&handle) {
                // Remove this task from the waiting receivers list
                let mut receivers = ch.waiting_receivers.lock();
                receivers.retain(|&id| id != task_id);
            }
        }
    }
}

/// Get the handle of a channel in the select set by index.
/// Returns -1 if index is out of bounds.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mpmc_chan_select_get_handle(index: i64) -> i64 {
    SELECT_CHANNELS.with(|channels| {
        let ch = channels.borrow();
        if index >= 0 && (index as usize) < ch.len() {
            ch[index as usize]
        } else {
            -1
        }
    })
}

// ============================================================================
// C11: Actor = Task + Channel
// ============================================================================
// Actors are tasks with mailboxes (MPMC channels) for message passing.
// Each actor runs on the thread pool like any other task and processes
// messages from its mailbox in FIFO order.

/// Actor state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum ActorState {
    /// Actor is running and processing messages
    Running = 0,
    /// Actor has been requested to stop gracefully
    Stopping = 1,
    /// Actor has stopped (mailbox closed)
    Stopped = 2,
    /// Actor encountered an error/panic
    Failed = 3,
}

/// C11-compliant Actor: Task + Channel
/// An actor has:
/// - A task handle (for the message loop)
/// - A mailbox (MPMC channel for incoming messages)
/// - State tracking (running, stopping, stopped, failed)
#[derive(Debug)]
struct Actor {
    /// Handle to the underlying task (0 if not yet spawned as task)
    task_handle: i64,
    /// Handle to the MPMC mailbox channel
    mailbox: i64,
    /// Current state of the actor
    state: AtomicU8,
    /// Message count (for testing/stats)
    message_count: AtomicUsize,
}

impl Actor {
    fn new(mailbox_capacity: i64) -> Self {
        let mailbox = __arth_mpmc_chan_create(mailbox_capacity);
        Self {
            task_handle: 0,
            mailbox,
            state: AtomicU8::new(ActorState::Running as u8),
            message_count: AtomicUsize::new(0),
        }
    }

    fn get_state(&self) -> ActorState {
        match self.state.load(Ordering::SeqCst) {
            0 => ActorState::Running,
            1 => ActorState::Stopping,
            2 => ActorState::Stopped,
            _ => ActorState::Failed,
        }
    }

    fn set_state(&self, state: ActorState) {
        self.state.store(state as u8, Ordering::SeqCst);
    }

    fn is_running(&self) -> bool {
        self.get_state() == ActorState::Running
    }

    fn increment_message_count(&self) {
        self.message_count.fetch_add(1, Ordering::SeqCst);
    }
}

fn actor_map() -> &'static Mutex<HashMap<i64, Actor>> {
    static MAP: OnceLock<Mutex<HashMap<i64, Actor>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_ACTOR: AtomicI64 = AtomicI64::new(20_000);

/// Create a new actor with the specified mailbox capacity.
/// The actor is in Running state but not yet associated with a task.
///
/// Parameters:
///   capacity: mailbox capacity (use 0 for unbounded)
///
/// Returns: actor handle (i64)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_create(cap: i64) -> i64 {
    let h = NEXT_ACTOR.fetch_add(1, Ordering::Relaxed);
    let actor = Actor::new(cap);
    if let Ok(mut m) = actor_map().lock() {
        m.insert(h, actor);
    }
    h
}

/// Spawn an actor with a task that will run the message loop.
/// The task_handle is stored in the actor for later reference.
///
/// Parameters:
///   capacity: mailbox capacity
///   task_handle: handle of the task running the actor loop
///
/// Returns: actor handle (i64)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_spawn(cap: i64, task_handle: i64) -> i64 {
    let h = NEXT_ACTOR.fetch_add(1, Ordering::Relaxed);
    let mut actor = Actor::new(cap);
    actor.task_handle = task_handle;
    if let Ok(mut m) = actor_map().lock() {
        m.insert(h, actor);
    }
    h
}

/// Non-blocking send to an actor's mailbox.
///
/// Parameters:
///   handle: actor handle
///   val: message value
///
/// Returns:
///   0 = success
///   1 = mailbox full
///   2 = actor stopped/stopping
///   3 = actor not found
///   4 = actor failed (crashed)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_send(handle: i64, val: i64) -> i32 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            // Check for failed state first
            if actor.get_state() == ActorState::Failed {
                return 4; // failed/crashed
            }
            // Reject sends when actor is not running (Stopping or Stopped)
            // For graceful stop: no new messages accepted, only drain existing
            if !actor.is_running() {
                return 2; // stopped or stopping
            }
            let result = __arth_mpmc_chan_send(actor.mailbox, val);
            if result == 0 {
                actor.increment_message_count();
            }
            return result;
        }
    }
    3 // not found
}

/// Blocking send to an actor's mailbox.
///
/// Parameters:
///   handle: actor handle
///   val: message value
///
/// Returns:
///   0 = success
///   2 = actor stopped/stopping
///   3 = actor not found
///   4 = actor failed (crashed)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_send_blocking(handle: i64, val: i64) -> i32 {
    let mailbox = {
        if let Ok(m) = actor_map().lock() {
            if let Some(actor) = m.get(&handle) {
                // Check for failed state first
                if actor.get_state() == ActorState::Failed {
                    return 4; // failed/crashed
                }
                // Reject sends when actor is not running (Stopping or Stopped)
                // For graceful stop: no new messages accepted, only drain existing
                if !actor.is_running() {
                    return 2; // stopped or stopping
                }
                Some(actor.mailbox)
            } else {
                None
            }
        } else {
            None
        }
    };

    match mailbox {
        Some(mb) => {
            let result = __arth_mpmc_chan_send_blocking(mb, val);
            if result == 0 {
                if let Ok(m) = actor_map().lock() {
                    if let Some(actor) = m.get(&handle) {
                        actor.increment_message_count();
                    }
                }
            }
            result
        }
        None => 3, // not found
    }
}

/// Non-blocking receive from an actor's mailbox.
/// Used by the actor's message loop to receive messages.
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   The message value on success
///   -2 = mailbox empty (would block)
///   -3 = actor stopped/closed
///   -4 = actor failed (crashed)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_recv(handle: i64) -> i64 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            // Check for failed state first
            if actor.get_state() == ActorState::Failed {
                return -4; // failed/crashed
            }
            return __arth_mpmc_chan_recv(actor.mailbox);
        }
    }
    -3 // not found = closed
}

/// Blocking receive from an actor's mailbox.
/// Used by the actor's message loop to receive messages.
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   The message value on success
///   -3 = actor stopped/closed
///   -4 = actor failed (crashed)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_recv_blocking(handle: i64) -> i64 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            // Check for failed state first
            if actor.get_state() == ActorState::Failed {
                return -4; // failed/crashed
            }
        }
    }

    let mailbox = {
        if let Ok(m) = actor_map().lock() {
            m.get(&handle).map(|actor| actor.mailbox)
        } else {
            None
        }
    };

    match mailbox {
        Some(mb) => __arth_mpmc_chan_recv_blocking(mb),
        None => -3, // not found = closed
    }
}

/// Close an actor's mailbox (request stop).
/// After closing, no new messages can be sent but existing messages can be drained.
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   0 = success
///   1 = actor not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_close(handle: i64) -> i32 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            actor.set_state(ActorState::Stopping);
            return __arth_mpmc_chan_close(actor.mailbox);
        }
    }
    1 // not found
}

/// Request graceful stop of an actor.
/// The actor will process remaining messages before stopping.
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   0 = success
///   1 = actor not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_stop(handle: i64) -> i32 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            actor.set_state(ActorState::Stopping);
            // Close the mailbox to signal stop
            __arth_mpmc_chan_close(actor.mailbox);
            return 0;
        }
    }
    1 // not found
}

/// Mark an actor as stopped.
/// Called when the actor's message loop exits.
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   0 = success
///   1 = actor not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_mark_stopped(handle: i64) -> i32 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            actor.set_state(ActorState::Stopped);
            return 0;
        }
    }
    1 // not found
}

/// Mark an actor as failed (crashed).
/// Called when the actor encounters an unrecoverable error.
/// The mailbox is closed and pending messages are discarded.
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   0 = success
///   1 = actor not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_mark_failed(handle: i64) -> i32 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            actor.set_state(ActorState::Failed);
            // Close mailbox to prevent further operations
            __arth_mpmc_chan_close(actor.mailbox);
            return 0;
        }
    }
    1 // not found
}

/// Check if an actor has failed (crashed).
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   1 = failed
///   0 = not failed (running, stopping, or stopped)
///   -1 = actor not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_is_failed(handle: i64) -> i32 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            return if actor.get_state() == ActorState::Failed { 1 } else { 0 };
        }
    }
    -1 // not found
}

/// Get the task handle of an actor.
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   task handle, or 0 if not spawned as task, or -1 if not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_get_task(handle: i64) -> i64 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            return actor.task_handle;
        }
    }
    -1 // not found
}

/// Get the mailbox channel handle of an actor.
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   mailbox channel handle, or -1 if not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_get_mailbox(handle: i64) -> i64 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            return actor.mailbox;
        }
    }
    -1 // not found
}

/// Check if an actor is running.
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   1 = running
///   0 = not running (stopping, stopped, or failed)
///   -1 = actor not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_is_running(handle: i64) -> i32 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            return if actor.is_running() { 1 } else { 0 };
        }
    }
    -1 // not found
}

/// Get the state of an actor.
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   0 = running
///   1 = stopping
///   2 = stopped
///   3 = failed
///   -1 = actor not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_get_state(handle: i64) -> i32 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            return actor.get_state() as i32;
        }
    }
    -1 // not found
}

/// Get the message count of an actor (for stats/testing).
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   message count, or -1 if not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_message_count(handle: i64) -> i64 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            return actor.message_count.load(Ordering::SeqCst) as i64;
        }
    }
    -1 // not found
}

/// Check if an actor's mailbox is empty.
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   1 = empty
///   0 = not empty
///   -1 = actor not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_mailbox_empty(handle: i64) -> i32 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            return __arth_mpmc_chan_is_empty(actor.mailbox);
        }
    }
    -1 // not found
}

/// Get the length of an actor's mailbox.
///
/// Parameters:
///   handle: actor handle
///
/// Returns:
///   mailbox length, or -1 if not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_mailbox_len(handle: i64) -> i64 {
    if let Ok(m) = actor_map().lock() {
        if let Some(actor) = m.get(&handle) {
            return __arth_mpmc_chan_len(actor.mailbox);
        }
    }
    -1 // not found
}

/// Set the task handle for an actor.
/// Used after spawning the actor's task on the executor.
///
/// Parameters:
///   handle: actor handle
///   task_handle: task handle to associate
///
/// Returns:
///   0 = success
///   1 = actor not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_actor_set_task(handle: i64, task_handle: i64) -> i32 {
    if let Ok(mut m) = actor_map().lock() {
        if let Some(actor) = m.get_mut(&handle) {
            actor.task_handle = task_handle;
            return 0;
        }
    }
    1 // not found
}

// Cancellation tokens
fn token_map() -> &'static Mutex<HashMap<i64, bool>> {
    static MAP: OnceLock<Mutex<HashMap<i64, bool>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_TOKEN: AtomicI64 = AtomicI64::new(30_000);

#[unsafe(no_mangle)]
pub extern "C" fn __arth_cancel_token_new() -> i64 {
    let h = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut m) = token_map().lock() {
        m.insert(h, false);
    }
    h
}

#[unsafe(no_mangle)]
pub extern "C" fn __arth_cancel_token_cancel(handle: i64) -> i32 {
    if let Ok(mut m) = token_map().lock() {
        m.insert(handle, true);
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn __arth_cancel_token_is_cancelled(handle: i64) -> i32 {
    if let Ok(m) = token_map().lock() {
        return if m.get(&handle).copied().unwrap_or(false) {
            1
        } else {
            0
        };
    }
    0
}

// =========================================================================
// C19: Atomic<T> Operations
// Thread-safe atomic integer operations with SeqCst memory ordering.
// Atomic<T> is both Sendable AND Shareable.
// =========================================================================

/// Storage for atomic integers (handle -> AtomicI64)
fn atomic_map() -> &'static Mutex<HashMap<i64, std::sync::Arc<AtomicI64>>> {
    static MAP: OnceLock<Mutex<HashMap<i64, std::sync::Arc<AtomicI64>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_ATOMIC: AtomicI64 = AtomicI64::new(40_000);

/// Create a new atomic integer with the specified initial value.
/// Returns: positive handle on success
#[unsafe(no_mangle)]
pub extern "C" fn __arth_atomic_create(initial_value: i64) -> i64 {
    let h = NEXT_ATOMIC.fetch_add(1, Ordering::Relaxed);
    let atomic = std::sync::Arc::new(AtomicI64::new(initial_value));
    if let Ok(mut m) = atomic_map().lock() {
        m.insert(h, atomic);
    }
    h
}

/// Atomically load the current value.
/// Returns: current value, or 0 if not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_atomic_load(handle: i64) -> i64 {
    if let Ok(m) = atomic_map().lock() {
        if let Some(atomic) = m.get(&handle) {
            return atomic.load(Ordering::SeqCst);
        }
    }
    0 // not found, return 0 as default
}

/// Atomically store a new value.
/// Returns: 0=success, -1=not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_atomic_store(handle: i64, value: i64) -> i64 {
    if let Ok(m) = atomic_map().lock() {
        if let Some(atomic) = m.get(&handle) {
            atomic.store(value, Ordering::SeqCst);
            return 0;
        }
    }
    -1 // not found
}

/// Compare-and-swap: if current value equals expected, replace with new_value.
/// Returns: 1=success (swapped), 0=failure (value was different), -1=not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_atomic_cas(handle: i64, expected: i64, new_value: i64) -> i64 {
    if let Ok(m) = atomic_map().lock() {
        if let Some(atomic) = m.get(&handle) {
            match atomic.compare_exchange(expected, new_value, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return 1,  // success
                Err(_) => return 0, // failure, value was different
            }
        }
    }
    -1 // not found
}

/// Atomically add delta to the value and return the old value.
/// Returns: old value before addition
#[unsafe(no_mangle)]
pub extern "C" fn __arth_atomic_fetch_add(handle: i64, delta: i64) -> i64 {
    if let Ok(m) = atomic_map().lock() {
        if let Some(atomic) = m.get(&handle) {
            return atomic.fetch_add(delta, Ordering::SeqCst);
        }
    }
    0 // not found
}

/// Atomically subtract delta from the value and return the old value.
/// Returns: old value before subtraction
#[unsafe(no_mangle)]
pub extern "C" fn __arth_atomic_fetch_sub(handle: i64, delta: i64) -> i64 {
    if let Ok(m) = atomic_map().lock() {
        if let Some(atomic) = m.get(&handle) {
            return atomic.fetch_sub(delta, Ordering::SeqCst);
        }
    }
    0 // not found
}

/// Atomically swap the value and return the old value.
/// Returns: old value before swap
#[unsafe(no_mangle)]
pub extern "C" fn __arth_atomic_swap(handle: i64, new_value: i64) -> i64 {
    if let Ok(m) = atomic_map().lock() {
        if let Some(atomic) = m.get(&handle) {
            return atomic.swap(new_value, Ordering::SeqCst);
        }
    }
    0 // not found
}

/// Alias for atomic_load (for ergonomics).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_atomic_get(handle: i64) -> i64 {
    __arth_atomic_load(handle)
}

/// Alias for atomic_store (for ergonomics).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_atomic_set(handle: i64, value: i64) -> i64 {
    __arth_atomic_store(handle, value)
}

/// Atomically increment by 1 and return the old value.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_atomic_inc(handle: i64) -> i64 {
    __arth_atomic_fetch_add(handle, 1)
}

/// Atomically decrement by 1 and return the old value.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_atomic_dec(handle: i64) -> i64 {
    __arth_atomic_fetch_sub(handle, 1)
}

// --- Panic and unwinding support ---

/// Panic state for the VM runtime. Tracks whether a panic is in progress
/// and the panic message for reporting to join().
#[derive(Clone, Debug, Default)]
struct PanicState {
    /// Whether a panic is currently in progress
    is_panicking: bool,
    /// The panic message (string)
    message: String,
}

/// Global panic state (for the current task).
fn panic_state() -> &'static Mutex<PanicState> {
    static STATE: OnceLock<Mutex<PanicState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(PanicState::default()))
}

/// Exception state - stores the currently thrown exception value.
/// Unlike panics, exceptions are catchable by try/catch blocks.
#[derive(Clone, Debug, Default)]
pub struct ExceptionState {
    /// Whether an exception is currently being thrown
    has_exception: bool,
    /// The exception value (stored as i64 handle to a struct)
    value: i64,
}

/// Global exception state (for the current task).
pub fn exception_state() -> &'static Mutex<ExceptionState> {
    static STATE: OnceLock<Mutex<ExceptionState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ExceptionState::default()))
}

/// Throw an exception value. Returns the handler info (IP, frame_depth) or None if no handler.
pub fn throw_exception(value: i64) -> Option<(u32, usize)> {
    // Store the exception value
    if let Ok(mut state) = exception_state().lock() {
        state.has_exception = true;
        state.value = value;
    }
    // Pop and return the handler info
    if let Ok(mut handlers) = unwind_handlers().lock() {
        return handlers.pop().map(|h| (h.handler_ip, h.frame_depth));
    }
    None
}

/// Get the current exception value and clear the exception state.
pub fn get_exception() -> i64 {
    if let Ok(mut state) = exception_state().lock() {
        let value = state.value;
        state.has_exception = false;
        state.value = 0;
        return value;
    }
    0
}

/// Unwind handler entry - stores handler IP and the frame depth at registration.
/// This allows proper frame unwinding: only pop frames when the handler is at a lower depth.
#[derive(Clone, Debug)]
struct UnwindHandler {
    handler_ip: u32,
    frame_depth: usize,
}

/// Unwind handler stack - each entry contains handler IP and frame depth.
/// Handlers are pushed when entering try blocks or regions with cleanup.
fn unwind_handlers() -> &'static Mutex<Vec<UnwindHandler>> {
    static HANDLERS: OnceLock<Mutex<Vec<UnwindHandler>>> = OnceLock::new();
    HANDLERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Task panic state - maps task handles to their panic information.
/// This allows join() to detect and propagate panic failures.
#[derive(Clone, Debug)]
struct TaskPanicInfo {
    panicked: bool,
    message: String,
}

fn task_panic_map() -> &'static Mutex<HashMap<i64, TaskPanicInfo>> {
    static MAP: OnceLock<Mutex<HashMap<i64, TaskPanicInfo>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Escape a string for JSON output (handles quotes, backslashes, control chars).
fn json_escape(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            c if c.is_control() => {
                result.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => result.push(c),
        }
    }
    result
}

/// A single frame in a stack trace.
#[derive(Clone, Debug)]
pub struct StackFrame {
    /// Function name (may be empty if debug info unavailable)
    pub function_name: String,
    /// Bytecode offset (instruction pointer)
    pub offset: u32,
    /// Source file (if available)
    pub source_file: Option<String>,
    /// Line number (if available, 0 = unknown)
    pub line: u32,
}

/// Capture a stack trace from the current call state.
/// Takes the current IP, the return stack, and the Program for symbolication.
pub fn capture_stack_trace(
    current_ip: usize,
    ret_stack: &[usize],
    program: &crate::Program,
) -> Vec<StackFrame> {
    let mut frames = Vec::with_capacity(ret_stack.len() + 1);

    // Current frame (where the panic occurred)
    if let Some(entry) = program.lookup_function(current_ip as u32) {
        frames.push(StackFrame {
            function_name: entry.function_name.clone(),
            offset: current_ip as u32,
            source_file: entry.source_file.clone(),
            line: entry.line,
        });
    } else {
        frames.push(StackFrame {
            function_name: "<unknown>".to_string(),
            offset: current_ip as u32,
            source_file: None,
            line: 0,
        });
    }

    // Caller frames (from return stack, most recent first)
    for &ret_ip in ret_stack.iter().rev() {
        if let Some(entry) = program.lookup_function(ret_ip as u32) {
            frames.push(StackFrame {
                function_name: entry.function_name.clone(),
                offset: ret_ip as u32,
                source_file: entry.source_file.clone(),
                line: entry.line,
            });
        } else {
            frames.push(StackFrame {
                function_name: "<unknown>".to_string(),
                offset: ret_ip as u32,
                source_file: None,
                line: 0,
            });
        }
    }

    frames
}

/// Output a panic in structured JSON format to stderr (without stack trace).
fn emit_panic_json(msg: &str) {
    let escaped = json_escape(msg);
    eprintln!(r#"{{"type":"panic","message":"{}"}}"#, escaped);
}

/// Output a panic in structured JSON format to stderr with stack trace.
fn emit_panic_json_with_stack(msg: &str, stack: &[StackFrame]) {
    // Also capture async stack if available
    let async_stack = crate::executor::get_current_async_stack();
    emit_panic_json_with_full_stack(msg, stack, &async_stack);
}

/// Output a panic in structured JSON format to stderr with both sync and async stack traces.
fn emit_panic_json_with_full_stack(
    msg: &str,
    stack: &[StackFrame],
    async_stack: &[crate::executor::AsyncStackFrame],
) {
    let escaped_msg = json_escape(msg);

    // Build synchronous stack trace JSON array
    let mut stack_json = String::from("[");
    for (i, frame) in stack.iter().enumerate() {
        if i > 0 {
            stack_json.push(',');
        }
        let escaped_fn = json_escape(&frame.function_name);
        stack_json.push_str(&format!(
            r#"{{"function":"{}","offset":{}"#,
            escaped_fn, frame.offset
        ));
        if let Some(ref file) = frame.source_file {
            let escaped_file = json_escape(file);
            stack_json.push_str(&format!(r#","file":"{}""#, escaped_file));
        }
        if frame.line > 0 {
            stack_json.push_str(&format!(r#","line":{}"#, frame.line));
        }
        stack_json.push('}');
    }
    stack_json.push(']');

    // Build async stack trace JSON array (if any)
    let async_stack_json = if async_stack.is_empty() {
        String::new()
    } else {
        let mut json = String::from("[");
        for (i, frame) in async_stack.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            let fn_name = frame.spawn_function.as_deref().unwrap_or("<async>");
            let escaped_fn = json_escape(fn_name);
            json.push_str(&format!(
                r#"{{"task":{},"function":"{}","fn_id":{}}}"#,
                frame.task_handle, escaped_fn, frame.fn_id
            ));
        }
        json.push(']');
        json
    };

    // Output JSON with optional async_stack field
    if async_stack_json.is_empty() {
        eprintln!(
            r#"{{"type":"panic","message":"{}","stack":{}}}"#,
            escaped_msg, stack_json
        );
    } else {
        eprintln!(
            r#"{{"type":"panic","message":"{}","stack":{},"async_stack":{}}}"#,
            escaped_msg, stack_json, async_stack_json
        );
    }
}

/// Trigger a panic with the given message.
/// This sets the panic state and begins unwinding.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_panic(msg_ptr: *const i8) -> i64 {
    let msg = if msg_ptr.is_null() {
        "panic".to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(msg_ptr) }
            .to_string_lossy()
            .to_string()
    };
    if let Ok(mut state) = panic_state().lock() {
        state.is_panicking = true;
        state.message = msg.clone();
    }
    emit_panic_json(&msg);
    // Return sentinel value indicating panic
    -2
}

/// Panic with a string message (for VM use where strings are in the string pool).
pub fn panic_with_message(msg: String) {
    if let Ok(mut state) = panic_state().lock() {
        state.is_panicking = true;
        state.message = msg.clone();
    }
    emit_panic_json(&msg);
}

/// Panic with a string message and stack trace (for VM use with debug info).
/// This captures the current call stack and includes it in the JSON output.
pub fn panic_with_message_and_stack(
    msg: String,
    current_ip: usize,
    ret_stack: &[usize],
    program: &crate::Program,
) {
    if let Ok(mut state) = panic_state().lock() {
        state.is_panicking = true;
        state.message = msg.clone();
    }
    let stack = capture_stack_trace(current_ip, ret_stack, program);
    emit_panic_json_with_stack(&msg, &stack);
}

/// Check if a panic is in progress.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_is_panicking() -> i32 {
    if let Ok(state) = panic_state().lock() {
        if state.is_panicking {
            return 1;
        }
    }
    0
}

/// Get the current panic message. Returns empty string if not panicking.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_panic_message_ptr() -> *const i8 {
    static MSG: std::sync::OnceLock<std::ffi::CString> = std::sync::OnceLock::new();
    if let Ok(state) = panic_state().lock() {
        if state.is_panicking {
            let cstr = std::ffi::CString::new(state.message.clone()).unwrap_or_default();
            return MSG.get_or_init(|| cstr).as_ptr();
        }
    }
    std::ptr::null()
}

/// Clear the panic state (used after handling or at task boundary).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_clear_panic() {
    if let Ok(mut state) = panic_state().lock() {
        state.is_panicking = false;
        state.message.clear();
    }
}

/// Push an unwind handler onto the stack with frame depth.
pub fn push_unwind_handler(handler_ip: u32, frame_depth: usize) {
    if let Ok(mut handlers) = unwind_handlers().lock() {
        handlers.push(UnwindHandler { handler_ip, frame_depth });
    }
}

/// Pop and discard the top unwind handler.
pub fn pop_unwind_handler() {
    if let Ok(mut handlers) = unwind_handlers().lock() {
        handlers.pop();
    }
}

/// FFI version: Push an unwind handler (frame depth defaults to 0 for compatibility).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_push_unwind_handler(handler_ip: u32) {
    push_unwind_handler(handler_ip, 0);
}

/// FFI version: Pop and return the top unwind handler IP, or 0 if none.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_pop_unwind_handler() -> u32 {
    if let Ok(mut handlers) = unwind_handlers().lock() {
        return handlers.pop().map(|h| h.handler_ip).unwrap_or(0);
    }
    0
}

/// Mark a task as panicked with the given message.
/// Called when a task completes due to panic.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_mark_panicked(handle: i64, msg_ptr: *const i8) -> i32 {
    let msg = if msg_ptr.is_null() {
        "panic".to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(msg_ptr) }
            .to_string_lossy()
            .to_string()
    };
    if let Ok(mut map) = task_panic_map().lock() {
        map.insert(
            handle,
            TaskPanicInfo {
                panicked: true,
                message: msg,
            },
        );
        return 0;
    }
    1
}

/// Check if a task panicked.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_did_panic(handle: i64) -> i32 {
    if let Ok(map) = task_panic_map().lock() {
        if let Some(info) = map.get(&handle) {
            return if info.panicked { 1 } else { 0 };
        }
    }
    0
}

/// Get the panic message for a task. Returns empty string if not panicked.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_task_panic_message_ptr(handle: i64) -> *const i8 {
    // Thread-local static for the returned string
    thread_local! {
        static TASK_MSG: std::cell::RefCell<std::ffi::CString> = std::cell::RefCell::new(std::ffi::CString::default());
    }
    if let Ok(map) = task_panic_map().lock() {
        if let Some(info) = map.get(&handle) {
            if info.panicked {
                let cstr = std::ffi::CString::new(info.message.clone()).unwrap_or_default();
                return TASK_MSG.with(|cell| {
                    *cell.borrow_mut() = cstr;
                    cell.borrow().as_ptr()
                });
            }
        }
    }
    std::ptr::null()
}

// --- Region-based allocation intrinsics for native backends (LLVM/Cranelift) ---

/// Enter a new region for bulk deallocation of loop-local values.
/// Called by native backends at loop body entry.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_region_enter(region_id: i32) {
    region_enter(region_id as u32);
}

/// Exit a region and bulk deallocate all tracked values.
/// Called by native backends at loop body exit.
/// Note: deinit calls for values are emitted by the compiler before this call.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_region_exit(region_id: i32) {
    let _ = region_exit(region_id as u32);
}

/// Allocate a value in the current region (for explicit tracking).
/// Optional - native backends may emit explicit drops instead.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_region_alloc(value: i64) {
    region_alloc(Value::I64(value));
}

// =========================================================================
// C21: Event Loop Operations
// Event-driven I/O using OS primitives (pipes, timers) with polling.
// Provides the foundation for async I/O integration.
// =========================================================================

use std::time::{Duration, Instant};

/// Event types returned by the event loop
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum EventType {
    Timer = 1,
    Read = 2,
    Write = 3,
    Error = 4,
}

/// Interest flags for file descriptor registration
pub const INTEREST_READ: i64 = 1;
pub const INTEREST_WRITE: i64 = 2;

/// Registration entry in the event loop
#[derive(Clone, Debug)]
enum Registration {
    /// Timer that fires after a deadline
    Timer { deadline: Instant, token: i64 },
    /// File descriptor monitoring (pipe read/write)
    Fd { fd: i64, interest: i64, token: i64 },
}

/// Event returned from polling
#[derive(Clone, Debug)]
pub struct Event {
    pub token: i64,
    pub event_type: EventType,
}

/// Event loop structure
struct EventLoop {
    /// All registrations (timers and file descriptors)
    registrations: Vec<Registration>,
    /// Next token to assign
    next_token: i64,
    /// Events from the last poll
    last_events: Vec<Event>,
    /// Whether the event loop is closed
    closed: bool,
}

impl EventLoop {
    fn new() -> Self {
        Self {
            registrations: Vec::new(),
            next_token: 1,
            last_events: Vec::new(),
            closed: false,
        }
    }

    fn register_timer(&mut self, timeout_ms: i64) -> i64 {
        if self.closed {
            return -1;
        }
        let token = self.next_token;
        self.next_token += 1;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        self.registrations.push(Registration::Timer { deadline, token });
        token
    }

    fn register_fd(&mut self, fd: i64, interest: i64) -> i64 {
        if self.closed {
            return -1;
        }
        let token = self.next_token;
        self.next_token += 1;
        self.registrations.push(Registration::Fd { fd, interest, token });
        token
    }

    fn deregister(&mut self, token: i64) -> i64 {
        if let Some(pos) = self.registrations.iter().position(|r| match r {
            Registration::Timer { token: t, .. } => *t == token,
            Registration::Fd { token: t, .. } => *t == token,
        }) {
            self.registrations.remove(pos);
            0
        } else {
            -1
        }
    }

    fn poll(&mut self, timeout_ms: i64) -> i64 {
        if self.closed {
            return -1;
        }

        self.last_events.clear();
        let poll_start = Instant::now();
        let timeout_duration = if timeout_ms < 0 {
            Duration::from_secs(u64::MAX / 2) // effectively infinite
        } else {
            Duration::from_millis(timeout_ms as u64)
        };

        // Collect events with polling loop
        loop {
            let now = Instant::now();
            let elapsed = now.duration_since(poll_start);
            if elapsed >= timeout_duration && !self.last_events.is_empty() {
                break;
            }

            // Check timers
            let mut expired_timers = Vec::new();
            for (idx, reg) in self.registrations.iter().enumerate() {
                if let Registration::Timer { deadline, token } = reg {
                    if now >= *deadline {
                        self.last_events.push(Event {
                            token: *token,
                            event_type: EventType::Timer,
                        });
                        expired_timers.push(idx);
                    }
                }
            }
            // Remove expired timers (reverse order to preserve indices)
            for idx in expired_timers.into_iter().rev() {
                self.registrations.remove(idx);
            }

            // Check file descriptors for readability/writability
            for reg in &self.registrations {
                if let Registration::Fd { fd, interest, token } = reg {
                    // Use non-blocking poll on the fd
                    if (*interest & INTEREST_READ) != 0 {
                        if is_fd_readable(*fd) {
                            self.last_events.push(Event {
                                token: *token,
                                event_type: EventType::Read,
                            });
                        }
                    }
                    if (*interest & INTEREST_WRITE) != 0 {
                        if is_fd_writable(*fd) {
                            self.last_events.push(Event {
                                token: *token,
                                event_type: EventType::Write,
                            });
                        }
                    }
                }
            }

            // If we got events or timed out, stop polling
            if !self.last_events.is_empty() {
                break;
            }

            if elapsed >= timeout_duration {
                break;
            }

            // Brief sleep to avoid busy-waiting
            std::thread::sleep(Duration::from_millis(1));
        }

        self.last_events.len() as i64
    }

    fn get_event(&self, index: usize) -> Option<&Event> {
        self.last_events.get(index)
    }
}

/// Check if a file descriptor is readable (has data available)
#[cfg(unix)]
fn is_fd_readable(fd: i64) -> bool {
    use std::os::unix::io::RawFd;
    let raw_fd = fd as RawFd;

    // Use poll(2) with zero timeout for non-blocking check
    let mut pollfd = libc::pollfd {
        fd: raw_fd,
        events: libc::POLLIN,
        revents: 0,
    };

    let result = unsafe { libc::poll(&mut pollfd, 1, 0) };
    result > 0 && (pollfd.revents & libc::POLLIN) != 0
}

#[cfg(not(unix))]
fn is_fd_readable(_fd: i64) -> bool {
    // Non-Unix platforms: not yet supported
    false
}

/// Check if a file descriptor is writable (can accept data)
#[cfg(unix)]
fn is_fd_writable(fd: i64) -> bool {
    use std::os::unix::io::RawFd;
    let raw_fd = fd as RawFd;

    let mut pollfd = libc::pollfd {
        fd: raw_fd,
        events: libc::POLLOUT,
        revents: 0,
    };

    let result = unsafe { libc::poll(&mut pollfd, 1, 0) };
    result > 0 && (pollfd.revents & libc::POLLOUT) != 0
}

#[cfg(not(unix))]
fn is_fd_writable(_fd: i64) -> bool {
    false
}

/// Storage for event loops (handle -> EventLoop)
fn event_loop_map() -> &'static Mutex<HashMap<i64, EventLoop>> {
    static MAP: OnceLock<Mutex<HashMap<i64, EventLoop>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_EVENT_LOOP: AtomicI64 = AtomicI64::new(60_000);

// Thread-local storage for pipe write fd and last events
thread_local! {
    static LAST_PIPE_WRITE_FD: std::cell::Cell<i64> = const { std::cell::Cell::new(-1) };
    static LAST_POLL_EVENTS: std::cell::RefCell<Vec<Event>> = std::cell::RefCell::new(Vec::new());
}

/// Create a new event loop.
/// Returns: positive handle on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_create() -> i64 {
    let h = NEXT_EVENT_LOOP.fetch_add(1, Ordering::Relaxed);
    let event_loop = EventLoop::new();
    if let Ok(mut m) = event_loop_map().lock() {
        m.insert(h, event_loop);
        return h;
    }
    -1
}

/// Register a one-shot timer with the event loop.
/// Returns: token (>= 0) on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_register_timer(handle: i64, timeout_ms: i64) -> i64 {
    if let Ok(mut m) = event_loop_map().lock() {
        if let Some(el) = m.get_mut(&handle) {
            return el.register_timer(timeout_ms);
        }
    }
    -1
}

/// Register a file descriptor for monitoring.
/// Interest: 1=READ, 2=WRITE, 3=READ|WRITE
/// Returns: token (>= 0) on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_register_fd(handle: i64, fd: i64, interest: i64) -> i64 {
    if let Ok(mut m) = event_loop_map().lock() {
        if let Some(el) = m.get_mut(&handle) {
            return el.register_fd(fd, interest);
        }
    }
    -1
}

/// Deregister a token from the event loop.
/// Returns: 0=success, -1=error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_deregister(handle: i64, token: i64) -> i64 {
    if let Ok(mut m) = event_loop_map().lock() {
        if let Some(el) = m.get_mut(&handle) {
            return el.deregister(token);
        }
    }
    -1
}

/// Poll for events with timeout.
/// Returns: number of events ready (0 = timeout, -1 = error)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_poll(handle: i64, timeout_ms: i64) -> i64 {
    if let Ok(mut m) = event_loop_map().lock() {
        if let Some(el) = m.get_mut(&handle) {
            let count = el.poll(timeout_ms);
            // Copy events to thread-local for retrieval
            LAST_POLL_EVENTS.with(|events| {
                *events.borrow_mut() = el.last_events.clone();
            });
            return count;
        }
    }
    -1
}

/// Get the token from an event at index.
/// Returns: token for the event, -1 if index out of bounds
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_get_event(index: i64) -> i64 {
    LAST_POLL_EVENTS.with(|events| {
        let events = events.borrow();
        if let Some(event) = events.get(index as usize) {
            event.token
        } else {
            -1
        }
    })
}

/// Get the type of event at index.
/// Returns: 1=TIMER, 2=READ, 3=WRITE, 4=ERROR, -1=invalid index
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_get_event_type(index: i64) -> i64 {
    LAST_POLL_EVENTS.with(|events| {
        let events = events.borrow();
        if let Some(event) = events.get(index as usize) {
            event.event_type as i64
        } else {
            -1
        }
    })
}

/// Close and destroy an event loop.
/// Returns: 0=success, -1=error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_close(handle: i64) -> i64 {
    if let Ok(mut m) = event_loop_map().lock() {
        if m.remove(&handle).is_some() {
            return 0;
        }
    }
    -1
}

/// Create a pipe pair for inter-thread communication.
/// Returns: read_fd (write_fd stored in thread-local storage)
#[cfg(unix)]
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_pipe_create() -> i64 {
    let mut fds: [libc::c_int; 2] = [0; 2];
    let result = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if result != 0 {
        return -1;
    }

    let read_fd = fds[0] as i64;
    let write_fd = fds[1] as i64;

    // Set both ends to non-blocking mode
    unsafe {
        let flags = libc::fcntl(fds[0], libc::F_GETFL);
        libc::fcntl(fds[0], libc::F_SETFL, flags | libc::O_NONBLOCK);
        let flags = libc::fcntl(fds[1], libc::F_GETFL);
        libc::fcntl(fds[1], libc::F_SETFL, flags | libc::O_NONBLOCK);
    }

    // Store write_fd in thread-local for retrieval
    LAST_PIPE_WRITE_FD.with(|cell| cell.set(write_fd));

    read_fd
}

#[cfg(not(unix))]
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_pipe_create() -> i64 {
    -1 // Not supported on non-Unix
}

/// Get the write fd from the last pipe creation.
/// Returns: write file descriptor, -1 if no pipe was created
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_pipe_get_write_fd() -> i64 {
    LAST_PIPE_WRITE_FD.with(|cell| cell.get())
}

/// Write a single i64 value to a pipe.
/// Returns: 8 on success (bytes written), -1 on error, 0 if would block
#[cfg(unix)]
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_pipe_write(write_fd: i64, value: i64) -> i64 {
    let bytes = value.to_le_bytes();
    let result = unsafe {
        libc::write(write_fd as libc::c_int, bytes.as_ptr() as *const libc::c_void, 8)
    };
    if result < 0 {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK {
            return 0; // would block
        }
        return -1; // error
    }
    result as i64
}

#[cfg(not(unix))]
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_pipe_write(_write_fd: i64, _value: i64) -> i64 {
    -1
}

/// Read a single i64 value from a pipe.
/// Returns: the value read, or -1 on error, -2 if would block
#[cfg(unix)]
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_pipe_read(read_fd: i64) -> i64 {
    let mut bytes = [0u8; 8];
    let result = unsafe {
        libc::read(read_fd as libc::c_int, bytes.as_mut_ptr() as *mut libc::c_void, 8)
    };
    if result < 0 {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK {
            return -2; // would block
        }
        return -1; // error
    }
    if result != 8 {
        return -1; // incomplete read
    }
    i64::from_le_bytes(bytes)
}

#[cfg(not(unix))]
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_pipe_read(_read_fd: i64) -> i64 {
    -1
}

/// Close a pipe file descriptor.
/// Returns: 0=success, -1=error
#[cfg(unix)]
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_pipe_close(fd: i64) -> i64 {
    let result = unsafe { libc::close(fd as libc::c_int) };
    if result == 0 { 0 } else { -1 }
}

#[cfg(not(unix))]
#[unsafe(no_mangle)]
pub extern "C" fn __arth_event_loop_pipe_close(_fd: i64) -> i64 {
    -1
}

// =========================================================================
// C22: Async Timer Operations
// Timer-based async operations with task suspension and wakeup.
// =========================================================================

/// Timer state: pending or expired
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimerState {
    Pending,
    Expired,
    Cancelled,
}

/// Timer entry in the registry
struct AsyncTimer {
    /// When the timer should fire
    deadline: Instant,
    /// The task waiting on this timer
    task_id: i64,
    /// Current state of the timer
    state: TimerState,
}

// Storage for async timers (timer_id -> AsyncTimer)
fn async_timer_map() -> &'static Mutex<HashMap<i64, AsyncTimer>> {
    static MAP: OnceLock<Mutex<HashMap<i64, AsyncTimer>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_ASYNC_TIMER: AtomicI64 = AtomicI64::new(70_000);

/// Blocking sleep for specified milliseconds.
/// This blocks the current thread.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_timer_sleep(ms: i64) {
    if ms > 0 {
        std::thread::sleep(Duration::from_millis(ms as u64));
    }
}

/// Register an async timer that will wake a task after ms milliseconds.
/// Returns: timer_id (>= 0) on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_timer_sleep_async(ms: i64, task_id: i64) -> i64 {
    if ms < 0 {
        return -1;
    }
    let timer_id = NEXT_ASYNC_TIMER.fetch_add(1, Ordering::Relaxed);
    let deadline = Instant::now() + Duration::from_millis(ms as u64);
    let timer = AsyncTimer {
        deadline,
        task_id,
        state: TimerState::Pending,
    };
    if let Ok(mut m) = async_timer_map().lock() {
        m.insert(timer_id, timer);
        return timer_id;
    }
    -1
}

/// Check if a timer has expired.
/// Returns: 1=expired, 0=pending, -1=not found/cancelled
#[unsafe(no_mangle)]
pub extern "C" fn __arth_timer_check_expired(timer_id: i64) -> i64 {
    if let Ok(mut m) = async_timer_map().lock() {
        if let Some(timer) = m.get_mut(&timer_id) {
            match timer.state {
                TimerState::Cancelled => return -1,
                TimerState::Expired => return 1,
                TimerState::Pending => {
                    if Instant::now() >= timer.deadline {
                        timer.state = TimerState::Expired;
                        return 1;
                    }
                    return 0;
                }
            }
        }
    }
    -1 // not found
}

/// Get the task ID waiting on a timer.
/// Returns: task_id (>= 0), or -1 if not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_timer_get_waiting_task(timer_id: i64) -> i64 {
    if let Ok(m) = async_timer_map().lock() {
        if let Some(timer) = m.get(&timer_id) {
            return timer.task_id;
        }
    }
    -1
}

/// Cancel a pending timer.
/// Returns: 0=success, -1=not found/already fired
#[unsafe(no_mangle)]
pub extern "C" fn __arth_timer_cancel(timer_id: i64) -> i64 {
    if let Ok(mut m) = async_timer_map().lock() {
        if let Some(timer) = m.get_mut(&timer_id) {
            if timer.state == TimerState::Pending {
                timer.state = TimerState::Cancelled;
                return 0;
            }
        }
    }
    -1
}

/// Poll for the next expired timer.
/// Returns: timer_id of an expired timer, or -1 if none expired
/// This also marks the timer as expired if its deadline has passed.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_timer_poll_expired() -> i64 {
    let now = Instant::now();
    if let Ok(mut m) = async_timer_map().lock() {
        for (timer_id, timer) in m.iter_mut() {
            if timer.state == TimerState::Pending && now >= timer.deadline {
                timer.state = TimerState::Expired;
                return *timer_id;
            }
        }
    }
    -1
}

/// Get current time in milliseconds since UNIX epoch.
/// Returns: current time in milliseconds
#[unsafe(no_mangle)]
pub extern "C" fn __arth_timer_now() -> i64 {
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(_) => 0,
    }
}

/// Get elapsed time since a previous timestamp.
/// Returns: elapsed milliseconds since start_ms
#[unsafe(no_mangle)]
pub extern "C" fn __arth_timer_elapsed(start_ms: i64) -> i64 {
    let now = __arth_timer_now();
    now - start_ms
}

/// Remove an expired or cancelled timer from the registry.
/// Call this after processing an expired timer to clean up.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_timer_remove(timer_id: i64) -> i64 {
    if let Ok(mut m) = async_timer_map().lock() {
        if m.remove(&timer_id).is_some() {
            return 0;
        }
    }
    -1
}

/// Get the remaining time until a timer fires.
/// Returns: milliseconds remaining (>= 0), 0 if expired, -1 if not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_timer_remaining(timer_id: i64) -> i64 {
    if let Ok(m) = async_timer_map().lock() {
        if let Some(timer) = m.get(&timer_id) {
            if timer.state != TimerState::Pending {
                return 0; // already expired or cancelled
            }
            let now = Instant::now();
            if now >= timer.deadline {
                return 0;
            }
            return timer.deadline.duration_since(now).as_millis() as i64;
        }
    }
    -1
}

// =========================================================================
// C23: Async TCP Socket Operations
// TCP listener and stream operations for network I/O.
// =========================================================================

use std::net::{TcpListener, TcpStream, SocketAddr, ToSocketAddrs};

/// TCP Listener entry in the registry
struct TcpListenerEntry {
    listener: TcpListener,
    local_addr: SocketAddr,
}

/// TCP Stream entry in the registry
struct TcpStreamEntry {
    stream: TcpStream,
    peer_addr: Option<SocketAddr>,
    local_addr: Option<SocketAddr>,
}

/// Async request state
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TcpRequestState {
    Pending,
    Ready,
    Error,
}

/// Async request types
#[derive(Clone, Debug)]
enum TcpRequestKind {
    Accept { listener_handle: i64 },
    Connect { host: String, port: u16 },
    Read { stream_handle: i64, max_bytes: usize },
    Write { stream_handle: i64, data: String },
}

/// Async request entry
struct TcpAsyncRequest {
    kind: TcpRequestKind,
    task_id: i64,
    state: TcpRequestState,
    result: i64,
    read_data: Option<String>,
}

// Handle allocators
static NEXT_TCP_LISTENER: AtomicI64 = AtomicI64::new(80_000);
static NEXT_TCP_STREAM: AtomicI64 = AtomicI64::new(90_000);
static NEXT_TCP_REQUEST: AtomicI64 = AtomicI64::new(100_000);

// Storage for TCP listeners
fn tcp_listener_map() -> &'static Mutex<HashMap<i64, TcpListenerEntry>> {
    static MAP: OnceLock<Mutex<HashMap<i64, TcpListenerEntry>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

// Storage for TCP streams
fn tcp_stream_map() -> &'static Mutex<HashMap<i64, TcpStreamEntry>> {
    static MAP: OnceLock<Mutex<HashMap<i64, TcpStreamEntry>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

// Storage for async requests
fn tcp_request_map() -> &'static Mutex<HashMap<i64, TcpAsyncRequest>> {
    static MAP: OnceLock<Mutex<HashMap<i64, TcpAsyncRequest>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

// Thread-local storage for last read data
thread_local! {
    static LAST_TCP_READ_DATA: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Bind a TCP listener to a port.
/// Returns: listener handle (>= 80000), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_listener_bind(port: i64) -> i64 {
    if port < 0 || port > 65535 {
        return -1;
    }

    let addr = format!("127.0.0.1:{}", port);
    match TcpListener::bind(&addr) {
        Ok(listener) => {
            // Set non-blocking mode for async operations
            if let Err(_) = listener.set_nonblocking(true) {
                return -1;
            }

            let local_addr = match listener.local_addr() {
                Ok(a) => a,
                Err(_) => return -1,
            };

            let handle = NEXT_TCP_LISTENER.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut m) = tcp_listener_map().lock() {
                m.insert(handle, TcpListenerEntry { listener, local_addr });
                return handle;
            }
            -1
        }
        Err(_) => -1,
    }
}

/// Accept a connection on a listener (blocking).
/// Returns: stream handle (>= 90000), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_listener_accept(listener_handle: i64) -> i64 {
    let listener_opt = {
        if let Ok(m) = tcp_listener_map().lock() {
            m.get(&listener_handle).map(|e| {
                // Clone the listener for blocking accept
                e.listener.try_clone().ok()
            }).flatten()
        } else {
            None
        }
    };

    if let Some(listener) = listener_opt {
        // Set blocking mode for this accept
        let _ = listener.set_nonblocking(false);

        match listener.accept() {
            Ok((stream, peer_addr)) => {
                // Set non-blocking mode on the stream
                let _ = stream.set_nonblocking(true);

                let local_addr = stream.local_addr().ok();
                let handle = NEXT_TCP_STREAM.fetch_add(1, Ordering::SeqCst);

                if let Ok(mut m) = tcp_stream_map().lock() {
                    m.insert(handle, TcpStreamEntry {
                        stream,
                        peer_addr: Some(peer_addr),
                        local_addr,
                    });
                    return handle;
                }
            }
            Err(_) => {}
        }
    }
    -1
}

/// Start async accept on a listener.
/// Returns: request ID for checking completion
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_listener_accept_async(listener_handle: i64, task_id: i64) -> i64 {
    // Verify listener exists
    if let Ok(m) = tcp_listener_map().lock() {
        if !m.contains_key(&listener_handle) {
            return -1;
        }
    } else {
        return -1;
    }

    let request_id = NEXT_TCP_REQUEST.fetch_add(1, Ordering::SeqCst);

    if let Ok(mut m) = tcp_request_map().lock() {
        m.insert(request_id, TcpAsyncRequest {
            kind: TcpRequestKind::Accept { listener_handle },
            task_id,
            state: TcpRequestState::Pending,
            result: 0,
            read_data: None,
        });

        // Try to accept immediately (non-blocking)
        drop(m);
        __arth_tcp_try_complete_request(request_id);

        return request_id;
    }
    -1
}

/// Close a TCP listener.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_listener_close(listener_handle: i64) -> i64 {
    if let Ok(mut m) = tcp_listener_map().lock() {
        if m.remove(&listener_handle).is_some() {
            return 0;
        }
    }
    -1
}

/// Get the local port of a listener.
/// Returns: port number, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_listener_local_port(listener_handle: i64) -> i64 {
    if let Ok(m) = tcp_listener_map().lock() {
        if let Some(entry) = m.get(&listener_handle) {
            return entry.local_addr.port() as i64;
        }
    }
    -1
}

/// Connect to a TCP server (blocking).
/// Returns: stream handle (>= 90000), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_stream_connect(host: *const u8, host_len: i64, port: i64) -> i64 {
    if host.is_null() || host_len < 0 || port < 0 || port > 65535 {
        return -1;
    }

    let host_str = unsafe {
        let slice = std::slice::from_raw_parts(host, host_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    let addr = format!("{}:{}", host_str, port);
    match addr.to_socket_addrs() {
        Ok(mut addrs) => {
            if let Some(addr) = addrs.next() {
                match TcpStream::connect(addr) {
                    Ok(stream) => {
                        let _ = stream.set_nonblocking(true);
                        let peer_addr = stream.peer_addr().ok();
                        let local_addr = stream.local_addr().ok();
                        let handle = NEXT_TCP_STREAM.fetch_add(1, Ordering::SeqCst);

                        if let Ok(mut m) = tcp_stream_map().lock() {
                            m.insert(handle, TcpStreamEntry {
                                stream,
                                peer_addr,
                                local_addr,
                            });
                            return handle;
                        }
                    }
                    Err(_) => {}
                }
            }
        }
        Err(_) => {}
    }
    -1
}

/// Start async connect.
/// Returns: request ID for checking completion
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_stream_connect_async(host: *const u8, host_len: i64, port: i64, task_id: i64) -> i64 {
    if host.is_null() || host_len < 0 || port < 0 || port > 65535 {
        return -1;
    }

    let host_str = unsafe {
        let slice = std::slice::from_raw_parts(host, host_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    let request_id = NEXT_TCP_REQUEST.fetch_add(1, Ordering::SeqCst);

    if let Ok(mut m) = tcp_request_map().lock() {
        m.insert(request_id, TcpAsyncRequest {
            kind: TcpRequestKind::Connect { host: host_str, port: port as u16 },
            task_id,
            state: TcpRequestState::Pending,
            result: 0,
            read_data: None,
        });

        // Try to connect immediately
        drop(m);
        __arth_tcp_try_complete_request(request_id);

        return request_id;
    }
    -1
}

/// Read from a TCP stream (blocking).
/// Returns: number of bytes read, 0 on EOF, -1 on error
/// Data available via __arth_tcp_stream_get_last_read
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_stream_read(stream_handle: i64, max_bytes: i64) -> i64 {
    if max_bytes <= 0 {
        return -1;
    }

    let stream_opt = {
        if let Ok(mut m) = tcp_stream_map().lock() {
            m.get_mut(&stream_handle).and_then(|e| e.stream.try_clone().ok())
        } else {
            None
        }
    };

    if let Some(mut stream) = stream_opt {
        // Set blocking mode for this read
        let _ = stream.set_nonblocking(false);

        let mut buf = vec![0u8; max_bytes as usize];
        match stream.read(&mut buf) {
            Ok(n) => {
                let data = String::from_utf8_lossy(&buf[..n]).to_string();
                LAST_TCP_READ_DATA.with(|cell| {
                    *cell.borrow_mut() = data;
                });
                return n as i64;
            }
            Err(_) => {}
        }
    }
    -1
}

/// Start async read from a TCP stream.
/// Returns: request ID for checking completion
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_stream_read_async(stream_handle: i64, max_bytes: i64, task_id: i64) -> i64 {
    if max_bytes <= 0 {
        return -1;
    }

    // Verify stream exists
    if let Ok(m) = tcp_stream_map().lock() {
        if !m.contains_key(&stream_handle) {
            return -1;
        }
    } else {
        return -1;
    }

    let request_id = NEXT_TCP_REQUEST.fetch_add(1, Ordering::SeqCst);

    if let Ok(mut m) = tcp_request_map().lock() {
        m.insert(request_id, TcpAsyncRequest {
            kind: TcpRequestKind::Read { stream_handle, max_bytes: max_bytes as usize },
            task_id,
            state: TcpRequestState::Pending,
            result: 0,
            read_data: None,
        });

        // Try to read immediately (non-blocking)
        drop(m);
        __arth_tcp_try_complete_request(request_id);

        return request_id;
    }
    -1
}

/// Write to a TCP stream (blocking).
/// Returns: number of bytes written, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_stream_write(stream_handle: i64, data: *const u8, data_len: i64) -> i64 {
    if data.is_null() || data_len < 0 {
        return -1;
    }

    let data_slice = unsafe {
        std::slice::from_raw_parts(data, data_len as usize)
    };

    let stream_opt = {
        if let Ok(mut m) = tcp_stream_map().lock() {
            m.get_mut(&stream_handle).and_then(|e| e.stream.try_clone().ok())
        } else {
            None
        }
    };

    if let Some(mut stream) = stream_opt {
        // Set blocking mode for this write
        let _ = stream.set_nonblocking(false);

        match stream.write(data_slice) {
            Ok(n) => {
                let _ = stream.flush();
                return n as i64;
            }
            Err(_) => {}
        }
    }
    -1
}

/// Start async write to a TCP stream.
/// Returns: request ID for checking completion
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_stream_write_async(stream_handle: i64, data: *const u8, data_len: i64, task_id: i64) -> i64 {
    if data.is_null() || data_len < 0 {
        return -1;
    }

    // Verify stream exists
    if let Ok(m) = tcp_stream_map().lock() {
        if !m.contains_key(&stream_handle) {
            return -1;
        }
    } else {
        return -1;
    }

    let data_str = unsafe {
        let slice = std::slice::from_raw_parts(data, data_len as usize);
        String::from_utf8_lossy(slice).to_string()
    };

    let request_id = NEXT_TCP_REQUEST.fetch_add(1, Ordering::SeqCst);

    if let Ok(mut m) = tcp_request_map().lock() {
        m.insert(request_id, TcpAsyncRequest {
            kind: TcpRequestKind::Write { stream_handle, data: data_str },
            task_id,
            state: TcpRequestState::Pending,
            result: 0,
            read_data: None,
        });

        // Try to write immediately
        drop(m);
        __arth_tcp_try_complete_request(request_id);

        return request_id;
    }
    -1
}

/// Close a TCP stream.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_stream_close(stream_handle: i64) -> i64 {
    if let Ok(mut m) = tcp_stream_map().lock() {
        if m.remove(&stream_handle).is_some() {
            return 0;
        }
    }
    -1
}

/// Get data from the last read operation.
/// Returns: pointer to string data (valid until next read)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_stream_get_last_read(out_len: *mut i64) -> *const u8 {
    LAST_TCP_READ_DATA.with(|cell| {
        let data = cell.borrow();
        if !out_len.is_null() {
            unsafe { *out_len = data.len() as i64; }
        }
        data.as_ptr()
    })
}

/// Get the last read data as a string (for VM use).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_stream_get_last_read_string() -> i64 {
    // This returns a placeholder - actual string handling is done in the interpreter
    LAST_TCP_READ_DATA.with(|cell| {
        cell.borrow().len() as i64
    })
}

/// Set read/write timeout on a stream.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_stream_set_timeout(stream_handle: i64, timeout_ms: i64) -> i64 {
    if timeout_ms < 0 {
        return -1;
    }

    let timeout = if timeout_ms == 0 {
        None
    } else {
        Some(std::time::Duration::from_millis(timeout_ms as u64))
    };

    if let Ok(mut m) = tcp_stream_map().lock() {
        if let Some(entry) = m.get_mut(&stream_handle) {
            if entry.stream.set_read_timeout(timeout).is_ok() &&
               entry.stream.set_write_timeout(timeout).is_ok() {
                return 0;
            }
        }
    }
    -1
}

/// Check if an async operation is ready.
/// Returns: 0=pending, 1=ready, -1=error/not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_check_ready(request_id: i64) -> i64 {
    // Try to complete the request first
    __arth_tcp_try_complete_request(request_id);

    if let Ok(m) = tcp_request_map().lock() {
        if let Some(req) = m.get(&request_id) {
            return match req.state {
                TcpRequestState::Pending => 0,
                TcpRequestState::Ready => 1,
                TcpRequestState::Error => -1,
            };
        }
    }
    -1
}

/// Get the result of a completed async operation.
/// Returns: operation result, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_get_result(request_id: i64) -> i64 {
    if let Ok(m) = tcp_request_map().lock() {
        if let Some(req) = m.get(&request_id) {
            if req.state == TcpRequestState::Ready {
                // If this was a read operation, store the data
                if let Some(ref data) = req.read_data {
                    LAST_TCP_READ_DATA.with(|cell| {
                        *cell.borrow_mut() = data.clone();
                    });
                }
                return req.result;
            }
        }
    }
    -1
}

/// Poll for the next ready async operation.
/// Returns: request ID of ready operation, -1 if none
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_poll_ready() -> i64 {
    // First, try to complete pending requests
    let pending_ids: Vec<i64> = {
        if let Ok(m) = tcp_request_map().lock() {
            m.iter()
                .filter(|(_, req)| req.state == TcpRequestState::Pending)
                .map(|(id, _)| *id)
                .collect()
        } else {
            Vec::new()
        }
    };

    for id in pending_ids {
        __arth_tcp_try_complete_request(id);
    }

    // Now find any ready request
    if let Ok(m) = tcp_request_map().lock() {
        for (id, req) in m.iter() {
            if req.state == TcpRequestState::Ready {
                return *id;
            }
        }
    }
    -1
}

/// Remove a completed request from the registry.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_remove_request(request_id: i64) -> i64 {
    if let Ok(mut m) = tcp_request_map().lock() {
        if m.remove(&request_id).is_some() {
            return 0;
        }
    }
    -1
}

/// Get the task ID waiting on a request.
/// Returns: task_id, -1 if not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_tcp_get_waiting_task(request_id: i64) -> i64 {
    if let Ok(m) = tcp_request_map().lock() {
        if let Some(req) = m.get(&request_id) {
            return req.task_id;
        }
    }
    -1
}

// Internal function to try completing a pending request
fn __arth_tcp_try_complete_request(request_id: i64) {
    let kind = {
        if let Ok(m) = tcp_request_map().lock() {
            m.get(&request_id).filter(|r| r.state == TcpRequestState::Pending).map(|r| r.kind.clone())
        } else {
            None
        }
    };

    if let Some(kind) = kind {
        match kind {
            TcpRequestKind::Accept { listener_handle } => {
                // Try non-blocking accept
                let listener_opt = {
                    if let Ok(m) = tcp_listener_map().lock() {
                        m.get(&listener_handle).and_then(|e| e.listener.try_clone().ok())
                    } else {
                        None
                    }
                };

                if let Some(listener) = listener_opt {
                    match listener.accept() {
                        Ok((stream, peer_addr)) => {
                            let _ = stream.set_nonblocking(true);
                            let local_addr = stream.local_addr().ok();
                            let handle = NEXT_TCP_STREAM.fetch_add(1, Ordering::SeqCst);

                            if let Ok(mut m) = tcp_stream_map().lock() {
                                m.insert(handle, TcpStreamEntry {
                                    stream,
                                    peer_addr: Some(peer_addr),
                                    local_addr,
                                });
                            }

                            if let Ok(mut m) = tcp_request_map().lock() {
                                if let Some(req) = m.get_mut(&request_id) {
                                    req.state = TcpRequestState::Ready;
                                    req.result = handle;
                                }
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            // Still pending
                        }
                        Err(_) => {
                            if let Ok(mut m) = tcp_request_map().lock() {
                                if let Some(req) = m.get_mut(&request_id) {
                                    req.state = TcpRequestState::Error;
                                    req.result = -1;
                                }
                            }
                        }
                    }
                }
            }
            TcpRequestKind::Connect { host, port } => {
                let addr = format!("{}:{}", host, port);
                match addr.to_socket_addrs() {
                    Ok(mut addrs) => {
                        if let Some(addr) = addrs.next() {
                            match TcpStream::connect(addr) {
                                Ok(stream) => {
                                    let _ = stream.set_nonblocking(true);
                                    let peer_addr = stream.peer_addr().ok();
                                    let local_addr = stream.local_addr().ok();
                                    let handle = NEXT_TCP_STREAM.fetch_add(1, Ordering::SeqCst);

                                    if let Ok(mut m) = tcp_stream_map().lock() {
                                        m.insert(handle, TcpStreamEntry {
                                            stream,
                                            peer_addr,
                                            local_addr,
                                        });
                                    }

                                    if let Ok(mut m) = tcp_request_map().lock() {
                                        if let Some(req) = m.get_mut(&request_id) {
                                            req.state = TcpRequestState::Ready;
                                            req.result = handle;
                                        }
                                    }
                                }
                                Err(_) => {
                                    if let Ok(mut m) = tcp_request_map().lock() {
                                        if let Some(req) = m.get_mut(&request_id) {
                                            req.state = TcpRequestState::Error;
                                            req.result = -1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => {
                        if let Ok(mut m) = tcp_request_map().lock() {
                            if let Some(req) = m.get_mut(&request_id) {
                                req.state = TcpRequestState::Error;
                                req.result = -1;
                            }
                        }
                    }
                }
            }
            TcpRequestKind::Read { stream_handle, max_bytes } => {
                let stream_opt = {
                    if let Ok(mut m) = tcp_stream_map().lock() {
                        m.get_mut(&stream_handle).and_then(|e| e.stream.try_clone().ok())
                    } else {
                        None
                    }
                };

                if let Some(mut stream) = stream_opt {
                    let mut buf = vec![0u8; max_bytes];
                    match stream.read(&mut buf) {
                        Ok(n) => {
                            let data = String::from_utf8_lossy(&buf[..n]).to_string();
                            if let Ok(mut m) = tcp_request_map().lock() {
                                if let Some(req) = m.get_mut(&request_id) {
                                    req.state = TcpRequestState::Ready;
                                    req.result = n as i64;
                                    req.read_data = Some(data);
                                }
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            // Still pending
                        }
                        Err(_) => {
                            if let Ok(mut m) = tcp_request_map().lock() {
                                if let Some(req) = m.get_mut(&request_id) {
                                    req.state = TcpRequestState::Error;
                                    req.result = -1;
                                }
                            }
                        }
                    }
                }
            }
            TcpRequestKind::Write { stream_handle, data } => {
                let stream_opt = {
                    if let Ok(mut m) = tcp_stream_map().lock() {
                        m.get_mut(&stream_handle).and_then(|e| e.stream.try_clone().ok())
                    } else {
                        None
                    }
                };

                if let Some(mut stream) = stream_opt {
                    match stream.write(data.as_bytes()) {
                        Ok(n) => {
                            let _ = stream.flush();
                            if let Ok(mut m) = tcp_request_map().lock() {
                                if let Some(req) = m.get_mut(&request_id) {
                                    req.state = TcpRequestState::Ready;
                                    req.result = n as i64;
                                }
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            // Still pending
                        }
                        Err(_) => {
                            if let Ok(mut m) = tcp_request_map().lock() {
                                if let Some(req) = m.get_mut(&request_id) {
                                    req.state = TcpRequestState::Error;
                                    req.result = -1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// =========================================================================
// C24: HTTP Client Operations
// HTTP request/response operations building on TCP sockets.
// =========================================================================

/// HTTP Response entry in the registry
struct HttpResponseEntry {
    status_code: i64,
    headers: HashMap<String, String>,
    body: String,
}

/// HTTP async request state
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HttpRequestState {
    Pending,
    Ready,
    Error,
}

/// HTTP async request types
#[derive(Clone, Debug)]
enum HttpRequestKind {
    Get { url: String, timeout_ms: i64 },
    Post { url: String, body: String, timeout_ms: i64 },
}

/// HTTP async request entry
struct HttpAsyncRequest {
    kind: HttpRequestKind,
    task_id: i64,
    state: HttpRequestState,
    result: i64, // response handle or -1 on error
    error_msg: Option<String>,
}

// Handle allocators for HTTP Client (C24)
// Note: Different from HTTP Server handles in core.inc.rs
static NEXT_HTTP_CLIENT_RESPONSE: AtomicI64 = AtomicI64::new(110_000);
static NEXT_HTTP_CLIENT_REQUEST: AtomicI64 = AtomicI64::new(120_000);

// Storage for HTTP responses
fn http_response_map() -> &'static Mutex<HashMap<i64, HttpResponseEntry>> {
    static MAP: OnceLock<Mutex<HashMap<i64, HttpResponseEntry>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

// Storage for HTTP async requests
fn http_request_map() -> &'static Mutex<HashMap<i64, HttpAsyncRequest>> {
    static MAP: OnceLock<Mutex<HashMap<i64, HttpAsyncRequest>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

// Thread-local storage for last HTTP response body and header lookup
thread_local! {
    static LAST_HTTP_BODY: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static LAST_HTTP_HEADER: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Parse a URL into host, port, and path components
fn parse_url(url: &str) -> Option<(String, u16, String, bool)> {
    let url = url.trim();

    // Determine if HTTPS
    let is_https = url.starts_with("https://");
    let is_http = url.starts_with("http://");

    if !is_http && !is_https {
        return None;
    }

    let url_without_scheme = if is_https {
        &url[8..]
    } else {
        &url[7..]
    };

    // Find path separator
    let (host_port, path) = match url_without_scheme.find('/') {
        Some(idx) => (&url_without_scheme[..idx], &url_without_scheme[idx..]),
        None => (url_without_scheme, "/"),
    };

    // Parse host and port
    let (host, port) = if let Some(idx) = host_port.rfind(':') {
        // Check if this is IPv6 (has brackets)
        if host_port.starts_with('[') {
            // IPv6: look for port after ]
            if let Some(bracket_idx) = host_port.find(']') {
                if bracket_idx + 1 < host_port.len() && host_port.chars().nth(bracket_idx + 1) == Some(':') {
                    let port_str = &host_port[bracket_idx + 2..];
                    let port: u16 = port_str.parse().ok()?;
                    (&host_port[1..bracket_idx], port)
                } else {
                    (&host_port[1..bracket_idx], if is_https { 443 } else { 80 })
                }
            } else {
                return None;
            }
        } else {
            // IPv4 or hostname with port
            let port_str = &host_port[idx + 1..];
            let port: u16 = port_str.parse().ok()?;
            (&host_port[..idx], port)
        }
    } else {
        (host_port, if is_https { 443 } else { 80 })
    };

    Some((host.to_string(), port, path.to_string(), is_https))
}

/// Perform an HTTP request and return the response
fn do_http_request(method: &str, url: &str, body: Option<&str>, timeout_ms: i64) -> Result<HttpResponseEntry, String> {
    let (host, port, path, is_https) = parse_url(url).ok_or("Invalid URL")?;

    // HTTPS is not supported in this simple implementation
    if is_https {
        return Err("HTTPS not supported (use HTTP for testing)".to_string());
    }

    // Connect to server
    let addr = format!("{}:{}", host, port);
    let stream = TcpStream::connect(&addr).map_err(|e| format!("Connection failed: {}", e))?;

    // Set timeout if specified
    if timeout_ms > 0 {
        let timeout = std::time::Duration::from_millis(timeout_ms as u64);
        stream.set_read_timeout(Some(timeout)).ok();
        stream.set_write_timeout(Some(timeout)).ok();
    }

    // Build HTTP request
    let mut request = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        method, path, host
    );

    if let Some(body_content) = body {
        request.push_str(&format!("Content-Length: {}\r\n", body_content.len()));
        request.push_str("Content-Type: application/x-www-form-urlencoded\r\n");
    }

    request.push_str("\r\n");

    if let Some(body_content) = body {
        request.push_str(body_content);
    }

    // Send request
    let mut stream = stream;
    use std::io::{Read, Write};
    stream.write_all(request.as_bytes()).map_err(|e| format!("Write failed: {}", e))?;
    stream.flush().ok();

    // Read response
    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(|e| format!("Read failed: {}", e))?;

    // Parse response
    let response_str = String::from_utf8_lossy(&response);
    parse_http_response(&response_str)
}

/// Parse an HTTP response string into components
fn parse_http_response(response: &str) -> Result<HttpResponseEntry, String> {
    let mut lines = response.lines();

    // Parse status line
    let status_line = lines.next().ok_or("Empty response")?;
    let parts: Vec<&str> = status_line.splitn(3, ' ').collect();
    if parts.len() < 2 {
        return Err("Invalid status line".to_string());
    }

    let status_code: i64 = parts[1].parse().map_err(|_| "Invalid status code")?;

    // Parse headers
    let mut headers = HashMap::new();
    let mut body_start = false;
    let mut body_lines = Vec::new();

    for line in lines {
        if body_start {
            body_lines.push(line);
        } else if line.is_empty() {
            body_start = true;
        } else if let Some(idx) = line.find(':') {
            let key = line[..idx].trim().to_lowercase();
            let value = line[idx + 1..].trim().to_string();
            headers.insert(key, value);
        }
    }

    let body = body_lines.join("\n");

    Ok(HttpResponseEntry {
        status_code,
        headers,
        body,
    })
}

/// Perform an HTTP fetch request (for HostNetOp::HttpFetch).
/// This is the primary HTTP client function used by the `http.fetch` intrinsic.
/// Takes URL, method, headers (map handle), and body (bytes handle).
/// Returns: response handle (>= 110000), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_fetch(
    url: *const u8, url_len: i64,
    method: *const u8, method_len: i64,
    _headers_handle: i64,
    _body: *const u8, _body_len: i64,
    timeout_ms: i64,
) -> i64 {
    if url.is_null() || url_len <= 0 {
        return -1;
    }

    let url_str = unsafe {
        let slice = std::slice::from_raw_parts(url, url_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    let method_str = if method.is_null() || method_len <= 0 {
        "GET"
    } else {
        unsafe {
            let slice = std::slice::from_raw_parts(method, method_len as usize);
            std::str::from_utf8(slice).unwrap_or("GET")
        }
    };

    // For now, we support GET and POST with basic body handling
    // Headers from map handle are not yet integrated
    let body = None; // TODO: extract body from body handle

    match do_http_request(method_str, url_str, body, timeout_ms) {
        Ok(response) => {
            let handle = NEXT_HTTP_CLIENT_RESPONSE.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut m) = http_response_map().lock() {
                m.insert(handle, response);
                return handle;
            }
            -1
        }
        Err(_e) => {
            // Could store error message for later retrieval
            -1
        }
    }
}

/// Simplified HTTP fetch for URL-only requests (GET).
/// Used when only URL is provided without full Request struct.
/// Returns: response handle (>= 110000), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_fetch_url(url: *const u8, url_len: i64, timeout_ms: i64) -> i64 {
    __arth_http_fetch(url, url_len, std::ptr::null(), 0, 0, std::ptr::null(), 0, timeout_ms)
}

/// Perform a blocking HTTP GET request
/// Returns: response handle (>= 110000), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_get(url: *const u8, url_len: i64, timeout_ms: i64) -> i64 {
    if url.is_null() || url_len <= 0 {
        return -1;
    }

    let url_str = unsafe {
        let slice = std::slice::from_raw_parts(url, url_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    match do_http_request("GET", url_str, None, timeout_ms) {
        Ok(response) => {
            let handle = NEXT_HTTP_CLIENT_RESPONSE.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut m) = http_response_map().lock() {
                m.insert(handle, response);
                return handle;
            }
            -1
        }
        Err(_) => -1,
    }
}

/// Perform a blocking HTTP POST request
/// Returns: response handle (>= 110000), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_post(url: *const u8, url_len: i64, body: *const u8, body_len: i64, timeout_ms: i64) -> i64 {
    if url.is_null() || url_len <= 0 {
        return -1;
    }

    let url_str = unsafe {
        let slice = std::slice::from_raw_parts(url, url_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    let body_str = if body.is_null() || body_len <= 0 {
        ""
    } else {
        unsafe {
            let slice = std::slice::from_raw_parts(body, body_len as usize);
            match std::str::from_utf8(slice) {
                Ok(s) => s,
                Err(_) => return -1,
            }
        }
    };

    match do_http_request("POST", url_str, Some(body_str), timeout_ms) {
        Ok(response) => {
            let handle = NEXT_HTTP_CLIENT_RESPONSE.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut m) = http_response_map().lock() {
                m.insert(handle, response);
                return handle;
            }
            -1
        }
        Err(_) => -1,
    }
}

/// Start async HTTP GET request
/// Returns: request ID for checking completion
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_get_async(url: *const u8, url_len: i64, timeout_ms: i64, task_id: i64) -> i64 {
    if url.is_null() || url_len <= 0 {
        return -1;
    }

    let url_str = unsafe {
        let slice = std::slice::from_raw_parts(url, url_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    let request_id = NEXT_HTTP_CLIENT_REQUEST.fetch_add(1, Ordering::SeqCst);

    if let Ok(mut m) = http_request_map().lock() {
        m.insert(request_id, HttpAsyncRequest {
            kind: HttpRequestKind::Get { url: url_str, timeout_ms },
            task_id,
            state: HttpRequestState::Pending,
            result: 0,
            error_msg: None,
        });

        // Try to complete immediately
        drop(m);
        __arth_http_try_complete_request(request_id);

        return request_id;
    }
    -1
}

/// Start async HTTP POST request
/// Returns: request ID for checking completion
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_post_async(url: *const u8, url_len: i64, body: *const u8, body_len: i64, timeout_ms: i64, task_id: i64) -> i64 {
    if url.is_null() || url_len <= 0 {
        return -1;
    }

    let url_str = unsafe {
        let slice = std::slice::from_raw_parts(url, url_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    let body_str = if body.is_null() || body_len <= 0 {
        String::new()
    } else {
        unsafe {
            let slice = std::slice::from_raw_parts(body, body_len as usize);
            match std::str::from_utf8(slice) {
                Ok(s) => s.to_string(),
                Err(_) => return -1,
            }
        }
    };

    let request_id = NEXT_HTTP_CLIENT_REQUEST.fetch_add(1, Ordering::SeqCst);

    if let Ok(mut m) = http_request_map().lock() {
        m.insert(request_id, HttpAsyncRequest {
            kind: HttpRequestKind::Post { url: url_str, body: body_str, timeout_ms },
            task_id,
            state: HttpRequestState::Pending,
            result: 0,
            error_msg: None,
        });

        // Try to complete immediately
        drop(m);
        __arth_http_try_complete_request(request_id);

        return request_id;
    }
    -1
}

/// Get HTTP response status code
/// Returns: HTTP status code (200, 404, etc), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_response_status(response_handle: i64) -> i64 {
    if let Ok(m) = http_response_map().lock() {
        if let Some(resp) = m.get(&response_handle) {
            return resp.status_code;
        }
    }
    -1
}

/// Get HTTP response header value
/// Returns: 1 if found (value in TLS), 0 if not found, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_response_header(response_handle: i64, key: *const u8, key_len: i64) -> i64 {
    if key.is_null() || key_len <= 0 {
        return -1;
    }

    let key_str = unsafe {
        let slice = std::slice::from_raw_parts(key, key_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_lowercase(),
            Err(_) => return -1,
        }
    };

    if let Ok(m) = http_response_map().lock() {
        if let Some(resp) = m.get(&response_handle) {
            if let Some(value) = resp.headers.get(&key_str) {
                LAST_HTTP_HEADER.with(|h| {
                    *h.borrow_mut() = value.clone();
                });
                return 1;
            }
            return 0;
        }
    }
    -1
}

/// Get the last HTTP header value that was found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_get_last_header(out_len: *mut i64) -> *const u8 {
    LAST_HTTP_HEADER.with(|h| {
        let header = h.borrow();
        if !out_len.is_null() {
            unsafe { *out_len = header.len() as i64; }
        }
        header.as_ptr()
    })
}

/// Get the last HTTP header value as string index (for VM)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_get_last_header_string() -> i64 {
    LAST_HTTP_HEADER.with(|h| {
        let header = h.borrow();
        // Return length for now - VM will handle string storage
        header.len() as i64
    })
}

/// Get HTTP response body
/// Returns: 1 on success (body in TLS), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_response_body(response_handle: i64) -> i64 {
    if let Ok(m) = http_response_map().lock() {
        if let Some(resp) = m.get(&response_handle) {
            LAST_HTTP_BODY.with(|b| {
                *b.borrow_mut() = resp.body.clone();
            });
            return 1;
        }
    }
    -1
}

/// Get the last HTTP body that was read
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_get_last_body(out_len: *mut i64) -> *const u8 {
    LAST_HTTP_BODY.with(|b| {
        let body = b.borrow();
        if !out_len.is_null() {
            unsafe { *out_len = body.len() as i64; }
        }
        body.as_ptr()
    })
}

/// Get the last HTTP body as string (for VM)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_get_last_body_string() -> i64 {
    LAST_HTTP_BODY.with(|b| {
        let body = b.borrow();
        body.len() as i64
    })
}

/// Close HTTP response and release resources
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_response_close(response_handle: i64) -> i64 {
    if let Ok(mut m) = http_response_map().lock() {
        if m.remove(&response_handle).is_some() {
            return 0;
        }
    }
    -1
}

/// Check if async HTTP request is ready
/// Returns: 0=pending, 1=ready, -1=error/not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_check_ready(request_id: i64) -> i64 {
    // Try to complete first
    __arth_http_try_complete_request(request_id);

    if let Ok(m) = http_request_map().lock() {
        if let Some(req) = m.get(&request_id) {
            return match req.state {
                HttpRequestState::Pending => 0,
                HttpRequestState::Ready => 1,
                HttpRequestState::Error => -1,
            };
        }
    }
    -1
}

/// Get result of completed async HTTP request
/// Returns: response handle, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_get_result(request_id: i64) -> i64 {
    if let Ok(m) = http_request_map().lock() {
        if let Some(req) = m.get(&request_id) {
            if req.state == HttpRequestState::Ready {
                return req.result;
            }
        }
    }
    -1
}

/// Poll for next ready async HTTP request
/// Returns: request ID of ready request, -1 if none
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_poll_ready() -> i64 {
    let ids: Vec<i64> = if let Ok(m) = http_request_map().lock() {
        m.keys().cloned().collect()
    } else {
        return -1;
    };

    for id in ids {
        __arth_http_try_complete_request(id);

        if let Ok(m) = http_request_map().lock() {
            if let Some(req) = m.get(&id) {
                if req.state == HttpRequestState::Ready || req.state == HttpRequestState::Error {
                    return id;
                }
            }
        }
    }
    -1
}

/// Remove completed HTTP request from registry
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_remove_request(request_id: i64) -> i64 {
    if let Ok(mut m) = http_request_map().lock() {
        if m.remove(&request_id).is_some() {
            return 0;
        }
    }
    -1
}

/// Get length of HTTP response body
/// Returns: body length in bytes, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_get_body_length(response_handle: i64) -> i64 {
    if let Ok(m) = http_response_map().lock() {
        if let Some(resp) = m.get(&response_handle) {
            return resp.body.len() as i64;
        }
    }
    -1
}

/// Get number of headers in response
/// Returns: header count, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_get_header_count(response_handle: i64) -> i64 {
    if let Ok(m) = http_response_map().lock() {
        if let Some(resp) = m.get(&response_handle) {
            return resp.headers.len() as i64;
        }
    }
    -1
}

/// Get task ID waiting on HTTP request
/// Returns: task_id, -1 if not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_get_waiting_task(request_id: i64) -> i64 {
    if let Ok(m) = http_request_map().lock() {
        if let Some(req) = m.get(&request_id) {
            return req.task_id;
        }
    }
    -1
}

/// Internal function to try completing a pending HTTP request
fn __arth_http_try_complete_request(request_id: i64) {
    let kind = {
        if let Ok(m) = http_request_map().lock() {
            m.get(&request_id).filter(|r| r.state == HttpRequestState::Pending).map(|r| r.kind.clone())
        } else {
            None
        }
    };

    if let Some(kind) = kind {
        match kind {
            HttpRequestKind::Get { url, timeout_ms } => {
                match do_http_request("GET", &url, None, timeout_ms) {
                    Ok(response) => {
                        let handle = NEXT_HTTP_CLIENT_RESPONSE.fetch_add(1, Ordering::SeqCst);
                        if let Ok(mut m) = http_response_map().lock() {
                            m.insert(handle, response);
                        }
                        if let Ok(mut m) = http_request_map().lock() {
                            if let Some(req) = m.get_mut(&request_id) {
                                req.state = HttpRequestState::Ready;
                                req.result = handle;
                            }
                        }
                    }
                    Err(e) => {
                        if let Ok(mut m) = http_request_map().lock() {
                            if let Some(req) = m.get_mut(&request_id) {
                                req.state = HttpRequestState::Error;
                                req.result = -1;
                                req.error_msg = Some(e);
                            }
                        }
                    }
                }
            }
            HttpRequestKind::Post { url, body, timeout_ms } => {
                match do_http_request("POST", &url, Some(&body), timeout_ms) {
                    Ok(response) => {
                        let handle = NEXT_HTTP_CLIENT_RESPONSE.fetch_add(1, Ordering::SeqCst);
                        if let Ok(mut m) = http_response_map().lock() {
                            m.insert(handle, response);
                        }
                        if let Ok(mut m) = http_request_map().lock() {
                            if let Some(req) = m.get_mut(&request_id) {
                                req.state = HttpRequestState::Ready;
                                req.result = handle;
                            }
                        }
                    }
                    Err(e) => {
                        if let Ok(mut m) = http_request_map().lock() {
                            if let Some(req) = m.get_mut(&request_id) {
                                req.state = HttpRequestState::Error;
                                req.result = -1;
                                req.error_msg = Some(e);
                            }
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// HTTP Server Operations (C25 - Async HTTP Server)
// ============================================================================

/// HTTP Server entry in the registry
struct HttpServerEntry {
    listener: TcpListener,
    port: u16,
}

/// HTTP Connection entry - represents a single HTTP request being processed
struct HttpConnectionEntry {
    stream: Option<TcpStream>,
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
    response_status: i64,
    response_headers: Vec<(String, String)>,
    response_body: String,
}

/// HTTP Server async request state
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HttpServerRequestState {
    Pending,
    Ready,
    Error,
}

/// HTTP Server async request types
#[derive(Clone, Debug)]
enum HttpServerRequestKind {
    Accept { server_handle: i64 },
    Send { conn_handle: i64 },
}

/// HTTP Server async request entry
#[derive(Clone, Debug)]
struct HttpServerAsyncRequest {
    kind: HttpServerRequestKind,
    task_id: i64,
    state: HttpServerRequestState,
    result: i64,
    error_msg: Option<String>,
}

// Handle allocators for HTTP Server (C25)
// Note: Different from HTTP server handles in core.inc.rs
static NEXT_HTTP_SERVER_C25: AtomicI64 = AtomicI64::new(130_000);
static NEXT_HTTP_CONNECTION_C25: AtomicI64 = AtomicI64::new(140_000);
static NEXT_HTTP_SERVER_REQUEST_C25: AtomicI64 = AtomicI64::new(150_000);

// Storage for HTTP servers
fn http_server_map() -> &'static Mutex<HashMap<i64, HttpServerEntry>> {
    static MAP: OnceLock<Mutex<HashMap<i64, HttpServerEntry>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

// Storage for HTTP connections
fn http_connection_map() -> &'static Mutex<HashMap<i64, HttpConnectionEntry>> {
    static MAP: OnceLock<Mutex<HashMap<i64, HttpConnectionEntry>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

// Storage for HTTP server async requests
fn http_server_request_map() -> &'static Mutex<HashMap<i64, HttpServerAsyncRequest>> {
    static MAP: OnceLock<Mutex<HashMap<i64, HttpServerAsyncRequest>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

// Thread-local storage for HTTP server request data
thread_local! {
    static LAST_HTTP_REQUEST_METHOD: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static LAST_HTTP_REQUEST_PATH: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static LAST_HTTP_REQUEST_HEADER: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static LAST_HTTP_REQUEST_BODY: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Create an HTTP server bound to a port
/// Returns: server handle (>= 130000), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_server_create(port: i64) -> i64 {
    if port <= 0 || port > 65535 {
        return -1;
    }

    let addr = format!("0.0.0.0:{}", port);
    match TcpListener::bind(&addr) {
        Ok(listener) => {
            // Set non-blocking for async operations
            listener.set_nonblocking(true).ok();

            let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port as u16);
            let handle = NEXT_HTTP_SERVER_C25.fetch_add(1, Ordering::SeqCst);

            if let Ok(mut m) = http_server_map().lock() {
                m.insert(handle, HttpServerEntry {
                    listener,
                    port: actual_port,
                });
                return handle;
            }
            -1
        }
        Err(_) => -1,
    }
}

/// Close an HTTP server
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_server_close(server_handle: i64) -> i64 {
    if let Ok(mut m) = http_server_map().lock() {
        if m.remove(&server_handle).is_some() {
            return 0;
        }
    }
    -1
}

/// Get the bound port of an HTTP server
/// Returns: port number, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_server_get_port(server_handle: i64) -> i64 {
    if let Ok(m) = http_server_map().lock() {
        if let Some(server) = m.get(&server_handle) {
            return server.port as i64;
        }
    }
    -1
}

/// Parse an HTTP request from a TcpStream
fn parse_http_request(stream: &mut TcpStream) -> Result<HttpConnectionEntry, String> {
    use std::io::{BufRead, BufReader};

    // Set a short timeout for reading the request
    stream.set_read_timeout(Some(std::time::Duration::from_millis(5000))).ok();

    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).map_err(|e| e.to_string())?;

    // Parse request line: "GET /path HTTP/1.1"
    let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
    if parts.len() < 2 {
        return Err("Invalid request line".to_string());
    }

    let method = parts[0].to_string();
    let path = parts[1].to_string();

    // Parse headers
    let mut headers = HashMap::new();
    let mut content_length: usize = 0;

    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        if let Some(colon_pos) = line.find(':') {
            let key = line[..colon_pos].trim().to_lowercase();
            let value = line[colon_pos + 1..].trim().to_string();
            if key == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.insert(key, value);
        }
    }

    // Read body if present
    let mut body = String::new();
    if content_length > 0 {
        let mut buf = vec![0u8; content_length];
        reader.read_exact(&mut buf).ok();
        body = String::from_utf8_lossy(&buf).to_string();
    }

    Ok(HttpConnectionEntry {
        stream: Some(stream.try_clone().ok().unwrap_or_else(|| stream.try_clone().unwrap())),
        method,
        path,
        headers,
        body,
        response_status: 200,
        response_headers: Vec::new(),
        response_body: String::new(),
    })
}

/// Accept an incoming HTTP request (blocking)
/// Returns: connection handle (>= 140000), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_server_accept(server_handle: i64) -> i64 {
    // Get the listener
    let listener_clone = {
        if let Ok(m) = http_server_map().lock() {
            if let Some(server) = m.get(&server_handle) {
                // Try to clone the listener for blocking accept
                match server.listener.try_clone() {
                    Ok(l) => Some(l),
                    Err(_) => return -1,
                }
            } else {
                return -1;
            }
        } else {
            return -1;
        }
    };

    if let Some(listener) = listener_clone {
        // Set blocking mode for this operation
        listener.set_nonblocking(false).ok();

        match listener.accept() {
            Ok((mut stream, _)) => {
                match parse_http_request(&mut stream) {
                    Ok(mut conn) => {
                        conn.stream = Some(stream);
                        let handle = NEXT_HTTP_CONNECTION_C25.fetch_add(1, Ordering::SeqCst);
                        if let Ok(mut m) = http_connection_map().lock() {
                            m.insert(handle, conn);
                            return handle;
                        }
                        -1
                    }
                    Err(_) => -1,
                }
            }
            Err(_) => -1,
        }
    } else {
        -1
    }
}

/// Start async accept for incoming HTTP request
/// Returns: request ID for checking completion
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_server_accept_async(server_handle: i64, task_id: i64) -> i64 {
    // Verify server exists
    if let Ok(m) = http_server_map().lock() {
        if !m.contains_key(&server_handle) {
            return -1;
        }
    } else {
        return -1;
    }

    let request_id = NEXT_HTTP_SERVER_REQUEST_C25.fetch_add(1, Ordering::SeqCst);

    if let Ok(mut m) = http_server_request_map().lock() {
        m.insert(request_id, HttpServerAsyncRequest {
            kind: HttpServerRequestKind::Accept { server_handle },
            task_id,
            state: HttpServerRequestState::Pending,
            result: 0,
            error_msg: None,
        });

        // Try to complete immediately
        drop(m);
        __arth_http_server_try_complete_request(request_id);

        return request_id;
    }
    -1
}

/// Get the HTTP method of a request
/// Returns: 1 on success (method in TLS), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_request_method(conn_handle: i64) -> i64 {
    if let Ok(m) = http_connection_map().lock() {
        if let Some(conn) = m.get(&conn_handle) {
            LAST_HTTP_REQUEST_METHOD.with(|method| {
                *method.borrow_mut() = conn.method.clone();
            });
            return 1;
        }
    }
    -1
}

/// Get the last HTTP request method
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_request_get_method(out_len: *mut i64) -> *const u8 {
    LAST_HTTP_REQUEST_METHOD.with(|method| {
        let m = method.borrow();
        if !out_len.is_null() {
            unsafe { *out_len = m.len() as i64; }
        }
        m.as_ptr()
    })
}

/// Get the HTTP request path
/// Returns: 1 on success (path in TLS), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_request_path(conn_handle: i64) -> i64 {
    if let Ok(m) = http_connection_map().lock() {
        if let Some(conn) = m.get(&conn_handle) {
            LAST_HTTP_REQUEST_PATH.with(|path| {
                *path.borrow_mut() = conn.path.clone();
            });
            return 1;
        }
    }
    -1
}

/// Get the last HTTP request path
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_request_get_path(out_len: *mut i64) -> *const u8 {
    LAST_HTTP_REQUEST_PATH.with(|path| {
        let p = path.borrow();
        if !out_len.is_null() {
            unsafe { *out_len = p.len() as i64; }
        }
        p.as_ptr()
    })
}

/// Get a request header value
/// Returns: 1 on success (header in TLS), 0 if not found, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_request_header(conn_handle: i64, name: *const u8, name_len: i64) -> i64 {
    if name.is_null() || name_len <= 0 {
        return -1;
    }

    let header_name = unsafe {
        let slice = std::slice::from_raw_parts(name, name_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_lowercase(),
            Err(_) => return -1,
        }
    };

    if let Ok(m) = http_connection_map().lock() {
        if let Some(conn) = m.get(&conn_handle) {
            if let Some(value) = conn.headers.get(&header_name) {
                LAST_HTTP_REQUEST_HEADER.with(|h| {
                    *h.borrow_mut() = value.clone();
                });
                return 1;
            }
            return 0;
        }
    }
    -1
}

/// Get the last HTTP request header value
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_request_get_header(out_len: *mut i64) -> *const u8 {
    LAST_HTTP_REQUEST_HEADER.with(|h| {
        let header = h.borrow();
        if !out_len.is_null() {
            unsafe { *out_len = header.len() as i64; }
        }
        header.as_ptr()
    })
}

/// Get the HTTP request body
/// Returns: 1 on success (body in TLS), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_request_body(conn_handle: i64) -> i64 {
    if let Ok(m) = http_connection_map().lock() {
        if let Some(conn) = m.get(&conn_handle) {
            LAST_HTTP_REQUEST_BODY.with(|b| {
                *b.borrow_mut() = conn.body.clone();
            });
            return 1;
        }
    }
    -1
}

/// Get the last HTTP request body
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_request_get_body(out_len: *mut i64) -> *const u8 {
    LAST_HTTP_REQUEST_BODY.with(|b| {
        let body = b.borrow();
        if !out_len.is_null() {
            unsafe { *out_len = body.len() as i64; }
        }
        body.as_ptr()
    })
}

/// Get number of headers in request
/// Returns: header count, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_request_header_count(conn_handle: i64) -> i64 {
    if let Ok(m) = http_connection_map().lock() {
        if let Some(conn) = m.get(&conn_handle) {
            return conn.headers.len() as i64;
        }
    }
    -1
}

/// Get length of request body
/// Returns: body length in bytes, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_request_body_length(conn_handle: i64) -> i64 {
    if let Ok(m) = http_connection_map().lock() {
        if let Some(conn) = m.get(&conn_handle) {
            return conn.body.len() as i64;
        }
    }
    -1
}

/// Set the response status code
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_writer_status(conn_handle: i64, status_code: i64) -> i64 {
    if let Ok(mut m) = http_connection_map().lock() {
        if let Some(conn) = m.get_mut(&conn_handle) {
            conn.response_status = status_code;
            return 0;
        }
    }
    -1
}

/// Add a response header
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_writer_header(conn_handle: i64, name: *const u8, name_len: i64, value: *const u8, value_len: i64) -> i64 {
    if name.is_null() || name_len <= 0 || value.is_null() || value_len < 0 {
        return -1;
    }

    let header_name = unsafe {
        let slice = std::slice::from_raw_parts(name, name_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    let header_value = unsafe {
        let slice = std::slice::from_raw_parts(value, value_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    if let Ok(mut m) = http_connection_map().lock() {
        if let Some(conn) = m.get_mut(&conn_handle) {
            conn.response_headers.push((header_name, header_value));
            return 0;
        }
    }
    -1
}

/// Set the response body
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_writer_body(conn_handle: i64, body: *const u8, body_len: i64) -> i64 {
    if body.is_null() {
        return -1;
    }

    let body_str = unsafe {
        let slice = std::slice::from_raw_parts(body, body_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    if let Ok(mut m) = http_connection_map().lock() {
        if let Some(conn) = m.get_mut(&conn_handle) {
            conn.response_body = body_str;
            return 0;
        }
    }
    -1
}

/// Build and send the HTTP response
fn do_http_server_send(conn_handle: i64) -> Result<(), String> {
    // Get connection data
    let (stream, status, headers, body) = {
        if let Ok(mut m) = http_connection_map().lock() {
            if let Some(conn) = m.get_mut(&conn_handle) {
                let stream = conn.stream.take();
                let status = conn.response_status;
                let headers = conn.response_headers.clone();
                let body = conn.response_body.clone();
                (stream, status, headers, body)
            } else {
                return Err("Connection not found".to_string());
            }
        } else {
            return Err("Lock failed".to_string());
        }
    };

    let mut stream = stream.ok_or("Stream not available")?;

    // Build HTTP response
    let status_text = match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Unknown",
    };

    let mut response = format!("HTTP/1.1 {} {}\r\n", status, status_text);

    // Add Content-Length if not present
    let has_content_length = headers.iter().any(|(k, _)| k.to_lowercase() == "content-length");
    if !has_content_length {
        response.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }

    // Add headers
    for (name, value) in &headers {
        response.push_str(&format!("{}: {}\r\n", name, value));
    }

    response.push_str("\r\n");
    response.push_str(&body);

    use std::io::Write;
    stream.write_all(response.as_bytes()).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    // Close the connection
    if let Ok(mut m) = http_connection_map().lock() {
        m.remove(&conn_handle);
    }

    Ok(())
}

/// Send the HTTP response (blocking)
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_writer_send(conn_handle: i64) -> i64 {
    match do_http_server_send(conn_handle) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

/// Send the HTTP response asynchronously
/// Returns: request ID for checking completion
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_writer_send_async(conn_handle: i64, task_id: i64) -> i64 {
    // Verify connection exists
    if let Ok(m) = http_connection_map().lock() {
        if !m.contains_key(&conn_handle) {
            return -1;
        }
    } else {
        return -1;
    }

    let request_id = NEXT_HTTP_SERVER_REQUEST_C25.fetch_add(1, Ordering::SeqCst);

    if let Ok(mut m) = http_server_request_map().lock() {
        m.insert(request_id, HttpServerAsyncRequest {
            kind: HttpServerRequestKind::Send { conn_handle },
            task_id,
            state: HttpServerRequestState::Pending,
            result: 0,
            error_msg: None,
        });

        // Try to complete immediately
        drop(m);
        __arth_http_server_try_complete_request(request_id);

        return request_id;
    }
    -1
}

/// Check if async HTTP server operation is ready
/// Returns: 0=pending, 1=ready, -1=error/not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_server_check_ready(request_id: i64) -> i64 {
    // Try to complete first
    __arth_http_server_try_complete_request(request_id);

    if let Ok(m) = http_server_request_map().lock() {
        if let Some(req) = m.get(&request_id) {
            return match req.state {
                HttpServerRequestState::Pending => 0,
                HttpServerRequestState::Ready => 1,
                HttpServerRequestState::Error => -1,
            };
        }
    }
    -1
}

/// Get result of completed async HTTP server operation
/// Returns: result value, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_server_get_result(request_id: i64) -> i64 {
    if let Ok(m) = http_server_request_map().lock() {
        if let Some(req) = m.get(&request_id) {
            if req.state == HttpServerRequestState::Ready {
                return req.result;
            }
        }
    }
    -1
}

/// Poll for next ready async HTTP server operation
/// Returns: request ID of ready operation, -1 if none
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_server_poll_ready() -> i64 {
    let ids: Vec<i64> = if let Ok(m) = http_server_request_map().lock() {
        m.keys().cloned().collect()
    } else {
        return -1;
    };

    for id in ids {
        __arth_http_server_try_complete_request(id);

        if let Ok(m) = http_server_request_map().lock() {
            if let Some(req) = m.get(&id) {
                if req.state == HttpServerRequestState::Ready || req.state == HttpServerRequestState::Error {
                    return id;
                }
            }
        }
    }
    -1
}

/// Remove completed HTTP server request from registry
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_server_remove_request(request_id: i64) -> i64 {
    if let Ok(mut m) = http_server_request_map().lock() {
        if m.remove(&request_id).is_some() {
            return 0;
        }
    }
    -1
}

/// Get task ID waiting on HTTP server request
/// Returns: task_id, -1 if not found
#[unsafe(no_mangle)]
pub extern "C" fn __arth_http_server_get_waiting_task(request_id: i64) -> i64 {
    if let Ok(m) = http_server_request_map().lock() {
        if let Some(req) = m.get(&request_id) {
            return req.task_id;
        }
    }
    -1
}

/// Internal function to try completing a pending HTTP server request
fn __arth_http_server_try_complete_request(request_id: i64) {
    let kind = {
        if let Ok(m) = http_server_request_map().lock() {
            m.get(&request_id).filter(|r| r.state == HttpServerRequestState::Pending).map(|r| r.kind.clone())
        } else {
            None
        }
    };

    if let Some(kind) = kind {
        match kind {
            HttpServerRequestKind::Accept { server_handle } => {
                // Try non-blocking accept
                let listener_clone = {
                    if let Ok(m) = http_server_map().lock() {
                        m.get(&server_handle).and_then(|s| s.listener.try_clone().ok())
                    } else {
                        None
                    }
                };

                if let Some(listener) = listener_clone {
                    // Keep non-blocking mode
                    listener.set_nonblocking(true).ok();

                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            match parse_http_request(&mut stream) {
                                Ok(mut conn) => {
                                    conn.stream = Some(stream);
                                    let conn_handle = NEXT_HTTP_CONNECTION_C25.fetch_add(1, Ordering::SeqCst);
                                    if let Ok(mut cm) = http_connection_map().lock() {
                                        cm.insert(conn_handle, conn);
                                    }
                                    if let Ok(mut rm) = http_server_request_map().lock() {
                                        if let Some(req) = rm.get_mut(&request_id) {
                                            req.state = HttpServerRequestState::Ready;
                                            req.result = conn_handle;
                                        }
                                    }
                                }
                                Err(e) => {
                                    if let Ok(mut rm) = http_server_request_map().lock() {
                                        if let Some(req) = rm.get_mut(&request_id) {
                                            req.state = HttpServerRequestState::Error;
                                            req.result = -1;
                                            req.error_msg = Some(e);
                                        }
                                    }
                                }
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            // Still pending, nothing to do
                        }
                        Err(e) => {
                            if let Ok(mut rm) = http_server_request_map().lock() {
                                if let Some(req) = rm.get_mut(&request_id) {
                                    req.state = HttpServerRequestState::Error;
                                    req.result = -1;
                                    req.error_msg = Some(e.to_string());
                                }
                            }
                        }
                    }
                }
            }
            HttpServerRequestKind::Send { conn_handle } => {
                match do_http_server_send(conn_handle) {
                    Ok(_) => {
                        if let Ok(mut rm) = http_server_request_map().lock() {
                            if let Some(req) = rm.get_mut(&request_id) {
                                req.state = HttpServerRequestState::Ready;
                                req.result = 0;
                            }
                        }
                    }
                    Err(e) => {
                        if let Ok(mut rm) = http_server_request_map().lock() {
                            if let Some(req) = rm.get_mut(&request_id) {
                                req.state = HttpServerRequestState::Error;
                                req.result = -1;
                                req.error_msg = Some(e);
                            }
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// End of HTTP Server Operations (C25)
// ============================================================================

// ============================================================================
// WebSocket Server Operations (Synchronous - using arth_rt sockets)
// ============================================================================

// Handle allocators for WebSocket
static NEXT_WS_SERVER_HANDLE: AtomicI64 = AtomicI64::new(170_000);
static NEXT_WS_CONNECTION_HANDLE: AtomicI64 = AtomicI64::new(171_000);

/// WebSocket server entry (synchronous implementation)
struct WsServerEntry {
    /// Channel to receive new connections
    conn_rx: crossbeam_channel::Receiver<WsConnectionEntry>,
    /// Shutdown flag
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    port: u16,
}

/// WebSocket connection entry (synchronous implementation)
struct WsConnectionEntry {
    /// Socket handle for sending (protected by mutex for thread safety)
    socket: std::sync::Arc<Mutex<i64>>,
    /// Channel to receive incoming messages
    in_rx: crossbeam_channel::Receiver<WsInMsg>,
    /// Connection open flag
    is_open: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Incoming WebSocket message
#[derive(Debug, Clone)]
enum WsInMsg {
    Text(String),
    Binary(Vec<u8>),
    Close(u16, String),
}

fn ws_server_map() -> &'static Mutex<HashMap<i64, WsServerEntry>> {
    static MAP: OnceLock<Mutex<HashMap<i64, WsServerEntry>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ws_conn_map() -> &'static Mutex<HashMap<i64, WsConnectionEntry>> {
    static MAP: OnceLock<Mutex<HashMap<i64, WsConnectionEntry>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

// Thread-local storage for last received WebSocket message
thread_local! {
    static LAST_WS_MSG_TEXT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static LAST_WS_MSG_BINARY: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
    static LAST_WS_MSG_TYPE: std::cell::Cell<i64> = const { std::cell::Cell::new(0) }; // 0=text, 1=binary, 2=close
}

/// Compute WebSocket Sec-WebSocket-Accept key
fn ws_compute_accept_key(client_key: &str) -> String {
    use sha1::{Digest, Sha1};
    const WS_MAGIC: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut hasher = Sha1::new();
    hasher.update(client_key.as_bytes());
    hasher.update(WS_MAGIC.as_bytes());
    let hash = hasher.finalize();

    // Base64 encode using arth_rt
    let hash_bytes = hash.as_slice();
    let enc_len = arth_rt::encoding::arth_rt_base64_encode_len(hash_bytes.len());
    let mut encoded = vec![0u8; enc_len];
    let len = arth_rt::encoding::arth_rt_base64_encode(
        hash_bytes.as_ptr(),
        hash_bytes.len(),
        encoded.as_mut_ptr(),
        encoded.len(),
    );
    if len > 0 {
        encoded.truncate(len as usize);
        String::from_utf8_lossy(&encoded).into_owned()
    } else {
        String::new()
    }
}

/// Read a line from socket (up to \r\n)
fn ws_read_line(sock: i64) -> Option<String> {
    let mut line = Vec::new();
    let mut buf = [0u8; 1];
    loop {
        let n = arth_rt::net::arth_rt_socket_recv(sock, buf.as_mut_ptr(), 1usize, 0);
        if n <= 0 {
            return None;
        }
        line.push(buf[0]);
        if line.len() >= 2 && line[line.len() - 2] == b'\r' && line[line.len() - 1] == b'\n' {
            line.truncate(line.len() - 2);
            break;
        }
        if line.len() > 8192 {
            return None; // Line too long
        }
    }
    String::from_utf8(line).ok()
}

/// Read exact bytes from socket
fn ws_read_exact(sock: i64, buf: &mut [u8]) -> bool {
    let mut offset = 0;
    while offset < buf.len() {
        let n = arth_rt::net::arth_rt_socket_recv(
            sock,
            buf[offset..].as_mut_ptr(),
            buf.len() - offset,
            0,
        );
        if n <= 0 {
            return false;
        }
        offset += n as usize;
    }
    true
}

/// Write all bytes to socket
fn ws_write_all(sock: i64, data: &[u8]) -> bool {
    let mut offset = 0;
    while offset < data.len() {
        let n = arth_rt::net::arth_rt_socket_send(
            sock,
            data[offset..].as_ptr(),
            data.len() - offset,
            0,
        );
        if n <= 0 {
            return false;
        }
        offset += n as usize;
    }
    true
}

/// Send a WebSocket frame
fn ws_send_frame(sock: i64, opcode: u8, data: &[u8]) -> bool {
    let len = data.len();
    let mut frame = Vec::with_capacity(10 + len);

    // FIN bit + opcode
    frame.push(0x80 | opcode);

    // Payload length (server doesn't mask)
    if len < 126 {
        frame.push(len as u8);
    } else if len < 65536 {
        frame.push(126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }

    // Payload
    frame.extend_from_slice(data);

    ws_write_all(sock, &frame)
}

/// Read a WebSocket frame, returns (opcode, payload) or None on error
fn ws_read_frame(sock: i64) -> Option<(u8, Vec<u8>)> {
    let mut header = [0u8; 2];
    if !ws_read_exact(sock, &mut header) {
        return None;
    }

    let _fin = (header[0] & 0x80) != 0;
    let opcode = header[0] & 0x0F;
    let masked = (header[1] & 0x80) != 0;
    let mut payload_len = (header[1] & 0x7F) as u64;

    // Extended payload length
    if payload_len == 126 {
        let mut ext = [0u8; 2];
        if !ws_read_exact(sock, &mut ext) {
            return None;
        }
        payload_len = u16::from_be_bytes(ext) as u64;
    } else if payload_len == 127 {
        let mut ext = [0u8; 8];
        if !ws_read_exact(sock, &mut ext) {
            return None;
        }
        payload_len = u64::from_be_bytes(ext);
    }

    // Read masking key if present
    let mask = if masked {
        let mut m = [0u8; 4];
        if !ws_read_exact(sock, &mut m) {
            return None;
        }
        Some(m)
    } else {
        None
    };

    // Read payload
    let mut payload = vec![0u8; payload_len as usize];
    if payload_len > 0 && !ws_read_exact(sock, &mut payload) {
        return None;
    }

    // Unmask if needed
    if let Some(mask) = mask {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
    }

    Some((opcode, payload))
}

/// Create a WebSocket server bound to a port and path.
/// Port 0 means let the OS assign a free port.
/// Returns: server handle (>= 170000), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_ws_serve(port: i64, _path: *const u8, _path_len: i64) -> i64 {
    if port < 0 || port > 65535 {
        return -1;
    }

    // Create server socket using arth_rt
    let sock = arth_rt::net::arth_rt_socket_create(libc::AF_INET, libc::SOCK_STREAM, 0);
    if sock < 0 {
        return -1;
    }

    // Set SO_REUSEADDR
    arth_rt::net::arth_rt_socket_setsockopt_int(sock, libc::SOL_SOCKET, libc::SO_REUSEADDR, 1);

    // Bind
    if arth_rt::net::arth_rt_socket_bind_port(sock, port as u16) < 0 {
        arth_rt::net::arth_rt_socket_close(sock);
        return -1;
    }

    // Listen
    if arth_rt::net::arth_rt_socket_listen(sock, 128) < 0 {
        arth_rt::net::arth_rt_socket_close(sock);
        return -1;
    }

    // Get actual port (for port 0)
    let actual_port = port as u16; // TODO: get actual port from getsockname

    let (conn_tx, conn_rx) = crossbeam_channel::unbounded::<WsConnectionEntry>();
    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    // Spawn accept thread
    std::thread::spawn(move || {
        while !shutdown_clone.load(std::sync::atomic::Ordering::SeqCst) {
            let client_sock = arth_rt::net::arth_rt_socket_accept(sock);
            if client_sock < 0 {
                if shutdown_clone.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }

            let conn_tx = conn_tx.clone();
            std::thread::spawn(move || {
                // Read HTTP request line (unused, but must be read)
                let _request_line = match ws_read_line(client_sock) {
                    Some(l) => l,
                    None => {
                        arth_rt::net::arth_rt_socket_close(client_sock);
                        return;
                    }
                };

                // Parse headers
                let mut headers = HashMap::new();
                loop {
                    let line = match ws_read_line(client_sock) {
                        Some(l) => l,
                        None => {
                            arth_rt::net::arth_rt_socket_close(client_sock);
                            return;
                        }
                    };
                    if line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        headers.insert(name.trim().to_lowercase(), value.trim().to_string());
                    }
                }

                // Check WebSocket upgrade
                let upgrade = headers.get("upgrade").map(|s| s.as_str()).unwrap_or("");
                let connection = headers.get("connection").map(|s| s.as_str()).unwrap_or("");
                let ws_key = headers.get("sec-websocket-key");

                if !upgrade.eq_ignore_ascii_case("websocket")
                    || !connection.to_lowercase().contains("upgrade")
                    || ws_key.is_none()
                {
                    let resp = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";
                    ws_write_all(client_sock, resp);
                    arth_rt::net::arth_rt_socket_close(client_sock);
                    return;
                }

                // Send upgrade response
                let accept_key = ws_compute_accept_key(ws_key.unwrap());
                let response = format!(
                    "HTTP/1.1 101 Switching Protocols\r\n\
                     Upgrade: websocket\r\n\
                     Connection: Upgrade\r\n\
                     Sec-WebSocket-Accept: {}\r\n\r\n",
                    accept_key
                );
                if !ws_write_all(client_sock, response.as_bytes()) {
                    arth_rt::net::arth_rt_socket_close(client_sock);
                    return;
                }

                // Create connection entry
                let (in_tx, in_rx) = crossbeam_channel::unbounded::<WsInMsg>();
                let is_open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
                let socket = std::sync::Arc::new(Mutex::new(client_sock));

                let _ = conn_tx.send(WsConnectionEntry {
                    socket: socket.clone(),
                    in_rx,
                    is_open: is_open.clone(),
                });

                // Read loop
                while is_open.load(std::sync::atomic::Ordering::SeqCst) {
                    match ws_read_frame(client_sock) {
                        Some((0x01, payload)) => {
                            // Text
                            let text = String::from_utf8_lossy(&payload).to_string();
                            let _ = in_tx.send(WsInMsg::Text(text));
                        }
                        Some((0x02, payload)) => {
                            // Binary
                            let _ = in_tx.send(WsInMsg::Binary(payload));
                        }
                        Some((0x08, payload)) => {
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
                            let _ = in_tx.send(WsInMsg::Close(code, reason));
                            is_open.store(false, std::sync::atomic::Ordering::SeqCst);
                            break;
                        }
                        Some((0x09, payload)) => {
                            // Ping - send pong
                            let _ = ws_send_frame(client_sock, 0x0A, &payload);
                        }
                        Some((0x0A, _)) => {
                            // Pong - ignore
                        }
                        _ => {
                            // Error or unknown opcode
                            is_open.store(false, std::sync::atomic::Ordering::SeqCst);
                            break;
                        }
                    }
                }

                arth_rt::net::arth_rt_socket_close(client_sock);
            });
        }
        arth_rt::net::arth_rt_socket_close(sock);
    });

    let handle = NEXT_WS_SERVER_HANDLE.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut m) = ws_server_map().lock() {
        m.insert(handle, WsServerEntry {
            conn_rx,
            shutdown,
            port: actual_port,
        });
        return handle;
    }
    -1
}

/// Accept a WebSocket connection from a server.
/// Returns: connection handle (>= 171000), -1 on error or no pending connection
#[unsafe(no_mangle)]
pub extern "C" fn __arth_ws_accept(server_handle: i64) -> i64 {
    // Try to receive a pending connection
    let conn = {
        if let Ok(m) = ws_server_map().lock() {
            if let Some(server) = m.get(&server_handle) {
                server.conn_rx.try_recv().ok()
            } else {
                None
            }
        } else {
            None
        }
    };

    if let Some(conn_entry) = conn {
        let handle = NEXT_WS_CONNECTION_HANDLE.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut m) = ws_conn_map().lock() {
            m.insert(handle, conn_entry);
            return handle;
        }
    }
    -1
}

/// Send a text message over a WebSocket connection.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_ws_send_text(conn_handle: i64, msg: *const u8, msg_len: i64) -> i64 {
    if msg.is_null() || msg_len < 0 {
        return -1;
    }

    let text = unsafe {
        std::slice::from_raw_parts(msg, msg_len as usize)
    };

    if let Ok(m) = ws_conn_map().lock() {
        if let Some(conn) = m.get(&conn_handle) {
            if conn.is_open.load(std::sync::atomic::Ordering::SeqCst) {
                if let Ok(sock) = conn.socket.lock() {
                    if ws_send_frame(*sock, 0x01, text) {
                        return 0;
                    }
                }
            }
        }
    }
    -1
}

/// Send a binary message over a WebSocket connection.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_ws_send_binary(conn_handle: i64, data: *const u8, data_len: i64) -> i64 {
    if data.is_null() || data_len < 0 {
        return -1;
    }

    let binary = unsafe {
        std::slice::from_raw_parts(data, data_len as usize)
    };

    if let Ok(m) = ws_conn_map().lock() {
        if let Some(conn) = m.get(&conn_handle) {
            if conn.is_open.load(std::sync::atomic::Ordering::SeqCst) {
                if let Ok(sock) = conn.socket.lock() {
                    if ws_send_frame(*sock, 0x02, binary) {
                        return 0;
                    }
                }
            }
        }
    }
    -1
}

/// Receive a message from a WebSocket connection.
/// Returns: message type (0=text, 1=binary, 2=close), -1 on error or no message
/// Message data is stored in thread-local storage
#[unsafe(no_mangle)]
pub extern "C" fn __arth_ws_recv(conn_handle: i64) -> i64 {
    let msg = {
        if let Ok(m) = ws_conn_map().lock() {
            if let Some(conn) = m.get(&conn_handle) {
                conn.in_rx.try_recv().ok()
            } else {
                None
            }
        } else {
            None
        }
    };

    match msg {
        Some(WsInMsg::Text(text)) => {
            LAST_WS_MSG_TEXT.with(|cell| *cell.borrow_mut() = text);
            LAST_WS_MSG_TYPE.with(|cell| cell.set(0));
            0
        }
        Some(WsInMsg::Binary(data)) => {
            LAST_WS_MSG_BINARY.with(|cell| *cell.borrow_mut() = data);
            LAST_WS_MSG_TYPE.with(|cell| cell.set(1));
            1
        }
        Some(WsInMsg::Close(code, reason)) => {
            LAST_WS_MSG_TEXT.with(|cell| *cell.borrow_mut() = format!("{}:{}", code, reason));
            LAST_WS_MSG_TYPE.with(|cell| cell.set(2));
            2
        }
        None => -1,
    }
}

/// Get the last received text message.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_ws_get_text(out_len: *mut i64) -> *const u8 {
    LAST_WS_MSG_TEXT.with(|cell| {
        let text = cell.borrow();
        if !out_len.is_null() {
            unsafe { *out_len = text.len() as i64; }
        }
        text.as_ptr()
    })
}

/// Get the last received binary message.
#[unsafe(no_mangle)]
pub extern "C" fn __arth_ws_get_binary(out_len: *mut i64) -> *const u8 {
    LAST_WS_MSG_BINARY.with(|cell| {
        let data = cell.borrow();
        if !out_len.is_null() {
            unsafe { *out_len = data.len() as i64; }
        }
        data.as_ptr()
    })
}

/// Close a WebSocket connection.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_ws_close(conn_handle: i64, code: i64, reason: *const u8, reason_len: i64) -> i64 {
    let reason_bytes = if reason.is_null() || reason_len <= 0 {
        Vec::new()
    } else {
        unsafe {
            std::slice::from_raw_parts(reason, reason_len as usize).to_vec()
        }
    };

    if let Ok(mut m) = ws_conn_map().lock() {
        if let Some(conn) = m.remove(&conn_handle) {
            conn.is_open.store(false, std::sync::atomic::Ordering::SeqCst);
            // Send close frame
            if let Ok(sock) = conn.socket.lock() {
                let mut payload = Vec::with_capacity(2 + reason_bytes.len());
                payload.extend_from_slice(&(code as u16).to_be_bytes());
                payload.extend_from_slice(&reason_bytes);
                let _ = ws_send_frame(*sock, 0x08, &payload);
                arth_rt::net::arth_rt_socket_close(*sock);
            }
            return 0;
        }
    }
    -1
}

/// Check if a WebSocket connection is open.
/// Returns: 1 if open, 0 if closed, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_ws_is_open(conn_handle: i64) -> i64 {
    if let Ok(m) = ws_conn_map().lock() {
        if let Some(conn) = m.get(&conn_handle) {
            return if conn.is_open.load(std::sync::atomic::Ordering::SeqCst) { 1 } else { 0 };
        }
    }
    -1
}

/// Close a WebSocket server.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_ws_server_close(server_handle: i64) -> i64 {
    if let Ok(mut m) = ws_server_map().lock() {
        if let Some(server) = m.remove(&server_handle) {
            server.shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            return 0;
        }
    }
    -1
}

// ============================================================================
// End of WebSocket Server Operations
// ============================================================================

// ============================================================================
// SSE (Server-Sent Events) Operations (Synchronous - using arth_rt sockets)
// ============================================================================

// Handle allocators for SSE
static NEXT_SSE_SERVER_HANDLE: AtomicI64 = AtomicI64::new(175_000);
static NEXT_SSE_EMITTER_HANDLE: AtomicI64 = AtomicI64::new(176_000);

/// SSE server entry (synchronous implementation)
struct SseServerEntry {
    /// Channel to receive new emitters (client connections)
    emitter_rx: crossbeam_channel::Receiver<SseEmitterEntry>,
    /// Shutdown flag
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    port: u16,
}

/// SSE emitter entry (synchronous implementation)
struct SseEmitterEntry {
    /// Socket handle for sending (protected by mutex for thread safety)
    socket: std::sync::Arc<Mutex<i64>>,
    /// Connection open flag
    is_open: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

fn sse_server_map() -> &'static Mutex<HashMap<i64, SseServerEntry>> {
    static MAP: OnceLock<Mutex<HashMap<i64, SseServerEntry>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sse_emitter_map() -> &'static Mutex<HashMap<i64, SseEmitterEntry>> {
    static MAP: OnceLock<Mutex<HashMap<i64, SseEmitterEntry>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Create an SSE server bound to a port and path.
/// Port 0 means let the OS assign a free port.
/// Returns: server handle (>= 175000), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_sse_serve(port: i64, _path: *const u8, _path_len: i64) -> i64 {
    if port < 0 || port > 65535 {
        return -1;
    }

    // Create server socket using arth_rt
    let sock = arth_rt::net::arth_rt_socket_create(libc::AF_INET, libc::SOCK_STREAM, 0);
    if sock < 0 {
        return -1;
    }

    // Set SO_REUSEADDR
    arth_rt::net::arth_rt_socket_setsockopt_int(sock, libc::SOL_SOCKET, libc::SO_REUSEADDR, 1);

    // Bind
    if arth_rt::net::arth_rt_socket_bind_port(sock, port as u16) < 0 {
        arth_rt::net::arth_rt_socket_close(sock);
        return -1;
    }

    // Listen
    if arth_rt::net::arth_rt_socket_listen(sock, 128) < 0 {
        arth_rt::net::arth_rt_socket_close(sock);
        return -1;
    }

    let actual_port = port as u16;

    let (emitter_tx, emitter_rx) = crossbeam_channel::unbounded::<SseEmitterEntry>();
    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    // Spawn accept thread
    std::thread::spawn(move || {
        while !shutdown_clone.load(std::sync::atomic::Ordering::SeqCst) {
            let client_sock = arth_rt::net::arth_rt_socket_accept(sock);
            if client_sock < 0 {
                if shutdown_clone.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }

            let emitter_tx = emitter_tx.clone();
            std::thread::spawn(move || {
                // Read HTTP request line (skip it)
                if ws_read_line(client_sock).is_none() {
                    arth_rt::net::arth_rt_socket_close(client_sock);
                    return;
                }

                // Skip remaining headers
                loop {
                    let line = match ws_read_line(client_sock) {
                        Some(l) => l,
                        None => {
                            arth_rt::net::arth_rt_socket_close(client_sock);
                            return;
                        }
                    };
                    if line.is_empty() {
                        break;
                    }
                }

                // Send SSE response headers
                let response = "HTTP/1.1 200 OK\r\n\
                    Content-Type: text/event-stream\r\n\
                    Cache-Control: no-cache\r\n\
                    Connection: keep-alive\r\n\
                    Access-Control-Allow-Origin: *\r\n\r\n";
                if !ws_write_all(client_sock, response.as_bytes()) {
                    arth_rt::net::arth_rt_socket_close(client_sock);
                    return;
                }

                // Create emitter entry
                let is_open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
                let socket = std::sync::Arc::new(Mutex::new(client_sock));

                let _ = emitter_tx.send(SseEmitterEntry {
                    socket,
                    is_open,
                });

                // Note: the connection stays open, events are sent via __arth_sse_send
                // The thread exits but the socket remains open until __arth_sse_close
            });
        }
        arth_rt::net::arth_rt_socket_close(sock);
    });

    let handle = NEXT_SSE_SERVER_HANDLE.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut m) = sse_server_map().lock() {
        m.insert(handle, SseServerEntry {
            emitter_rx,
            shutdown,
            port: actual_port,
        });
        return handle;
    }
    -1
}

/// Accept an SSE client connection from a server.
/// Returns: emitter handle (>= 176000), -1 on error or no pending connection
#[unsafe(no_mangle)]
pub extern "C" fn __arth_sse_accept(server_handle: i64) -> i64 {
    // Try to receive a pending emitter
    let emitter = {
        if let Ok(m) = sse_server_map().lock() {
            if let Some(server) = m.get(&server_handle) {
                server.emitter_rx.try_recv().ok()
            } else {
                None
            }
        } else {
            None
        }
    };

    if let Some(emitter_entry) = emitter {
        let handle = NEXT_SSE_EMITTER_HANDLE.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut m) = sse_emitter_map().lock() {
            m.insert(handle, emitter_entry);
            return handle;
        }
    }
    -1
}

/// Send an SSE event to a client.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_sse_send(
    emitter_handle: i64,
    event_type: *const u8, event_type_len: i64,
    data: *const u8, data_len: i64,
    id: *const u8, id_len: i64,
) -> i64 {
    let event_type_str = if event_type.is_null() || event_type_len <= 0 {
        String::new()
    } else {
        unsafe {
            let slice = std::slice::from_raw_parts(event_type, event_type_len as usize);
            std::str::from_utf8(slice).unwrap_or("").to_string()
        }
    };

    let data_str = if data.is_null() || data_len <= 0 {
        String::new()
    } else {
        unsafe {
            let slice = std::slice::from_raw_parts(data, data_len as usize);
            std::str::from_utf8(slice).unwrap_or("").to_string()
        }
    };

    let id_str = if id.is_null() || id_len <= 0 {
        String::new()
    } else {
        unsafe {
            let slice = std::slice::from_raw_parts(id, id_len as usize);
            std::str::from_utf8(slice).unwrap_or("").to_string()
        }
    };

    if let Ok(m) = sse_emitter_map().lock() {
        if let Some(emitter) = m.get(&emitter_handle) {
            if emitter.is_open.load(std::sync::atomic::Ordering::SeqCst) {
                if let Ok(sock) = emitter.socket.lock() {
                    // Build SSE message
                    let mut message = String::new();
                    if !id_str.is_empty() {
                        message.push_str(&format!("id: {}\n", id_str));
                    }
                    if !event_type_str.is_empty() {
                        message.push_str(&format!("event: {}\n", event_type_str));
                    }
                    for line in data_str.lines() {
                        message.push_str(&format!("data: {}\n", line));
                    }
                    if data_str.is_empty() {
                        message.push_str("data: \n");
                    }
                    message.push('\n');

                    if ws_write_all(*sock, message.as_bytes()) {
                        return 0;
                    } else {
                        // Mark as closed on write failure
                        emitter.is_open.store(false, std::sync::atomic::Ordering::SeqCst);
                    }
                }
            }
        }
    }
    -1
}

/// Close an SSE emitter (client connection).
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_sse_close(emitter_handle: i64) -> i64 {
    if let Ok(mut m) = sse_emitter_map().lock() {
        if let Some(emitter) = m.remove(&emitter_handle) {
            emitter.is_open.store(false, std::sync::atomic::Ordering::SeqCst);
            if let Ok(sock) = emitter.socket.lock() {
                arth_rt::net::arth_rt_socket_close(*sock);
            }
            return 0;
        }
    }
    -1
}

/// Check if an SSE emitter is open.
/// Returns: 1 if open, 0 if closed, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_sse_is_open(emitter_handle: i64) -> i64 {
    if let Ok(m) = sse_emitter_map().lock() {
        if let Some(emitter) = m.get(&emitter_handle) {
            return if emitter.is_open.load(std::sync::atomic::Ordering::SeqCst) { 1 } else { 0 };
        }
    }
    -1
}

/// Close an SSE server.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_sse_server_close(server_handle: i64) -> i64 {
    if let Ok(mut m) = sse_server_map().lock() {
        if let Some(server) = m.remove(&server_handle) {
            server.shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            return 0;
        }
    }
    -1
}

// ============================================================================
// End of SSE Operations
// ============================================================================

// ============================================================================
// File I/O Operations (HostCallIo)
// ============================================================================

// Note: std::fs, std::io imports are already available from core.inc.rs

// Handle allocator for file handles (starting at 160_000)
static NEXT_FILE_HANDLE: AtomicI64 = AtomicI64::new(160_000);

// Storage for open file handles
struct FileEntry {
    file: File,
    path: String,
}

fn file_handle_map() -> &'static Mutex<HashMap<i64, FileEntry>> {
    static MAP: OnceLock<Mutex<HashMap<i64, FileEntry>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

// Thread-local storage for last read result (for string returns)
thread_local! {
    static LAST_FILE_READ: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static LAST_PATH_RESULT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static LAST_CONSOLE_LINE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Open a file with the specified mode.
/// mode: 0=read, 1=write (create/truncate), 2=append, 3=read+write
/// Returns: file handle (>= 160000), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_file_open(path: *const u8, path_len: i64, mode: i64) -> i64 {
    if path.is_null() || path_len < 0 {
        return -1;
    }
    let path_str = unsafe {
        let slice = std::slice::from_raw_parts(path, path_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s.to_string(),
            Err(_) => return -1,
        }
    };

    let file_result = match mode {
        0 => File::open(&path_str),
        1 => File::create(&path_str),
        2 => OpenOptions::new().append(true).create(true).open(&path_str),
        3 => OpenOptions::new().read(true).write(true).open(&path_str),
        _ => return -1,
    };

    match file_result {
        Ok(file) => {
            let handle = NEXT_FILE_HANDLE.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut m) = file_handle_map().lock() {
                m.insert(handle, FileEntry { file, path: path_str });
                return handle;
            }
            -1
        }
        Err(_) => -1,
    }
}

/// Close an open file handle.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_file_close(handle: i64) -> i64 {
    if let Ok(mut m) = file_handle_map().lock() {
        if m.remove(&handle).is_some() {
            return 0;
        }
    }
    -1
}

/// Read up to num_bytes from a file.
/// Returns: number of bytes read, -1 on error
/// The actual data is stored in thread-local LAST_FILE_READ
#[unsafe(no_mangle)]
pub extern "C" fn __arth_file_read(handle: i64, num_bytes: i64) -> i64 {
    if num_bytes < 0 {
        return -1;
    }

    if let Ok(mut m) = file_handle_map().lock() {
        if let Some(entry) = m.get_mut(&handle) {
            let mut buffer = vec![0u8; num_bytes as usize];
            match entry.file.read(&mut buffer) {
                Ok(n) => {
                    buffer.truncate(n);
                    let data = String::from_utf8_lossy(&buffer).to_string();
                    LAST_FILE_READ.with(|cell| {
                        *cell.borrow_mut() = data;
                    });
                    return n as i64;
                }
                Err(_) => return -1,
            }
        }
    }
    -1
}

/// Get the last read data as a string (call after __arth_file_read).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_file_get_read_data(out_len: *mut i64) -> *const u8 {
    LAST_FILE_READ.with(|cell| {
        let data = cell.borrow();
        if !out_len.is_null() {
            unsafe { *out_len = data.len() as i64; }
        }
        data.as_ptr()
    })
}

/// Write a string to a file.
/// Returns: number of bytes written, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_file_write_str(handle: i64, data: *const u8, data_len: i64) -> i64 {
    if data.is_null() || data_len < 0 {
        return -1;
    }

    let data_str = unsafe {
        let slice = std::slice::from_raw_parts(data, data_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    if let Ok(mut m) = file_handle_map().lock() {
        if let Some(entry) = m.get_mut(&handle) {
            match entry.file.write(data_str.as_bytes()) {
                Ok(n) => return n as i64,
                Err(_) => return -1,
            }
        }
    }
    -1
}

/// Flush a file.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_file_flush(handle: i64) -> i64 {
    if let Ok(mut m) = file_handle_map().lock() {
        if let Some(entry) = m.get_mut(&handle) {
            match entry.file.flush() {
                Ok(_) => return 0,
                Err(_) => return -1,
            }
        }
    }
    -1
}

/// Seek in a file.
/// whence: 0=start, 1=current, 2=end
/// Returns: new position, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_file_seek(handle: i64, offset: i64, whence: i64) -> i64 {
    let seek_from = match whence {
        0 => SeekFrom::Start(offset as u64),
        1 => SeekFrom::Current(offset),
        2 => SeekFrom::End(offset),
        _ => return -1,
    };

    if let Ok(mut m) = file_handle_map().lock() {
        if let Some(entry) = m.get_mut(&handle) {
            match entry.file.seek(seek_from) {
                Ok(pos) => return pos as i64,
                Err(_) => return -1,
            }
        }
    }
    -1
}

/// Get the size of a file by path.
/// Returns: file size in bytes, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_file_size(path: *const u8, path_len: i64) -> i64 {
    if path.is_null() || path_len < 0 {
        return -1;
    }
    let path_str = unsafe {
        let slice = std::slice::from_raw_parts(path, path_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    match fs::metadata(path_str) {
        Ok(meta) => meta.len() as i64,
        Err(_) => -1,
    }
}

/// Check if a file exists.
/// Returns: 1 if exists, 0 if not
#[unsafe(no_mangle)]
pub extern "C" fn __arth_file_exists(path: *const u8, path_len: i64) -> i64 {
    if path.is_null() || path_len < 0 {
        return 0;
    }
    let path_str = unsafe {
        let slice = std::slice::from_raw_parts(path, path_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    if std::path::Path::new(path_str).exists() { 1 } else { 0 }
}

/// Delete a file.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_file_delete(path: *const u8, path_len: i64) -> i64 {
    if path.is_null() || path_len < 0 {
        return -1;
    }
    let path_str = unsafe {
        let slice = std::slice::from_raw_parts(path, path_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    match fs::remove_file(path_str) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

/// Copy a file from src to dst.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_file_copy(
    src: *const u8, src_len: i64,
    dst: *const u8, dst_len: i64
) -> i64 {
    if src.is_null() || dst.is_null() || src_len < 0 || dst_len < 0 {
        return -1;
    }
    let src_str = unsafe {
        let slice = std::slice::from_raw_parts(src, src_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };
    let dst_str = unsafe {
        let slice = std::slice::from_raw_parts(dst, dst_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    match fs::copy(src_str, dst_str) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

/// Move (rename) a file from src to dst.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_file_move(
    src: *const u8, src_len: i64,
    dst: *const u8, dst_len: i64
) -> i64 {
    if src.is_null() || dst.is_null() || src_len < 0 || dst_len < 0 {
        return -1;
    }
    let src_str = unsafe {
        let slice = std::slice::from_raw_parts(src, src_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };
    let dst_str = unsafe {
        let slice = std::slice::from_raw_parts(dst, dst_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    match fs::rename(src_str, dst_str) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

// ============================================================================
// Directory Operations
// ============================================================================

/// Create a directory.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_dir_create(path: *const u8, path_len: i64) -> i64 {
    if path.is_null() || path_len < 0 {
        return -1;
    }
    let path_str = unsafe {
        let slice = std::slice::from_raw_parts(path, path_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    match fs::create_dir(path_str) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

/// Create a directory and all parent directories.
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_dir_create_all(path: *const u8, path_len: i64) -> i64 {
    if path.is_null() || path_len < 0 {
        return -1;
    }
    let path_str = unsafe {
        let slice = std::slice::from_raw_parts(path, path_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    match fs::create_dir_all(path_str) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

/// Delete a directory (must be empty).
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_dir_delete(path: *const u8, path_len: i64) -> i64 {
    if path.is_null() || path_len < 0 {
        return -1;
    }
    let path_str = unsafe {
        let slice = std::slice::from_raw_parts(path, path_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    match fs::remove_dir(path_str) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

/// List entries in a directory.
/// Returns: list handle containing entry names, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_dir_list(path: *const u8, path_len: i64) -> i64 {
    if path.is_null() || path_len < 0 {
        return -1;
    }
    let path_str = unsafe {
        let slice = std::slice::from_raw_parts(path, path_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    match fs::read_dir(path_str) {
        Ok(entries) => {
            let list_handle = list_new();
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    list_push(list_handle, Value::Str(name.to_string()));
                }
            }
            list_handle
        }
        Err(_) => -1,
    }
}

/// Check if a directory exists.
/// Returns: 1 if exists and is directory, 0 otherwise
#[unsafe(no_mangle)]
pub extern "C" fn __arth_dir_exists(path: *const u8, path_len: i64) -> i64 {
    if path.is_null() || path_len < 0 {
        return 0;
    }
    let path_str = unsafe {
        let slice = std::slice::from_raw_parts(path, path_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    let p = std::path::Path::new(path_str);
    if p.exists() && p.is_dir() { 1 } else { 0 }
}

/// Check if a path is a directory.
/// Returns: 1 if is directory, 0 otherwise
#[unsafe(no_mangle)]
pub extern "C" fn __arth_is_dir(path: *const u8, path_len: i64) -> i64 {
    if path.is_null() || path_len < 0 {
        return 0;
    }
    let path_str = unsafe {
        let slice = std::slice::from_raw_parts(path, path_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    if std::path::Path::new(path_str).is_dir() { 1 } else { 0 }
}

/// Check if a path is a file.
/// Returns: 1 if is file, 0 otherwise
#[unsafe(no_mangle)]
pub extern "C" fn __arth_is_file(path: *const u8, path_len: i64) -> i64 {
    if path.is_null() || path_len < 0 {
        return 0;
    }
    let path_str = unsafe {
        let slice = std::slice::from_raw_parts(path, path_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    if std::path::Path::new(path_str).is_file() { 1 } else { 0 }
}

// ============================================================================
// Path Operations
// ============================================================================

/// Get the absolute path of a relative path.
/// Returns: 0 on success (result in LAST_PATH_RESULT), -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_path_absolute(path: *const u8, path_len: i64) -> i64 {
    if path.is_null() || path_len < 0 {
        return -1;
    }
    let path_str = unsafe {
        let slice = std::slice::from_raw_parts(path, path_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    match fs::canonicalize(path_str) {
        Ok(abs_path) => {
            let abs_str = abs_path.to_string_lossy().to_string();
            LAST_PATH_RESULT.with(|cell| {
                *cell.borrow_mut() = abs_str;
            });
            0
        }
        Err(_) => -1,
    }
}

/// Get the last path result (call after __arth_path_absolute).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_path_get_result(out_len: *mut i64) -> *const u8 {
    LAST_PATH_RESULT.with(|cell| {
        let data = cell.borrow();
        if !out_len.is_null() {
            unsafe { *out_len = data.len() as i64; }
        }
        data.as_ptr()
    })
}

// ============================================================================
// Console I/O Operations
// ============================================================================

/// Read a line from stdin.
/// Returns: 0 on success (result in LAST_CONSOLE_LINE), -1 on error/EOF
#[unsafe(no_mangle)]
pub extern "C" fn __arth_console_read_line() -> i64 {
    let stdin = std::io::stdin();
    let mut line = String::new();
    match stdin.lock().read_line(&mut line) {
        Ok(0) => -1, // EOF
        Ok(_) => {
            // Remove trailing newline
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            LAST_CONSOLE_LINE.with(|cell| {
                *cell.borrow_mut() = line;
            });
            0
        }
        Err(_) => -1,
    }
}

/// Get the last console line (call after __arth_console_read_line).
#[unsafe(no_mangle)]
pub extern "C" fn __arth_console_get_line(out_len: *mut i64) -> *const u8 {
    LAST_CONSOLE_LINE.with(|cell| {
        let data = cell.borrow();
        if !out_len.is_null() {
            unsafe { *out_len = data.len() as i64; }
        }
        data.as_ptr()
    })
}

/// Write a string to stdout (without newline).
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_console_write(data: *const u8, data_len: i64) -> i64 {
    if data.is_null() || data_len < 0 {
        return -1;
    }
    let data_str = unsafe {
        let slice = std::slice::from_raw_parts(data, data_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    use std::io::Write;
    match std::io::stdout().write_all(data_str.as_bytes()) {
        Ok(_) => {
            let _ = std::io::stdout().flush();
            0
        }
        Err(_) => -1,
    }
}

/// Write a string to stderr (without newline).
/// Returns: 0 on success, -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn __arth_console_write_err(data: *const u8, data_len: i64) -> i64 {
    if data.is_null() || data_len < 0 {
        return -1;
    }
    let data_str = unsafe {
        let slice = std::slice::from_raw_parts(data, data_len as usize);
        match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    use std::io::Write;
    match std::io::stderr().write_all(data_str.as_bytes()) {
        Ok(_) => {
            let _ = std::io::stderr().flush();
            0
        }
        Err(_) => -1,
    }
}

// ============================================================================
// End of File I/O Operations (HostCallIo)
// ============================================================================

// --- Deterministic runtime tests for Phase 9 primitives ---
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_ping_pong_and_backpressure() {
        // capacity 2
        let ch = __arth_chan_create(2);
        // empty -> would block sentinel
        assert_eq!(__arth_chan_recv(ch), -2);
        // send up to capacity
        assert_eq!(__arth_chan_send(ch, 42), 0);
        assert_eq!(__arth_chan_send(ch, 43), 0);
        // now full -> backpressure code 1
        assert_eq!(__arth_chan_send(ch, 44), 1);
        // FIFO order
        assert_eq!(__arth_chan_recv(ch), 42);
        assert_eq!(__arth_chan_recv(ch), 43);
        // now empty again
        assert_eq!(__arth_chan_recv(ch), -2);
        // close then recv -> closed sentinel
        assert_eq!(__arth_chan_close(ch), 0);
        assert_eq!(__arth_chan_recv(ch), -3);
        // send after close -> closed code 2
        assert_eq!(__arth_chan_send(ch, 99), 2);
    }

    #[test]
    fn recv_await_alias_matches_chan_recv() {
        let ch = __arth_chan_create(1);
        // empty -> would block sentinel
        assert_eq!(__arth_recv_await(ch), -2);
        assert_eq!(__arth_chan_send(ch, 7), 0);
        // value delivered
        assert_eq!(__arth_recv_await(ch), 7);
        // closed and empty -> closed sentinel
        assert_eq!(__arth_chan_close(ch), 0);
        assert_eq!(__arth_recv_await(ch), -3);
    }

    #[test]
    fn actor_orders_messages_fifo() {
        let a = __arth_actor_create(2);
        // send succeeds until full
        assert_eq!(__arth_actor_send(a, 1), 0);
        assert_eq!(__arth_actor_send(a, 2), 0);
        assert_eq!(__arth_actor_send(a, 3), 1); // backpressure (full)
        // FIFO delivery
        assert_eq!(__arth_actor_recv(a), 1);
        assert_eq!(__arth_actor_recv(a), 2);
        // empty
        assert_eq!(__arth_actor_recv(a), -2);
        // close and then recv -> closed sentinel
        assert_eq!(__arth_actor_close(a), 0);
        assert_eq!(__arth_actor_recv(a), -3);
    }

    #[test]
    fn task_cancel_affects_await_and_join() {
        // Spawn a fake task handle
        let h = __arth_task_spawn_fn(0, 0);
        // before cancel, await returns immediate default
        assert_eq!(__arth_await(h), 0);
        // cancel
        assert_eq!(__arth_task_cancel(h), 0);
        // now await and join should surface cancellation (-1 sentinel)
        assert_eq!(__arth_await(h), -1);
        assert_eq!(__arth_task_join(h), -1);
        // current task is deterministic (no ambient task)
        assert_eq!(__arth_task_current(), 0);
    }

    #[test]
    fn task_detach_then_join_ok_when_not_cancelled() {
        let h = __arth_task_spawn_fn(0, 0);
        // detach bookkeeping succeeds
        assert_eq!(__arth_task_detach(h), 0);
        // not cancelled -> join returns default value 0
        assert_eq!(__arth_task_join(h), 0);
    }

    #[test]
    fn cancel_tokens_report_state() {
        let t = __arth_cancel_token_new();
        assert_eq!(__arth_cancel_token_is_cancelled(t), 0);
        assert_eq!(__arth_cancel_token_cancel(t), 0);
        assert_eq!(__arth_cancel_token_is_cancelled(t), 1);
    }

    #[test]
    fn task_check_cancel_reports_runtime_state() {
        let h = __arth_task_spawn_fn(0, 0);
        assert_eq!(__arth_task_is_cancelled(h), 0);
        assert_eq!(__arth_task_cancel(h), 0);
        assert_eq!(__arth_task_is_cancelled(h), 1);
    }

    #[test]
    fn task_yield_returns_success() {
        assert_eq!(__arth_task_yield(), 0);
    }

    #[test]
    fn task_check_cancelled_without_ambient_task_is_false() {
        assert_eq!(__arth_task_check_cancelled(), 0);
    }

    #[test]
    fn timer_sleep_handles_zero_and_negative_values() {
        let start = std::time::Instant::now();
        __arth_timer_sleep(0);
        __arth_timer_sleep(-1);
        assert!(
            start.elapsed() < std::time::Duration::from_millis(100),
            "zero/negative sleep should return quickly"
        );
    }

    #[test]
    fn task_panic_propagates_to_await() {
        // Clear any prior panic state
        __arth_clear_panic();

        // Spawn a task
        let h = __arth_task_spawn_fn(0, 0);

        // Mark it as panicked
        let msg = std::ffi::CString::new("test panic").unwrap();
        assert_eq!(__arth_task_set_panicked(h, msg.as_ptr()), 0);

        // await should return -2 (panicked sentinel) and set panic state
        assert_eq!(__arth_await(h), -2);

        // Verify panic state was propagated
        assert_eq!(__arth_is_panicking(), 1);

        // Clean up panic state
        __arth_clear_panic();
    }

    #[test]
    fn task_panic_propagates_to_join() {
        // Clear any prior panic state
        __arth_clear_panic();

        // Spawn a task
        let h = __arth_task_spawn_fn(0, 0);

        // Mark it as panicked
        let msg = std::ffi::CString::new("join test panic").unwrap();
        assert_eq!(__arth_task_set_panicked(h, msg.as_ptr()), 0);

        // join should return -2 (panicked sentinel) and set panic state
        assert_eq!(__arth_task_join(h), -2);

        // Verify panic state was propagated
        assert_eq!(__arth_is_panicking(), 1);

        // Clean up panic state
        __arth_clear_panic();
    }

    #[test]
    fn task_panic_state_stored_correctly() {
        // Spawn a task
        let h = __arth_task_spawn_fn(0, 0);

        // Initially not panicked
        assert_eq!(__arth_task_is_completed(h), 0);

        // Mark as panicked
        let msg = std::ffi::CString::new("state test").unwrap();
        assert_eq!(__arth_task_set_panicked(h, msg.as_ptr()), 0);

        // Now should be "completed" (panicked counts as finished)
        assert_eq!(__arth_task_is_completed(h), 1);
    }

    // --- WebSocket FFI tests ---

    #[test]
    fn websocket_server_create_and_close() {
        // Create a WebSocket server on a random high port
        let server = __arth_ws_serve(0, "/ws".as_ptr(), 4);
        // Server handle should be valid (>= 170000)
        assert!(server >= 170_000, "Server handle should be >= 170000, got {}", server);

        // Accept should return -1 since no client has connected
        let conn = __arth_ws_accept(server);
        assert_eq!(conn, -1);

        // Close the server
        let close_result = __arth_ws_server_close(server);
        assert_eq!(close_result, 0);

        // Closing again should fail
        let close_again = __arth_ws_server_close(server);
        assert_eq!(close_again, -1);
    }

    #[test]
    fn websocket_invalid_connection_operations() {
        // Operations on invalid connection handle should fail
        let invalid_handle = 999_999;

        // Send text on invalid handle
        let result = __arth_ws_send_text(invalid_handle, "test".as_ptr(), 4);
        assert_eq!(result, -1);

        // Receive on invalid handle
        let result = __arth_ws_recv(invalid_handle);
        assert_eq!(result, -1);

        // Check is_open on invalid handle
        let result = __arth_ws_is_open(invalid_handle);
        assert_eq!(result, -1);

        // Close invalid handle
        let result = __arth_ws_close(invalid_handle, 1000, "".as_ptr(), 0);
        assert_eq!(result, -1);
    }

    #[test]
    fn websocket_accept_invalid_server() {
        // Accept on non-existent server
        let result = __arth_ws_accept(999_999);
        assert_eq!(result, -1);
    }

    // --- SSE FFI tests ---

    #[test]
    fn sse_server_create_and_close() {
        // Create an SSE server on a random high port
        let server = __arth_sse_serve(0, "/events".as_ptr(), 7);
        // Server handle should be valid (>= 175000)
        assert!(server >= 175_000, "Server handle should be >= 175000, got {}", server);

        // Accept should return -1 since no client has connected
        let emitter = __arth_sse_accept(server);
        assert_eq!(emitter, -1);

        // Close the server
        let close_result = __arth_sse_server_close(server);
        assert_eq!(close_result, 0);

        // Closing again should fail
        let close_again = __arth_sse_server_close(server);
        assert_eq!(close_again, -1);
    }

    #[test]
    fn sse_invalid_emitter_operations() {
        // Operations on invalid emitter handle should fail
        let invalid_handle = 999_999;

        // Send on invalid handle
        let result = __arth_sse_send(
            invalid_handle,
            "event".as_ptr(), 5,
            "data".as_ptr(), 4,
            "1".as_ptr(), 1,
        );
        assert_eq!(result, -1);

        // Check is_open on invalid handle
        let result = __arth_sse_is_open(invalid_handle);
        assert_eq!(result, -1);

        // Close invalid handle
        let result = __arth_sse_close(invalid_handle);
        assert_eq!(result, -1);
    }

    #[test]
    fn sse_accept_invalid_server() {
        // Accept on non-existent server
        let result = __arth_sse_accept(999_999);
        assert_eq!(result, -1);
    }

    // --- HTTP Fetch FFI tests ---

    #[test]
    fn http_fetch_null_url_returns_error() {
        // Null URL should return -1
        let result = __arth_http_fetch_url(std::ptr::null(), 0, 1000);
        assert_eq!(result, -1);
    }

    #[test]
    fn http_fetch_empty_url_returns_error() {
        // Empty URL should return -1
        let result = __arth_http_fetch_url("".as_ptr(), 0, 1000);
        assert_eq!(result, -1);
    }

    #[test]
    fn http_fetch_invalid_url_returns_error() {
        // Invalid URL (no scheme) should return -1
        let url = "not-a-valid-url";
        let result = __arth_http_fetch_url(url.as_ptr(), url.len() as i64, 1000);
        assert_eq!(result, -1);
    }

    #[test]
    fn http_fetch_https_url_returns_error() {
        // HTTPS is not supported yet, should return -1
        let url = "https://example.com/test";
        let result = __arth_http_fetch_url(url.as_ptr(), url.len() as i64, 1000);
        assert_eq!(result, -1);
    }

    #[test]
    fn http_fetch_full_function_null_url_returns_error() {
        // Full __arth_http_fetch with null URL should return -1
        let result = __arth_http_fetch(
            std::ptr::null(), 0, // url
            std::ptr::null(), 0, // method
            0,                   // headers
            std::ptr::null(), 0, // body
            1000,                // timeout
        );
        assert_eq!(result, -1);
    }

    #[test]
    fn http_response_invalid_handle() {
        // Operations on invalid response handle should return appropriate errors
        let invalid_handle = 999_999;

        // Status on invalid handle
        let result = __arth_http_response_status(invalid_handle);
        assert_eq!(result, -1);

        // Body on invalid handle
        let result = __arth_http_response_body(invalid_handle);
        assert_eq!(result, -1);

        // Close invalid handle
        let result = __arth_http_response_close(invalid_handle);
        assert_eq!(result, -1);
    }

    // --- CancelledError tests ---

    #[test]
    fn cancelled_error_creates_valid_struct() {
        // Create a CancelledError with task handle 42
        let exc = __arth_create_cancelled_error(42);

        // Verify the struct was created (handle should be >= 90000 for structs)
        assert!(exc >= 90_000, "CancelledError handle should be >= 90000, got {}", exc);

        // Verify it's recognized as a CancelledError
        assert_eq!(__arth_is_cancelled_error(exc), 1);

        // Verify the fields are set correctly
        // Field 0 should be taskHandle (42)
        let task_handle = struct_get(exc, 0);
        match task_handle {
            Value::I64(h) => assert_eq!(h, 42),
            _ => panic!("taskHandle should be I64"),
        }

        // Field 1 should be message ("Task was cancelled")
        let message = struct_get(exc, 1);
        match message {
            Value::Str(s) => assert_eq!(s, "Task was cancelled"),
            _ => panic!("message should be Str"),
        }
    }

    #[test]
    fn cancelled_error_with_custom_message() {
        // Create a CancelledError with custom message
        let msg = std::ffi::CString::new("Custom cancellation reason").unwrap();
        let exc = __arth_create_cancelled_error_with_message(123, msg.as_ptr());

        // Verify the struct was created
        assert!(exc >= 90_000, "CancelledError handle should be >= 90000");

        // Verify it's recognized as a CancelledError
        assert_eq!(__arth_is_cancelled_error(exc), 1);

        // Verify taskHandle field
        let task_handle = struct_get(exc, 0);
        match task_handle {
            Value::I64(h) => assert_eq!(h, 123),
            _ => panic!("taskHandle should be I64"),
        }

        // Verify custom message field
        let message = struct_get(exc, 1);
        match message {
            Value::Str(s) => assert_eq!(s, "Custom cancellation reason"),
            _ => panic!("message should be Str"),
        }
    }

    #[test]
    fn is_cancelled_error_returns_false_for_non_cancelled_error() {
        // Create a regular struct that's not a CancelledError
        let regular_struct = struct_new("SomeOtherType".to_string(), 1);
        struct_set(regular_struct, 0, Value::I64(1), "field".to_string());

        // Should not be recognized as CancelledError
        assert_eq!(__arth_is_cancelled_error(regular_struct), 0);
    }

    #[test]
    fn is_cancelled_error_returns_false_for_invalid_handle() {
        // Invalid handle should return 0
        assert_eq!(__arth_is_cancelled_error(-1), 0);
        assert_eq!(__arth_is_cancelled_error(0), 0);
        assert_eq!(__arth_is_cancelled_error(999_999), 0);
    }

    #[test]
    fn cancelled_error_type_name_is_correct() {
        let exc = __arth_create_cancelled_error(0);

        // Get the struct and verify type name
        if let Ok(m) = struct_store().lock() {
            if let Some(s) = m.get(&exc) {
                assert_eq!(s.type_name, "concurrent.CancelledError");
            } else {
                panic!("CancelledError struct not found in store");
            }
        } else {
            panic!("Could not lock struct store");
        }
    }
}
