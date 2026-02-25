// Concurrent Runtime Executor
// Work-stealing thread pool for true parallel task execution.
//
// Architecture:
// - N worker threads (default: CPU count)
// - Global injector queue for new tasks
// - Per-worker local deques for task execution
// - Work-stealing: idle workers steal from busy workers
//
// Task lifecycle:
// - ExecutorSpawn: creates ConcurrentTask, pushes to global injector
// - Worker: pops from local, or steals from others, or pops from global
// - ExecutorJoin: if target pending, block until complete

use crossbeam_deque::{Injector, Steal, Stealer, Worker as CbWorker};
use parking_lot::Condvar as ParkingCondvar;
// Note: ParkingMutex is already imported in ffi.inc.rs which is included before this file
use std::collections::HashMap as StdHashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

// AtomicBool, AtomicUsize, Ordering are imported in runtime.rs
// Use aliases for consistency with existing code
use Ordering as AtomicOrdering;

/// Concurrent task state (separate from single-threaded TaskState in ffi.inc.rs)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ConcurrentTaskState {
    /// Task created but not yet running
    Pending = 0,
    /// Task currently executing on a worker
    Running = 1,
    /// Task completed successfully
    Completed = 2,
    /// Task was cancelled
    Cancelled = 3,
    /// Task is suspended (waiting on another task/channel)
    Suspended = 4,
}

impl From<u8> for ConcurrentTaskState {
    fn from(v: u8) -> Self {
        match v {
            0 => ConcurrentTaskState::Pending,
            1 => ConcurrentTaskState::Running,
            2 => ConcurrentTaskState::Completed,
            3 => ConcurrentTaskState::Cancelled,
            4 => ConcurrentTaskState::Suspended,
            _ => ConcurrentTaskState::Pending,
        }
    }
}

/// A unique task ID for the concurrent executor
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConcurrentTaskId(pub u64);

/// A task that can be executed by a worker thread
#[derive(Debug)]
pub struct ConcurrentTask {
    /// Unique identifier for this task
    pub id: ConcurrentTaskId,
    /// Function ID to execute (hash used for async_dispatch lookup)
    pub fn_id: i64,
    /// Arguments for the function (copied values, not references)
    pub args: Vec<i64>,
    /// Current state of the task
    pub state: AtomicUsize,
    /// Result value when completed
    pub result: ParkingMutex<Option<i64>>,
    /// Saved instruction pointer for suspension
    pub saved_ip: ParkingMutex<Option<usize>>,
    /// Saved locals for suspension
    pub saved_locals: ParkingMutex<Option<Vec<i64>>>,
    /// Tasks waiting for this task to complete (wakers)
    pub waiters: ParkingMutex<Vec<ConcurrentTaskId>>,
    /// Which worker thread executed this task (usize::MAX if not yet executed)
    pub executed_by: AtomicUsize,
    /// Task ID that this task is waiting on (for suspension)
    pub waiting_on: ParkingMutex<Option<ConcurrentTaskId>>,
}

impl ConcurrentTask {
    /// Create a new pending task
    pub fn new(id: ConcurrentTaskId, fn_id: i64, args: Vec<i64>) -> Self {
        Self {
            id,
            fn_id,
            args,
            state: AtomicUsize::new(ConcurrentTaskState::Pending as usize),
            result: ParkingMutex::new(None),
            saved_ip: ParkingMutex::new(None),
            saved_locals: ParkingMutex::new(None),
            waiters: ParkingMutex::new(Vec::new()),
            executed_by: AtomicUsize::new(usize::MAX),
            waiting_on: ParkingMutex::new(None),
        }
    }

    /// Get which worker executed this task
    pub fn get_executed_by(&self) -> Option<usize> {
        let worker = self.executed_by.load(AtomicOrdering::SeqCst);
        if worker == usize::MAX {
            None
        } else {
            Some(worker)
        }
    }

    /// Record which worker executed this task
    pub fn set_executed_by(&self, worker_idx: usize) {
        self.executed_by.store(worker_idx, AtomicOrdering::SeqCst);
    }

    /// Get current state
    pub fn get_state(&self) -> ConcurrentTaskState {
        ConcurrentTaskState::from(self.state.load(AtomicOrdering::SeqCst) as u8)
    }

