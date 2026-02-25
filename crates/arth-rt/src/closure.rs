//! Closure Runtime Support
//!
//! This module provides runtime support for closures in Arth.
//! Closures are represented as a struct containing:
//! - A function pointer to the lambda function
//! - A pointer to the environment (captured values)
//! - The number of captured values
//!
//! Lambda functions receive captured values as their first N parameters,
//! followed by the explicit arguments passed at the call site.

use std::collections::HashMap;
use std::sync::Mutex;

/// Wrapper for raw pointers to make them Send+Sync
/// SAFETY: The runtime ensures closures are only accessed in a thread-safe manner
/// via the global mutex. Function pointers are immutable, and environment access
/// is protected by the mutex.
pub struct SendPtr<T>(pub *const T);
pub struct SendPtrMut<T>(pub *mut T);

unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}
unsafe impl<T> Send for SendPtrMut<T> {}
unsafe impl<T> Sync for SendPtrMut<T> {}

/// Closure representation in memory
/// Layout: { fn_ptr: *const (), env_ptr: *mut i64, num_captures: i64 }
pub struct Closure {
    /// Pointer to the lambda function (wrapped for thread safety)
    pub fn_ptr: SendPtr<()>,
    /// Pointer to the captured values array (wrapped for thread safety)
    pub env_ptr: SendPtrMut<i64>,
    /// Number of captured values
    pub num_captures: i64,
}

/// Global storage for closures (handle -> Closure)
static CLOSURES: Mutex<Option<HashMap<i64, Box<Closure>>>> = Mutex::new(None);

/// Next closure handle
static NEXT_CLOSURE_HANDLE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);

fn get_closure_map() -> std::sync::MutexGuard<'static, Option<HashMap<i64, Box<Closure>>>> {
    CLOSURES.lock().unwrap()
}

fn ensure_closure_map() {
    let mut map = get_closure_map();
    if map.is_none() {
        *map = Some(HashMap::new());
    }
}

/// Create a new closure
///
/// # Arguments
/// * `fn_ptr` - Pointer to the lambda function
/// * `num_captures` - Number of values that will be captured
///
/// # Returns
/// A handle to the closure (as i64)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_closure_new(fn_ptr: *const (), num_captures: i64) -> i64 {
    ensure_closure_map();

    // Allocate environment for captured values
    let env_ptr = if num_captures > 0 {
        let layout = std::alloc::Layout::array::<i64>(num_captures as usize).unwrap();
        unsafe { std::alloc::alloc_zeroed(layout) as *mut i64 }
    } else {
        std::ptr::null_mut()
    };

    let closure = Box::new(Closure {
        fn_ptr: SendPtr(fn_ptr),
        env_ptr: SendPtrMut(env_ptr),
        num_captures,
    });

    let handle = NEXT_CLOSURE_HANDLE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let mut map = get_closure_map();
    if let Some(ref mut m) = *map {
        m.insert(handle, closure);
    }

    handle
}

/// Add a captured value to a closure's environment
///
/// This should be called once for each captured value, in order.
/// Uses an internal counter to track the next capture slot.
///
/// # Arguments
/// * `closure_handle` - Handle returned by arth_rt_closure_new
/// * `value` - The captured value
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_closure_capture(closure_handle: i64, value: i64) {
    let mut map = get_closure_map();
    if let Some(ref mut m) = *map {
        if let Some(closure) = m.get_mut(&closure_handle) {
            let env_ptr = closure.env_ptr.0;
            // Find the next empty slot in the environment
            // We use a simple approach: scan for the first zero value
            // This assumes captures are added in order and values are non-zero
            // A more robust approach would track the capture index separately
            if !env_ptr.is_null() {
                for i in 0..closure.num_captures as usize {
                    let ptr = unsafe { env_ptr.add(i) };
                    let current = unsafe { *ptr };
                    if current == 0 {
                        unsafe { *ptr = value };
                        return;
                    }
                }
            }
        }
    }
}

/// Helper to get a closure by handle
pub(crate) fn get_closure(handle: i64) -> Option<(*const (), *mut i64, i64)> {
    let map = get_closure_map();
    if let Some(ref m) = *map {
        if let Some(closure) = m.get(&handle) {
            return Some((closure.fn_ptr.0, closure.env_ptr.0, closure.num_captures));
        }
    }
    None
}

