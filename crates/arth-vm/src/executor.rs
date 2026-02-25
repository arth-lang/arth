//! Multi-threaded executor for Arth async runtime.
//!
//! This module provides a work-stealing thread pool executor that supports both
//! single-threaded deterministic mode (for tests) and multi-threaded mode for
//! true parallelism with Sendable tasks.
//!
//! # Executor Architecture
//!
//! The executor follows the recommended design from Phase 5:
//! - **Internal by default**: The default executor is fully internal to the VM
//! - **Pluggable via trait**: The `ArthExecutor` trait allows embedders to provide
//!   alternative runtimes (e.g., Tokio adapter) without leaking runtime semantics
//!
//! The VM owns:
//! - Task state machine
//! - Cancellation flags
//! - Await/yield semantics
//!
//! The executor owns:
//! - Scheduling
//! - Parking/unparking
//! - Thread usage

#![allow(clippy::collapsible_if)]
#![allow(clippy::derivable_impls)]
#![allow(clippy::type_complexity)]
#![allow(clippy::manual_map)]

use crossbeam_deque::{Injector, Steal, Stealer, Worker};
use parking_lot::{Condvar, Mutex, RwLock};
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

// Thread-local storage for the current task handle (for async stack stitching).
// This allows spawn_with_parent to find the current task without requiring
// the OS thread ID to map to executor synthetic thread IDs.
thread_local! {
    static CURRENT_TASK_HANDLE: Cell<i64> = const { Cell::new(0) };
}

// =============================================================================
// Pluggable Executor Trait (Phase 5 Recommendation)
// =============================================================================

/// Result of polling a task
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollState {
    /// Task completed successfully with result
    Ready(i64),
    /// Task is pending (needs to be polled again)
    Pending,
    /// Task was cancelled
    Cancelled,
    /// Task panicked
    Panicked,
}

/// Task handle for the executor
pub type TaskHandle = i64;

/// Task ID for tracking
pub type TaskId = i64;

/// Pluggable executor trait for Arth async runtime.
///
/// This trait allows embedders to provide alternative executor implementations
/// (e.g., a Tokio adapter) while keeping Arth semantics VM-defined.
///
/// The VM owns task state machines, cancellation flags, and await semantics.
/// The executor owns scheduling, parking/unparking, and thread usage.
pub trait ArthExecutor: Send + Sync {
    /// Spawn a new task and return its handle
    fn spawn(&self, fn_id: i64, is_sendable: bool) -> TaskHandle;

    /// Block the current thread until the task completes
    fn block_on(&self, task: TaskHandle) -> PollState;

    /// Yield execution to allow other tasks to run
    fn yield_now(&self);

    /// Poll a specific task
    fn poll_task(&self, task_id: TaskId) -> PollState;

    /// Cancel a task
    fn cancel(&self, task: TaskHandle);

    /// Check if executor has runnable tasks
    fn has_runnable_tasks(&self) -> bool;

    /// Get the current task handle (if any)
    fn current_task(&self) -> Option<TaskHandle>;
}

// =============================================================================
// Executor Mode Configuration
// =============================================================================

/// Executor operating mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutorMode {
    /// Single-threaded deterministic mode (FIFO scheduling, reproducible)
    /// Default for tests and TS guest environments
    SingleThreaded,
    /// Multi-threaded mode with work-stealing for parallel execution
    /// Only Sendable tasks can be executed across threads
    MultiThreaded {
        /// Number of worker threads (0 = use num_cpus)
        num_threads: usize,
    },
}

impl Default for ExecutorMode {
    fn default() -> Self {
        ExecutorMode::SingleThreaded
    }
}

/// Global executor configuration
#[derive(Debug)]
pub struct ExecutorConfig {
    /// Operating mode
    pub mode: ExecutorMode,
    /// Whether to enable fairness guarantees (bounded starvation)
    pub fair_scheduling: bool,
    /// Maximum tasks per worker before yielding (for fairness)
    pub max_tasks_per_yield: usize,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            mode: ExecutorMode::SingleThreaded,
            fair_scheduling: true,
            max_tasks_per_yield: 64,
        }
    }
}

// =============================================================================
// Task Markers and Metadata
// =============================================================================

/// Task safety markers for cross-thread execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskMarkers {
    /// True if the task captures only Sendable values
    pub is_sendable: bool,
    /// True if the task captures only Shareable values
    pub is_shareable: bool,
    /// Thread ID that spawned this task (for affinity)
    pub origin_thread: u64,
}