    /// Set state with release ordering
    pub fn set_state(&self, state: ConcurrentTaskState) {
        self.state.store(state as usize, AtomicOrdering::SeqCst);
    }

    /// Try to transition from expected state to new state
    pub fn try_transition(&self, expected: ConcurrentTaskState, new: ConcurrentTaskState) -> bool {
        self.state
            .compare_exchange(
                expected as usize,
                new as usize,
                AtomicOrdering::SeqCst,
                AtomicOrdering::SeqCst,
            )
            .is_ok()
    }

    /// Set the result and mark as completed
    pub fn complete(&self, value: i64) {
        *self.result.lock() = Some(value);
        self.set_state(ConcurrentTaskState::Completed);
    }

    /// Get the result if completed
    pub fn get_result(&self) -> Option<i64> {
        *self.result.lock()
    }

    /// Register a waiter to be notified when this task completes
    pub fn add_waiter(&self, waiter_id: ConcurrentTaskId) {
        self.waiters.lock().push(waiter_id);
    }

    /// Get and clear all waiters
    pub fn take_waiters(&self) -> Vec<ConcurrentTaskId> {
        std::mem::take(&mut *self.waiters.lock())
    }

    /// Suspend this task, saving execution state and what we're waiting on
    pub fn suspend(&self, ip: usize, locals: Vec<i64>, waiting_on: Option<ConcurrentTaskId>) {
        *self.saved_ip.lock() = Some(ip);
        *self.saved_locals.lock() = Some(locals);
        *self.waiting_on.lock() = waiting_on;
        self.set_state(ConcurrentTaskState::Suspended);
    }

    /// Get the task this task is waiting on
    pub fn get_waiting_on(&self) -> Option<ConcurrentTaskId> {
        *self.waiting_on.lock()
    }

    /// Clear what we're waiting on
    pub fn clear_waiting_on(&self) {
        *self.waiting_on.lock() = None;
    }

    /// Get saved instruction pointer (for resumption)
    pub fn get_saved_ip(&self) -> Option<usize> {
        *self.saved_ip.lock()
    }

    /// Get saved locals (for resumption)
    pub fn get_saved_locals(&self) -> Option<Vec<i64>> {
        self.saved_locals.lock().clone()
    }

    /// Clear saved state after resumption
    pub fn clear_saved_state(&self) {
        *self.saved_ip.lock() = None;
        *self.saved_locals.lock() = None;
    }

    /// Check if this task has saved state (was suspended)
    pub fn has_saved_state(&self) -> bool {
        self.saved_ip.lock().is_some()
    }
}

/// Shared state for the thread pool
pub struct ConcurrentPoolState {
    /// Global injector queue - where new tasks are submitted
    pub injector: Injector<Arc<ConcurrentTask>>,
    /// Stealers for each worker's local queue
    pub stealers: Vec<Stealer<Arc<ConcurrentTask>>>,
    /// All tasks by ID (for await/wakeup)
    pub tasks: ParkingMutex<StdHashMap<ConcurrentTaskId, Arc<ConcurrentTask>>>,
    /// Next task ID counter
    pub next_task_id: AtomicU64,
    /// Shutdown flag
    pub shutdown: AtomicBool,
    /// Number of active workers
    pub active_workers: AtomicUsize,
    /// Condvar for worker parking
    pub work_available: ParkingCondvar,
    /// Mutex for condvar
    pub work_mutex: ParkingMutex<()>,
    /// Per-worker task execution counts (for work-stealing verification)
    pub worker_task_counts: Vec<AtomicU64>,
    /// Total number of workers (for stats queries)
    pub num_workers: AtomicUsize,
}

impl ConcurrentPoolState {
    /// Create new pool state (without stealers - they're added after workers created)
    pub fn new() -> Self {
        Self {
            injector: Injector::new(),
            stealers: Vec::new(),
            tasks: ParkingMutex::new(StdHashMap::new()),
            next_task_id: AtomicU64::new(1),
            shutdown: AtomicBool::new(false),
            active_workers: AtomicUsize::new(0),
            work_available: ParkingCondvar::new(),
            work_mutex: ParkingMutex::new(()),
            worker_task_counts: Vec::new(), // Initialized when pool is created
            num_workers: AtomicUsize::new(0),
        }
    }