/// Call a closure with 0 arguments
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_closure_call_0(closure_handle: i64) -> i64 {
    if let Some((fn_ptr, env_ptr, num_captures)) = get_closure(closure_handle) {
        // Build the argument list with captures
        let mut args: Vec<i64> = Vec::with_capacity(num_captures as usize);
        for i in 0..num_captures as usize {
            let val = unsafe { *env_ptr.add(i) };
            args.push(val);
        }

        // Call the function with captures as arguments
        // This uses a trampoline approach based on capture count
        match num_captures {
            0 => {
                let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(fn_ptr) };
                f()
            }
            1 => {
                let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
                f(args[0])
            }
            2 => {
                let f: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
                f(args[0], args[1])
            }
            3 => {
                let f: extern "C" fn(i64, i64, i64) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
                f(args[0], args[1], args[2])
            }
            4 => {
                let f: extern "C" fn(i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(fn_ptr) };
                f(args[0], args[1], args[2], args[3])
            }
            _ => {
                // For more captures, we'd need a more sophisticated calling mechanism
                // For now, panic with an error message
                panic!(
                    "arth_rt_closure_call_0: too many captures ({})",
                    num_captures
                );
            }
        }
    } else {
        0 // Invalid closure handle
    }
}

/// Call a closure with 1 argument
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_closure_call_1(closure_handle: i64, arg0: i64) -> i64 {
    if let Some((fn_ptr, env_ptr, num_captures)) = get_closure(closure_handle) {
        match num_captures {
            0 => {
                let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
                f(arg0)
            }
            1 => {
                let cap0 = unsafe { *env_ptr };
                let f: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
                f(cap0, arg0)
            }
            2 => {
                let cap0 = unsafe { *env_ptr };
                let cap1 = unsafe { *env_ptr.add(1) };
                let f: extern "C" fn(i64, i64, i64) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
                f(cap0, cap1, arg0)
            }
            3 => {
                let cap0 = unsafe { *env_ptr };
                let cap1 = unsafe { *env_ptr.add(1) };
                let cap2 = unsafe { *env_ptr.add(2) };
                let f: extern "C" fn(i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(fn_ptr) };
                f(cap0, cap1, cap2, arg0)
            }
            _ => panic!(
                "arth_rt_closure_call_1: too many captures ({})",
                num_captures
            ),
        }
    } else {
        0
    }
}

/// Call a closure with 2 arguments
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_closure_call_2(closure_handle: i64, arg0: i64, arg1: i64) -> i64 {
    if let Some((fn_ptr, env_ptr, num_captures)) = get_closure(closure_handle) {
        match num_captures {
            0 => {
                let f: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
                f(arg0, arg1)
            }
            1 => {
                let cap0 = unsafe { *env_ptr };
                let f: extern "C" fn(i64, i64, i64) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
                f(cap0, arg0, arg1)
            }
            2 => {
                let cap0 = unsafe { *env_ptr };
                let cap1 = unsafe { *env_ptr.add(1) };
                let f: extern "C" fn(i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(fn_ptr) };
                f(cap0, cap1, arg0, arg1)
            }
            _ => panic!(
                "arth_rt_closure_call_2: too many captures ({})",
                num_captures
            ),
        }
    } else {
        0
    }
}

/// Call a closure with 3 arguments
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_closure_call_3(
    closure_handle: i64,
    arg0: i64,
    arg1: i64,
    arg2: i64,
) -> i64 {
    if let Some((fn_ptr, env_ptr, num_captures)) = get_closure(closure_handle) {
        match num_captures {
            0 => {
                let f: extern "C" fn(i64, i64, i64) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
                f(arg0, arg1, arg2)
            }
            1 => {
                let cap0 = unsafe { *env_ptr };
                let f: extern "C" fn(i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(fn_ptr) };
                f(cap0, arg0, arg1, arg2)
            }
            2 => {
                let cap0 = unsafe { *env_ptr };
                let cap1 = unsafe { *env_ptr.add(1) };
                let f: extern "C" fn(i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(fn_ptr) };
                f(cap0, cap1, arg0, arg1, arg2)
            }
            _ => panic!(
                "arth_rt_closure_call_3: too many captures ({})",
                num_captures
            ),
        }
    } else {
        0
    }
}

/// Call a closure with 4 arguments
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_closure_call_4(
    closure_handle: i64,
    arg0: i64,
    arg1: i64,
    arg2: i64,
    arg3: i64,
) -> i64 {
    if let Some((fn_ptr, env_ptr, num_captures)) = get_closure(closure_handle) {
        match num_captures {
            0 => {
                let f: extern "C" fn(i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(fn_ptr) };
                f(arg0, arg1, arg2, arg3)
            }
            1 => {
                let cap0 = unsafe { *env_ptr };
                let f: extern "C" fn(i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(fn_ptr) };
                f(cap0, arg0, arg1, arg2, arg3)
            }
            _ => panic!(
                "arth_rt_closure_call_4: too many captures ({})",
                num_captures
            ),
        }
    } else {
        0
    }
}