impl Default for TaskMarkers {
    fn default() -> Self {
        Self {
            is_sendable: true, // Default to sendable (conservative)
            is_shareable: true,
            origin_thread: 0,
        }
    }
}

/// Extended task state for multi-threaded executor
#[derive(Debug, Clone)]
pub enum MTTaskState {
    /// Task is pending execution
    Pending,
    /// Task is currently running on a specific thread
    Running { thread_id: u64 },
    /// Task completed successfully with a result
    Completed(i64),
    /// Task was cancelled
    Cancelled,
    /// Task panicked with a message
    Panicked(String),
    /// Task is blocked waiting on something
    Blocked(MTBlockReason),
}

/// Reason for task being blocked (multi-threaded version)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MTBlockReason {
    /// Waiting for another task to complete
    AwaitingTask(i64),
    /// Waiting for data on a channel (recv)
    AwaitingChannelRecv(i64),
    /// Waiting to send on a full channel (backpressure)
    AwaitingChannelSend(i64),
    /// Waiting for a timer/sleep
    AwaitingTimer { deadline: u64 }, // epoch millis
    /// Waiting for a network operation
    AwaitingNetOp(i64),
}

/// A frame in the async stack trace.
/// Represents one task in the async call chain from child to parent.
#[derive(Debug, Clone)]
pub struct AsyncStackFrame {
    /// Task handle
    pub task_handle: i64,
    /// Function ID (hash of the async function name)
    pub fn_id: i64,
    /// Name of the function that spawned this task (if available)
    pub spawn_function: Option<String>,
    /// Current state of the task
    pub state: MTTaskState,
}

impl std::fmt::Display for AsyncStackFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let fn_name = self.spawn_function.as_deref().unwrap_or("<async>");
        let state_str = match &self.state {
            MTTaskState::Pending => "pending",
            MTTaskState::Running { .. } => "running",
            MTTaskState::Completed(_) => "completed",
            MTTaskState::Cancelled => "cancelled",
            MTTaskState::Panicked(_) => "panicked",
            MTTaskState::Blocked(_) => "blocked",
        };
        write!(f, "{} (task {}, {})", fn_name, self.task_handle, state_str)
    }
}

/// Multi-threaded task info
#[derive(Debug)]
pub struct MTTaskInfo {
    /// Unique task handle
    pub handle: i64,
    /// Function ID for the async body
    pub fn_id: i64,
    /// Arguments for the task
    pub args: Vec<i64>,
    /// Current state
    pub state: MTTaskState,
    /// Safety markers
    pub markers: TaskMarkers,
    /// Whether this task has been detached
    pub detached: bool,
    /// Priority (lower = higher priority, for fair scheduling)
    pub priority: u64,
    /// Parent task handle (for async stack stitching)
    /// None if this is a root task (not spawned from another async context)
    pub parent_task: Option<i64>,
    /// Function name that spawned this task (for debugging)
    pub spawn_function: Option<String>,
}

impl MTTaskInfo {
    pub fn new(handle: i64, fn_id: i64, markers: TaskMarkers) -> Self {
        Self {
            handle,
            fn_id,
            args: Vec::new(),
            state: MTTaskState::Pending,
            markers,
            detached: false,
            priority: 0,
            parent_task: None,
            spawn_function: None,
        }
    }

    /// Create a new task info with parent tracking for async stack stitching
    pub fn with_parent(
        handle: i64,
        fn_id: i64,
        markers: TaskMarkers,
        parent_task: Option<i64>,
        spawn_function: Option<String>,
    ) -> Self {
        Self {
            handle,
            fn_id,
            args: Vec::new(),
            state: MTTaskState::Pending,
            markers,
            detached: false,
            priority: 0,
            parent_task,
            spawn_function,
        }
    }

    pub fn is_completed(&self) -> bool {
        matches!(
            self.state,
            MTTaskState::Completed(_) | MTTaskState::Cancelled | MTTaskState::Panicked(_)
        )
    }
}

// =============================================================================
// Work-Stealing Thread Pool
// =============================================================================

/// A task that can be scheduled for execution
#[derive(Debug, Clone)]
struct ScheduledTask {
    handle: i64,
    /// True if this task must run on its origin thread (non-Sendable)
    thread_affine: bool,
    /// If thread_affine, which thread ID
    affine_thread: u64,
}

/// Stealer handle that can be shared between threads
struct StealerHandle {
    stealer: Stealer<ScheduledTask>,
    thread_id: u64,
}

