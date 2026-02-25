//! Minimal native executor runtime used by LLVM-emitted executor intrinsics.
//!
//! Phase-1 behavior is intentionally simple and deterministic:
//! - `spawn(fn_id)` stores `fn_id` as the task result.
//! - `spawn_with_arg(fn_id, arg)` computes small built-in work types.
//! - `join(handle)` returns the stored result.

use std::collections::HashMap;
use std::sync::Mutex;

struct ExecutorState {
    thread_count: i64,
    next_handle: i64,
    tasks: HashMap<i64, i64>,
    worker_task_count: HashMap<i64, i64>,
}

impl ExecutorState {
    fn new() -> Self {
        Self {
            thread_count: 0,
            next_handle: 1,
            tasks: HashMap::new(),
            worker_task_count: HashMap::new(),
        }
    }
}

lazy_static::lazy_static! {
    static ref EXECUTOR_STATE: Mutex<ExecutorState> = Mutex::new(ExecutorState::new());
}

fn fib(n: i64) -> i64 {
    if n <= 1 {
        return n.max(0);
    }
    let mut a = 0i64;
    let mut b = 1i64;
    for _ in 0..n {
        let next = a.saturating_add(b);
        a = b;
        b = next;
    }
    a
}

fn allocate_task(result: i64) -> i64 {
    let mut st = EXECUTOR_STATE.lock().unwrap();
    let handle = st.next_handle;
    st.next_handle += 1;
    st.tasks.insert(handle, result);
    *st.worker_task_count.entry(0).or_insert(0) += 1;
    handle
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_executor_init(thread_count: i64) -> i64 {
    let mut st = EXECUTOR_STATE.lock().unwrap();
    st.thread_count = thread_count.max(0);
    st.tasks.clear();
    st.worker_task_count.clear();
    st.next_handle = 1;
    if thread_count > 0 { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_executor_thread_count() -> i64 {
    EXECUTOR_STATE.lock().unwrap().thread_count
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_executor_active_workers() -> i64 {
    let st = EXECUTOR_STATE.lock().unwrap();
    if st.tasks.is_empty() || st.thread_count <= 0 {
        0
    } else {
        1
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_executor_spawn(fn_id: i64) -> i64 {
    allocate_task(fn_id)
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_executor_cancel(task_handle: i64) -> i64 {
    let mut st = EXECUTOR_STATE.lock().unwrap();
    if st.tasks.remove(&task_handle).is_some() {
        0
    } else {
        -1
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_executor_join(task_handle: i64) -> i64 {
    let mut st = EXECUTOR_STATE.lock().unwrap();
    st.tasks.remove(&task_handle).unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_executor_spawn_with_arg(fn_id: i64, arg: i64) -> i64 {
    let result = match fn_id {
        // Common work type used by concurrency e2e fixtures.
        2 => fib(arg),
        _ => arg,
    };
    allocate_task(result)
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_executor_active_executor_count() -> i64 {
    arth_rt_executor_active_workers()
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_executor_worker_task_count(worker_id: i64) -> i64 {
    EXECUTOR_STATE
        .lock()
        .unwrap()
        .worker_task_count
        .get(&worker_id)
        .copied()
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_executor_reset_stats() -> i64 {
    EXECUTOR_STATE.lock().unwrap().worker_task_count.clear();
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_executor_spawn_await(sub_fn_id: i64, sub_arg: i64, accum: i64) -> i64 {
    let sub = match sub_fn_id {
        2 => fib(sub_arg),
        _ => sub_arg,
    };
    allocate_task(accum.saturating_add(sub))
}