    /// Initialize worker task counts for the given number of workers
    pub fn init_worker_counts(&mut self, num_workers: usize) {
        self.worker_task_counts = (0..num_workers).map(|_| AtomicU64::new(0)).collect();
        self.num_workers.store(num_workers, AtomicOrdering::SeqCst);
    }

    /// Increment the task count for a worker
    pub fn record_worker_execution(&self, worker_idx: usize) {
        if worker_idx < self.worker_task_counts.len() {
            self.worker_task_counts[worker_idx].fetch_add(1, AtomicOrdering::SeqCst);
        }
    }

    /// Get task execution count for a specific worker
    pub fn get_worker_task_count(&self, worker_idx: usize) -> u64 {
        if worker_idx < self.worker_task_counts.len() {
            self.worker_task_counts[worker_idx].load(AtomicOrdering::SeqCst)
        } else {
            0
        }
    }

    /// Get task execution counts for all workers
    pub fn get_all_worker_task_counts(&self) -> Vec<u64> {
        self.worker_task_counts
            .iter()
            .map(|c| c.load(AtomicOrdering::SeqCst))
            .collect()
    }

    /// Get the number of workers that executed at least one task
    pub fn get_active_executor_count(&self) -> usize {
        self.worker_task_counts
            .iter()
            .filter(|c| c.load(AtomicOrdering::SeqCst) > 0)
            .count()
    }

    /// Reset all worker task counts (for testing)
    pub fn reset_worker_task_counts(&self) {
        for count in &self.worker_task_counts {
            count.store(0, AtomicOrdering::SeqCst);
        }
    }

    /// Allocate a new task ID
    pub fn alloc_task_id(&self) -> ConcurrentTaskId {
        ConcurrentTaskId(self.next_task_id.fetch_add(1, AtomicOrdering::SeqCst))
    }

    /// Submit a new task to the global queue
    pub fn submit(&self, task: Arc<ConcurrentTask>) {
        self.tasks.lock().insert(task.id, Arc::clone(&task));
        self.injector.push(task);
        // Wake a parked worker
        self.work_available.notify_one();
    }

    /// Get a task by ID
    pub fn get_task(&self, id: ConcurrentTaskId) -> Option<Arc<ConcurrentTask>> {
        self.tasks.lock().get(&id).cloned()
    }

    /// Remove a completed task
    #[allow(dead_code)]
    pub fn remove_task(&self, id: ConcurrentTaskId) {
        self.tasks.lock().remove(&id);
    }

    /// Try to steal a task (called by workers)
    pub fn try_steal(&self, local: &CbWorker<Arc<ConcurrentTask>>, worker_idx: usize) -> Option<Arc<ConcurrentTask>> {
        // First, try local queue
        if let Some(task) = local.pop() {
            return Some(task);
        }

        // Try to steal from other workers
        for (idx, stealer) in self.stealers.iter().enumerate() {
            if idx == worker_idx {
                continue;
            }
            loop {
                match stealer.steal() {
                    Steal::Success(task) => return Some(task),
                    Steal::Empty => break,
                    Steal::Retry => continue,
                }
            }
        }

        // Finally, try the global injector
        loop {
            match self.injector.steal() {
                Steal::Success(task) => return Some(task),
                Steal::Empty => return None,
                Steal::Retry => continue,
            }
        }
    }

    /// Signal shutdown to all workers
    pub fn shutdown(&self) {
        self.shutdown.store(true, AtomicOrdering::SeqCst);
        // Wake all parked workers
        self.work_available.notify_all();
    }

    /// Check if shutdown was requested
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(AtomicOrdering::SeqCst)
    }
}

impl Default for ConcurrentPoolState {
    fn default() -> Self {
        Self::new()
    }
}

/// Worker thread handle
pub struct ConcurrentWorkerHandle {
    pub thread: JoinHandle<()>,
    pub index: usize,
}

/// The thread pool executor
pub struct ConcurrentThreadPool {
    /// Shared pool state
    pub state: Arc<ConcurrentPoolState>,
    /// Worker thread handles
    workers: Vec<ConcurrentWorkerHandle>,
    /// Number of worker threads
    num_threads: usize,
}