// Safety: Stealer is Send+Sync when T is Send
unsafe impl Send for StealerHandle {}
unsafe impl Sync for StealerHandle {}

/// Thread-local queues for non-sendable tasks (keyed by worker index)
struct ThreadLocalQueues {
    queues: Vec<Mutex<VecDeque<i64>>>,
}

impl ThreadLocalQueues {
    fn new(num_workers: usize) -> Self {
        Self {
            queues: (0..num_workers)
                .map(|_| Mutex::new(VecDeque::new()))
                .collect(),
        }
    }

    fn push(&self, worker_idx: usize, handle: i64) {
        if worker_idx < self.queues.len() {
            self.queues[worker_idx].lock().push_back(handle);
        }
    }

    fn pop(&self, worker_idx: usize) -> Option<i64> {
        if worker_idx < self.queues.len() {
            self.queues[worker_idx].lock().pop_front()
        } else {
            None
        }
    }
}

/// The multi-threaded executor
pub struct ThreadedExecutor {
    /// Configuration
    config: ExecutorConfig,
    /// Global injector queue (for tasks from outside workers)
    injector: Injector<ScheduledTask>,
    /// Stealers from each worker (for work stealing)
    stealers: RwLock<Vec<StealerHandle>>,
    /// Thread-local queues for non-sendable tasks
    thread_local_queues: ThreadLocalQueues,
    /// Number of workers
    num_workers: usize,
    /// Task storage (handle -> info)
    tasks: RwLock<HashMap<i64, MTTaskInfo>>,
    /// Tasks waiting on other tasks (waited_task -> Vec<waiter_handles>)
    task_waiters: Mutex<HashMap<i64, Vec<i64>>>,
    /// Tasks waiting on channel recv
    channel_recv_waiters: Mutex<HashMap<i64, Vec<i64>>>,
    /// Tasks waiting on channel send
    channel_send_waiters: Mutex<HashMap<i64, Vec<i64>>>,
    /// Tasks waiting on network operations
    net_op_waiters: Mutex<HashMap<i64, Vec<i64>>>,
    /// Timer deadlines (task_handle -> deadline instant)
    timer_deadlines: Mutex<HashMap<i64, Instant>>,
    /// Next task handle
    next_handle: AtomicI64,
    /// Currently running task per thread (thread_id -> task_handle)
    current_tasks: RwLock<HashMap<u64, i64>>,
    /// Shutdown flag
    shutdown: AtomicBool,
    /// Number of active worker threads
    active_workers: AtomicUsize,
    /// Condvar for worker thread parking
    park_condvar: Condvar,
    /// Mutex for condvar
    park_mutex: Mutex<()>,
    /// Task completion callback
    task_callback: Mutex<Option<Box<dyn Fn(i64) -> i64 + Send + Sync>>>,
    /// Total completed tasks (for stats)
    completed_count: AtomicU64,
}

impl ThreadedExecutor {
    /// Create a new threaded executor with the given configuration
    pub fn new(config: ExecutorConfig) -> Arc<Self> {
        let num_workers = match config.mode {
            ExecutorMode::SingleThreaded => 1,
            ExecutorMode::MultiThreaded { num_threads } => {
                if num_threads == 0 {
                    std::thread::available_parallelism()
                        .map(|n| n.get())
                        .unwrap_or(1)
                } else {
                    num_threads
                }
            }
        };

        Arc::new(Self {
            config,
            injector: Injector::new(),
            stealers: RwLock::new(Vec::new()),
            thread_local_queues: ThreadLocalQueues::new(num_workers),
            num_workers,
            tasks: RwLock::new(HashMap::new()),
            task_waiters: Mutex::new(HashMap::new()),
            channel_recv_waiters: Mutex::new(HashMap::new()),
            channel_send_waiters: Mutex::new(HashMap::new()),
            net_op_waiters: Mutex::new(HashMap::new()),
            timer_deadlines: Mutex::new(HashMap::new()),
            next_handle: AtomicI64::new(1),
            current_tasks: RwLock::new(HashMap::new()),
            shutdown: AtomicBool::new(false),
            active_workers: AtomicUsize::new(0),
            park_condvar: Condvar::new(),
            park_mutex: Mutex::new(()),
            task_callback: Mutex::new(None),
            completed_count: AtomicU64::new(0),
        })
    }

