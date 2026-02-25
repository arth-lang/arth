//! Variadic closure dispatch for native backend.
//!
//! This provides a non-truncating path for closure calls with argument counts
//! above the fixed `arth_rt_closure_call_0..8` entrypoints.

use crate::closure::get_closure;

const MAX_TOTAL_ARITY: usize = 16;

fn dispatch_closure_call(fn_ptr: *const (), args: &[i64]) -> Option<i64> {
    match args.len() {
        0 => {
            let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(fn_ptr) };
            Some(f())
        }
        1 => {
            let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
            Some(f(args[0]))
        }
        2 => {
            let f: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
            Some(f(args[0], args[1]))
        }
        3 => {
            let f: extern "C" fn(i64, i64, i64) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
            Some(f(args[0], args[1], args[2]))
        }
        4 => {
            let f: extern "C" fn(i64, i64, i64, i64) -> i64 =
                unsafe { std::mem::transmute(fn_ptr) };
            Some(f(args[0], args[1], args[2], args[3]))
        }
        5 => {
            let f: extern "C" fn(i64, i64, i64, i64, i64) -> i64 =
                unsafe { std::mem::transmute(fn_ptr) };
            Some(f(args[0], args[1], args[2], args[3], args[4]))
        }
        6 => {
            let f: extern "C" fn(i64, i64, i64, i64, i64, i64) -> i64 =
                unsafe { std::mem::transmute(fn_ptr) };
            Some(f(args[0], args[1], args[2], args[3], args[4], args[5]))
        }
        7 => {
            let f: extern "C" fn(i64, i64, i64, i64, i64, i64, i64) -> i64 =
                unsafe { std::mem::transmute(fn_ptr) };
            Some(f(
                args[0], args[1], args[2], args[3], args[4], args[5], args[6],
            ))
        }
        8 => {
            let f: extern "C" fn(i64, i64, i64, i64, i64, i64, i64, i64) -> i64 =
                unsafe { std::mem::transmute(fn_ptr) };
            Some(f(
                args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7],
            ))
        }
        9 => {
            let f: extern "C" fn(i64, i64, i64, i64, i64, i64, i64, i64, i64) -> i64 =
                unsafe { std::mem::transmute(fn_ptr) };
            Some(f(
                args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7], args[8],
            ))
        }
        10 => {
            let f: extern "C" fn(i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) -> i64 =
                unsafe { std::mem::transmute(fn_ptr) };
            Some(f(
                args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7], args[8],
                args[9],
            ))
        }
        11 => {
            let f: extern "C" fn(i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) -> i64 =
                unsafe { std::mem::transmute(fn_ptr) };
            Some(f(
                args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7], args[8],
                args[9], args[10],
            ))
        }
        12 => {
            let f: extern "C" fn(
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
            ) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
            Some(f(
                args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7], args[8],
                args[9], args[10], args[11],
            ))
        }
        13 => {
            let f: extern "C" fn(
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
            ) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
            Some(f(
                args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7], args[8],
                args[9], args[10], args[11], args[12],
            ))
        }
        14 => {
            let f: extern "C" fn(
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
            ) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
            Some(f(
                args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7], args[8],
                args[9], args[10], args[11], args[12], args[13],
            ))
        }
        15 => {
            let f: extern "C" fn(
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
            ) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
            Some(f(
                args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7], args[8],
                args[9], args[10], args[11], args[12], args[13], args[14],
            ))
        }
        16 => {
            let f: extern "C" fn(
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
            ) -> i64 = unsafe { std::mem::transmute(fn_ptr) };
            Some(f(
                args[0], args[1], args[2], args[3], args[4], args[5], args[6], args[7], args[8],
                args[9], args[10], args[11], args[12], args[13], args[14], args[15],
            ))
        }
        _ => None,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_closure_call_variadic(
    closure_handle: i64,
    args_ptr: *const i64,
    num_args: i64,
) -> i64 {
    if num_args < 0 {
        return 0;
    }

    let num_args = num_args as usize;
    if let Some((fn_ptr, env_ptr, num_captures)) = get_closure(closure_handle) {
        if num_captures < 0 {
            return 0;
        }

        let num_captures = num_captures as usize;
        let total_arity = num_captures + num_args;
        if total_arity > MAX_TOTAL_ARITY {
            return 0;
        }

        let mut all_args = Vec::with_capacity(total_arity);
        for i in 0..num_captures {
            let cap = unsafe { *env_ptr.add(i) };
            all_args.push(cap);
        }

        if num_args > 0 {
            if args_ptr.is_null() {
                return 0;
            }
            for i in 0..num_args {
                let arg = unsafe { *args_ptr.add(i) };
                all_args.push(arg);
            }
        }

        return dispatch_closure_call(fn_ptr, &all_args).unwrap_or(0);
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::closure::{arth_rt_closure_capture, arth_rt_closure_new};

    extern "C" fn sum9(
        a0: i64,
        a1: i64,
        a2: i64,
        a3: i64,
        a4: i64,
        a5: i64,
        a6: i64,
        a7: i64,
        a8: i64,
    ) -> i64 {
        a0 + a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8
    }

    extern "C" fn cap_plus8(
        cap: i64,
        a0: i64,
        a1: i64,
        a2: i64,
        a3: i64,
        a4: i64,
        a5: i64,
        a6: i64,
        a7: i64,
    ) -> i64 {
        cap + a0 + a1 + a2 + a3 + a4 + a5 + a6 + a7
    }

    #[test]
    fn variadic_dispatch_supports_nine_args() {
        let fn_ptr = sum9 as *const ();
        let handle = arth_rt_closure_new(fn_ptr, 0);
        let args = [1_i64, 2, 3, 4, 5, 6, 7, 8, 9];
        let result = arth_rt_closure_call_variadic(handle, args.as_ptr(), args.len() as i64);
        assert_eq!(result, 45);
    }

    #[test]
    fn variadic_dispatch_supports_capture_plus_eight_args() {
        let fn_ptr = cap_plus8 as *const ();
        let handle = arth_rt_closure_new(fn_ptr, 1);
        arth_rt_closure_capture(handle, 10);
        let args = [1_i64, 2, 3, 4, 5, 6, 7, 8];
        let result = arth_rt_closure_call_variadic(handle, args.as_ptr(), args.len() as i64);
        assert_eq!(result, 46);
    }
}