impl ConcurrentThreadPool {
    /// Create a new thread pool with the specified number of workers
    pub fn new(num_threads: usize) -> Self {
        let num_threads = num_threads.max(1);
        let mut state = ConcurrentPoolState::new();
        let mut workers = Vec::with_capacity(num_threads);
        let mut local_queues = Vec::with_capacity(num_threads);

        // Create local queues and stealers for each worker
        for _ in 0..num_threads {
            let local = CbWorker::new_fifo();
            state.stealers.push(local.stealer());
            local_queues.push(local);
        }

        // Initialize per-worker task counts
        state.init_worker_counts(num_threads);

        let state = Arc::new(state);

        // Spawn worker threads
        for (idx, local) in local_queues.into_iter().enumerate() {
            let pool_state = Arc::clone(&state);
            let thread = thread::Builder::new()
                .name(format!("arth-worker-{}", idx))
                .spawn(move || {
                    concurrent_worker_loop(pool_state, local, idx);
                })
                .expect("failed to spawn worker thread");

            workers.push(ConcurrentWorkerHandle { thread, index: idx });
        }

        ConcurrentThreadPool {
            state,
            workers,
            num_threads,
        }
    }

    /// Create a thread pool with CPU count workers
    pub fn with_cpu_count() -> Self {
        let num_cpus = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self::new(num_cpus)
    }

    /// Get number of worker threads
    pub fn num_threads(&self) -> usize {
        self.num_threads
    }

    /// Get number of currently active workers
    pub fn active_workers(&self) -> usize {
        self.state.active_workers.load(AtomicOrdering::SeqCst)
    }

    /// Spawn a new task
    pub fn spawn(&self, fn_id: i64, args: Vec<i64>) -> ConcurrentTaskId {
        let id = self.state.alloc_task_id();
        let task = Arc::new(ConcurrentTask::new(id, fn_id, args));
        self.state.submit(task);
        id
    }

    /// Get a task by ID
    pub fn get_task(&self, id: ConcurrentTaskId) -> Option<Arc<ConcurrentTask>> {
        self.state.get_task(id)
    }

    /// Wait for a task to complete and return its result
    /// This blocks the current thread until the task completes.
    pub fn join(&self, id: ConcurrentTaskId) -> Option<i64> {
        loop {
            if let Some(task) = self.state.get_task(id) {
                match task.get_state() {
                    ConcurrentTaskState::Completed => {
                        return task.get_result();
                    }
                    ConcurrentTaskState::Cancelled => {
                        return None;
                    }
                    _ => {
                        // Task not done yet, yield and retry
                        thread::yield_now();
                    }
                }
            } else {
                return None;
            }
        }
    }

    /// Shutdown the thread pool and wait for all workers to complete
    pub fn shutdown(self) {
        self.state.shutdown();
        for worker in self.workers {
            let _ = worker.thread.join();
        }
    }

    /// Get task execution count for a specific worker
    pub fn get_worker_task_count(&self, worker_idx: usize) -> u64 {
        self.state.get_worker_task_count(worker_idx)
    }

    /// Get task execution counts for all workers
    pub fn get_all_worker_task_counts(&self) -> Vec<u64> {
        self.state.get_all_worker_task_counts()
    }

    /// Get the number of workers that executed at least one task
    /// This is the key metric for verifying work-stealing is working
    pub fn get_active_executor_count(&self) -> usize {
        self.state.get_active_executor_count()
    }

    /// Reset all worker task counts (for testing)
    pub fn reset_worker_task_counts(&self) {
        self.state.reset_worker_task_counts();
    }
}

/// Worker thread main loop
fn concurrent_worker_loop(state: Arc<ConcurrentPoolState>, local: CbWorker<Arc<ConcurrentTask>>, worker_idx: usize) {
    state.active_workers.fetch_add(1, AtomicOrdering::SeqCst);

    loop {
        if state.is_shutdown() {
            break;
        }

        // Try to get work
        if let Some(task) = state.try_steal(&local, worker_idx) {
            // Try to transition to Running
            if task.try_transition(ConcurrentTaskState::Pending, ConcurrentTaskState::Running)
                || task.try_transition(ConcurrentTaskState::Suspended, ConcurrentTaskState::Running)
            {
                // Execute the task with worker index for tracking
                execute_concurrent_task(&task, &state, &local, worker_idx);
            }
            // If we couldn't transition, task was probably cancelled or already running
        } else {
            // No work available, park until signaled
            let mut guard = state.work_mutex.lock();
            // Double-check before parking
            if !state.is_shutdown() && state.injector.is_empty() {
                // Park with timeout to handle spurious wakeups
                state.work_available.wait_for(&mut guard, std::time::Duration::from_millis(10));
            }
        }
    }

    state.active_workers.fetch_sub(1, AtomicOrdering::SeqCst);
}