    /// Register the task execution callback
    pub fn register_callback<F>(&self, callback: F)
    where
        F: Fn(i64) -> i64 + Send + Sync + 'static,
    {
        let mut cb = self.task_callback.lock();
        *cb = Some(Box::new(callback));
    }

    /// Spawn a new task
    pub fn spawn(&self, fn_id: i64, markers: TaskMarkers) -> i64 {
        self.spawn_with_parent(fn_id, markers, None)
    }

    /// Spawn a new task with explicit parent tracking
    pub fn spawn_with_parent(
        &self,
        fn_id: i64,
        markers: TaskMarkers,
        spawn_function: Option<String>,
    ) -> i64 {
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);

        // Look up the current running task to set as parent (for async stack stitching)
        // Uses thread-local storage set by set_current_task when task execution begins
        let parent_task = CURRENT_TASK_HANDLE.with(|h| {
            let current = h.get();
            if current > 0 { Some(current) } else { None }
        });

        let task_info =
            MTTaskInfo::with_parent(handle, fn_id, markers, parent_task, spawn_function);

        {
            let mut tasks = self.tasks.write();
            tasks.insert(handle, task_info);
        }

        handle
    }

    /// Push an argument to a pending task
    pub fn push_arg(&self, handle: i64, arg: i64) -> bool {
        let mut tasks = self.tasks.write();
        if let Some(task) = tasks.get_mut(&handle) {
            if matches!(task.state, MTTaskState::Pending) {
                task.args.push(arg);
                return true;
            }
        }
        false
    }

    /// Start a task (enqueue it for execution)
    pub fn start(&self, handle: i64) -> bool {
        let tasks = self.tasks.read();
        let task = match tasks.get(&handle) {
            Some(t) if matches!(t.state, MTTaskState::Pending) => t,
            _ => return false,
        };

        let scheduled = ScheduledTask {
            handle,
            thread_affine: !task.markers.is_sendable,
            affine_thread: task.markers.origin_thread,
        };
        drop(tasks);

        if scheduled.thread_affine && scheduled.affine_thread > 0 {
            // Enqueue to specific worker's thread-local queue
            let worker_idx = (scheduled.affine_thread - 1) as usize % self.num_workers;
            self.thread_local_queues.push(worker_idx, handle);
        } else {
            // Enqueue to global injector
            self.injector.push(scheduled);
        }

        // Wake up a worker
        self.park_condvar.notify_one();
        true
    }

    /// Get a task's function ID
    pub fn get_fn_id(&self, handle: i64) -> Option<i64> {
        self.tasks.read().get(&handle).map(|t| t.fn_id)
    }

    /// Get a task's arguments
    pub fn get_args(&self, handle: i64) -> Option<Vec<i64>> {
        self.tasks.read().get(&handle).map(|t| t.args.clone())
    }

    /// Get a task's parent task handle
    pub fn get_parent_task(&self, handle: i64) -> Option<i64> {
        self.tasks.read().get(&handle).and_then(|t| t.parent_task)
    }

    /// Build the async stack trace by walking up the parent chain.
    /// Returns a list of (task_handle, fn_id, spawn_function) tuples,
    /// from the given task up to the root task.
    pub fn build_async_stack(&self, start_handle: i64) -> Vec<AsyncStackFrame> {
        let mut frames = Vec::new();
        let tasks = self.tasks.read();
        let mut current = Some(start_handle);

        // Walk up the parent chain, collecting frames
        while let Some(handle) = current {
            if let Some(task) = tasks.get(&handle) {
                frames.push(AsyncStackFrame {
                    task_handle: handle,
                    fn_id: task.fn_id,
                    spawn_function: task.spawn_function.clone(),
                    state: task.state.clone(),
                });
                current = task.parent_task;
            } else {
                break;
            }

            // Safety: prevent infinite loops (shouldn't happen, but be safe)
            if frames.len() > 1000 {
                break;
            }
        }

        frames
    }

    /// Get the current task for a thread
    pub fn current_task(&self, thread_id: u64) -> i64 {
        self.current_tasks
            .read()
            .get(&thread_id)
            .copied()
            .unwrap_or(0)
    }

    /// Set the current task for a thread
    fn set_current_task(&self, thread_id: u64, handle: Option<i64>) {
        // Update the HashMap for the executor's internal tracking
        let mut current = self.current_tasks.write();
        match handle {
            Some(h) => {
                current.insert(thread_id, h);
            }
            None => {
                current.remove(&thread_id);
            }
        }
        drop(current);

        // Also update thread-local storage for async stack stitching
        // This allows spawn_with_parent to find the current task without
        // needing to map OS thread IDs to executor synthetic thread IDs
        CURRENT_TASK_HANDLE.with(|h| {
            h.set(handle.unwrap_or(0));
        });
    }

    /// Block a task
    pub fn block_task(&self, handle: i64, reason: MTBlockReason) {
        // Update task state
        {
            let mut tasks = self.tasks.write();
            if let Some(task) = tasks.get_mut(&handle) {
                task.state = MTTaskState::Blocked(reason.clone());
            }
        }

        // Register in appropriate waiter list
        match &reason {
            MTBlockReason::AwaitingTask(target) => {
                self.task_waiters
                    .lock()
                    .entry(*target)
                    .or_default()
                    .push(handle);
            }
            MTBlockReason::AwaitingChannelRecv(chan) => {
                self.channel_recv_waiters
                    .lock()
                    .entry(*chan)
                    .or_default()
                    .push(handle);
            }
            MTBlockReason::AwaitingChannelSend(chan) => {
                self.channel_send_waiters
                    .lock()
                    .entry(*chan)
                    .or_default()
                    .push(handle);
            }
            MTBlockReason::AwaitingTimer { deadline } => {
                let instant = Instant::now() + Duration::from_millis(*deadline);
                self.timer_deadlines.lock().insert(handle, instant);
            }
            MTBlockReason::AwaitingNetOp(op) => {
                self.net_op_waiters
                    .lock()
                    .entry(*op)
                    .or_default()
                    .push(handle);
            }
        }
    }

    /// Wake a blocked task
    pub fn wake_task(&self, handle: i64) {
        // Update state to pending
        let markers = {
            let mut tasks = self.tasks.write();
            if let Some(task) = tasks.get_mut(&handle) {
                if matches!(task.state, MTTaskState::Blocked(_)) {
                    task.state = MTTaskState::Pending;
                    Some(task.markers)
                } else {
                    None
                }
            } else {
                None
            }
        };

        // Re-enqueue
        if let Some(markers) = markers {
            let scheduled = ScheduledTask {
                handle,
                thread_affine: !markers.is_sendable,
                affine_thread: markers.origin_thread,
            };

            if scheduled.thread_affine && scheduled.affine_thread > 0 {
                let worker_idx = (scheduled.affine_thread - 1) as usize % self.num_workers;
                self.thread_local_queues.push(worker_idx, handle);
            } else {
                self.injector.push(scheduled);
            }

            self.park_condvar.notify_one();
        }
    }

    /// Complete a task
    pub fn complete_task(&self, handle: i64, result: i64) {
        {
            let mut tasks = self.tasks.write();
            if let Some(task) = tasks.get_mut(&handle) {
                task.state = MTTaskState::Completed(result);
            }
        }

        // Wake waiters
        let waiters = self.task_waiters.lock().remove(&handle);
        if let Some(waiters) = waiters {
            for waiter in waiters {
                self.wake_task(waiter);
            }
        }

        self.completed_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Cancel a task
    pub fn cancel_task(&self, handle: i64) -> bool {
        {
            let mut tasks = self.tasks.write();
            if let Some(task) = tasks.get_mut(&handle) {
                if task.is_completed() {
                    return false;
                }
                task.state = MTTaskState::Cancelled;
            } else {
                return false;
            }
        }

        // Wake waiters
        let waiters = self.task_waiters.lock().remove(&handle);
        if let Some(waiters) = waiters {
            for waiter in waiters {
                self.wake_task(waiter);
            }
        }

        true
    }

    /// Mark task as panicked
    pub fn panic_task(&self, handle: i64, message: String) {
        {
            let mut tasks = self.tasks.write();
            if let Some(task) = tasks.get_mut(&handle) {
                task.state = MTTaskState::Panicked(message);
            }
        }

        // Wake waiters
        let waiters = self.task_waiters.lock().remove(&handle);
        if let Some(waiters) = waiters {
            for waiter in waiters {
                self.wake_task(waiter);
            }
        }
    }

    /// Get task state
    pub fn get_state(&self, handle: i64) -> Option<MTTaskState> {
        self.tasks.read().get(&handle).map(|t| t.state.clone())
    }

    /// Wake one receiver waiting on a channel
    pub fn wake_channel_recv_waiter(&self, channel: i64) -> bool {
        let waiter = self
            .channel_recv_waiters
            .lock()
            .get_mut(&channel)
            .and_then(|v| v.pop());
        if let Some(handle) = waiter {
            self.wake_task(handle);
            true
        } else {
            false
        }
    }

    /// Wake one sender waiting on a channel
    pub fn wake_channel_send_waiter(&self, channel: i64) -> bool {
        let waiter = self
            .channel_send_waiters
            .lock()
            .get_mut(&channel)
            .and_then(|v| v.pop());
        if let Some(handle) = waiter {
            self.wake_task(handle);
            true
        } else {
            false
        }
    }

    /// Wake all waiters on a channel (for close)
    pub fn wake_all_channel_waiters(&self, channel: i64) {
        let recv_waiters = self.channel_recv_waiters.lock().remove(&channel);
        let send_waiters = self.channel_send_waiters.lock().remove(&channel);

        for waiter in recv_waiters.into_iter().flatten() {
            self.wake_task(waiter);
        }
        for waiter in send_waiters.into_iter().flatten() {
            self.wake_task(waiter);
        }
    }

    /// Wake all waiters on a network operation
    pub fn wake_net_op_waiters(&self, net_op: i64) {
        let waiters = self.net_op_waiters.lock().remove(&net_op);
        if let Some(waiters) = waiters {
            for waiter in waiters {
                self.wake_task(waiter);
            }
        }
    }

    /// Poll timers and wake completed ones
    pub fn poll_timers(&self) -> usize {
        let now = Instant::now();
        let mut completed = Vec::new();

        {
            let mut timers = self.timer_deadlines.lock();
            timers.retain(|handle, deadline| {
                if now >= *deadline {
                    completed.push(*handle);
                    false
                } else {
                    true
                }
            });
        }

        for handle in &completed {
            self.wake_task(*handle);
        }

        completed.len()
    }

    /// Run the executor until quiescent (single-threaded mode)
    pub fn run_until_quiescent(&self) -> u64 {
        let thread_id = 1u64;
        let mut completed = 0u64;

        loop {
            // Poll timers
            self.poll_timers();

            // Try to get a task
            let task = self.try_steal_task(0);

            let handle = match task {
                Some(t) => t.handle,
                None => {
                    // Check if there's still pending work
                    let has_pending = {
                        let tasks = self.tasks.read();
                        tasks.values().any(|t| {
                            matches!(t.state, MTTaskState::Pending | MTTaskState::Blocked(_))
                        })
                    };

                    let has_timers = !self.timer_deadlines.lock().is_empty();

                    if has_pending || has_timers {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    break;
                }
            };

            // Execute the task
            if let Some(result) = self.execute_task(handle, thread_id) {
                completed += 1;
                // Check if task completed (wasn't blocked)
                let tasks = self.tasks.read();
                if let Some(task) = tasks.get(&handle) {
                    if !task.is_completed() && !matches!(task.state, MTTaskState::Blocked(_)) {
                        drop(tasks);
                        self.complete_task(handle, result);
                    }
                }
            }
        }

        completed
    }

    /// Try to steal a task (for work-stealing)
    fn try_steal_task(&self, worker_idx: usize) -> Option<ScheduledTask> {
        // First check thread-local queue
        if let Some(handle) = self.thread_local_queues.pop(worker_idx) {
            return Some(ScheduledTask {
                handle,
                thread_affine: true,
                affine_thread: (worker_idx + 1) as u64,
            });
        }

        // Try global injector
        loop {
            match self.injector.steal() {
                Steal::Success(task) => return Some(task),
                Steal::Empty => break,
                Steal::Retry => continue,
            }
        }

        // Try stealing from other workers' stealers
        let stealers = self.stealers.read();
        for stealer_handle in stealers.iter() {
            if stealer_handle.thread_id == (worker_idx + 1) as u64 {
                continue; // Don't steal from self
            }
            loop {
                match stealer_handle.stealer.steal() {
                    Steal::Success(task) => {
                        // Don't steal thread-affine tasks
                        if task.thread_affine {
                            // Put it back
                            self.injector.push(task);
                            continue;
                        }
                        return Some(task);
                    }
                    Steal::Empty => break,
                    Steal::Retry => continue,
                }
            }
        }

        None
    }

    /// Execute a single task
    fn execute_task(&self, handle: i64, thread_id: u64) -> Option<i64> {
        // Mark as running
        {
            let mut tasks = self.tasks.write();
            if let Some(task) = tasks.get_mut(&handle) {
                if !matches!(task.state, MTTaskState::Pending) {
                    return None;
                }
                task.state = MTTaskState::Running { thread_id };
            } else {
                return None;
            }
        }

        self.set_current_task(thread_id, Some(handle));

        // Execute via callback
        let result = {
            let cb = self.task_callback.lock();
            if let Some(ref callback) = *cb {
                Some(callback(handle))
            } else {
                None
            }
        };

        self.set_current_task(thread_id, None);
        result
    }

    /// Start worker threads (for multi-threaded mode)
    pub fn start_workers(self: &Arc<Self>) {
        if matches!(self.config.mode, ExecutorMode::SingleThreaded) {
            return;
        }

        for i in 0..self.num_workers {
            let executor = Arc::clone(self);
            let worker_idx = i;

            thread::spawn(move || {
                executor.active_workers.fetch_add(1, Ordering::Relaxed);
                executor.worker_loop_with_local_queue(worker_idx);
                executor.active_workers.fetch_sub(1, Ordering::Relaxed);
            });
        }
    }

    /// Worker thread main loop (creates local Worker queue)
    fn worker_loop_with_local_queue(&self, worker_idx: usize) {
        let thread_id = (worker_idx + 1) as u64;

        // Create a thread-local worker queue
        let local_worker = Worker::new_fifo();
        let stealer = local_worker.stealer();

        // Register our stealer so other threads can steal from us
        {
            let mut stealers = self.stealers.write();
            stealers.push(StealerHandle { stealer, thread_id });
        }

        while !self.shutdown.load(Ordering::Relaxed) {
            // Poll timers
            self.poll_timers();

            // Try to get a task from our local queue first
            let task = local_worker
                .pop()
                .or_else(|| self.try_steal_task(worker_idx));

            if let Some(task) = task {
                if let Some(result) = self.execute_task(task.handle, thread_id) {
                    let tasks = self.tasks.read();
                    if let Some(task_info) = tasks.get(&task.handle) {
                        if !task_info.is_completed()
                            && !matches!(task_info.state, MTTaskState::Blocked(_))
                        {
                            drop(tasks);
                            self.complete_task(task.handle, result);
                        }
                    }
                }
            } else {
                // Park until notified
                let mut guard = self.park_mutex.lock();
                self.park_condvar
                    .wait_for(&mut guard, Duration::from_millis(10));
            }
        }
    }

    /// Shutdown the executor
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.park_condvar.notify_all();
    }

    /// Reset the executor state (for testing)
    pub fn reset(&self) {
        self.tasks.write().clear();
        self.task_waiters.lock().clear();
        self.channel_recv_waiters.lock().clear();
        self.channel_send_waiters.lock().clear();
        self.net_op_waiters.lock().clear();
        self.timer_deadlines.lock().clear();
        self.current_tasks.write().clear();
        self.completed_count.store(0, Ordering::Relaxed);

        // Clear injector queue
        while self.injector.steal().is_success() {}

        // Clear thread-local queues
        for i in 0..self.num_workers {
            while self.thread_local_queues.pop(i).is_some() {}
        }
    }

    /// Get statistics
    pub fn stats(&self) -> ExecutorStats {
        let tasks = self.tasks.read();
        let mut pending = 0;
        let mut running = 0;
        let mut blocked = 0;
        let mut completed = 0;

        for task in tasks.values() {
            match &task.state {
                MTTaskState::Pending => pending += 1,
                MTTaskState::Running { .. } => running += 1,
                MTTaskState::Blocked(_) => blocked += 1,
                MTTaskState::Completed(_) | MTTaskState::Cancelled | MTTaskState::Panicked(_) => {
                    completed += 1;
                }
            }
        }

        ExecutorStats {
            pending,
            running,
            blocked,
            completed,
            total_completed: self.completed_count.load(Ordering::Relaxed),
            active_workers: self.active_workers.load(Ordering::Relaxed),
        }
    }
}