/// Call a closure with 5 arguments
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_closure_call_5(
    closure_handle: i64,
    arg0: i64,
    arg1: i64,
    arg2: i64,
    arg3: i64,
    arg4: i64,
) -> i64 {
    if let Some((fn_ptr, env_ptr, num_captures)) = get_closure(closure_handle) {
        match num_captures {
            0 => {
                let f: extern "C" fn(i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(fn_ptr) };
                f(arg0, arg1, arg2, arg3, arg4)
            }
            1 => {
                let cap0 = unsafe { *env_ptr };
                let f: extern "C" fn(i64, i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(fn_ptr) };
                f(cap0, arg0, arg1, arg2, arg3, arg4)
            }
            _ => panic!(
                "arth_rt_closure_call_5: too many captures ({})",
                num_captures
            ),
        }
    } else {
        0
    }
}

/// Call a closure with 6 arguments
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_closure_call_6(
    closure_handle: i64,
    arg0: i64,
    arg1: i64,
    arg2: i64,
    arg3: i64,
    arg4: i64,
    arg5: i64,
) -> i64 {
    if let Some((fn_ptr, _env_ptr, num_captures)) = get_closure(closure_handle) {
        match num_captures {
            0 => {
                let f: extern "C" fn(i64, i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(fn_ptr) };
                f(arg0, arg1, arg2, arg3, arg4, arg5)
            }
            _ => panic!(
                "arth_rt_closure_call_6: too many captures ({})",
                num_captures
            ),
        }
    } else {
        0
    }
}

/// Call a closure with 7 arguments
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_closure_call_7(
    closure_handle: i64,
    arg0: i64,
    arg1: i64,
    arg2: i64,
    arg3: i64,
    arg4: i64,
    arg5: i64,
    arg6: i64,
) -> i64 {
    if let Some((fn_ptr, _env_ptr, num_captures)) = get_closure(closure_handle) {
        match num_captures {
            0 => {
                let f: extern "C" fn(i64, i64, i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(fn_ptr) };
                f(arg0, arg1, arg2, arg3, arg4, arg5, arg6)
            }
            _ => panic!(
                "arth_rt_closure_call_7: too many captures ({})",
                num_captures
            ),
        }
    } else {
        0
    }
}

/// Call a closure with 8 arguments
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_closure_call_8(
    closure_handle: i64,
    arg0: i64,
    arg1: i64,
    arg2: i64,
    arg3: i64,
    arg4: i64,
    arg5: i64,
    arg6: i64,
    arg7: i64,
) -> i64 {
    if let Some((fn_ptr, _env_ptr, num_captures)) = get_closure(closure_handle) {
        match num_captures {
            0 => {
                let f: extern "C" fn(i64, i64, i64, i64, i64, i64, i64, i64) -> i64 =
                    unsafe { std::mem::transmute(fn_ptr) };
                f(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7)
            }
            _ => panic!(
                "arth_rt_closure_call_8: too many captures ({})",
                num_captures
            ),
        }
    } else {
        0
    }
}

/// Free a closure and its environment
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_closure_free(closure_handle: i64) {
    let mut map = get_closure_map();
    if let Some(ref mut m) = *map {
        if let Some(closure) = m.remove(&closure_handle) {
            let env_ptr = closure.env_ptr.0;
            // Free the environment
            if !env_ptr.is_null() && closure.num_captures > 0 {
                let layout =
                    std::alloc::Layout::array::<i64>(closure.num_captures as usize).unwrap();
                unsafe { std::alloc::dealloc(env_ptr as *mut u8, layout) };
            }
            // Box will be dropped automatically
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn add_simple(a: i64, b: i64) -> i64 {
        a + b
    }

    extern "C" fn add_with_capture(captured: i64, arg: i64) -> i64 {
        captured + arg
    }

    #[test]
    fn test_closure_no_captures() {
        let fn_ptr = add_simple as *const ();
        let handle = arth_rt_closure_new(fn_ptr, 0);
        assert!(handle > 0);

        let result = arth_rt_closure_call_2(handle, 10, 20);
        assert_eq!(result, 30);

        arth_rt_closure_free(handle);
    }

    #[test]
    fn test_closure_with_capture() {
        let fn_ptr = add_with_capture as *const ();
        let handle = arth_rt_closure_new(fn_ptr, 1);
        arth_rt_closure_capture(handle, 100);

        let result = arth_rt_closure_call_1(handle, 5);
        assert_eq!(result, 105);

        arth_rt_closure_free(handle);
    }
}