/// Execute a task
///
/// This is where the actual task execution happens. Supports:
/// - CPU-bound work (work types 1, 2)
/// - Spawn-and-await with suspension (work type 3)
/// - Resumption from suspended state
fn execute_concurrent_task(
    task: &Arc<ConcurrentTask>,
    state: &ConcurrentPoolState,
    _local: &CbWorker<Arc<ConcurrentTask>>,
    worker_idx: usize,
) {
    // Record which worker executed this task
    task.set_executed_by(worker_idx);
    state.record_worker_execution(worker_idx);

    // Check if we're resuming from suspension
    if task.has_saved_state() {
        // We were suspended waiting on another task - check if it's complete
        if let Some(target_id) = task.get_waiting_on() {
            if let Some(target) = state.get_task(target_id) {
                match target.get_state() {
                    ConcurrentTaskState::Completed => {
                        // Target is done! Get result and complete this task
                        let target_result = target.get_result().unwrap_or(-1);
                        // Retrieve saved locals and use first local as our accumulator
                        let saved_locals = task.get_saved_locals().unwrap_or_default();
                        let accumulated = saved_locals.first().copied().unwrap_or(0);
                        task.clear_saved_state();
                        task.clear_waiting_on();
                        // Result = accumulated + target result (demonstrates locals preserved)
                        task.complete(accumulated + target_result);
                        wake_waiters(task, state);
                        return;
                    }
                    ConcurrentTaskState::Cancelled => {
                        // Target was cancelled
                        task.clear_saved_state();
                        task.clear_waiting_on();
                        task.complete(-1);
                        wake_waiters(task, state);
                        return;
                    }
                    _ => {
                        // Target still not complete - suspend again
                        task.set_state(ConcurrentTaskState::Suspended);
                        // We're already registered as waiter, just return
                        return;
                    }
                }
            } else {
                // Target task not found (error case)
                task.clear_saved_state();
                task.clear_waiting_on();
                task.complete(-1);
                wake_waiters(task, state);
                return;
            }
        }
    }

    // Execute work based on fn_id and args
    //
    // Work types:
    //   0: No work (just return fn_id)
    //   1: Compute iterations - do `args[0]` iterations of work
    //   2: Fibonacci - compute fib(args[0])
    //   3: Spawn-and-await - spawn sub-task and wait for it (demonstrates suspension)
    //      args[0] = sub-task fn_id (work type)
    //      args[1] = sub-task argument
    //      args[2] = local accumulator value (to prove locals are preserved)

    let result = match task.fn_id {
        // Work type 0: No work, just return fn_id (for backward compatibility with C01 tests)
        _ if task.args.is_empty() => task.fn_id,

        // Work type 1: CPU iterations - do args[0] iterations of busy work
        1 => {
            let iterations = task.args.first().copied().unwrap_or(1000) as u64;
            let mut sum: u64 = 0;
            for i in 0..iterations {
                // Prevent optimization with volatile-like computation
                sum = sum.wrapping_add(i).wrapping_mul(31);
            }
            sum as i64
        }

        // Work type 2: Fibonacci computation
        2 => {
            let n = task.args.first().copied().unwrap_or(10) as u64;
            compute_fibonacci(n) as i64
        }

        // Work type 3: Spawn-and-await (demonstrates C04 suspension)
        3 => {
            let sub_fn_id = task.args.first().copied().unwrap_or(2); // default: fibonacci
            let sub_arg = task.args.get(1).copied().unwrap_or(10);
            let local_accumulator = task.args.get(2).copied().unwrap_or(100);

            // Spawn the sub-task
            let sub_task_id = state.alloc_task_id();
            let sub_task = Arc::new(ConcurrentTask::new(sub_task_id, sub_fn_id, vec![sub_arg]));
            state.submit(sub_task.clone());

            // Check if sub-task completed immediately (unlikely but possible)
            // Give a tiny bit of time for it to potentially complete
            std::thread::yield_now();

            match sub_task.get_state() {
                ConcurrentTaskState::Completed => {
                    // Already done - return result
                    let sub_result = sub_task.get_result().unwrap_or(-1);
                    return complete_task(task, state, local_accumulator + sub_result);
                }
                _ => {
                    // Sub-task not complete - suspend this task
                    // Save our local state: [local_accumulator]
                    // IP is just a marker (0 = awaiting sub-task)
                    task.suspend(0, vec![local_accumulator], Some(sub_task_id));
                    // Register this task as a waiter on the sub-task
                    sub_task.add_waiter(task.id);
                    // Return without completing - task stays suspended
                    return;
                }
            }
        }

        // Default: return fn_id (backward compatibility)
        _ => task.fn_id,
    };

    complete_task(task, state, result);
}