/// Executor statistics
#[derive(Debug, Clone)]
pub struct ExecutorStats {
    pub pending: usize,
    pub running: usize,
    pub blocked: usize,
    pub completed: usize,
    pub total_completed: u64,
    pub active_workers: usize,
}

// =============================================================================
// Global Executor Instance
// =============================================================================

static GLOBAL_EXECUTOR: OnceLock<Arc<ThreadedExecutor>> = OnceLock::new();

/// Get or create the global executor
pub fn global_executor() -> &'static Arc<ThreadedExecutor> {
    GLOBAL_EXECUTOR.get_or_init(|| ThreadedExecutor::new(ExecutorConfig::default()))
}

/// Initialize the global executor with a custom configuration
pub fn init_global_executor(config: ExecutorConfig) -> &'static Arc<ThreadedExecutor> {
    // Note: OnceLock only initializes once, so this will only work
    // if called before any other access to global_executor()
    GLOBAL_EXECUTOR.get_or_init(|| {
        let executor = ThreadedExecutor::new(config);
        executor.start_workers();
        executor
    })
}

// =============================================================================
// C-ABI Functions for Multi-Threaded Executor
// =============================================================================

/// Initialize the executor in single-threaded mode (default)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_executor_init_single() {
    init_global_executor(ExecutorConfig {
        mode: ExecutorMode::SingleThreaded,
        ..Default::default()
    });
}

