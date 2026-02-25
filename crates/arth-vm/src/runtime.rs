use crate::host::{FileMode, HostContext, HostGenericCall, InstantHandle, SeekPosition};
use crate::ops::{HostCryptoOp, HostDbOp, HostIoOp, HostMailOp, HostNetOp, HostTimeOp};
use crate::{Op, Program};
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

// Re-export concurrent executor types for external use
#[allow(unused_imports)]
pub use self::concurrent_executor_types::*;

/// Concurrent executor types module
#[allow(unused_imports)]
mod concurrent_executor_types {
    pub use super::{
        ConcurrentPoolState, ConcurrentTask, ConcurrentTaskId, ConcurrentTaskState,
        ConcurrentThreadPool, ConcurrentWorkerHandle, global_active_executor_count,
        global_all_worker_task_counts, global_concurrent_pool,
        global_concurrent_pool_active_workers, global_concurrent_pool_thread_count,
        global_reset_worker_task_counts, global_worker_task_count, init_global_concurrent_pool,
        join_concurrent_task, spawn_concurrent_task, spawn_concurrent_task_with_args,
    };
}

// This module used to be a single ~5k LOC `runtime.rs`. We keep all items in the
// same `runtime` module scope (to avoid large privacy/visibility churn), but
// split implementation across a few focused include files for reviewability.
include!("runtime/core.inc.rs");
include!("runtime/html.inc.rs");
include!("runtime/template.inc.rs");
include!("runtime/ffi.inc.rs"); // Must be before interpreter.inc.rs for panic_with_message_and_stack
include!("runtime/interpreter.inc.rs");
include!("runtime/host.inc.rs");
include!("runtime/executor.inc.rs");