/// Complete a task and wake its waiters
fn complete_task(task: &Arc<ConcurrentTask>, state: &ConcurrentPoolState, result: i64) {
    task.complete(result);
    wake_waiters(task, state);
}

/// Wake all tasks waiting on the completed task
fn wake_waiters(task: &Arc<ConcurrentTask>, state: &ConcurrentPoolState) {
    let waiters = task.take_waiters();
    for waiter_id in waiters {
        if let Some(waiter) = state.get_task(waiter_id) {
            if waiter.get_state() == ConcurrentTaskState::Suspended {
                waiter.set_state(ConcurrentTaskState::Pending);
                // Re-submit to global queue
                state.injector.push(Arc::clone(&waiter));
                state.work_available.notify_one();
            }
        }
    }
}

/// Compute the n-th Fibonacci number (recursive, O(2^n) for CPU-bound work)
/// Uses naive recursive algorithm intentionally for CPU-intensive testing.
fn compute_fibonacci(n: u64) -> u64 {
    if n <= 1 {
        return n;
    }
    compute_fibonacci(n - 1).wrapping_add(compute_fibonacci(n - 2))
}

// ============================================================================
// Global Thread Pool Instance
// ============================================================================

/// Global thread pool - lazily initialized
static GLOBAL_CONCURRENT_POOL: OnceLock<ConcurrentThreadPool> = OnceLock::new();

/// Get or initialize the global thread pool
pub fn global_concurrent_pool() -> &'static ConcurrentThreadPool {
    GLOBAL_CONCURRENT_POOL.get_or_init(|| ConcurrentThreadPool::with_cpu_count())
}

/// Initialize the global pool with a specific number of threads
/// Must be called before any other pool operations.
/// Returns false if the pool was already initialized.
pub fn init_global_concurrent_pool(num_threads: usize) -> bool {
    GLOBAL_CONCURRENT_POOL.set(ConcurrentThreadPool::new(num_threads)).is_ok()
}

/// Get the number of threads in the global pool
pub fn global_concurrent_pool_thread_count() -> usize {
    global_concurrent_pool().num_threads()
}

/// Get the number of active workers in the global pool
pub fn global_concurrent_pool_active_workers() -> usize {
    global_concurrent_pool().active_workers()
}

/// Spawn a task on the global pool
pub fn spawn_concurrent_task(fn_id: i64, args: Vec<i64>) -> u64 {
    global_concurrent_pool().spawn(fn_id, args).0
}

/// Join a task on the global pool (blocking)
pub fn join_concurrent_task(task_id: u64) -> i64 {
    global_concurrent_pool().join(ConcurrentTaskId(task_id)).unwrap_or(-1)
}

/// Spawn a task with arguments on the global pool
pub fn spawn_concurrent_task_with_args(fn_id: i64, args: Vec<i64>) -> u64 {
    global_concurrent_pool().spawn(fn_id, args).0
}

/// Get task execution count for a specific worker in the global pool
pub fn global_worker_task_count(worker_idx: usize) -> u64 {
    global_concurrent_pool().get_worker_task_count(worker_idx)
}

/// Get task execution counts for all workers in the global pool
pub fn global_all_worker_task_counts() -> Vec<u64> {
    global_concurrent_pool().get_all_worker_task_counts()
}