/// Initialize the executor in multi-threaded mode
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_executor_init_multi(num_threads: i64) {
    init_global_executor(ExecutorConfig {
        mode: ExecutorMode::MultiThreaded {
            num_threads: num_threads as usize,
        },
        ..Default::default()
    });
}

/// Spawn a task with markers
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_task_spawn(fn_id: i64, is_sendable: i32, origin_thread: i64) -> i64 {
    let markers = TaskMarkers {
        is_sendable: is_sendable != 0,
        is_shareable: true,
        origin_thread: origin_thread as u64,
    };
    global_executor().spawn(fn_id, markers)
}

/// Push argument to task
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_task_push_arg(handle: i64, arg: i64) -> i32 {
    if global_executor().push_arg(handle, arg) {
        0
    } else {
        1
    }
}

/// Start a task
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_task_start(handle: i64) -> i32 {
    if global_executor().start(handle) {
        0
    } else {
        1
    }
}

/// Get current task for calling thread
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_task_current() -> i64 {
    // In single-threaded mode, use thread ID 1
    global_executor().current_task(1)
}

/// Get the async stack trace for the current task.
/// Returns a vector of AsyncStackFrame from current task up to root.
pub fn get_current_async_stack() -> Vec<AsyncStackFrame> {
    // Use thread-local storage to get the current task handle
    let current_task = CURRENT_TASK_HANDLE.with(|h| h.get());
    if current_task > 0 {
        global_executor().build_async_stack(current_task)
    } else {
        Vec::new()
    }
}