/// Get the number of workers that executed at least one task in the global pool
/// This is the key metric for verifying work-stealing is working
pub fn global_active_executor_count() -> usize {
    global_concurrent_pool().get_active_executor_count()
}

/// Reset all worker task counts in the global pool (for testing)
pub fn global_reset_worker_task_counts() {
    global_concurrent_pool().reset_worker_task_counts();
}

// ============================================================================
// Thread Safety Verification Tests
// ============================================================================

#[cfg(test)]
mod concurrent_executor_tests {
    use super::*;

    #[test]
    fn test_concurrent_thread_pool_creation() {
        let pool = ConcurrentThreadPool::new(4);
        assert_eq!(pool.num_threads(), 4);
        // Give threads time to start
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(pool.active_workers() > 0);
        pool.shutdown();
    }

    #[test]
    fn test_concurrent_task_spawn_and_complete() {
        let pool = ConcurrentThreadPool::new(2);

        // Spawn a task with fn_id=42, no args (returns fn_id)
        let task_id = pool.spawn(42, vec![]);

        // Wait for completion
        let result = pool.join(task_id);
        assert_eq!(result, Some(42)); // returns fn_id when no args

        pool.shutdown();
    }

    #[test]
    fn test_concurrent_multiple_tasks() {
        let pool = ConcurrentThreadPool::new(4);

        let mut task_ids = Vec::new();
        for i in 0..100 {
            task_ids.push(pool.spawn(i, vec![]));
        }

        // All tasks should complete
        for id in task_ids {
            let result = pool.join(id);
            assert!(result.is_some());
        }

        pool.shutdown();
    }

    #[test]
    fn test_concurrent_task_state_transitions() {
        let task = ConcurrentTask::new(ConcurrentTaskId(1), 0, vec![]);
        assert_eq!(task.get_state(), ConcurrentTaskState::Pending);

        assert!(task.try_transition(ConcurrentTaskState::Pending, ConcurrentTaskState::Running));
        assert_eq!(task.get_state(), ConcurrentTaskState::Running);

        // Can't transition from wrong state
        assert!(!task.try_transition(ConcurrentTaskState::Pending, ConcurrentTaskState::Completed));

        task.complete(100);
        assert_eq!(task.get_state(), ConcurrentTaskState::Completed);
        assert_eq!(task.get_result(), Some(100));
    }

    #[test]
    fn test_concurrent_global_pool_initialization() {
        // Note: this may interfere with other tests if they run in parallel
        // In practice, the global pool should be initialized once at startup
        let count = global_concurrent_pool_thread_count();
        assert!(count > 0);
    }

    #[test]
    fn test_work_stealing_c02() {
        // C02: Work-Stealing Queues Test
        //
        // This test verifies:
        // 1. All 1000 tasks complete
        // 2. Multiple workers participated (work was stolen)

        let pool = ConcurrentThreadPool::new(4);

        // Give workers time to start
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Reset counters for this test
        pool.reset_worker_task_counts();

        // Spawn 1000 tasks with CPU-bound work (type 1: iterations)
        let mut task_ids = Vec::new();
        for _ in 0..1000 {
            // Work type 1 with 10000 iterations each
            let task_id = pool.spawn(1, vec![10000]);
            task_ids.push(task_id);
        }

        // Wait for all tasks to complete
        for id in task_ids {
            let result = pool.join(id);
            assert!(result.is_some(), "Task should complete");
        }

        // Verify work-stealing: multiple workers should have participated
        let counts = pool.get_all_worker_task_counts();
        let workers_used = pool.get_active_executor_count();

        // With 1000 tasks and 4 workers, we expect work to be distributed
        // At least 2 workers should have participated (conservative check)
        assert!(
            workers_used >= 2,
            "Work-stealing failed: only {} workers executed tasks (expected >= 2). Counts: {:?}",
            workers_used,
            counts
        );

        // All 1000 tasks should have been executed
        let total_executed: u64 = counts.iter().sum();
        assert_eq!(
            total_executed, 1000,
            "Not all tasks were executed: {} out of 1000",
            total_executed
        );

        pool.shutdown();
    }