/// Get the async stack trace for a specific task.
pub fn get_async_stack_for_task(task_handle: i64) -> Vec<AsyncStackFrame> {
    if task_handle > 0 {
        global_executor().build_async_stack(task_handle)
    } else {
        Vec::new()
    }
}

/// Complete a task
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_task_complete(handle: i64, result: i64) {
    global_executor().complete_task(handle, result);
}

/// Cancel a task
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_task_cancel(handle: i64) -> i32 {
    if global_executor().cancel_task(handle) {
        0
    } else {
        1
    }
}

/// Block current task waiting for another task
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_task_block_on_task(current: i64, target: i64) {
    global_executor().block_task(current, MTBlockReason::AwaitingTask(target));
}

/// Block current task waiting for channel recv
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_task_block_on_recv(current: i64, channel: i64) {
    global_executor().block_task(current, MTBlockReason::AwaitingChannelRecv(channel));
}

/// Block current task waiting for channel send
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_task_block_on_send(current: i64, channel: i64) {
    global_executor().block_task(current, MTBlockReason::AwaitingChannelSend(channel));
}

/// Wake channel recv waiter
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_wake_recv_waiter(channel: i64) -> i32 {
    if global_executor().wake_channel_recv_waiter(channel) {
        1
    } else {
        0
    }
}

/// Wake channel send waiter
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_wake_send_waiter(channel: i64) -> i32 {
    if global_executor().wake_channel_send_waiter(channel) {
        1
    } else {
        0
    }
}

/// Wake all channel waiters (for close)
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_wake_all_channel_waiters(channel: i64) {
    global_executor().wake_all_channel_waiters(channel);
}

/// Run executor until quiescent
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_executor_run() -> i64 {
    global_executor().run_until_quiescent() as i64
}

/// Reset executor state
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_executor_reset() {
    global_executor().reset();
}

/// Get executor statistics
#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_executor_pending_count() -> i64 {
    global_executor().stats().pending as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_executor_blocked_count() -> i64 {
    global_executor().stats().blocked as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn __arth_mt_executor_completed_count() -> i64 {
    global_executor().stats().total_completed as i64
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests;