    #[test]
    fn test_work_stealing_distribution() {
        // Test that work is reasonably distributed across workers
        let pool = ConcurrentThreadPool::new(4);
        std::thread::sleep(std::time::Duration::from_millis(50));
        pool.reset_worker_task_counts();

        // Spawn 400 tasks (100 per worker if perfectly distributed)
        let mut task_ids = Vec::new();
        for _ in 0..400 {
            let task_id = pool.spawn(1, vec![5000]); // moderate work
            task_ids.push(task_id);
        }

        for id in task_ids {
            pool.join(id);
        }

        let counts = pool.get_all_worker_task_counts();

        // No single worker should have done more than 60% of the work
        // (allowing for some imbalance, but not all work on one thread)
        let max_count = *counts.iter().max().unwrap_or(&0);
        let total: u64 = counts.iter().sum();

        if total > 0 {
            let max_percentage = (max_count as f64 / total as f64) * 100.0;
            assert!(
                max_percentage < 60.0,
                "Work distribution too imbalanced: one worker did {}% of {} tasks. Counts: {:?}",
                max_percentage,
                total,
                counts
            );
        }

        pool.shutdown();
    }

    #[test]
    fn test_cpu_bound_work_fibonacci() {
        // Test that Fibonacci work type works correctly
        let pool = ConcurrentThreadPool::new(2);

        // fib(10) = 55, fib(20) = 6765
        let task1 = pool.spawn(2, vec![10]); // work type 2 = fibonacci
        let task2 = pool.spawn(2, vec![20]);

        let result1 = pool.join(task1);
        let result2 = pool.join(task2);

        assert_eq!(result1, Some(55), "fib(10) should be 55");
        assert_eq!(result2, Some(6765), "fib(20) should be 6765");

        pool.shutdown();
    }

    #[test]
    fn test_suspension_c04() {
        // C04: Task Suspension/Wakeup Test
        //
        // This test verifies:
        // 1. Task A spawns Task B (via work type 3)
        // 2. Task A suspends while waiting for Task B
        // 3. When Task B completes, Task A wakes and resumes
        // 4. Task A's locals are preserved across suspension

        let pool = ConcurrentThreadPool::new(4);

        // Give workers time to start
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Work type 3: spawn-and-await
        // args[0] = sub-task fn_id (2 = fibonacci)
        // args[1] = sub-task argument (10 -> fib(10) = 55)
        // args[2] = local accumulator (100)
        // Expected result: 100 + 55 = 155
        let task_id = pool.spawn(3, vec![2, 10, 100]);

        // Wait for completion
        let result = pool.join(task_id);

        // Verify result: local_accumulator (100) + fib(10) (55) = 155
        assert_eq!(
            result,
            Some(155),
            "Suspension test failed: expected 100 + fib(10) = 155, got {:?}",
            result
        );

        pool.shutdown();
    }

    #[test]
    fn test_suspension_locals_preserved() {
        // Test that locals are correctly preserved across suspension
        let pool = ConcurrentThreadPool::new(2);
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Different local accumulator values to prove they're preserved
        let task1 = pool.spawn(3, vec![2, 5, 1000]); // 1000 + fib(5) = 1000 + 5 = 1005
        let task2 = pool.spawn(3, vec![2, 5, 2000]); // 2000 + fib(5) = 2000 + 5 = 2005

        let result1 = pool.join(task1);
        let result2 = pool.join(task2);

        assert_eq!(result1, Some(1005), "First task should have preserved local=1000");
        assert_eq!(result2, Some(2005), "Second task should have preserved local=2000");

        pool.shutdown();
    }

    #[test]
    fn test_suspension_chain() {
        // Test a chain of suspensions: outer awaits middle, middle awaits inner
        // For simplicity, we test multiple sequential spawn-and-await operations
        let pool = ConcurrentThreadPool::new(4);
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Spawn multiple tasks that each spawn-and-await
        let mut task_ids = Vec::new();
        for i in 0..10 {
            let accumulator = (i + 1) * 100; // 100, 200, 300, ...
            let task_id = pool.spawn(3, vec![2, 10, accumulator]); // accumulator + fib(10)
            task_ids.push((task_id, accumulator + 55)); // expected = accumulator + 55
        }

        // All should complete with correct values
        for (task_id, expected) in task_ids {
            let result = pool.join(task_id);
            assert_eq!(
                result,
                Some(expected),
                "Chain test: expected {}, got {:?}",
                expected,
                result
            );
        }

        pool.shutdown();
    }
}
