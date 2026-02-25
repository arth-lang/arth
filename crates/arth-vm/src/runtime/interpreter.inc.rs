use std::path::PathBuf;

const VM_EXTERN_CALL_MAX_ARGS: usize = 4;

// JIT integration: function parameter count cache
#[cfg(feature = "jit")]
use std::sync::RwLock;

#[cfg(feature = "jit")]
lazy_static::lazy_static! {
    /// Cache of function parameter counts, indexed by bytecode offset.
    /// Discovered on first call by scanning for max LocalGet index.
    static ref FUNC_PARAM_COUNTS: RwLock<std::collections::HashMap<u32, u32>> =
        RwLock::new(std::collections::HashMap::new());
}

/// Scan a function's bytecode to determine its local variable count.
/// Returns the highest LocalGet/LocalSet index + 1, or 0 if none found.
/// This is used to pre-allocate the local frame before calling a function.
fn scan_function_local_count(p: &Program, offset: u32) -> u32 {
    let mut max_local: i32 = -1;
    let mut ip = offset as usize;

    while ip < p.code.len() {
        match &p.code[ip] {
            Op::LocalGet(idx) | Op::LocalSet(idx) => {
                max_local = max_local.max(*idx as i32);
            }
            Op::Ret => break,
            Op::Jump(tgt) => {
                // Follow jumps within the function
                if (*tgt as usize) < offset as usize {
                    break; // Backward jump out of function
                }
            }
            Op::Call(_) | Op::CallSymbol(_) => {
                // Stop at nested calls - they have their own locals
            }
            _ => {}
        }
        ip += 1;

        // Safety: don't scan too far
        if ip > offset as usize + 10000 {
            break;
        }
    }

    if max_local < 0 { 0 } else { (max_local + 1) as u32 }
}

/// Find the first LocalGet in a function that doesn't have a prior LocalSet for the same index.
/// This identifies where the function expects its first parameter to be.
fn find_first_local_get(p: &Program, offset: u32) -> Option<u32> {
    let mut ip = offset as usize;
    let mut set_locals = std::collections::HashSet::new();
    let start_offset = offset as usize;

    while ip < p.code.len() {
        match &p.code[ip] {
            Op::LocalSet(idx) => {
                set_locals.insert(*idx);
            }
            Op::LocalGet(idx) => {
                // If this local was never set before, it's likely a parameter
                if !set_locals.contains(idx) {
                    return Some(*idx);
                }
            }
            Op::Ret | Op::Halt => break,
            Op::Jump(tgt) => {
                // Follow jump target for parameter scanning
                ip = *tgt as usize;
                continue;
            }
            _ => {}
        }
        ip += 1;

        // Safety: don't scan too far
        if ip > start_offset + 100 {
            break;
        }
    }

    None
}

/// Scan a function's bytecode to determine its parameter count.
/// Returns the highest LocalGet/LocalSet index + 1, or 0 if none found.
#[cfg(feature = "jit")]
fn scan_function_param_count(p: &Program, offset: u32) -> u32 {
    let mut max_local: i32 = -1;
    let mut ip = offset as usize;

    while ip < p.code.len() {
        match &p.code[ip] {
            Op::LocalGet(idx) | Op::LocalSet(idx) => {
                max_local = max_local.max(*idx as i32);
            }
            Op::Ret => break,
            Op::Jump(tgt) => {
                // Follow jumps within the function
                if (*tgt as usize) < offset as usize {
                    break; // Backward jump out of function
                }
            }
            Op::Call(_) | Op::CallSymbol(_) => {
                // Stop at nested calls - they have their own locals
                // But we still need to scan past them
            }
            _ => {}
        }
        ip += 1;

        // Safety: don't scan too far
        if ip > offset as usize + 10000 {
            break;
        }
    }

    if max_local < 0 { 0 } else { (max_local + 1) as u32 }
}

/// Get or compute the parameter count for a function.
#[cfg(feature = "jit")]
fn get_func_param_count(p: &Program, offset: u32) -> u32 {
    // Check cache first
    if let Ok(cache) = FUNC_PARAM_COUNTS.read() {
        if let Some(&count) = cache.get(&offset) {
            return count;
        }
    }

    // Scan to determine param count
    let count = scan_function_param_count(p, offset);

    // Cache the result
    if let Ok(mut cache) = FUNC_PARAM_COUNTS.write() {
        cache.insert(offset, count);
    }

    count
}

/// External package library paths set by the compiler before VM execution.
/// These are native libraries (.dylib/.so/.dll) from packages in ~/.arth/libs.
static EXTERNAL_PACKAGE_LIB_PATHS: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// Set external package library paths for FFI loading.
/// Must be called before any extern symbol resolution.
pub fn set_external_lib_paths(paths: Vec<PathBuf>) {
    let _ = EXTERNAL_PACKAGE_LIB_PATHS.set(paths);
}

/// Get external package library paths.
pub fn get_external_lib_paths() -> Option<&'static Vec<PathBuf>> {
    EXTERNAL_PACKAGE_LIB_PATHS.get()
}

/// Symbol table for cross-library function calls.
/// Maps fully-qualified function names (e.g., "demo.Math.add") to bytecode offsets.
static LINKED_SYMBOL_TABLE: OnceLock<HashMap<String, u32>> = OnceLock::new();

/// Set the symbol table for cross-library function calls.
/// Must be called before running a linked program that uses CallSymbol opcodes.
pub fn set_linked_symbol_table(table: HashMap<String, u32>) {
    let _ = LINKED_SYMBOL_TABLE.set(table);
}

/// Get the linked symbol table.
pub fn get_linked_symbol_table() -> Option<&'static HashMap<String, u32>> {
    LINKED_SYMBOL_TABLE.get()
}

/// Look up a symbol in the linked symbol table.
/// Tries exact match first, then tries with common package prefixes.
fn lookup_symbol(name: &str) -> Option<u32> {
    let table = LINKED_SYMBOL_TABLE.get()?;

    // Try exact match first
    if let Some(&offset) = table.get(name) {
        return Some(offset);
    }

    // Try to find a matching symbol with a package prefix
    // For "Math.add", look for "*.Math.add" patterns
    for (symbol, &offset) in table.iter() {
        // Check if symbol ends with the name we're looking for
        // e.g., "demo.Math.add".ends_with(".Math.add")
        if symbol.ends_with(&format!(".{}", name)) {
            return Some(offset);
        }
        // Also check for exact suffix match for single-segment names
        if symbol == name {
            return Some(offset);
        }
    }

    None
}

/// Get a handle to the current process for symbol lookup.
/// Uses dlopen(NULL, ...) on Unix, GetModuleHandle(NULL) on Windows.
#[cfg(unix)]
fn vm_extern_library_handle() -> *mut std::ffi::c_void {
    use std::sync::atomic::{AtomicPtr, Ordering};
    static HANDLE: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

    let h = HANDLE.load(Ordering::Relaxed);
    if !h.is_null() {
        return h;
    }

    // RTLD_NOW = 0x2 on most Unix systems
    let handle = unsafe { libc::dlopen(std::ptr::null(), libc::RTLD_NOW) };
    if handle.is_null() {
        // Fallback: try RTLD_DEFAULT (pseudo-handle for global symbol lookup)
        return libc::RTLD_DEFAULT;
    }
    HANDLE.store(handle, Ordering::Relaxed);
    handle
}

#[cfg(windows)]
fn vm_extern_library_handle() -> *mut std::ffi::c_void {
    use std::sync::atomic::{AtomicPtr, Ordering};
    static HANDLE: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

    let h = HANDLE.load(Ordering::Relaxed);
    if !h.is_null() {
        return h;
    }

    // GetModuleHandleW(NULL) returns handle to current process
    extern "system" {
        fn GetModuleHandleW(lpModuleName: *const u16) -> *mut std::ffi::c_void;
    }
    let handle = unsafe { GetModuleHandleW(std::ptr::null()) };
    HANDLE.store(handle, Ordering::Relaxed);
    handle
}

#[cfg(unix)]
fn vm_dlsym(handle: *mut std::ffi::c_void, name: &std::ffi::CStr) -> *mut std::ffi::c_void {
    unsafe { libc::dlsym(handle, name.as_ptr()) }
}

#[cfg(windows)]
fn vm_dlsym(handle: *mut std::ffi::c_void, name: &std::ffi::CStr) -> *mut std::ffi::c_void {
    extern "system" {
        fn GetProcAddress(hModule: *mut std::ffi::c_void, lpProcName: *const i8) -> *mut std::ffi::c_void;
    }
    unsafe { GetProcAddress(handle, name.as_ptr()) }
}

/// Optionally load additional shared libraries for `extern "C"` calls.
///
/// Set `ARTH_EXTERN_LIBS` to a platform path-list (uses `std::env::split_paths`)
/// to enable loading external `.so`/`.dylib`/`.dll` files at runtime.
///
/// This keeps handles alive for the lifetime of the process.
#[cfg(unix)]
fn vm_extern_extra_library_handles() -> &'static Vec<usize> {
    use std::os::unix::ffi::OsStrExt;
    static HANDLES: OnceLock<Vec<usize>> = OnceLock::new();
    HANDLES.get_or_init(|| {
        let mut out = Vec::new();
        let Some(var) = std::env::var_os("ARTH_EXTERN_LIBS") else {
            return out;
        };
        for path in std::env::split_paths(&var) {
            let Some(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()).ok() else {
                continue;
            };
            let h = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW) };
            if !h.is_null() {
                out.push(h as usize);
            }
        }
        out
    })
}

#[cfg(windows)]
fn vm_extern_extra_library_handles() -> &'static Vec<usize> {
    use std::os::windows::ffi::OsStrExt;
    static HANDLES: OnceLock<Vec<usize>> = OnceLock::new();
    HANDLES.get_or_init(|| {
        let mut out = Vec::new();
        let Some(var) = std::env::var_os("ARTH_EXTERN_LIBS") else {
            return out;
        };
        extern "system" {
            fn LoadLibraryW(lpLibFileName: *const u16) -> *mut std::ffi::c_void;
        }
        for path in std::env::split_paths(&var) {
            let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
            wide.push(0);
            let h = unsafe { LoadLibraryW(wide.as_ptr()) };
            if !h.is_null() {
                out.push(h as usize);
            }
        }
        out
    })
}

/// Load native libraries from external packages (set via set_external_lib_paths).
#[cfg(unix)]
fn vm_extern_package_library_handles() -> &'static Vec<usize> {
    use std::os::unix::ffi::OsStrExt;
    static HANDLES: OnceLock<Vec<usize>> = OnceLock::new();
    HANDLES.get_or_init(|| {
        let mut out = Vec::new();
        let Some(paths) = EXTERNAL_PACKAGE_LIB_PATHS.get() else {
            return out;
        };
        for path in paths {
            let Some(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()).ok() else {
                continue;
            };
            let h = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW) };
            if !h.is_null() {
                out.push(h as usize);
            }
        }
        out
    })
}

#[cfg(windows)]
fn vm_extern_package_library_handles() -> &'static Vec<usize> {
    use std::os::windows::ffi::OsStrExt;
    static HANDLES: OnceLock<Vec<usize>> = OnceLock::new();
    HANDLES.get_or_init(|| {
        let mut out = Vec::new();
        let Some(paths) = EXTERNAL_PACKAGE_LIB_PATHS.get() else {
            return out;
        };
        extern "system" {
            fn LoadLibraryW(lpLibFileName: *const u16) -> *mut std::ffi::c_void;
        }
        for path in paths {
            let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
            wide.push(0);
            let h = unsafe { LoadLibraryW(wide.as_ptr()) };
            if !h.is_null() {
                out.push(h as usize);
            }
        }
        out
    })
}

fn vm_resolve_extern_symbol(name: &str) -> Result<*const std::ffi::c_void, String> {
    // Cache raw addresses (`usize`) to avoid `*const c_void` in a shared static.
    static CACHE: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(m) = cache.lock()
        && let Some(&addr) = m.get(name)
    {
        return Ok(addr as *const std::ffi::c_void);
    }

    let c_name = std::ffi::CString::new(name)
        .map_err(|_| format!("extern symbol '{name}' contains NUL byte"))?;

    let handle = vm_extern_library_handle();
    let ptr = vm_dlsym(handle, &c_name);

    if ptr.is_null() {
        // Try ARTH_EXTERN_LIBS environment variable
        for &lib in vm_extern_extra_library_handles() {
            let p = vm_dlsym(lib as *mut std::ffi::c_void, &c_name);
            if !p.is_null() {
                if let Ok(mut m) = cache.lock() {
                    m.insert(name.to_string(), p as usize);
                }
                return Ok(p as *const std::ffi::c_void);
            }
        }

        // Try external package libraries (from ~/.arth/libs)
        for &lib in vm_extern_package_library_handles() {
            let p = vm_dlsym(lib as *mut std::ffi::c_void, &c_name);
            if !p.is_null() {
                if let Ok(mut m) = cache.lock() {
                    m.insert(name.to_string(), p as usize);
                }
                return Ok(p as *const std::ffi::c_void);
            }
        }

        return Err(format!(
            "extern symbol '{name}' not found (set ARTH_EXTERN_LIBS or install package with native lib to ~/.arth/libs)"
        ));
    }

    if let Ok(mut m) = cache.lock() {
        m.insert(name.to_string(), ptr as usize);
    }
    Ok(ptr as *const std::ffi::c_void)
}

fn vm_value_to_i64(v: &Value) -> Option<i64> {
    match v {
        Value::I64(n) => Some(*n),
        Value::Bool(b) => Some(if *b { 1 } else { 0 }),
        Value::F64(f) => Some(*f as i64),
        Value::Str(_) => None,
    }
}

fn vm_value_to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::F64(f) => Some(*f),
        Value::I64(n) => Some(*n as f64),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Str(_) => None,
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn vm_call_extern_i64(
    ptr: *const std::ffi::c_void,
    argc: usize,
    float_mask: u32,
    i: &[i64; VM_EXTERN_CALL_MAX_ARGS],
    f: &[f64; VM_EXTERN_CALL_MAX_ARGS],
) -> i64 {
    let mask = if argc == 0 {
        0
    } else {
        float_mask & ((1u32 << (argc as u32)) - 1)
    };
    match argc {
        0 => {
            let fun: unsafe extern "C" fn() -> i64 = std::mem::transmute(ptr);
            fun()
        }
        1 => match mask {
            0 => {
                let fun: unsafe extern "C" fn(i64) -> i64 = std::mem::transmute(ptr);
                fun(i[0])
            }
            1 => {
                let fun: unsafe extern "C" fn(f64) -> i64 = std::mem::transmute(ptr);
                fun(f[0])
            }
            _ => unreachable!(),
        },
        2 => match mask {
            0 => {
                let fun: unsafe extern "C" fn(i64, i64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], i[1])
            }
            1 => {
                let fun: unsafe extern "C" fn(f64, i64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], i[1])
            }
            2 => {
                let fun: unsafe extern "C" fn(i64, f64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], f[1])
            }
            3 => {
                let fun: unsafe extern "C" fn(f64, f64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], f[1])
            }
            _ => unreachable!(),
        },
        3 => match mask {
            0 => {
                let fun: unsafe extern "C" fn(i64, i64, i64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], i[1], i[2])
            }
            1 => {
                let fun: unsafe extern "C" fn(f64, i64, i64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], i[1], i[2])
            }
            2 => {
                let fun: unsafe extern "C" fn(i64, f64, i64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], f[1], i[2])
            }
            3 => {
                let fun: unsafe extern "C" fn(f64, f64, i64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], f[1], i[2])
            }
            4 => {
                let fun: unsafe extern "C" fn(i64, i64, f64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], i[1], f[2])
            }
            5 => {
                let fun: unsafe extern "C" fn(f64, i64, f64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], i[1], f[2])
            }
            6 => {
                let fun: unsafe extern "C" fn(i64, f64, f64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], f[1], f[2])
            }
            7 => {
                let fun: unsafe extern "C" fn(f64, f64, f64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], f[1], f[2])
            }
            _ => unreachable!(),
        },
        4 => match mask {
            0 => {
                let fun: unsafe extern "C" fn(i64, i64, i64, i64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], i[1], i[2], i[3])
            }
            1 => {
                let fun: unsafe extern "C" fn(f64, i64, i64, i64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], i[1], i[2], i[3])
            }
            2 => {
                let fun: unsafe extern "C" fn(i64, f64, i64, i64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], f[1], i[2], i[3])
            }
            3 => {
                let fun: unsafe extern "C" fn(f64, f64, i64, i64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], f[1], i[2], i[3])
            }
            4 => {
                let fun: unsafe extern "C" fn(i64, i64, f64, i64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], i[1], f[2], i[3])
            }
            5 => {
                let fun: unsafe extern "C" fn(f64, i64, f64, i64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], i[1], f[2], i[3])
            }
            6 => {
                let fun: unsafe extern "C" fn(i64, f64, f64, i64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], f[1], f[2], i[3])
            }
            7 => {
                let fun: unsafe extern "C" fn(f64, f64, f64, i64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], f[1], f[2], i[3])
            }
            8 => {
                let fun: unsafe extern "C" fn(i64, i64, i64, f64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], i[1], i[2], f[3])
            }
            9 => {
                let fun: unsafe extern "C" fn(f64, i64, i64, f64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], i[1], i[2], f[3])
            }
            10 => {
                let fun: unsafe extern "C" fn(i64, f64, i64, f64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], f[1], i[2], f[3])
            }
            11 => {
                let fun: unsafe extern "C" fn(f64, f64, i64, f64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], f[1], i[2], f[3])
            }
            12 => {
                let fun: unsafe extern "C" fn(i64, i64, f64, f64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], i[1], f[2], f[3])
            }
            13 => {
                let fun: unsafe extern "C" fn(f64, i64, f64, f64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], i[1], f[2], f[3])
            }
            14 => {
                let fun: unsafe extern "C" fn(i64, f64, f64, f64) -> i64 = std::mem::transmute(ptr);
                fun(i[0], f[1], f[2], f[3])
            }
            15 => {
                let fun: unsafe extern "C" fn(f64, f64, f64, f64) -> i64 = std::mem::transmute(ptr);
                fun(f[0], f[1], f[2], f[3])
            }
            _ => unreachable!(),
        },
        _ => unreachable!(),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn vm_call_extern_f64(
    ptr: *const std::ffi::c_void,
    argc: usize,
    float_mask: u32,
    i: &[i64; VM_EXTERN_CALL_MAX_ARGS],
    f: &[f64; VM_EXTERN_CALL_MAX_ARGS],
) -> f64 {
    let mask = if argc == 0 {
        0
    } else {
        float_mask & ((1u32 << (argc as u32)) - 1)
    };
    match argc {
        0 => {
            let fun: unsafe extern "C" fn() -> f64 = std::mem::transmute(ptr);
            fun()
        }
        1 => match mask {
            0 => {
                let fun: unsafe extern "C" fn(i64) -> f64 = std::mem::transmute(ptr);
                fun(i[0])
            }
            1 => {
                let fun: unsafe extern "C" fn(f64) -> f64 = std::mem::transmute(ptr);
                fun(f[0])
            }
            _ => unreachable!(),
        },
        2 => match mask {
            0 => {
                let fun: unsafe extern "C" fn(i64, i64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], i[1])
            }
            1 => {
                let fun: unsafe extern "C" fn(f64, i64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], i[1])
            }
            2 => {
                let fun: unsafe extern "C" fn(i64, f64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], f[1])
            }
            3 => {
                let fun: unsafe extern "C" fn(f64, f64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], f[1])
            }
            _ => unreachable!(),
        },
        3 => match mask {
            0 => {
                let fun: unsafe extern "C" fn(i64, i64, i64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], i[1], i[2])
            }
            1 => {
                let fun: unsafe extern "C" fn(f64, i64, i64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], i[1], i[2])
            }
            2 => {
                let fun: unsafe extern "C" fn(i64, f64, i64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], f[1], i[2])
            }
            3 => {
                let fun: unsafe extern "C" fn(f64, f64, i64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], f[1], i[2])
            }
            4 => {
                let fun: unsafe extern "C" fn(i64, i64, f64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], i[1], f[2])
            }
            5 => {
                let fun: unsafe extern "C" fn(f64, i64, f64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], i[1], f[2])
            }
            6 => {
                let fun: unsafe extern "C" fn(i64, f64, f64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], f[1], f[2])
            }
            7 => {
                let fun: unsafe extern "C" fn(f64, f64, f64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], f[1], f[2])
            }
            _ => unreachable!(),
        },
        4 => match mask {
            0 => {
                let fun: unsafe extern "C" fn(i64, i64, i64, i64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], i[1], i[2], i[3])
            }
            1 => {
                let fun: unsafe extern "C" fn(f64, i64, i64, i64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], i[1], i[2], i[3])
            }
            2 => {
                let fun: unsafe extern "C" fn(i64, f64, i64, i64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], f[1], i[2], i[3])
            }
            3 => {
                let fun: unsafe extern "C" fn(f64, f64, i64, i64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], f[1], i[2], i[3])
            }
            4 => {
                let fun: unsafe extern "C" fn(i64, i64, f64, i64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], i[1], f[2], i[3])
            }
            5 => {
                let fun: unsafe extern "C" fn(f64, i64, f64, i64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], i[1], f[2], i[3])
            }
            6 => {
                let fun: unsafe extern "C" fn(i64, f64, f64, i64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], f[1], f[2], i[3])
            }
            7 => {
                let fun: unsafe extern "C" fn(f64, f64, f64, i64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], f[1], f[2], i[3])
            }
            8 => {
                let fun: unsafe extern "C" fn(i64, i64, i64, f64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], i[1], i[2], f[3])
            }
            9 => {
                let fun: unsafe extern "C" fn(f64, i64, i64, f64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], i[1], i[2], f[3])
            }
            10 => {
                let fun: unsafe extern "C" fn(i64, f64, i64, f64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], f[1], i[2], f[3])
            }
            11 => {
                let fun: unsafe extern "C" fn(f64, f64, i64, f64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], f[1], i[2], f[3])
            }
            12 => {
                let fun: unsafe extern "C" fn(i64, i64, f64, f64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], i[1], f[2], f[3])
            }
            13 => {
                let fun: unsafe extern "C" fn(f64, i64, f64, f64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], i[1], f[2], f[3])
            }
            14 => {
                let fun: unsafe extern "C" fn(i64, f64, f64, f64) -> f64 = std::mem::transmute(ptr);
                fun(i[0], f[1], f[2], f[3])
            }
            15 => {
                let fun: unsafe extern "C" fn(f64, f64, f64, f64) -> f64 = std::mem::transmute(ptr);
                fun(f[0], f[1], f[2], f[3])
            }
            _ => unreachable!(),
        },
        _ => unreachable!(),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn vm_call_extern_void(
    ptr: *const std::ffi::c_void,
    argc: usize,
    float_mask: u32,
    i: &[i64; VM_EXTERN_CALL_MAX_ARGS],
    f: &[f64; VM_EXTERN_CALL_MAX_ARGS],
) {
    let mask = if argc == 0 {
        0
    } else {
        float_mask & ((1u32 << (argc as u32)) - 1)
    };
    match argc {
        0 => {
            let fun: unsafe extern "C" fn() = std::mem::transmute(ptr);
            fun()
        }
        1 => match mask {
            0 => {
                let fun: unsafe extern "C" fn(i64) = std::mem::transmute(ptr);
                fun(i[0])
            }
            1 => {
                let fun: unsafe extern "C" fn(f64) = std::mem::transmute(ptr);
                fun(f[0])
            }
            _ => unreachable!(),
        },
        2 => match mask {
            0 => {
                let fun: unsafe extern "C" fn(i64, i64) = std::mem::transmute(ptr);
                fun(i[0], i[1])
            }
            1 => {
                let fun: unsafe extern "C" fn(f64, i64) = std::mem::transmute(ptr);
                fun(f[0], i[1])
            }
            2 => {
                let fun: unsafe extern "C" fn(i64, f64) = std::mem::transmute(ptr);
                fun(i[0], f[1])
            }
            3 => {
                let fun: unsafe extern "C" fn(f64, f64) = std::mem::transmute(ptr);
                fun(f[0], f[1])
            }
            _ => unreachable!(),
        },
        3 => match mask {
            0 => {
                let fun: unsafe extern "C" fn(i64, i64, i64) = std::mem::transmute(ptr);
                fun(i[0], i[1], i[2])
            }
            1 => {
                let fun: unsafe extern "C" fn(f64, i64, i64) = std::mem::transmute(ptr);
                fun(f[0], i[1], i[2])
            }
            2 => {
                let fun: unsafe extern "C" fn(i64, f64, i64) = std::mem::transmute(ptr);
                fun(i[0], f[1], i[2])
            }
            3 => {
                let fun: unsafe extern "C" fn(f64, f64, i64) = std::mem::transmute(ptr);
                fun(f[0], f[1], i[2])
            }
            4 => {
                let fun: unsafe extern "C" fn(i64, i64, f64) = std::mem::transmute(ptr);
                fun(i[0], i[1], f[2])
            }
            5 => {
                let fun: unsafe extern "C" fn(f64, i64, f64) = std::mem::transmute(ptr);
                fun(f[0], i[1], f[2])
            }
            6 => {
                let fun: unsafe extern "C" fn(i64, f64, f64) = std::mem::transmute(ptr);
                fun(i[0], f[1], f[2])
            }
            7 => {
                let fun: unsafe extern "C" fn(f64, f64, f64) = std::mem::transmute(ptr);
                fun(f[0], f[1], f[2])
            }
            _ => unreachable!(),
        },
        4 => match mask {
            0 => {
                let fun: unsafe extern "C" fn(i64, i64, i64, i64) = std::mem::transmute(ptr);
                fun(i[0], i[1], i[2], i[3])
            }
            1 => {
                let fun: unsafe extern "C" fn(f64, i64, i64, i64) = std::mem::transmute(ptr);
                fun(f[0], i[1], i[2], i[3])
            }
            2 => {
                let fun: unsafe extern "C" fn(i64, f64, i64, i64) = std::mem::transmute(ptr);
                fun(i[0], f[1], i[2], i[3])
            }
            3 => {
                let fun: unsafe extern "C" fn(f64, f64, i64, i64) = std::mem::transmute(ptr);
                fun(f[0], f[1], i[2], i[3])
            }
            4 => {
                let fun: unsafe extern "C" fn(i64, i64, f64, i64) = std::mem::transmute(ptr);
                fun(i[0], i[1], f[2], i[3])
            }
            5 => {
                let fun: unsafe extern "C" fn(f64, i64, f64, i64) = std::mem::transmute(ptr);
                fun(f[0], i[1], f[2], i[3])
            }
            6 => {
                let fun: unsafe extern "C" fn(i64, f64, f64, i64) = std::mem::transmute(ptr);
                fun(i[0], f[1], f[2], i[3])
            }
            7 => {
                let fun: unsafe extern "C" fn(f64, f64, f64, i64) = std::mem::transmute(ptr);
                fun(f[0], f[1], f[2], i[3])
            }
            8 => {
                let fun: unsafe extern "C" fn(i64, i64, i64, f64) = std::mem::transmute(ptr);
                fun(i[0], i[1], i[2], f[3])
            }
            9 => {
                let fun: unsafe extern "C" fn(f64, i64, i64, f64) = std::mem::transmute(ptr);
                fun(f[0], i[1], i[2], f[3])
            }
            10 => {
                let fun: unsafe extern "C" fn(i64, f64, i64, f64) = std::mem::transmute(ptr);
                fun(i[0], f[1], i[2], f[3])
            }
            11 => {
                let fun: unsafe extern "C" fn(f64, f64, i64, f64) = std::mem::transmute(ptr);
                fun(f[0], f[1], i[2], f[3])
            }
            12 => {
                let fun: unsafe extern "C" fn(i64, i64, f64, f64) = std::mem::transmute(ptr);
                fun(i[0], i[1], f[2], f[3])
            }
            13 => {
                let fun: unsafe extern "C" fn(f64, i64, f64, f64) = std::mem::transmute(ptr);
                fun(f[0], i[1], f[2], f[3])
            }
            14 => {
                let fun: unsafe extern "C" fn(i64, f64, f64, f64) = std::mem::transmute(ptr);
                fun(i[0], f[1], f[2], f[3])
            }
            15 => {
                let fun: unsafe extern "C" fn(f64, f64, f64, f64) = std::mem::transmute(ptr);
                fun(f[0], f[1], f[2], f[3])
            }
            _ => unreachable!(),
        },
        _ => unreachable!(),
    }
}

/// Run a program without capability checking (for backward compatibility).
///
/// WARNING: This function does not enforce host capabilities. Programs can
/// access all host functions (IO, network, DB, mail, crypto) without restriction.
/// For sandboxed execution, use `run_program_with_host()` instead.
pub fn run_program(p: &Program) -> i32 {
    match run_program_internal(p, None, None) {
        InterpreterResult::ExitCode(code) => code,
        InterpreterResult::Failed(code) => code,
        InterpreterResult::ReturnValue(_) => 0, // Shouldn't happen for run_program
    }
}

/// Internal interpreter that optionally enforces host capabilities.
///
/// When `ctx` is Some, host calls are checked against the context's capability
/// configuration. When `ctx` is None, all host calls are allowed (legacy behavior).
///
/// When `export_config` is Some, the interpreter runs in "export call" mode:
/// - Starts execution at the specified offset
/// - Sets up function arguments in the initial frame
/// - Returns the function's return value instead of an exit code
fn run_program_internal(
    p: &Program,
    ctx: Option<&HostContext>,
    export_config: Option<ExportCallConfig>,
) -> InterpreterResult {
    // Determine if we're in export call mode
    let is_export_call = export_config.is_some();
    let (start_offset, args) = export_config
        .map(|c| (c.start_offset, c.args))
        .unwrap_or((0, &[][..]));

    let mut ip = start_offset;
    let mut stack: Vec<Value> = Vec::new();
    // Call frames: each has its own locals
    let mut frames: Vec<Vec<Option<Value>>> = vec![Vec::new()];
    let mut ret_stack: Vec<usize> = Vec::new();
    // Instant handles for time measurement (HostCallTime)
    let mut instants: Vec<std::time::Instant> = Vec::new();

    // For export calls, set up the initial frame with arguments
    if is_export_call && !args.is_empty() {
        let local_count = scan_function_local_count(p, start_offset as u32);
        let mut initial_frame: Vec<Option<Value>> = vec![None; local_count as usize];

        // Check for prologue pattern - if function starts with LocalSet, it has a prologue
        let first_op = p.code.get(start_offset);
        let has_prologue = matches!(first_op, Some(Op::LocalSet(_)));

        // Also find first_local_get regardless of prologue for debugging
        let first_local_get_for_debug = find_first_local_get(p, start_offset as u32);

        // Debug: print first 10 opcodes
        eprintln!("[VM_DEBUG] export call: start_offset={}, local_count={}, has_prologue={}, args={:?}",
            start_offset, local_count, has_prologue, args);
        eprintln!("[VM_DEBUG] first_local_get={:?}, first 10 opcodes:", first_local_get_for_debug);
        for i in 0..10 {
            if let Some(op) = p.code.get(start_offset + i) {
                eprintln!("[VM_DEBUG]   [{}]: {:?}", start_offset + i, op);
            }
        }

        if !has_prologue {
            // Find where the first argument is expected
            let first_local_get = find_first_local_get(p, start_offset as u32);
            let target_local = first_local_get.unwrap_or(0) as usize;

            eprintln!("[VM_DEBUG] no prologue: first_local_get={:?}, target_local={}, frame_len={}",
                first_local_get, target_local, initial_frame.len());

            // Place arguments in locals starting at target_local
            for (i, arg) in args.iter().enumerate() {
                let local_idx = target_local + i;
                if local_idx < initial_frame.len() {
                    eprintln!("[VM_DEBUG] placing arg[{}]='{}' into local[{}]", i, arg, local_idx);
                    initial_frame[local_idx] = Some(Value::Str((*arg).to_string()));
                } else {
                    eprintln!("[VM_DEBUG] ERROR: local_idx {} >= frame_len {}, arg '{}' DROPPED!", local_idx, initial_frame.len(), arg);
                }
            }
        } else {
            // Push arguments onto stack in reverse order (callee will pop them)
            eprintln!("[VM_DEBUG] has prologue: pushing {} args to stack", args.len());
            for arg in args.iter().rev() {
                stack.push(Value::Str((*arg).to_string()));
            }
        }

        frames[0] = initial_frame;
    }

    // Safety: instruction and stack budgets to prevent non-terminating or
    // unbounded runs from hanging the host. These are intentionally coarse
    // and can be tuned per-environment via env vars so that TS guest code
    // can run under stricter limits (see docs/ts-subset.md §6.4).
    let max_steps: u64 = std::env::var("ARTH_VM_MAX_STEPS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(2_000_000);
    let max_stack_depth: usize = std::env::var("ARTH_VM_MAX_STACK_DEPTH")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1_000_000);
    // frames.len() includes the top-level frame; treat additional frames as call depth.
    let max_call_depth: usize = std::env::var("ARTH_VM_MAX_CALL_DEPTH")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(64_000);
    let mut steps: u64 = 0;
    while ip < p.code.len() {
        steps += 1;
        if steps > max_steps {
            eprintln!(
                "vm: aborted after {} steps (possible infinite loop). Set ARTH_VM_MAX_STEPS to increase.",
                max_steps
            );
            return InterpreterResult::Failed(1);
        }
        if stack.len() > max_stack_depth {
            eprintln!(
                "vm: aborted after growing stack to {} values (limit {}). Set ARTH_VM_MAX_STACK_DEPTH to increase.",
                stack.len(),
                max_stack_depth
            );
            return InterpreterResult::Failed(1);
        }
        let call_depth = frames.len().saturating_sub(1);
        if call_depth > max_call_depth {
            eprintln!(
                "vm: aborted after exceeding call depth {} (limit {}). Set ARTH_VM_MAX_CALL_DEPTH to increase.",
                call_depth, max_call_depth
            );
            return InterpreterResult::Failed(1);
        }
        // Instruction trace - enabled via ARTH_VM_TRACE=1 or ARTH_VM_TRACE=<limit>
        let trace_limit: u64 = std::env::var("ARTH_VM_TRACE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if trace_limit > 0 && steps < trace_limit {
            eprintln!("[VM TRACE] ip={} op={:?} stack.len={}", ip, &p.code[ip], stack.len());
        }
        // DEBUG: Trace critical region around ip=1084 where Optional handle is produced
        if ip >= 1082 && ip <= 1086 {
            eprintln!("[VM_DEBUG_CRITICAL] ip={} op={:?} stack.len={}", ip, &p.code[ip], stack.len());
        }
        // DEBUG: Trace findById function execution (offset 704-787)
        if ip >= 704 && ip < 787 {
            eprintln!("[VM_FINDBYID] ip={} op={:?} stack.len={}", ip, &p.code[ip], stack.len());
        }
        let current_op = p.code[ip].clone();
        let current_ip = ip;
        // Helper macro to log errors before returning
        macro_rules! vm_error {
            ($msg:expr) => {{
                eprintln!("[VM ERROR] {} at ip={} op={:?}", $msg, current_ip, current_op);
                return InterpreterResult::Failed(1);
            }};
        }
        match current_op {
            Op::Print(ix) => {
                let i = ix as usize;
                if let Some(s) = p.strings.get(i) {
                    // Use stderr to avoid polluting stdout (used for JSON-RPC)
                    eprintln!("{}", s);
                }
                ip += 1;
            }
            Op::Halt => break,
            Op::PrintStrVal(ix) => {
                // Use stderr to avoid polluting stdout (used for JSON-RPC)
                let i = ix as usize;
                let s = p.strings.get(i).map(|x| x.as_str()).unwrap_or("");
                match stack.pop() {
                    Some(Value::I64(n)) => eprintln!("{}{}", s, n),
                    Some(Value::F64(f)) => {
                        if f.is_finite() && (f.fract() == 0.0) {
                            eprintln!("{}{}", s, format!("{:.1}", f));
                        } else {
                            eprintln!("{}{}", s, f);
                        }
                    }
                    Some(Value::Bool(b)) => eprintln!("{}{}", s, if b { "true" } else { "false" }),
                    Some(Value::Str(t)) => eprintln!("{}{}", s, t),
                    None => eprintln!("{}", s),
                }
                ip += 1;
            }
            Op::PushStr(ix) => {
                let i = ix as usize;
                let s = p.strings.get(i).cloned().unwrap_or_default();
                // DEBUG: Trace string pushes in findById range
                if ip >= 704 && ip < 787 {
                    eprintln!("[VM_FINDBYID] PushStr({}) at ip={}: value='{}'", ix, ip, &s);
                }
                stack.push(Value::Str(s));
                ip += 1;
            }
            Op::PushI64(n) => {
                stack.push(Value::I64(n));
                ip += 1;
            }
            Op::PushF64(x) => {
                stack.push(Value::F64(x));
                ip += 1;
            }
            Op::PushBool(b) => {
                stack.push(Value::Bool(b != 0));
                ip += 1;
            }
            Op::AddI64 => {
                let (Some(vb), Some(va)) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                match (&va, &vb) {
                    // String concatenation: if either operand is a string, concatenate
                    (Value::Str(a), Value::Str(b)) => {
                        stack.push(Value::Str(format!("{}{}", a, b)));
                    }
                    (Value::Str(a), b) => {
                        // Convert non-string to string and concatenate
                        let b_str = match b {
                            Value::I64(n) => n.to_string(),
                            Value::F64(f) => f.to_string(),
                            Value::Bool(x) => if *x { "true" } else { "false" }.to_string(),
                            Value::Str(s) => s.clone(),
                        };
                        stack.push(Value::Str(format!("{}{}", a, b_str)));
                    }
                    (a, Value::Str(b)) => {
                        // Convert non-string to string and concatenate
                        let a_str = match a {
                            Value::I64(n) => n.to_string(),
                            Value::F64(f) => f.to_string(),
                            Value::Bool(x) => if *x { "true" } else { "false" }.to_string(),
                            Value::Str(s) => s.clone(),
                        };
                        stack.push(Value::Str(format!("{}{}", a_str, b)));
                    }
                    // Numeric addition
                    (Value::I64(a), Value::I64(b)) => {
                        // Integer arithmetic is defined as two's-complement wrapping.
                        stack.push(Value::I64(a.wrapping_add(*b)))
                    }
                    (a, b) => {
                        // numeric promotion to f64 if any operand is non-int
                        let af = match a {
                            Value::F64(f) => *f,
                            Value::I64(n) => *n as f64,
                            Value::Bool(x) => {
                                if *x {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            Value::Str(_) => 0.0, // Should not reach here after string checks
                        };
                        let bf = match b {
                            Value::F64(f) => *f,
                            Value::I64(n) => *n as f64,
                            Value::Bool(x) => {
                                if *x {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            Value::Str(_) => 0.0, // Should not reach here after string checks
                        };
                        stack.push(Value::F64(af + bf));
                    }
                }
                ip += 1;
            }
            Op::SubI64 => {
                let (Some(vb), Some(va)) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                match (va, vb) {
                    (Value::I64(a), Value::I64(b)) => {
                        // Integer arithmetic is defined as two's-complement wrapping.
                        stack.push(Value::I64(a.wrapping_sub(b)))
                    }
                    (a, b) => {
                        let af = match a {
                            Value::F64(f) => f,
                            Value::I64(n) => n as f64,
                            Value::Bool(x) => {
                                if x {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            Value::Str(_) => 0.0,
                        };
                        let bf = match b {
                            Value::F64(f) => f,
                            Value::I64(n) => n as f64,
                            Value::Bool(x) => {
                                if x {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            Value::Str(_) => 0.0,
                        };
                        stack.push(Value::F64(af - bf));
                    }
                }
                ip += 1;
            }
            Op::MulI64 => {
                let (Some(vb), Some(va)) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                match (va, vb) {
                    (Value::I64(a), Value::I64(b)) => {
                        // Integer arithmetic is defined as two's-complement wrapping.
                        stack.push(Value::I64(a.wrapping_mul(b)))
                    }
                    (a, b) => {
                        let af = match a {
                            Value::F64(f) => f,
                            Value::I64(n) => n as f64,
                            Value::Bool(x) => {
                                if x {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            Value::Str(_) => 0.0,
                        };
                        let bf = match b {
                            Value::F64(f) => f,
                            Value::I64(n) => n as f64,
                            Value::Bool(x) => {
                                if x {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            Value::Str(_) => 0.0,
                        };
                        stack.push(Value::F64(af * bf));
                    }
                }
                ip += 1;
            }
            Op::DivI64 => {
                let (Some(vb), Some(va)) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                match (va, vb) {
                    (Value::I64(a), Value::I64(b)) => {
                        if b == 0 {
                            panic_with_message("division by zero".to_string());
                            return InterpreterResult::Failed(2);
                        } else {
                            // Division by zero panics; otherwise integer division is
                            // two's-complement wrapping for overflow cases.
                            stack.push(Value::I64(a.wrapping_div(b)));
                        }
                    }
                    (a, b) => {
                        let af = match a {
                            Value::F64(f) => f,
                            Value::I64(n) => n as f64,
                            Value::Bool(x) => {
                                if x {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            Value::Str(_) => 0.0,
                        };
                        let bf = match b {
                            Value::F64(f) => f,
                            Value::I64(n) => n as f64,
                            Value::Bool(x) => {
                                if x {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            Value::Str(_) => 0.0,
                        };
                        if bf == 0.0 {
                            panic_with_message("division by zero".to_string());
                            return InterpreterResult::Failed(2);
                        } else {
                            stack.push(Value::F64(af / bf));
                        }
                    }
                }
                ip += 1;
            }
            Op::ModI64 => {
                let (Some(vb), Some(va)) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                match (va, vb) {
                    (Value::I64(a), Value::I64(b)) => {
                        if b == 0 {
                            panic_with_message("modulo by zero".to_string());
                            return InterpreterResult::Failed(2);
                        } else {
                            // Modulo by zero panics; otherwise modulo is
                            // two's-complement wrapping for overflow cases.
                            stack.push(Value::I64(a.wrapping_rem(b)));
                        }
                    }
                    (a, b) => {
                        let af = match a {
                            Value::F64(f) => f,
                            Value::I64(n) => n as f64,
                            Value::Bool(x) => {
                                if x {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            Value::Str(_) => 0.0,
                        };
                        let bf = match b {
                            Value::F64(f) => f,
                            Value::I64(n) => n as f64,
                            Value::Bool(x) => {
                                if x {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            Value::Str(_) => 0.0,
                        };
                        if bf == 0.0 {
                            panic_with_message("modulo by zero".to_string());
                            return InterpreterResult::Failed(2);
                        } else {
                            stack.push(Value::F64(af % bf));
                        }
                    }
                }
                ip += 1;
            }
            Op::LtI64 => {
                let (Some(vb), Some(va)) = (stack.pop(), stack.pop()) else {
                    vm_error!("LtI64: stack underflow");
                };
                // Support mixed numeric types and strings (for WAID character comparison)
                let result = match (&va, &vb) {
                    (Value::I64(a), Value::I64(b)) => *a < *b,
                    (Value::F64(a), Value::F64(b)) => *a < *b,
                    (Value::I64(a), Value::F64(b)) => (*a as f64) < *b,
                    (Value::F64(a), Value::I64(b)) => *a < (*b as f64),
                    // Support string comparison (WAID uses LtI64 for charAt results)
                    (Value::Str(a), Value::Str(b)) => a < b,
                    _ => {
                        vm_error!(format!("LtI64: type mismatch: {:?} < {:?}", va, vb));
                    }
                };
                stack.push(Value::Bool(result));
                ip += 1;
            }
            Op::EqI64 => {
                let (Some(vb), Some(va)) = (stack.pop(), stack.pop()) else {
                    vm_error!("EqI64: stack underflow");
                };
                // Support mixed numeric types and strings
                let result = match (&va, &vb) {
                    (Value::I64(a), Value::I64(b)) => *a == *b,
                    (Value::F64(a), Value::F64(b)) => *a == *b,
                    (Value::I64(a), Value::F64(b)) => (*a as f64) == *b,
                    (Value::F64(a), Value::I64(b)) => *a == (*b as f64),
                    (Value::Bool(a), Value::Bool(b)) => *a == *b,
                    (Value::Str(a), Value::Str(b)) => a == b,
                    _ => {
                        vm_error!(format!("EqI64: type mismatch: {:?} == {:?}", va, vb));
                    }
                };
                stack.push(Value::Bool(result));
                ip += 1;
            }
            Op::EqStr => {
                // String equality comparison.
                // Note: Strings are always on the stack as Value::Str (loaded via PushStr).
                // Value::I64 here is an actual numeric value, NOT a string pool index.
                // Only Value::Str values can be compared for string equality.
                let (Some(b_val), Some(a_val)) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                // DEBUG: Trace string comparisons in findById range
                if ip >= 704 && ip < 787 {
                    eprintln!("[VM_FINDBYID] EqStr at ip={}: a={:?}, b={:?}", ip, &a_val, &b_val);
                }
                let result = match (&a_val, &b_val) {
                    (Value::Str(a), Value::Str(b)) => a == b,
                    // Type mismatch - different types are never equal
                    _ => false,
                };
                // DEBUG: Trace comparison result in findById range
                if ip >= 704 && ip < 787 {
                    eprintln!("[VM_FINDBYID] EqStr result: {}", result);
                }
                stack.push(Value::Bool(result));
                ip += 1;
            }
            Op::ConcatStr => {
                // Helper to convert a Value to its string representation.
                // Note: Strings are always on the stack as Value::Str (loaded via PushStr).
                // Value::I64 here is an actual numeric value, NOT a string pool index.
                fn value_to_string(v: &Value) -> String {
                    match v {
                        Value::Str(s) => s.clone(),
                        Value::I64(n) => n.to_string(),
                        Value::F64(f) => {
                            if f.is_finite() && f.fract() == 0.0 {
                                format!("{:.1}", f)
                            } else {
                                f.to_string()
                            }
                        }
                        Value::Bool(b) => {
                            if *b {
                                "true".to_string()
                            } else {
                                "false".to_string()
                            }
                        }
                    }
                }
                let (Some(b_val), Some(a_val)) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                let sa = value_to_string(&a_val);
                let sb = value_to_string(&b_val);
                let result = format!("{}{}", sa, sb);
                stack.push(Value::Str(result));
                ip += 1;
            }
            Op::ShlI64 => {
                let (Some(Value::I64(b)), Some(Value::I64(a))) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                // Shift amount is masked to 0..63 for stable behavior.
                let shift = (b as u32) & 63;
                stack.push(Value::I64(a.wrapping_shl(shift)));
                ip += 1;
            }
            Op::ShrI64 => {
                let (Some(Value::I64(b)), Some(Value::I64(a))) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                // Shift amount is masked to 0..63 for stable behavior.
                let shift = (b as u32) & 63;
                stack.push(Value::I64(a.wrapping_shr(shift)));
                ip += 1;
            }
            Op::AndI64 => {
                let (Some(Value::I64(b)), Some(Value::I64(a))) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                stack.push(Value::I64(a & b));
                ip += 1;
            }
            Op::OrI64 => {
                let (Some(Value::I64(b)), Some(Value::I64(a))) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                stack.push(Value::I64(a | b));
                ip += 1;
            }
            Op::XorI64 => {
                let (Some(Value::I64(b)), Some(Value::I64(a))) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                stack.push(Value::I64(a ^ b));
                ip += 1;
            }
            // --- List ops (6 core intrinsics) ---
            // Removed ops are now pure Arth code in stdlib/src/arth/array.arth
            Op::ListNew => {
                let h = list_new();
                // DEBUG: Trace list creation
                eprintln!("[VM_DEBUG] ListNew at ip={}: created handle={}", ip, h);
                stack.push(Value::I64(h));
                ip += 1;
            }
            Op::ListPush => {
                let (Some(v), Some(Value::I64(h))) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                // DEBUG: Trace list push operations
                eprintln!("[VM_DEBUG] ListPush at ip={}: handle={}, value={:?}", ip, h, &v);
                let len = list_push(h, v);
                stack.push(Value::I64(len as i64));
                ip += 1;
            }
            Op::ListGet => {
                // Pop index (allow both I64 and F64 for TypeScript compatibility)
                let idx_val = stack.pop();
                let idx: i64 = match idx_val {
                    Some(Value::I64(i)) => i,
                    Some(Value::F64(f)) => f as i64,
                    _ => return InterpreterResult::Failed(1),
                };
                // Pop list handle
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                // DEBUG: Trace list get operations in findById range
                if ip >= 704 && ip <= 850 {
                    eprintln!("[VM_DEBUG] ListGet at ip={}: handle={}, idx={}", ip, h, idx);
                }
                match list_get(h, idx as usize) {
                    Some(v) => {
                        if ip >= 704 && ip <= 850 {
                            eprintln!("[VM_DEBUG] ListGet result: {:?}", &v);
                        }
                        stack.push(v)
                    }
                    None => {
                        let len = list_len(h);
                        panic_with_message(format!(
                            "list index out of bounds: index {} but length is {}",
                            idx, len
                        ));
                        return InterpreterResult::Failed(2);
                    }
                }
                ip += 1;
            }
            Op::ListSet => {
                let (Some(val), Some(Value::I64(idx)), Some(Value::I64(h))) =
                    (stack.pop(), stack.pop(), stack.pop())
                else {
                    return InterpreterResult::Failed(1);
                };
                if !list_set(h, idx as usize, val) {
                    let len = list_len(h);
                    panic_with_message(format!(
                        "list set index out of bounds: index {} but length is {}",
                        idx, len
                    ));
                    return InterpreterResult::Failed(2);
                }
                stack.push(Value::I64(0)); // return 0 to indicate success
                ip += 1;
            }
            Op::ListLen => {
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let len = list_len(h) as i64;
                // DEBUG: Trace list length operations in findById range (ip 704-800)
                if ip >= 704 && ip <= 850 {
                    eprintln!("[VM_DEBUG] ListLen at ip={}: handle={}, len={}", ip, h, len);
                }
                stack.push(Value::I64(len));
                ip += 1;
            }
            Op::ListRemove => {
                let (Some(Value::I64(idx)), Some(Value::I64(h))) = (stack.pop(), stack.pop())
                else {
                    return InterpreterResult::Failed(1);
                };
                match list_remove(h, idx as usize) {
                    Some(v) => stack.push(v),
                    None => {
                        let len = list_len(h);
                        panic_with_message(format!(
                            "list remove index out of bounds: index {} but length is {}",
                            idx, len
                        ));
                        return InterpreterResult::Failed(2);
                    }
                }
                ip += 1;
            }
            Op::ListSort => {
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                list_sort(h);
                stack.push(Value::I64(0));
                ip += 1;
            }
            // --- Map ops (7 core intrinsics) ---
            // Removed ops are now pure Arth code in stdlib/src/arth/map.arth
            Op::MapNew => {
                let h = map_new();
                stack.push(Value::I64(h));
                ip += 1;
            }
            Op::MapPut => {
                let (Some(val), Some(key), Some(Value::I64(h))) =
                    (stack.pop(), stack.pop(), stack.pop())
                else {
                    return InterpreterResult::Failed(1);
                };
                let _ = map_put(h, key, val);
                stack.push(Value::I64(0));
                ip += 1;
            }
            Op::MapGet => {
                let (Some(key), Some(Value::I64(h))) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                match map_get(h, &key) {
                    Some(v) => stack.push(v),
                    None => {
                        let key_str = match &key {
                            Value::Str(s) => format!("\"{}\"", s),
                            Value::I64(n) => n.to_string(),
                            Value::F64(f) => f.to_string(),
                            Value::Bool(b) => b.to_string(),
                        };
                        panic_with_message(format!("map key not found: {}", key_str));
                        return InterpreterResult::Failed(2);
                    }
                }
                ip += 1;
            }
            Op::MapLen => {
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let len = map_len(h) as i64;
                stack.push(Value::I64(len));
                ip += 1;
            }
            Op::MapContainsKey => {
                let (Some(key), Some(Value::I64(h))) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                let found = map_contains_key(h, &key);
                stack.push(Value::I64(if found { 1 } else { 0 }));
                ip += 1;
            }
            Op::MapRemove => {
                let (Some(key), Some(Value::I64(h))) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                match map_remove(h, &key) {
                    Some(v) => stack.push(v),
                    None => {
                        let key_str = match &key {
                            Value::Str(s) => format!("\"{}\"", s),
                            Value::I64(n) => n.to_string(),
                            Value::F64(f) => f.to_string(),
                            Value::Bool(b) => b.to_string(),
                        };
                        panic_with_message(format!("map remove key not found: {}", key_str));
                        return InterpreterResult::Failed(2);
                    }
                }
                ip += 1;
            }
            Op::MapKeys => {
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let list_h = map_keys(h);
                stack.push(Value::I64(list_h));
                ip += 1;
            }
            Op::MapMerge => {
                // Pop src, dest from stack (dest pushed first, so popped second)
                let Some(Value::I64(src)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(Value::I64(dest)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let result = map_merge(dest, src);
                stack.push(Value::I64(result));
                ip += 1;
            }
            // --- String operations (18 core intrinsics) ---
            Op::StrLen => {
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let len = s.chars().count() as i64;
                stack.push(Value::I64(len));
                ip += 1;
            }
            Op::StrSubstring => {
                let Some(Value::I64(end)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(Value::I64(start)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let chars: Vec<char> = s.chars().collect();
                let start = start.max(0) as usize;
                let end = end.max(0) as usize;
                let result: String = if start < chars.len() {
                    chars[start..end.min(chars.len())].iter().collect()
                } else {
                    String::new()
                };
                stack.push(Value::Str(result));
                ip += 1;
            }
            Op::StrIndexOf => {
                let search = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let result = s.find(&search).map_or(-1i64, |i| i as i64);
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::StrLastIndexOf => {
                let search = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let result = s.rfind(&search).map_or(-1i64, |i| i as i64);
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::StrStartsWith => {
                let prefix = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let result = if s.starts_with(&prefix) { 1i64 } else { 0i64 };
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::StrEndsWith => {
                let suffix = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let result = if s.ends_with(&suffix) { 1i64 } else { 0i64 };
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::StrSplit => {
                let delimiter = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let parts: Vec<&str> = s.split(&delimiter).collect();
                let list_h = list_new();
                for part in parts {
                    list_push(list_h, Value::Str(part.to_string()));
                }
                stack.push(Value::I64(list_h));
                ip += 1;
            }
            Op::StrTrim => {
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                stack.push(Value::Str(s.trim().to_string()));
                ip += 1;
            }
            Op::StrToLower => {
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                stack.push(Value::Str(s.to_lowercase()));
                ip += 1;
            }
            Op::StrToUpper => {
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                stack.push(Value::Str(s.to_uppercase()));
                ip += 1;
            }
            Op::StrReplace => {
                let new_str = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let old_str = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                stack.push(Value::Str(s.replace(&old_str, &new_str)));
                ip += 1;
            }
            Op::StrCharAt => {
                let Some(Value::I64(idx)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(str_idx)) => {
                        p.strings.get(str_idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let char_count = s.chars().count();
                if idx < 0 || (idx as usize) >= char_count {
                    panic_with_message(format!(
                        "string index out of bounds: index {} but length is {}",
                        idx, char_count
                    ));
                    return InterpreterResult::Failed(2);
                }
                let result = s.chars().nth(idx as usize).unwrap() as i64;
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::StrContains => {
                let search = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let result = if s.contains(&search) { 1i64 } else { 0i64 };
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::StrRepeat => {
                let Some(Value::I64(count)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let result = if count > 0 {
                    s.repeat(count as usize)
                } else {
                    String::new()
                };
                stack.push(Value::Str(result));
                ip += 1;
            }
            Op::StrParseInt => {
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let result = s.trim().parse::<i64>().unwrap_or(0);
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::StrParseFloat => {
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                let result = s.trim().parse::<f64>().unwrap_or(0.0);
                stack.push(Value::F64(result));
                ip += 1;
            }
            Op::StrFromInt => {
                let Some(Value::I64(n)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                stack.push(Value::Str(n.to_string()));
                ip += 1;
            }
            Op::StrFromFloat => {
                let f = match stack.pop() {
                    Some(Value::F64(f)) => f,
                    Some(Value::I64(n)) => n as f64,
                    _ => return InterpreterResult::Failed(1),
                };
                stack.push(Value::Str(f.to_string()));
                ip += 1;
            }
            // --- Optional operations ---
            Op::OptSome => {
                let Some(val) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let h = opt_some(val);
                stack.push(Value::I64(h));
                ip += 1;
            }
            Op::OptNone => {
                let h = opt_none();
                stack.push(Value::I64(h));
                ip += 1;
            }
            Op::OptIsSome => {
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let result = if opt_is_some(h) { 1i64 } else { 0i64 };
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::OptUnwrap => {
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let val = opt_unwrap(h);
                stack.push(val);
                ip += 1;
            }
            Op::OptOrElse => {
                // Pop default, then optional handle
                let Some(default) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let val = opt_or_else(h, default);
                stack.push(val);
                ip += 1;
            }
            // --- Native Struct operations ---
            Op::StructNew => {
                // Pop field_count, then type_name_idx
                let Some(Value::I64(field_count)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let type_name = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => p.strings.get(idx as usize).cloned().unwrap_or_default(),
                    _ => String::new(),
                };
                let h = struct_new(type_name, field_count as usize);
                stack.push(Value::I64(h));
                ip += 1;
            }
            Op::StructSet => {
                // Pop field_name_idx, value, field_idx, struct_handle
                let field_name = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => p.strings.get(idx as usize).cloned().unwrap_or_default(),
                    _ => String::new(),
                };
                let Some(val) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(Value::I64(field_idx)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                struct_set(h, field_idx as usize, val, field_name);
                stack.push(Value::I64(0));
                ip += 1;
            }
            Op::StructGet => {
                // Pop field_idx, struct_handle
                let Some(Value::I64(field_idx)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let val = struct_get(h, field_idx as usize);
                stack.push(val);
                ip += 1;
            }
            Op::StructGetNamed => {
                // Pop field_name_idx, struct_handle
                let field_name = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => p.strings.get(idx as usize).cloned().unwrap_or_default(),
                    _ => String::new(),
                };
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                // DEBUG: Trace StructGetNamed for common fields
                let is_interesting = field_name == "subject" || field_name == "sender" || field_name == "body" || field_name == "preview" || field_name == "date" || field_name == "id";
                if is_interesting {
                    eprintln!("[VM_DEBUG] StructGetNamed at ip={}: handle={}, field='{}'", ip, h, field_name);
                }
                let val = struct_get_named(h, &field_name);
                if is_interesting {
                    eprintln!("[VM_DEBUG] StructGetNamed result: {:?}", &val);
                }
                stack.push(val);
                ip += 1;
            }
            Op::StructSetNamed => {
                // Pop value, field_name_idx, struct_handle
                let Some(val) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let field_name = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => p.strings.get(idx as usize).cloned().unwrap_or_default(),
                    _ => String::new(),
                };
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                struct_set_named(h, &field_name, val);
                stack.push(Value::I64(0));
                ip += 1;
            }
            Op::StructCopy => {
                // Pop src_handle, dest_handle
                let Some(Value::I64(src)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(Value::I64(dest)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                struct_copy(dest, src);
                stack.push(Value::I64(0));
                ip += 1;
            }
            Op::StructTypeName => {
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let name = struct_type_name(h);
                stack.push(Value::Str(name));
                ip += 1;
            }
            Op::StructFieldCount => {
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let count = struct_field_count(h) as i64;
                stack.push(Value::I64(count));
                ip += 1;
            }
            // --- Native Enum operations ---
            Op::EnumNew => {
                // Pop payload_count, tag, variant_name_idx, enum_name_idx
                let Some(Value::I64(payload_count)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(Value::I64(tag)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let variant_name = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => p.strings.get(idx as usize).cloned().unwrap_or_default(),
                    _ => String::new(),
                };
                let enum_name = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => p.strings.get(idx as usize).cloned().unwrap_or_default(),
                    _ => String::new(),
                };
                let h = enum_new(enum_name, variant_name, tag, payload_count as usize);
                stack.push(Value::I64(h));
                ip += 1;
            }
            Op::EnumSetPayload => {
                // Pop value, payload_idx, enum_handle
                let Some(val) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(Value::I64(payload_idx)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                enum_set_payload(h, payload_idx as usize, val);
                stack.push(Value::I64(0));
                ip += 1;
            }
            Op::EnumGetPayload => {
                // Pop payload_idx, enum_handle
                let Some(Value::I64(payload_idx)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let val = enum_get_payload(h, payload_idx as usize);
                stack.push(val);
                ip += 1;
            }
            Op::EnumGetTag => {
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let tag = enum_get_tag(h);
                stack.push(Value::I64(tag));
                ip += 1;
            }
            Op::EnumGetVariant => {
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let name = enum_get_variant(h);
                stack.push(Value::Str(name));
                ip += 1;
            }
            Op::EnumTypeName => {
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let name = enum_type_name(h);
                stack.push(Value::Str(name));
                ip += 1;
            }
            // Removed: HTTP handlers (HttpFetch, HttpServe, HttpAccept, HttpRespond)
            // These operations have been migrated to HostCallNet with host functions.
            // HTTP functionality is now implemented through the host module interface.

            // --- JSON serialization operations ---
            Op::JsonStringify => {
                let Some(val) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let json_str = json_stringify(&val);
                stack.push(Value::Str(json_str));
                ip += 1;
            }
            Op::JsonParse => {
                let Some(val) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let s = match val {
                    Value::Str(s) => s,
                    Value::I64(n) => n.to_string(),
                    Value::F64(f) => f.to_string(),
                    Value::Bool(b) => b.to_string(),
                };
                // DEBUG: Trace JsonParse for event data
                let is_event_related = s.contains("has_value") || s.contains("payload") || s.contains("intent");
                if is_event_related {
                    eprintln!("[VM_DEBUG] JsonParse at ip={}: input='{}' (len={})", ip, &s.chars().take(200).collect::<String>(), s.len());
                }
                let handle = json_parse_and_store(&s);
                if is_event_related {
                    eprintln!("[VM_DEBUG] JsonParse result: handle={}", handle);
                }
                stack.push(Value::I64(handle));
                ip += 1;
            }
            Op::StructToJson => {
                // Pop field meta string, then struct handle
                let Some(Value::Str(field_meta)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(Value::I64(struct_handle)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                match struct_to_json(struct_handle, &field_meta) {
                    StructToJsonResult::Ok(json) => {
                        stack.push(Value::Str(json));
                    }
                    StructToJsonResult::CycleDetected => {
                        // Cycle detected - panic with error message
                        eprintln!(
                            "[PANIC] Cycle detected during JSON serialization of struct handle {}",
                            struct_handle
                        );
                        return InterpreterResult::Failed(1);
                    }
                }
                ip += 1;
            }
            Op::JsonToStruct => {
                // Pop field meta string, then JSON string
                // Returns: handle on success, -1 on parse error, -2 on unknown field error
                let Some(Value::Str(field_meta)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(val) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let json_str = match val {
                    Value::Str(s) => s,
                    _ => {
                        stack.push(Value::I64(-1));
                        ip += 1;
                        continue;
                    }
                };
                let handle = json_to_struct(&json_str, &field_meta);
                stack.push(Value::I64(handle));
                ip += 1;
            }

            // --- JSON value accessor operations ---
            Op::JsonGetField => {
                // Pop key string, then json handle
                let Some(Value::Str(key)) = stack.pop() else {
                    vm_error!("JsonGetField: expected string key");
                };
                let Some(Value::I64(handle)) = stack.pop() else {
                    vm_error!("JsonGetField: expected json handle");
                };
                let result = json_get_field(handle, &key);
                // Debug: trace property access in handleSelectThread range
                if ip >= 1056 && ip < 1230 {
                    eprintln!("[VM_DEBUG] JsonGetField at ip={}: handle={}, key='{}', result={}", ip, handle, key, result);
                }
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::JsonGetIndex => {
                // Pop index (can be i64 or f64), then json handle
                let Some(idx_val) = stack.pop() else {
                    vm_error!("JsonGetIndex: stack underflow");
                };
                let index = match idx_val {
                    Value::I64(i) => i,
                    Value::F64(f) => f as i64,
                    _ => {
                        vm_error!("JsonGetIndex: expected numeric index");
                    }
                };
                let Some(Value::I64(handle)) = stack.pop() else {
                    vm_error!("JsonGetIndex: expected json handle");
                };
                let result = json_get_index(handle, index);
                // DEBUG: Trace in findById and handleSelectThread ranges
                if ip >= 700 && ip < 1230 {
                    eprintln!("[VM_DEBUG] JsonGetIndex at ip={}: handle={}, index={}, result={}", ip, handle, index, result);
                }
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::JsonGetString => {
                let Some(Value::I64(handle)) = stack.pop() else {
                    vm_error!("JsonGetString: expected json handle");
                };
                let result = json_get_string(handle);
                // Debug: trace string extraction in handleSelectThread range
                if ip >= 1056 && ip < 1230 {
                    eprintln!("[VM_DEBUG] JsonGetString at ip={}: handle={}, result='{}'", ip, handle, &result.chars().take(50).collect::<String>());
                }
                stack.push(Value::Str(result));
                ip += 1;
            }
            Op::JsonGetNumber => {
                let Some(Value::I64(handle)) = stack.pop() else {
                    vm_error!("JsonGetNumber: expected json handle");
                };
                let result = json_get_number(handle);
                stack.push(Value::F64(result));
                ip += 1;
            }
            Op::JsonGetBool => {
                let Some(Value::I64(handle)) = stack.pop() else {
                    vm_error!("JsonGetBool: expected json handle");
                };
                let result = json_get_bool(handle);
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::JsonIsNull => {
                let Some(Value::I64(handle)) = stack.pop() else {
                    vm_error!("JsonIsNull: expected json handle");
                };
                let result = json_is_null(handle);
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::JsonIsObject => {
                let Some(Value::I64(handle)) = stack.pop() else {
                    vm_error!("JsonIsObject: expected json handle");
                };
                let result = json_is_object(handle);
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::JsonIsArray => {
                let Some(Value::I64(handle)) = stack.pop() else {
                    vm_error!("JsonIsArray: expected json handle");
                };
                let result = json_is_array(handle);
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::JsonArrayLen => {
                let Some(Value::I64(handle)) = stack.pop() else {
                    vm_error!("JsonArrayLen: expected json handle");
                };
                let result = json_array_len(handle);
                // DEBUG: Trace in findById range (704-800) and handleSelectThread range (1056-1230)
                if ip >= 700 && ip < 1230 {
                    eprintln!("[VM_DEBUG] JsonArrayLen at ip={}: handle={}, result={}", ip, handle, result);
                }
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::JsonKeys => {
                let Some(Value::I64(handle)) = stack.pop() else {
                    vm_error!("JsonKeys: expected json handle");
                };
                let result = json_keys(handle);
                stack.push(Value::I64(result));
                ip += 1;
            }

            // --- HTML parsing operations ---
            Op::HtmlParse => {
                let Some(val) = stack.pop() else { return InterpreterResult::Failed(1); };
                let s = match val {
                    Value::Str(s) => s,
                    _ => String::new(),
                };
                let handle = html_parse(&s);
                stack.push(Value::I64(handle));
                ip += 1;
            }
            Op::HtmlParseFragment => {
                let Some(val) = stack.pop() else { return InterpreterResult::Failed(1); };
                let s = match val {
                    Value::Str(s) => s,
                    _ => String::new(),
                };
                let handle = html_parse_fragment(&s);
                stack.push(Value::I64(handle));
                ip += 1;
            }
            Op::HtmlStringify => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let result = html_stringify(h);
                stack.push(Value::Str(result));
                ip += 1;
            }
            Op::HtmlStringifyPretty => {
                let Some(Value::I64(indent)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let result = html_stringify_pretty(h, indent);
                stack.push(Value::Str(result));
                ip += 1;
            }
            Op::HtmlFree => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                html_free(h);
                ip += 1;
            }
            Op::HtmlNodeType => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let ntype = html_node_type(h);
                stack.push(Value::I64(ntype));
                ip += 1;
            }
            Op::HtmlTagName => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let tag = html_tag_name(h);
                stack.push(Value::Str(tag));
                ip += 1;
            }
            Op::HtmlTextContent => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let text = html_text_content(h);
                stack.push(Value::Str(text));
                ip += 1;
            }
            Op::HtmlInnerHtml => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let inner = html_inner_html(h);
                stack.push(Value::Str(inner));
                ip += 1;
            }
            Op::HtmlOuterHtml => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let outer = html_outer_html(h);
                stack.push(Value::Str(outer));
                ip += 1;
            }
            Op::HtmlGetAttr => {
                let Some(Value::Str(attr_name)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let value = html_get_attr(h, &attr_name);
                stack.push(Value::Str(value));
                ip += 1;
            }
            Op::HtmlHasAttr => {
                let Some(Value::Str(attr_name)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let has = html_has_attr(h, &attr_name);
                stack.push(Value::Bool(has));
                ip += 1;
            }
            Op::HtmlAttrNames => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let list_h = html_attr_names(h);
                stack.push(Value::I64(list_h));
                ip += 1;
            }
            Op::HtmlParent => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let parent_h = html_parent(h);
                stack.push(Value::I64(parent_h));
                ip += 1;
            }
            Op::HtmlChildren => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let list_h = html_children(h);
                stack.push(Value::I64(list_h));
                ip += 1;
            }
            Op::HtmlElementChildren => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let list_h = html_element_children(h);
                stack.push(Value::I64(list_h));
                ip += 1;
            }
            Op::HtmlFirstChild => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let child_h = html_first_child(h);
                stack.push(Value::I64(child_h));
                ip += 1;
            }
            Op::HtmlLastChild => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let child_h = html_last_child(h);
                stack.push(Value::I64(child_h));
                ip += 1;
            }
            Op::HtmlNextSibling => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let sib_h = html_next_sibling(h);
                stack.push(Value::I64(sib_h));
                ip += 1;
            }
            Op::HtmlPrevSibling => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let sib_h = html_prev_sibling(h);
                stack.push(Value::I64(sib_h));
                ip += 1;
            }
            Op::HtmlQuerySelector => {
                let Some(Value::Str(selector)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let elem_h = html_query_selector(h, &selector);
                stack.push(Value::I64(elem_h));
                ip += 1;
            }
            Op::HtmlQuerySelectorAll => {
                let Some(Value::Str(selector)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let list_h = html_query_selector_all(h, &selector);
                stack.push(Value::I64(list_h));
                ip += 1;
            }
            Op::HtmlGetById => {
                let Some(Value::Str(id)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let elem_h = html_get_by_id(h, &id);
                stack.push(Value::I64(elem_h));
                ip += 1;
            }
            Op::HtmlGetByTag => {
                let Some(Value::Str(tag)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let list_h = html_get_by_tag(h, &tag);
                stack.push(Value::I64(list_h));
                ip += 1;
            }
            Op::HtmlGetByClass => {
                let Some(Value::Str(class_name)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let list_h = html_get_by_class(h, &class_name);
                stack.push(Value::I64(list_h));
                ip += 1;
            }
            Op::HtmlHasClass => {
                let Some(Value::Str(class_name)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let has = html_has_class(h, &class_name);
                stack.push(Value::Bool(has));
                ip += 1;
            }

            // Template engine operations
            Op::TemplateCompile => {
                let Some(Value::Str(html)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let h = template_compile(&html);
                stack.push(Value::I64(h));
                ip += 1;
            }
            Op::TemplateCompileFile => {
                let Some(Value::Str(path)) = stack.pop() else { return InterpreterResult::Failed(1); };
                match template_compile_file(&path) {
                    Ok(h) => stack.push(Value::I64(h)),
                    Err(_) => stack.push(Value::I64(-1)),
                }
                ip += 1;
            }
            Op::TemplateRender => {
                let Some(Value::I64(ctx_handle)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let Some(Value::I64(tpl_handle)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let result = template_render(tpl_handle, ctx_handle);
                stack.push(Value::Str(result));
                ip += 1;
            }
            Op::TemplateRegisterPartial => {
                let Some(Value::I64(tpl_handle)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let Some(Value::Str(name)) = stack.pop() else { return InterpreterResult::Failed(1); };
                template_register_partial(&name, tpl_handle);
                ip += 1;
            }
            Op::TemplateGetPartial => {
                let Some(Value::Str(name)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let h = template_get_partial(&name);
                stack.push(Value::I64(h));
                ip += 1;
            }
            Op::TemplateUnregisterPartial => {
                let Some(Value::Str(name)) = stack.pop() else { return InterpreterResult::Failed(1); };
                template_unregister_partial(&name);
                ip += 1;
            }
            Op::TemplateFree => {
                let Some(Value::I64(h)) = stack.pop() else { return InterpreterResult::Failed(1); };
                template_free(h);
                ip += 1;
            }
            Op::TemplateEscapeHtml => {
                let Some(Value::Str(text)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let escaped = template_escape_html(&text);
                stack.push(Value::Str(escaped));
                ip += 1;
            }
            Op::TemplateUnescapeHtml => {
                let Some(Value::Str(text)) = stack.pop() else { return InterpreterResult::Failed(1); };
                let unescaped = template_unescape_html(&text);
                stack.push(Value::Str(unescaped));
                ip += 1;
            }

            Op::SharedGetByName(ix) => {
                let i = ix as usize;
                let name = p.strings.get(i).cloned().unwrap_or_default();
                let h = if let Ok(mut m) = named_shared_map().lock() {
                    if let Some(&h) = m.get(&name) {
                        h
                    } else {
                        let nh = shared_new();
                        m.insert(name, nh);
                        nh
                    }
                } else {
                    shared_new()
                };
                stack.push(Value::I64(h));
                ip += 1;
            }
            // --- Shared cells ---
            Op::SharedNew => {
                let h = shared_new();
                stack.push(Value::I64(h));
                ip += 1;
            }
            Op::SharedStore => {
                let (Some(v), Some(Value::I64(h))) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                shared_store_val(h, v);
                stack.push(Value::I64(0));
                ip += 1;
            }
            Op::SharedLoad => {
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let v = shared_load_val(h);
                stack.push(v);
                ip += 1;
            }
            // --- Closure operations ---
            Op::ClosureNew(func_id, num_captures) => {
                let h = closure_new(func_id, num_captures);
                stack.push(Value::I64(h));
                ip += 1;
            }
            Op::ClosureCapture => {
                // Stack order: [captured_value, closure_handle]
                // Pop closure handle first (top), then captured value
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let Some(value) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                closure_capture(h, value);
                stack.push(Value::I64(0));
                ip += 1;
            }
            Op::ClosureCall(num_args) => {
                // Pop closure handle
                let Some(Value::I64(closure_handle)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };

                // Get function ID and captures
                let Some(func_id) = closure_get_func_id(closure_handle) else {
                    return InterpreterResult::Failed(1);
                };
                let captures = closure_get_captures(closure_handle);

                // Pop function arguments from stack (they were pushed before closure handle)
                let mut args = Vec::with_capacity(num_args as usize);
                for _ in 0..num_args {
                    let Some(arg) = stack.pop() else {
                        return InterpreterResult::Failed(1);
                    };
                    args.push(arg);
                }

                // Push captures first (they become the first parameters)
                for cap in captures {
                    stack.push(cap);
                }

                // Push function arguments back (in original order - reverse since we popped)
                for arg in args.into_iter().rev() {
                    stack.push(arg);
                }

                // Save return address and push new locals frame, then jump to function
                ret_stack.push(ip + 1);
                frames.push(Vec::new());
                ip = func_id as usize;
            }
            // --- Reference Counting operations ---
            Op::RcAlloc => {
                // Pop value, wrap in RC cell with count=1, push handle
                let Some(value) = stack.pop() else { return InterpreterResult::Failed(1) };
                let h = rc_alloc(value);
                stack.push(Value::I64(h));
                ip += 1;
            }
            Op::RcInc => {
                // Pop handle, increment count, push handle back
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                rc_inc(h);
                stack.push(Value::I64(h));
                ip += 1;
            }
            Op::RcDec => {
                // Pop handle, decrement count, push 0
                // If count reaches 0, the cell is deallocated
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let _ = rc_dec(h); // Ignore the returned value (no deinit call)
                stack.push(Value::I64(0));
                ip += 1;
            }
            Op::RcDecWithDeinit(func_offset) => {
                // Pop handle, decrement count
                // If count reaches 0, call deinit function then deallocate
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                if let Some(value) = rc_dec(h) {
                    // Value was deallocated - call deinit function
                    // Push the value for deinit to consume
                    stack.push(value);
                    // Save return address and call deinit
                    ret_stack.push(ip + 1);
                    frames.push(Vec::new());
                    ip = func_offset as usize;
                    // Don't push result - will be pushed by the continuing code
                    continue;
                }
                stack.push(Value::I64(0));
                ip += 1;
            }
            Op::RcLoad => {
                // Pop handle, push the contained value
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let value = rc_load(h).unwrap_or(Value::I64(0));
                stack.push(value);
                ip += 1;
            }
            Op::RcStore => {
                // Pop value, pop handle, store value in cell, push 0
                let Some(value) = stack.pop() else { return InterpreterResult::Failed(1) };
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                rc_store_value(h, value);
                stack.push(Value::I64(0));
                ip += 1;
            }
            Op::RcGetCount => {
                // Pop handle, push current reference count
                let Some(Value::I64(h)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                let count = rc_get_count(h);
                stack.push(Value::I64(count as i64));
                ip += 1;
            }
            // Region-based allocation operations
            Op::RegionEnter(region_id) => {
                region_enter(region_id);
                stack.push(Value::I64(0)); // Push unit/void result
                ip += 1;
            }
            Op::RegionExit(region_id) => {
                // Exit region and get the allocations (for potential deinit calls)
                // Note: deinit calls are emitted by the compiler before RegionExit
                let _allocations = region_exit(region_id);
                // The allocations are now deallocated (in Rust terms, dropped)
                stack.push(Value::I64(0)); // Push unit/void result
                ip += 1;
            }
            // --- Panic and unwinding operations ---
            Op::Panic(msg_idx) => {
                // Get panic message from string pool
                let msg = p
                    .strings
                    .get(msg_idx as usize)
                    .cloned()
                    .unwrap_or_else(|| "panic".to_string());
                // Emit panic with stack trace if debug info is available
                panic_with_message_and_stack(msg, ip, &ret_stack, p);
                // Check for unwind handler
                if let Ok(mut handlers) = unwind_handlers().lock() {
                    if let Some(handler) = handlers.pop() {
                        // Panics always unwind frames to reach the handler
                        let current_depth = frames.len();
                        if current_depth > handler.frame_depth {
                            while frames.len() > handler.frame_depth {
                                let _ = frames.pop();
                                let _ = ret_stack.pop();
                            }
                        }
                        // Jump to unwind handler to run drops
                        ip = handler.handler_ip as usize;
                        continue;
                    }
                }
                // No handler - abort the VM with panic exit code
                return InterpreterResult::Failed(2); // Panic exit code
            }
            Op::SetUnwindHandler(handler_ip) => {
                // Register handler with current frame depth
                push_unwind_handler(handler_ip, frames.len());
                ip += 1;
            }
            Op::ClearUnwindHandler => {
                pop_unwind_handler();
                ip += 1;
            }
            Op::GetPanicMessage => {
                // Push the current panic message onto the stack as a string
                let msg = if let Ok(state) = panic_state().lock() {
                    if state.is_panicking {
                        state.message.clone()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };
                stack.push(Value::Str(msg));
                ip += 1;
            }
            // --- Exception handling operations ---
            Op::Throw => {
                // Pop exception value from stack
                let Some(exc_val) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                // Convert to i64 handle (struct handle)
                let exc_handle = match exc_val {
                    Value::I64(h) => h,
                    _ => 0, // Should be a struct handle
                };
                // Throw the exception
                if let Some((handler_ip, handler_depth)) = throw_exception(exc_handle) {
                    // Only pop frames if the handler is at a lower depth (in a caller function).
                    // If the handler is at the same depth, it's in the same function (nested try)
                    // and we just jump without popping frames.
                    let current_depth = frames.len();
                    if current_depth > handler_depth {
                        // Pop frames until we reach the handler's depth
                        while frames.len() > handler_depth {
                            let _ = frames.pop();
                            let _ = ret_stack.pop();
                        }
                    }
                    ip = handler_ip as usize;
                    continue;
                } else {
                    // No handler - uncaught exception terminates VM
                    eprintln!("uncaught exception");
                    return InterpreterResult::Failed(3); // Exception exit code
                }
            }
            Op::GetException => {
                // Get the current exception value and push it
                let exc_handle = get_exception();
                stack.push(Value::I64(exc_handle));
                ip += 1;
            }
            // PrintTop handled earlier
            // --- Float math ops ---
            Op::SqrtF64 => {
                let Some(v) = stack.pop() else { return InterpreterResult::Failed(1) };
                let x = match v {
                    Value::F64(f) => f,
                    Value::I64(n) => n as f64,
                    Value::Bool(b) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    Value::Str(_) => 0.0,
                };
                stack.push(Value::F64(x.sqrt()));
                ip += 1;
            }
            Op::PowF64 => {
                let (Some(vb), Some(va)) = (stack.pop(), stack.pop()) else {
                    return InterpreterResult::Failed(1);
                };
                let a = match va {
                    Value::F64(f) => f,
                    Value::I64(n) => n as f64,
                    Value::Bool(b) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    Value::Str(_) => 0.0,
                };
                let b = match vb {
                    Value::F64(f) => f,
                    Value::I64(n) => n as f64,
                    Value::Bool(b) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    Value::Str(_) => 0.0,
                };
                stack.push(Value::F64(a.powf(b)));
                ip += 1;
            }
            Op::SinF64 => {
                let x = match stack.pop() {
                    Some(Value::F64(f)) => f,
                    Some(Value::I64(n)) => n as f64,
                    Some(Value::Bool(b)) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                stack.push(Value::F64(x.sin()));
                ip += 1;
            }
            Op::CosF64 => {
                let x = match stack.pop() {
                    Some(Value::F64(f)) => f,
                    Some(Value::I64(n)) => n as f64,
                    Some(Value::Bool(b)) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                stack.push(Value::F64(x.cos()));
                ip += 1;
            }
            Op::TanF64 => {
                let x = match stack.pop() {
                    Some(Value::F64(f)) => f,
                    Some(Value::I64(n)) => n as f64,
                    Some(Value::Bool(b)) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                stack.push(Value::F64(x.tan()));
                ip += 1;
            }
            Op::FloorF64 => {
                let x = match stack.pop() {
                    Some(Value::F64(f)) => f,
                    Some(Value::I64(n)) => n as f64,
                    Some(Value::Bool(b)) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                stack.push(Value::F64(x.floor()));
                ip += 1;
            }
            Op::CeilF64 => {
                let x = match stack.pop() {
                    Some(Value::F64(f)) => f,
                    Some(Value::I64(n)) => n as f64,
                    Some(Value::Bool(b)) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                stack.push(Value::F64(x.ceil()));
                ip += 1;
            }
            Op::RoundF64 => {
                let x = match stack.pop() {
                    Some(Value::F64(f)) => f,
                    Some(Value::I64(n)) => n as f64,
                    Some(Value::Bool(b)) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    _ => return InterpreterResult::Failed(1),
                };
                stack.push(Value::F64(x.round()));
                ip += 1;
            }
            // Removed: Op::RoundF64N, Op::MinF64, Op::MaxF64, Op::ClampF64, Op::AbsF64,
            //          Op::MinI64, Op::MaxI64, Op::ClampI64, Op::AbsI64
            // These are now pure Arth in stdlib/src/math/Math.arth
            Op::Jump(tgt) => {
                ip = tgt as usize;
            }
            Op::JumpIfFalse(tgt) => {
                let Some(Value::Bool(c)) = stack.pop() else {
                    return InterpreterResult::Failed(1);
                };
                if !c {
                    ip = tgt as usize;
                } else {
                    ip += 1;
                }
            }
            Op::Pop => {
                let _ = stack.pop();
                ip += 1;
            }
            Op::PrintTop => {
                // Use stderr to avoid polluting stdout (used for JSON-RPC)
                match stack.pop() {
                    Some(Value::I64(n)) => eprintln!("{}", n),
                    Some(Value::F64(f)) => {
                        if f.is_finite() && (f.fract() == 0.0) {
                            eprintln!("{:.1}", f);
                        } else {
                            eprintln!("{}", f);
                        }
                    }
                    Some(Value::Bool(b)) => eprintln!("{}", if b { "true" } else { "false" }),
                    Some(Value::Str(s)) => eprintln!("{}", s),
                    None => {}
                }
                ip += 1;
            }
            Op::PrintRaw(ix) => {
                // Use stderr to avoid polluting stdout (used for JSON-RPC)
                let i = ix as usize;
                if let Some(s) = p.strings.get(i) {
                    eprint!("{}", s);
                    let _ = std::io::stderr().flush();
                }
                ip += 1;
            }
            Op::PrintRawStrVal(ix) => {
                // Use stderr to avoid polluting stdout (used for JSON-RPC)
                let i = ix as usize;
                let s = p.strings.get(i).map(|x| x.as_str()).unwrap_or("");
                match stack.pop() {
                    Some(Value::I64(n)) => eprint!("{}{}", s, n),
                    Some(Value::F64(f)) => {
                        if f.is_finite() && (f.fract() == 0.0) {
                            eprint!("{}{}", s, format!("{:.1}", f));
                        } else {
                            eprint!("{}{}", s, f);
                        }
                    }
                    Some(Value::Bool(b)) => eprint!("{}{}", s, if b { "true" } else { "false" }),
                    Some(Value::Str(t)) => eprint!("{}{}", s, t),
                    None => eprint!("{}", s),
                }
                let _ = std::io::stderr().flush();
                ip += 1;
            }
            Op::PrintLn => {
                // Use stderr to avoid polluting stdout (used for JSON-RPC)
                eprintln!("");
                ip += 1;
            }
            Op::LocalGet(ix) => {
                let i = ix as usize;
                let locals = frames.last().unwrap();
                if i >= locals.len() {
                    // Debug: trace access to out-of-bounds locals for handleSelectThread
                    if ip >= 1056 && ip < 1230 {
                        eprintln!("[VM_DEBUG] LocalGet({}) at ip={}: out of bounds (locals.len={}), defaulting to I64(0)", i, ip, locals.len());
                    }
                    stack.push(Value::I64(0));
                } else {
                    let val = locals[i].clone();
                    // Debug: trace ALL local reads in handleSelectThread (to find where 70000 comes from)
                    if ip >= 1056 && ip < 1230 {
                        eprintln!("[VM_DEBUG] LocalGet({}) at ip={}: value={:?}", i, ip, &val);
                    }
                    match val {
                        Some(v) => stack.push(v),
                        None => stack.push(Value::I64(0)),
                    }
                }
                ip += 1;
            }
            Op::LocalSet(ix) => {
                let i = ix as usize;
                let locals = frames.last_mut().unwrap();
                if i >= locals.len() {
                    locals.resize(i + 1, None);
                }
                let v = stack.pop().unwrap_or(Value::I64(0));
                // Debug: trace ALL LocalSet in handleSelectThread range
                if ip >= 1056 && ip < 1230 {
                    eprintln!("[VM_DEBUG] LocalSet({}) at ip={}: value={:?}",
                        i, ip, &v);
                }
                locals[i] = Some(v);
                ip += 1;
            }
            Op::ToF64 => {
                let v = stack.pop().unwrap_or(Value::I64(0));
                let f = match v {
                    Value::F64(f) => f,
                    Value::I64(n) => n as f64,
                    Value::Bool(b) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    Value::Str(s) => {
                        let t = s.trim();
                        if let Ok(x) = t.parse::<f64>() {
                            x
                        } else {
                            s.chars().next().map(|c| c as u32 as f64).unwrap_or(0.0)
                        }
                    }
                };
                stack.push(Value::F64(f));
                ip += 1;
            }
            Op::ToI64 => {
                let v = stack.pop().unwrap_or(Value::I64(0));
                let n = match v {
                    Value::I64(n) => n,
                    Value::F64(f) => f as i64,
                    Value::Bool(b) => {
                        if b {
                            1
                        } else {
                            0
                        }
                    }
                    Value::Str(s) => {
                        let t = s.trim();
                        if let Ok(x) = t.parse::<i64>() {
                            x
                        } else {
                            s.chars().next().map(|c| c as u32 as i64).unwrap_or(0)
                        }
                    }
                };
                stack.push(Value::I64(n));
                ip += 1;
            }
            Op::ToI64OrEnumTag => {
                // If value is a list handle (enum), return tag at index 0; else behave like ToI64
                let v = stack.pop().unwrap_or(Value::I64(0));
                match v {
                    Value::I64(h) => {
                        // list_store contains enum/array handles
                        if let Ok(m) = list_store().lock() {
                            if let Some(vec) = m.get(&h) {
                                if let Some(Value::I64(tag)) = vec.get(0) {
                                    stack.push(Value::I64(*tag));
                                    ip += 1;
                                    continue;
                                }
                            }
                        }
                        stack.push(Value::I64(h));
                        ip += 1;
                    }
                    Value::F64(f) => {
                        stack.push(Value::I64(f as i64));
                        ip += 1;
                    }
                    Value::Bool(b) => {
                        stack.push(Value::I64(if b { 1 } else { 0 }));
                        ip += 1;
                    }
                    Value::Str(s) => {
                        let t = s.trim();
                        if let Ok(x) = t.parse::<i64>() {
                            stack.push(Value::I64(x));
                        } else {
                            let n = s.chars().next().map(|c| c as u32 as i64).unwrap_or(0);
                            stack.push(Value::I64(n));
                        }
                        ip += 1;
                    }
                }
            }
            Op::ToI8 => {
                let v = stack.pop().unwrap_or(Value::I64(0));
                let n = match v {
                    Value::I64(n) => n,
                    Value::F64(f) => f as i64,
                    Value::Bool(b) => {
                        if b {
                            1
                        } else {
                            0
                        }
                    }
                    Value::Str(s) => s
                        .parse::<i64>()
                        .unwrap_or_else(|_| s.chars().next().map(|c| c as u32 as i64).unwrap_or(0)),
                };
                stack.push(Value::I64((n as i8) as i64));
                ip += 1;
            }
            Op::ToI16 => {
                let v = stack.pop().unwrap_or(Value::I64(0));
                let n = match v {
                    Value::I64(n) => n,
                    Value::F64(f) => f as i64,
                    Value::Bool(b) => {
                        if b {
                            1
                        } else {
                            0
                        }
                    }
                    Value::Str(s) => s
                        .parse::<i64>()
                        .unwrap_or_else(|_| s.chars().next().map(|c| c as u32 as i64).unwrap_or(0)),
                };
                stack.push(Value::I64((n as i16) as i64));
                ip += 1;
            }
            Op::ToI32 => {
                let v = stack.pop().unwrap_or(Value::I64(0));
                let n = match v {
                    Value::I64(n) => n,
                    Value::F64(f) => f as i64,
                    Value::Bool(b) => {
                        if b {
                            1
                        } else {
                            0
                        }
                    }
                    Value::Str(s) => s
                        .parse::<i64>()
                        .unwrap_or_else(|_| s.chars().next().map(|c| c as u32 as i64).unwrap_or(0)),
                };
                stack.push(Value::I64((n as i32) as i64));
                ip += 1;
            }
            Op::ToU8 => {
                let v = stack.pop().unwrap_or(Value::I64(0));
                let n = match v {
                    Value::I64(n) => n,
                    Value::F64(f) => f as i64,
                    Value::Bool(b) => {
                        if b {
                            1
                        } else {
                            0
                        }
                    }
                    Value::Str(s) => s
                        .parse::<i64>()
                        .unwrap_or_else(|_| s.chars().next().map(|c| c as u32 as i64).unwrap_or(0)),
                };
                stack.push(Value::I64((n as u8) as i64));
                ip += 1;
            }
            Op::ToU16 => {
                let v = stack.pop().unwrap_or(Value::I64(0));
                let n = match v {
                    Value::I64(n) => n,
                    Value::F64(f) => f as i64,
                    Value::Bool(b) => {
                        if b {
                            1
                        } else {
                            0
                        }
                    }
                    Value::Str(s) => s
                        .parse::<i64>()
                        .unwrap_or_else(|_| s.chars().next().map(|c| c as u32 as i64).unwrap_or(0)),
                };
                stack.push(Value::I64((n as u16) as i64));
                ip += 1;
            }
            Op::ToU32 => {
                let v = stack.pop().unwrap_or(Value::I64(0));
                let n = match v {
                    Value::I64(n) => n,
                    Value::F64(f) => f as i64,
                    Value::Bool(b) => {
                        if b {
                            1
                        } else {
                            0
                        }
                    }
                    Value::Str(s) => s
                        .parse::<i64>()
                        .unwrap_or_else(|_| s.chars().next().map(|c| c as u32 as i64).unwrap_or(0)),
                };
                stack.push(Value::I64((n as u32) as i64));
                ip += 1;
            }
            Op::ToU64 => {
                let v = stack.pop().unwrap_or(Value::I64(0));
                let n = match v {
                    Value::I64(n) => n,
                    Value::F64(f) => f as i64,
                    Value::Bool(b) => {
                        if b {
                            1
                        } else {
                            0
                        }
                    }
                    Value::Str(s) => s
                        .parse::<i64>()
                        .unwrap_or_else(|_| s.chars().next().map(|c| c as u32 as i64).unwrap_or(0)),
                };
                let u = n as u64;
                let wrapped = u as i64; // lossy if > i64::MAX
                stack.push(Value::I64(wrapped));
                ip += 1;
            }
            Op::ToF32 => {
                let v = stack.pop().unwrap_or(Value::I64(0));
                let f64v = match v {
                    Value::F64(f) => f,
                    Value::I64(n) => n as f64,
                    Value::Bool(b) => {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    Value::Str(s) => s.parse::<f64>().unwrap_or_else(|_| {
                        s.chars().next().map(|c| c as u32 as f64).unwrap_or(0.0)
                    }),
                };
                let f32v = f64v as f32;
                stack.push(Value::F64(f32v as f64));
                ip += 1;
            }
            Op::ToBool => {
                let v = stack.pop().unwrap_or(Value::I64(0));
                let b = match v {
                    Value::Bool(b) => b,
                    Value::I64(n) => n != 0,
                    Value::F64(f) => f != 0.0,
                    Value::Str(s) => !s.is_empty(),
                };
                stack.push(Value::Bool(b));
                ip += 1;
            }
            Op::ToChar => {
                let v = stack.pop().unwrap_or(Value::I64(0));
                let ch = match v {
                    Value::I64(n) => char::from_u32(n as u32).unwrap_or('\u{0}'),
                    Value::F64(f) => char::from_u32(f as u32).unwrap_or('\u{0}'),
                    Value::Bool(b) => {
                        if b {
                            '1'
                        } else {
                            '0'
                        }
                    }
                    Value::Str(s) => s.chars().next().unwrap_or('\u{0}'),
                };
                stack.push(Value::Str(ch.to_string()));
                ip += 1;
            }
            // Removed: File I/O handlers (FileOpen, FileClose, FileRead, FileWrite, FileWriteStr,
            //          FileFlush, FileSeek, FileSize, FileExists, FileDelete, FileCopy, FileMove)
            // These operations have been migrated to HostCallIo with host functions.
            // File I/O functionality is now implemented through the host module interface.
            // Removed: Directory handlers (DirCreate, DirCreateAll, DirDelete, DirList,
            //          DirExists, IsDir, IsFile)
            // These operations have been migrated to HostCallIo with host functions.
            // Directory functionality is now implemented through the host module interface.
            // Removed: Path handlers (PathJoin, PathParent, PathFileName, PathExtension, PathAbsolute)
            // These operations have been migrated to HostCallIo with host functions or pure Arth code.
            // Path functionality is now implemented through the host module interface.
            // Removed: Console handlers (ConsoleReadLine, ConsoleWrite, ConsoleWriteErr)
            // These operations have been migrated to HostCallIo with host functions.
            // Console I/O functionality is now implemented through the host module interface.
            // Removed: DateTime handlers (DateTimeNow, DateTimeParse, DateTimeFormat)
            // These operations have been migrated to HostCallTime with host functions or pure Arth code.
            // DateTime functionality is now implemented through the host module interface.
            // Removed: Instant handlers (InstantNow, InstantElapsed)
            // These operations have been migrated to HostCallTime with host functions.
            // Instant functionality is now implemented through the host module interface.
            // --- BigDecimal operations ---
            Op::BigDecimalNew => {
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => String::new(),
                };
                stack.push(Value::I64(bigdecimal_new(&s)));
                ip += 1;
            }
            Op::BigDecimalFromInt => {
                let n = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigdecimal_from_int(n)));
                ip += 1;
            }
            Op::BigDecimalFromFloat => {
                let f = match stack.pop() {
                    Some(Value::F64(f)) => f,
                    Some(Value::I64(n)) => n as f64,
                    _ => 0.0,
                };
                stack.push(Value::I64(bigdecimal_from_float(f)));
                ip += 1;
            }
            Op::BigDecimalAdd => {
                let h2 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h1 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigdecimal_add(h1, h2)));
                ip += 1;
            }
            Op::BigDecimalSub => {
                let h2 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h1 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigdecimal_sub(h1, h2)));
                ip += 1;
            }
            Op::BigDecimalMul => {
                let h2 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h1 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigdecimal_mul(h1, h2)));
                ip += 1;
            }
            Op::BigDecimalDiv => {
                let scale = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 10,
                };
                let h2 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h1 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigdecimal_div(h1, h2, scale)));
                ip += 1;
            }
            Op::BigDecimalRem => {
                let h2 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h1 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigdecimal_rem(h1, h2)));
                ip += 1;
            }
            Op::BigDecimalPow => {
                let exp = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigdecimal_pow(h, exp)));
                ip += 1;
            }
            // Removed: Op::BigDecimalAbs - now pure Arth in stdlib/src/numeric/BigDecimal.arth
            Op::BigDecimalNegate => {
                let h = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigdecimal_negate(h)));
                ip += 1;
            }
            Op::BigDecimalCompare => {
                let h2 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h1 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigdecimal_compare(h1, h2)));
                ip += 1;
            }
            Op::BigDecimalToString => {
                let h = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let s = bigdecimal_get(h).unwrap_or_else(|| "0".to_string());
                stack.push(Value::Str(s));
                ip += 1;
            }
            Op::BigDecimalToInt => {
                let h = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigdecimal_to_int(h)));
                ip += 1;
            }
            Op::BigDecimalToFloat => {
                let h = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::F64(bigdecimal_to_float(h)));
                ip += 1;
            }
            Op::BigDecimalScale => {
                let h = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigdecimal_scale(h)));
                ip += 1;
            }
            Op::BigDecimalSetScale => {
                let rounding_mode = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let scale = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigdecimal_round(h, scale, rounding_mode)));
                ip += 1;
            }
            Op::BigDecimalRound => {
                let rounding_mode = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let scale = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigdecimal_round(h, scale, rounding_mode)));
                ip += 1;
            }
            // --- BigInt operations ---
            Op::BigIntNew => {
                let s = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    Some(Value::I64(idx)) => {
                        p.strings.get(idx as usize).cloned().unwrap_or_default()
                    }
                    _ => String::new(),
                };
                stack.push(Value::I64(bigint_new(&s)));
                ip += 1;
            }
            Op::BigIntFromInt => {
                let n = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigint_from_int(n)));
                ip += 1;
            }
            Op::BigIntAdd => {
                let h2 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h1 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigint_add(h1, h2)));
                ip += 1;
            }
            Op::BigIntSub => {
                let h2 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h1 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigint_sub(h1, h2)));
                ip += 1;
            }
            Op::BigIntMul => {
                let h2 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h1 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigint_mul(h1, h2)));
                ip += 1;
            }
            Op::BigIntDiv => {
                let h2 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h1 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigint_div(h1, h2)));
                ip += 1;
            }
            Op::BigIntRem => {
                let h2 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h1 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigint_rem(h1, h2)));
                ip += 1;
            }
            Op::BigIntPow => {
                let exp = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigint_pow(h, exp)));
                ip += 1;
            }
            // Removed: Op::BigIntAbs - now pure Arth in stdlib/src/numeric/BigInt.arth
            Op::BigIntNegate => {
                let h = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigint_negate(h)));
                ip += 1;
            }
            Op::BigIntCompare => {
                let h2 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let h1 = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigint_compare(h1, h2)));
                ip += 1;
            }
            Op::BigIntToString => {
                let h = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let s = bigint_get(h).unwrap_or_else(|| "0".to_string());
                stack.push(Value::Str(s));
                ip += 1;
            }
            Op::BigIntToInt => {
                let h = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                stack.push(Value::I64(bigint_to_int(h)));
                ip += 1;
            }
            // Removed: Op::BigIntGcd, Op::BigIntModPow - now pure Arth in stdlib/src/numeric/BigInt.arth
            // Removed: WebSocket handlers (WsServe, WsAccept, WsSendText, WsSendBinary, WsRecv, WsClose, WsIsOpen)
            // These operations have been migrated to HostCallNet with host functions.
            // WebSocket functionality is now implemented through the host module interface.
            // Removed: SSE handlers (SseServe, SseAccept, SseSend, SseClose, SseIsOpen)
            // These operations have been migrated to HostCallNet with host functions.
            // SSE functionality is now implemented through the host module interface.

            // Host function calls (new implementation)
            Op::HostCallIo(op) => {
                // Capability check: if context provided, verify IO is allowed
                if let Some(c) = ctx {
                    if !c.config.allow_io {
                        stack.push(Value::I64(-1));
                        ip += 1;
                        continue;
                    }
                }
                use crate::ops::HostIoOp;
                match op {
                    HostIoOp::FileOpen => {
                        // Stack: [path_str, mode] -> [handle]
                        let mode = match stack.pop() {
                            Some(Value::I64(m)) => m,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let path = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let handle = __arth_file_open(path.as_ptr(), path.len() as i64, mode);
                        stack.push(Value::I64(handle));
                    }
                    HostIoOp::FileClose => {
                        // Stack: [handle] -> [status]
                        let handle = match stack.pop() {
                            Some(Value::I64(h)) => h,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let status = __arth_file_close(handle);
                        stack.push(Value::I64(status));
                    }
                    HostIoOp::FileRead => {
                        // Stack: [handle, num_bytes] -> [string_result or handle]
                        let num_bytes = match stack.pop() {
                            Some(Value::I64(n)) => n,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let handle = match stack.pop() {
                            Some(Value::I64(h)) => h,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let bytes_read = __arth_file_read(handle, num_bytes);
                        if bytes_read >= 0 {
                            // Get the read data from thread-local storage
                            let mut out_len: i64 = 0;
                            let ptr = __arth_file_get_read_data(&mut out_len);
                            let data = if !ptr.is_null() && out_len > 0 {
                                unsafe {
                                    let slice = std::slice::from_raw_parts(ptr, out_len as usize);
                                    String::from_utf8_lossy(slice).to_string()
                                }
                            } else {
                                String::new()
                            };
                            stack.push(Value::Str(data));
                        } else {
                            stack.push(Value::I64(-1));
                        }
                    }
                    HostIoOp::FileWrite => {
                        // Stack: [handle, data] -> [bytes_written]
                        // Note: FileWrite is not emitted by compiler, FileWriteStr is used instead
                        let data = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            Some(Value::I64(n)) => n.to_string(),
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let handle = match stack.pop() {
                            Some(Value::I64(h)) => h,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let written = __arth_file_write_str(handle, data.as_ptr(), data.len() as i64);
                        stack.push(Value::I64(written));
                    }
                    HostIoOp::FileWriteStr => {
                        // Stack: [handle, string] -> [bytes_written]
                        let data = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            Some(Value::I64(n)) => n.to_string(),
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let handle = match stack.pop() {
                            Some(Value::I64(h)) => h,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let written = __arth_file_write_str(handle, data.as_ptr(), data.len() as i64);
                        stack.push(Value::I64(written));
                    }
                    HostIoOp::FileFlush => {
                        // Stack: [handle] -> [status]
                        let handle = match stack.pop() {
                            Some(Value::I64(h)) => h,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let status = __arth_file_flush(handle);
                        stack.push(Value::I64(status));
                    }
                    HostIoOp::FileSeek => {
                        // Stack: [handle, offset, whence] -> [new_position]
                        let whence = match stack.pop() {
                            Some(Value::I64(w)) => w,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let offset = match stack.pop() {
                            Some(Value::I64(o)) => o,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let handle = match stack.pop() {
                            Some(Value::I64(h)) => h,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let pos = __arth_file_seek(handle, offset, whence);
                        stack.push(Value::I64(pos));
                    }
                    HostIoOp::FileSize => {
                        // Stack: [path_str] -> [size]
                        let path = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let size = __arth_file_size(path.as_ptr(), path.len() as i64);
                        stack.push(Value::I64(size));
                    }
                    HostIoOp::FileExists => {
                        // Stack: [path_str] -> [1 or 0]
                        let path = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(0)); ip += 1; continue; }
                        };
                        let exists = __arth_file_exists(path.as_ptr(), path.len() as i64);
                        stack.push(Value::I64(exists));
                    }
                    HostIoOp::FileDelete => {
                        // Stack: [path_str] -> [status]
                        let path = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let status = __arth_file_delete(path.as_ptr(), path.len() as i64);
                        stack.push(Value::I64(status));
                    }
                    HostIoOp::FileCopy => {
                        // Stack: [src_str, dst_str] -> [status]
                        let dst = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let src = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let status = __arth_file_copy(
                            src.as_ptr(), src.len() as i64,
                            dst.as_ptr(), dst.len() as i64
                        );
                        stack.push(Value::I64(status));
                    }
                    HostIoOp::FileMove => {
                        // Stack: [src_str, dst_str] -> [status]
                        let dst = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let src = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let status = __arth_file_move(
                            src.as_ptr(), src.len() as i64,
                            dst.as_ptr(), dst.len() as i64
                        );
                        stack.push(Value::I64(status));
                    }
                    HostIoOp::DirCreate => {
                        // Stack: [path_str] -> [status]
                        let path = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let status = __arth_dir_create(path.as_ptr(), path.len() as i64);
                        stack.push(Value::I64(status));
                    }
                    HostIoOp::DirCreateAll => {
                        // Stack: [path_str] -> [status]
                        let path = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let status = __arth_dir_create_all(path.as_ptr(), path.len() as i64);
                        stack.push(Value::I64(status));
                    }
                    HostIoOp::DirDelete => {
                        // Stack: [path_str] -> [status]
                        let path = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let status = __arth_dir_delete(path.as_ptr(), path.len() as i64);
                        stack.push(Value::I64(status));
                    }
                    HostIoOp::DirList => {
                        // Stack: [path_str] -> [list_handle]
                        let path = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let list_handle = __arth_dir_list(path.as_ptr(), path.len() as i64);
                        stack.push(Value::I64(list_handle));
                    }
                    HostIoOp::DirExists => {
                        // Stack: [path_str] -> [1 or 0]
                        let path = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(0)); ip += 1; continue; }
                        };
                        let exists = __arth_dir_exists(path.as_ptr(), path.len() as i64);
                        stack.push(Value::I64(exists));
                    }
                    HostIoOp::IsDir => {
                        // Stack: [path_str] -> [1 or 0]
                        let path = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(0)); ip += 1; continue; }
                        };
                        let is_dir = __arth_is_dir(path.as_ptr(), path.len() as i64);
                        stack.push(Value::I64(is_dir));
                    }
                    HostIoOp::IsFile => {
                        // Stack: [path_str] -> [1 or 0]
                        let path = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::I64(0)); ip += 1; continue; }
                        };
                        let is_file = __arth_is_file(path.as_ptr(), path.len() as i64);
                        stack.push(Value::I64(is_file));
                    }
                    HostIoOp::PathAbsolute => {
                        // Stack: [path_str] -> [absolute_path_str]
                        let path = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            _ => { stack.push(Value::Str(String::new())); ip += 1; continue; }
                        };
                        let status = __arth_path_absolute(path.as_ptr(), path.len() as i64);
                        if status == 0 {
                            let mut out_len: i64 = 0;
                            let ptr = __arth_path_get_result(&mut out_len);
                            let result = if !ptr.is_null() && out_len > 0 {
                                unsafe {
                                    let slice = std::slice::from_raw_parts(ptr, out_len as usize);
                                    String::from_utf8_lossy(slice).to_string()
                                }
                            } else {
                                String::new()
                            };
                            stack.push(Value::Str(result));
                        } else {
                            stack.push(Value::Str(String::new()));
                        }
                    }
                    HostIoOp::ConsoleReadLine => {
                        // Stack: [] -> [string_result]
                        let status = __arth_console_read_line();
                        if status == 0 {
                            let mut out_len: i64 = 0;
                            let ptr = __arth_console_get_line(&mut out_len);
                            let result = if !ptr.is_null() && out_len > 0 {
                                unsafe {
                                    let slice = std::slice::from_raw_parts(ptr, out_len as usize);
                                    String::from_utf8_lossy(slice).to_string()
                                }
                            } else {
                                String::new()
                            };
                            stack.push(Value::Str(result));
                        } else {
                            stack.push(Value::Str(String::new()));
                        }
                    }
                    HostIoOp::ConsoleWrite => {
                        // Stack: [string] -> [status]
                        let data = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            Some(Value::I64(n)) => n.to_string(),
                            Some(Value::F64(f)) => f.to_string(),
                            Some(Value::Bool(b)) => if b { "true".to_string() } else { "false".to_string() },
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let status = __arth_console_write(data.as_ptr(), data.len() as i64);
                        stack.push(Value::I64(status));
                    }
                    HostIoOp::ConsoleWriteErr => {
                        // Stack: [string] -> [status]
                        let data = match stack.pop() {
                            Some(Value::Str(s)) => s,
                            Some(Value::I64(n)) => n.to_string(),
                            Some(Value::F64(f)) => f.to_string(),
                            Some(Value::Bool(b)) => if b { "true".to_string() } else { "false".to_string() },
                            _ => { stack.push(Value::I64(-1)); ip += 1; continue; }
                        };
                        let status = __arth_console_write_err(data.as_ptr(), data.len() as i64);
                        stack.push(Value::I64(status));
                    }
                }
                ip += 1;
            }
            Op::HostCallNet(op) => {
                // Capability check: if context provided, verify Net is allowed
                if let Some(c) = ctx {
                    if !c.config.allow_net {
                        stack.push(Value::I64(-1));
                        ip += 1;
                        continue;
                    }
                }
                use crate::ops::HostNetOp;
                // Network operations dispatch to FFI functions.
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
                                ip += 1;
                                continue;
                            }
                            _ => {
                                stack.push(Value::I64(-1));
                                ip += 1;
                                continue;
                            }
                        };
                        let url = p.strings.get(url_idx).map(|s| s.as_str()).unwrap_or("");
                        let handle = __arth_http_fetch_url(url.as_ptr(), url.len() as i64, 30000);
                        stack.push(Value::I64(handle));
                    }
                    HostNetOp::HttpServe => {
                        // Stack: [port] -> [server_handle or -1]
                        // Uses the C25 HTTP server implementation
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
                        // Simplified: just mark the connection for response
                        let _ = stack.pop(); // body
                        let _ = stack.pop(); // status
                        let _ = stack.pop(); // request handle
                        // No return value (response handled via HttpWriter ops)
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
                        let path = p.strings.get(path_idx).map(|s| s.as_str()).unwrap_or("/");
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
                                let status = __arth_ws_send_text(
                                    match stack.pop() {
                                        Some(Value::I64(h)) => h,
                                        _ => -1,
                                    },
                                    s.as_ptr(),
                                    s.len() as i64,
                                );
                                stack.push(Value::I64(status));
                                ip += 1;
                                continue;
                            }
                            _ => 0,
                        };
                        let conn = match stack.pop() {
                            Some(Value::I64(n)) => n,
                            _ => -1,
                        };
                        let msg = p.strings.get(msg_idx).map(|s| s.as_str()).unwrap_or("");
                        let status = __arth_ws_send_text(conn, msg.as_ptr(), msg.len() as i64);
                        stack.push(Value::I64(status));
                    }
                    HostNetOp::WsSendBinary => {
                        // Stack: [conn_handle, data_handle] -> [status]
                        // Binary data handling via list handle - simplified for now
                        let _ = stack.pop(); // data handle
                        let conn = match stack.pop() {
                            Some(Value::I64(n)) => n,
                            _ => -1,
                        };
                        // For now just send empty binary
                        let status = __arth_ws_send_binary(conn, std::ptr::null(), 0);
                        stack.push(Value::I64(status));
                    }
                    HostNetOp::WsRecv => {
                        // Stack: [conn_handle] -> [message_type or -1]
                        // Returns: 0=text, 1=binary, 2=close, -1=no message
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
                        let reason = p.strings.get(reason_idx).map(|s| s.as_str()).unwrap_or("");
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
                        let path = p.strings.get(path_idx).map(|s| s.as_str()).unwrap_or("/");
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
                        let event_type = p.strings.get(event_idx).map(|s| s.as_str()).unwrap_or("");
                        let data = p.strings.get(data_idx).map(|s| s.as_str()).unwrap_or("");
                        let id = p.strings.get(id_idx).map(|s| s.as_str()).unwrap_or("");
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
                ip += 1;
            }
            Op::HostCallTime(op) => {
                // Capability check: if context provided, verify Time is allowed
                if let Some(c) = ctx {
                    if !c.config.allow_time {
                        stack.push(Value::I64(-1));
                        ip += 1;
                        continue;
                    }
                }
                use crate::ops::HostTimeOp;
                match op {
                    HostTimeOp::InstantNow => {
                        // Stack: [] -> [handle]
                        // Create a new instant and return its handle
                        let handle = instants.len() as i64;
                        instants.push(std::time::Instant::now());
                        stack.push(Value::I64(handle));
                    }
                    HostTimeOp::InstantElapsed => {
                        // Stack: [handle] -> [elapsed_millis]
                        let handle = match stack.pop() {
                            Some(Value::I64(h)) => h as usize,
                            _ => {
                                stack.push(Value::I64(-1));
                                ip += 1;
                                continue;
                            }
                        };
                        if handle < instants.len() {
                            let elapsed = instants[handle].elapsed().as_millis() as i64;
                            stack.push(Value::I64(elapsed));
                        } else {
                            stack.push(Value::I64(-1));
                        }
                    }
                    HostTimeOp::DateTimeNow => {
                        // Stack: [] -> [millis]
                        // Return current time as millis since epoch
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        stack.push(Value::I64(now));
                    }
                    HostTimeOp::Sleep => {
                        // Stack: [millis] -> []
                        let millis = match stack.pop() {
                            Some(Value::I64(n)) => n as u64,
                            _ => 0,
                        };
                        std::thread::sleep(std::time::Duration::from_millis(millis));
                    }
                    _ => {
                        // DateTimeParse, DateTimeFormat not needed for C05
                        eprintln!("[VM] HostCallTime {:?} not yet implemented", op);
                        return InterpreterResult::Failed(1);
                    }
                }
                ip += 1;
            }
            Op::HostCallDb(op) => {
                // Database operations require HostContext for full functionality.
                if let Some(c) = ctx {
                    if !c.config.allow_db {
                        stack.push(Value::I64(-1));
                        ip += 1;
                        continue;
                    }
                    // Use dispatch function with context
                    dispatch_host_db(op, &mut stack, &p.strings, c);
                } else {
                    // No context - legacy mode, DB not available
                    stack.pop();
                    stack.push(Value::I64(-1));
                }
                ip += 1;
            }
            Op::HostCallMail(op) => {
                // Mail operations require HostContext for full functionality.
                if let Some(c) = ctx {
                    if !c.config.allow_mail {
                        stack.push(Value::I64(-1));
                        ip += 1;
                        continue;
                    }
                    // Use dispatch function with context
                    dispatch_host_mail(op, &mut stack, &p.strings, c);
                } else {
                    // No context - legacy mode, Mail not available
                    stack.pop();
                    stack.push(Value::I64(-1));
                }
                ip += 1;
            }
            Op::HostCallCrypto(op) => {
                // Crypto operations require HostContext for full functionality.
                if let Some(c) = ctx {
                    if !c.config.allow_crypto {
                        stack.push(Value::I64(-1));
                        ip += 1;
                        continue;
                    }
                    // Use dispatch function with context
                    dispatch_host_crypto(op, &mut stack, c);
                } else {
                    // No context - legacy mode, Crypto not available
                    stack.pop();
                    stack.push(Value::I64(-1));
                }
                ip += 1;
            }

            Op::HostCallGeneric => {
                // Generic host call: delegate to ctx.generic.call()
                // Stack: [json_payload_string] -> [result_string]
                //
                // The VM parses the JSON payload, normalizes any VM handle values,
                // then delegates to the host's HostGenericCall implementation.
                let payload = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    _ => {
                        eprintln!("[VM] HostCallGeneric: expected string payload");
                        stack.push(Value::Str("{\"error\":\"invalid payload\"}".to_string()));
                        ip += 1;
                        continue;
                    }
                };

                // Debug: trace host calls with set_data
                if payload.contains("set_data") {
                    eprintln!("[VM_DEBUG] HostCallGeneric (set_data) at ip={}: payload='{}'", ip, &payload.chars().take(200).collect::<String>());
                }

                // Parse JSON payload
                let result = match serde_json::from_str::<serde_json::Value>(&payload) {
                    Ok(json) => {
                        let fn_name = json.get("fn").and_then(|v| v.as_str()).unwrap_or("");
                        let mut args = json.get("args").cloned().unwrap_or(serde_json::Value::Null);

                        // Normalize VM handles in args before passing to host.
                        // The VM is the only place that knows about handle ranges and can serialize them.
                        // This allows the host to receive plain JSON values instead of opaque handles.
                        if let Some(obj) = args.as_object_mut() {
                            if let Some(serde_json::Value::String(s)) = obj.get("value") {
                                if let Ok(handle) = s.parse::<i64>() {
                                    let serialized = if (50_000..60_000).contains(&handle) {
                                        Some(json_stringify_list_recursive(handle))
                                    } else if (60_000..70_000).contains(&handle) {
                                        Some(json_stringify_map_recursive(handle))
                                    } else if (90_000..100_000).contains(&handle) {
                                        Some(json_stringify_struct(handle))
                                    } else if (110_000..200_000).contains(&handle) {
                                        if let Ok(store) = json_store().lock() {
                                            store.get(&handle).map(json_stringify_jsonval)
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    };
                                    if let Some(json_str) = serialized {
                                        obj.insert("value".to_string(), serde_json::Value::String(json_str));
                                    }
                                }
                            }
                        }

                        // Delegate to the host's HostGenericCall implementation
                        if let Some(c) = ctx {
                            c.generic.call(fn_name, &args, &c.local_state)
                        } else {
                            // No context - use default behavior
                            crate::StdHostGenericCall::new().call(
                                fn_name,
                                &args,
                                &std::sync::RwLock::new(std::collections::HashMap::new()),
                            )
                        }
                    }
                    Err(e) => {
                        eprintln!("[VM] HostCallGeneric: failed to parse JSON: {}", e);
                        format!("{{\"error\":\"JSON parse error: {}\"}}", e)
                    }
                };

                stack.push(Value::Str(result));
                ip += 1;
            }

            // --- Async/Task operations ---
            Op::TaskSpawn => {
                // Stack: fn_id, argc -> task_handle
                // Creates a task handle for an async function
                let (Some(Value::I64(argc)), Some(Value::I64(fn_id))) =
                    (stack.pop(), stack.pop())
                else {
                    eprintln!("[VM] TaskSpawn: expected fn_id, argc on stack");
                    return InterpreterResult::Failed(1);
                };
                let handle = __arth_task_spawn_fn(fn_id, argc);
                stack.push(Value::I64(handle));
                ip += 1;
            }
            Op::TaskPushArg => {
                // Stack: task_handle, arg_value -> 0
                // Pushes an argument to a pending task
                let (Some(arg_val), Some(Value::I64(handle))) = (stack.pop(), stack.pop()) else {
                    eprintln!("[VM] TaskPushArg: expected handle, arg on stack");
                    return InterpreterResult::Failed(1);
                };
                let arg = match arg_val {
                    Value::I64(n) => n,
                    Value::Bool(b) => {
                        if b {
                            1
                        } else {
                            0
                        }
                    }
                    Value::F64(f) => f.to_bits() as i64,
                    // For strings, we pass the raw bits; async args are primarily i64
                    Value::Str(_s) => 0, // Strings need special handling in real impl
                };
                __arth_task_push_arg(handle, arg);
                stack.push(Value::I64(0));
                ip += 1;
            }
            Op::TaskAwait => {
                // Stack: task_handle -> result_value
                // Cooperative State Machine: Execute body if task is pending, then return result
                let Some(Value::I64(handle)) = stack.pop() else {
                    eprintln!("[VM] TaskAwait: expected task_handle on stack");
                    return InterpreterResult::Failed(1);
                };

                // Check task state
                let (is_pending, fn_id, argc) = {
                    if let Ok(m) = task_store().lock() {
                        if let Some(info) = m.get(&handle) {
                            match info.state {
                                TaskState::Pending => (true, info.fn_id, info.argc),
                                TaskState::Completed(result) => {
                                    // Already completed - return result directly
                                    stack.push(Value::I64(result));
                                    ip += 1;
                                    continue;
                                }
                                TaskState::Cancelled => {
                                    // Task was cancelled - throw CancelledError
                                    let exc = __arth_create_cancelled_error(handle);
                                    stack.push(Value::I64(exc));
                                    if let Some((handler_ip, handler_depth)) = throw_exception(exc) {
                                        while frames.len() > handler_depth {
                                            frames.pop();
                                            ret_stack.pop();
                                        }
                                        ip = handler_ip as usize;
                                        continue;
                                    } else {
                                        // No handler - propagate as unhandled exception
                                        eprintln!("uncaught CancelledError from awaited task");
                                        return InterpreterResult::Failed(3);
                                    }
                                }
                                TaskState::Panicked(ref msg) => {
                                    panic_with_message(msg.clone());
                                    stack.push(Value::I64(-2)); // Panicked sentinel
                                    ip += 1;
                                    continue;
                                }
                                TaskState::Running => {
                                    // Task is already running - shouldn't happen in single-threaded
                                    stack.push(Value::I64(0));
                                    ip += 1;
                                    continue;
                                }
                                TaskState::Error(exc) => {
                                    // Task threw an exception - propagate it to the awaiter
                                    // Push the exception onto the stack and throw
                                    stack.push(Value::I64(exc));
                                    if let Some((handler_ip, handler_depth)) = throw_exception(exc) {
                                        while frames.len() > handler_depth {
                                            frames.pop();
                                            ret_stack.pop();
                                        }
                                        ip = handler_ip as usize;
                                        continue;
                                    } else {
                                        // No handler - propagate as unhandled exception
                                        eprintln!("uncaught exception from awaited task");
                                        return InterpreterResult::Failed(3);
                                    }
                                }
                            }
                        } else {
                            // Task not found - return 0
                            stack.push(Value::I64(0));
                            ip += 1;
                            continue;
                        }
                    } else {
                        stack.push(Value::I64(0));
                        ip += 1;
                        continue;
                    }
                };

                if is_pending {
                    // Look up body function offset from async_dispatch table
                    if let Some(func_offset) = p.get_async_body_offset(fn_id) {
                        // Mark task as running
                        __arth_task_mark_running(handle);

                        // Push arguments in reverse order (will be popped by callee)
                        for i in (0..argc).rev() {
                            let arg = __arth_task_get_arg(handle, i as i64);
                            stack.push(Value::I64(arg));
                        }

                        // Store task handle for result capture on return
                        // The return value will be on stack after body executes
                        // We use a continuation-style approach: after body returns,
                        // we need to capture result and complete the task
                        // For now, we push a special marker and handle in Ret

                        // Store pending task handle for result capture
                        static PENDING_AWAIT_TASK: AtomicI64 = AtomicI64::new(-1);
                        PENDING_AWAIT_TASK.store(handle, std::sync::atomic::Ordering::SeqCst);

                        // Call body function - result will be on stack after return
                        ret_stack.push(ip + 1);
                        frames.push(Vec::new());
                        ip = func_offset as usize;
                    } else {
                        // Body function not found in dispatch table
                        eprintln!("[VM] TaskAwait: body function for fn_id {} not found", fn_id);
                        stack.push(Value::I64(0));
                        ip += 1;
                    }
                } else {
                    // This shouldn't be reached due to the continue statements above
                    let result = __arth_await(handle);
                    stack.push(Value::I64(result));
                    ip += 1;
                }
            }
            Op::TaskRunBody(func_offset) => {
                // Execute async body function and store result in task
                // Stack: task_handle -> task_handle (task now completed)
                // This opcode is emitted after TaskSpawn+TaskPushArg to execute the body
                let Some(Value::I64(handle)) = stack.pop() else {
                    eprintln!("[VM] TaskRunBody: expected task_handle on stack");
                    return InterpreterResult::Failed(1);
                };

                // Mark task as running
                __arth_task_mark_running(handle);

                // Get the argument count
                let argc = {
                    if let Ok(m) = task_store().lock() {
                        m.get(&handle).map(|info| info.argc).unwrap_or(0)
                    } else {
                        0
                    }
                };

                // Push arguments in correct order (will be popped by callee)
                for i in (0..argc).rev() {
                    let arg = __arth_task_get_arg(handle, i as i64);
                    stack.push(Value::I64(arg));
                }

                // Store the task handle so we can capture the result on return
                // Use a static to track pending async execution
                static PENDING_TASK: AtomicI64 = AtomicI64::new(-1);
                PENDING_TASK.store(handle, std::sync::atomic::Ordering::SeqCst);

                // Push special return marker and call the body function
                // We'll intercept the return value after the call
                ret_stack.push(ip + 1);
                frames.push(Vec::new());
                ip = func_offset as usize;
            }

            Op::Call(tgt) => {
                // DEBUG: Trace ALL calls to key functions BEFORE JIT
                if tgt == 330 || tgt == 704 || tgt == 1056 {
                    let fn_name = match tgt {
                        330 => "getStaticThreadList",
                        704 => "findById",
                        1056 => "handleSelectThread",
                        _ => "unknown"
                    };
                    eprintln!("[VM_DEBUG] Op::Call at ip={}: target {}() (offset {})", ip, fn_name, tgt);
                }
                // JIT dispatch: try to execute via JIT if available
                #[cfg(feature = "jit")]
                {
                    use crate::jit_interp::{try_jit_call, JitCallResult};

                    // Get param count for this function
                    let param_count = get_func_param_count(p, tgt) as usize;

                    // Pop args from stack (in reverse order since stack is LIFO)
                    let mut args: Vec<i64> = Vec::with_capacity(param_count);
                    let mut popped_values: Vec<Value> = Vec::with_capacity(param_count);
                    let mut all_i64 = true;

                    for _ in 0..param_count {
                        if let Some(val) = stack.pop() {
                            popped_values.push(val.clone());
                            match val {
                                Value::I64(n) => args.push(n),
                                _ => {
                                    all_i64 = false;
                                    break;
                                }
                            }
                        } else {
                            all_i64 = false;
                            break;
                        }
                    }

                    if all_i64 && args.len() == param_count {
                        // Reverse args to restore original order (first arg first)
                        args.reverse();

                        match try_jit_call(tgt, &args, p) {
                            JitCallResult::Executed(result) => {
                                // JIT execution succeeded, push result and continue
                                stack.push(Value::I64(result));
                                ip += 1;
                                continue;
                            }
                            JitCallResult::ShouldCompile | JitCallResult::UseInterpreter => {
                                // Fall back to interpreter - push args back in reverse order
                                for val in popped_values.into_iter().rev() {
                                    stack.push(val);
                                }
                            }
                            JitCallResult::Error(_e) => {
                                // JIT error, fall back to interpreter
                                for val in popped_values.into_iter().rev() {
                                    stack.push(val);
                                }
                            }
                        }
                    } else {
                        // Not all i64 or wrong arg count - push back and fall through
                        for val in popped_values.into_iter().rev() {
                            stack.push(val);
                        }
                    }
                }

                // Normal interpreter path (fallback or non-JIT)
                // DEBUG: Trace calls to key functions
                if tgt == 330 || tgt == 704 || tgt == 1056 {
                    let fn_name = match tgt {
                        330 => "getStaticThreadList",
                        704 => "findById",
                        1056 => "handleSelectThread",
                        _ => "unknown"
                    };
                    eprintln!("[VM_DEBUG] Call at ip={}: calling {}() (offset {}), stack.len={}", ip, fn_name, tgt, stack.len());
                    // Print top of stack
                    if let Some(top) = stack.last() {
                        eprintln!("[VM_DEBUG]   stack top: {:?}", top);
                    }
                }
                ret_stack.push(ip + 1);

                // Pre-allocate locals for the callee based on scanning the function.
                let local_count = scan_function_local_count(p, tgt);
                let mut new_frame: Vec<Option<Value>> = vec![None; local_count as usize];

                // Check if the function has a prologue that pops args (starts with LocalSet).
                // If not, we need to place the argument in the first local the function reads.
                // This handles WAID-compiled functions which expect args already in specific locals.
                let first_op = p.code.get(tgt as usize);
                let has_prologue = matches!(first_op, Some(Op::LocalSet(_)));

                if !has_prologue && !stack.is_empty() {
                    // Find the first LocalGet in the function to determine where arg goes
                    let first_local_get = find_first_local_get(p, tgt);
                    if let Some(arg) = stack.pop() {
                        let target_local = first_local_get.unwrap_or(0) as usize;
                        if target_local < new_frame.len() {
                            new_frame[target_local] = Some(arg);
                        }
                    }
                }

                frames.push(new_frame);
                ip = tgt as usize;
            }
            Op::CallSymbol(sym) => {
                // Call a function by symbolic name (for cross-library calls)
                let name = match p.strings.get(sym as usize) {
                    Some(s) => s.as_str(),
                    None => {
                        panic_with_message(format!(
                            "CallSymbol: invalid symbol string index {}",
                            sym
                        ));
                        return InterpreterResult::Failed(2);
                    }
                };

                // Handle WAID built-in functions (string methods, JSON, etc.)
                if name == "s.charAt" {
                    // Built-in string method: s.charAt(index)
                    // Stack: [string, index] -> [char_string]
                    let index = match stack.pop() {
                        Some(Value::I64(i)) => i as usize,
                        Some(Value::F64(f)) => f as usize,
                        _ => 0,
                    };
                    let s = match stack.pop() {
                        Some(Value::Str(s)) => s,
                        _ => {
                            // String not on stack - look in local 1 (typical calling convention)
                            if let Some(frame) = frames.last() {
                                if let Some(Some(Value::Str(s))) = frame.get(1) {
                                    s.clone()
                                } else {
                                    String::new()
                                }
                            } else {
                                String::new()
                            }
                        }
                    };
                    let result = s.chars().nth(index).map(|c| c.to_string()).unwrap_or_default();
                    stack.push(Value::Str(result));
                    ip += 1;
                } else if name == "s.length" {
                    // Built-in string property: s.length
                    // Stack: [string] -> [length]
                    let s = match stack.pop() {
                        Some(Value::Str(s)) => s,
                        _ => String::new(),
                    };
                    stack.push(Value::I64(s.chars().count() as i64));
                    ip += 1;
                } else if name == "Json.decode" {
                    // Pop the JSON string to decode
                    let json_str = match stack.pop() {
                        Some(Value::Str(s)) => s,
                        _ => "{}".to_string(),
                    };
                    // Parse and store, return handle
                    let handle = json_parse_and_store(&json_str);
                    stack.push(Value::I64(handle));
                    ip += 1;
                } else if name == "Json.encode" {
                    // Pop a value and convert to JSON string
                    // Handles: JSON store handles, strings, numbers, booleans
                    let result = match stack.pop() {
                        Some(Value::I64(handle)) => {
                            // Try to get from JSON store first
                            if let Ok(store) = json_store().lock() {
                                if let Some(value) = store.get(&handle) {
                                    json_stringify_jsonval(value)
                                } else {
                                    // Not in store, treat as number
                                    handle.to_string()
                                }
                            } else {
                                handle.to_string()
                            }
                        }
                        Some(Value::Str(s)) => {
                            // Escape the string for JSON output
                            json_escape_string(&s)
                        }
                        Some(Value::F64(f)) => {
                            if f.is_nan() || f.is_infinite() {
                                "null".to_string()
                            } else {
                                f.to_string()
                            }
                        }
                        Some(Value::Bool(b)) => if b { "true" } else { "false" }.to_string(),
                        None => "null".to_string(),
                    };
                    stack.push(Value::Str(result));
                    ip += 1;
                } else if name == "s.indexOf" {
                    // Built-in string method: s.indexOf(search)
                    // Stack: [string, search] -> [index]
                    let search = match stack.pop() {
                        Some(Value::Str(s)) => s,
                        _ => String::new(),
                    };
                    let s = match stack.pop() {
                        Some(Value::Str(s)) => s,
                        _ => String::new(),
                    };
                    let result = s.find(&search).map(|i| i as i64).unwrap_or(-1);
                    stack.push(Value::I64(result));
                    ip += 1;
                } else if name == "s.substring" {
                    // Built-in string method: s.substring(start, end)
                    // Stack: [string, start, end] -> [substring]
                    let end = match stack.pop() {
                        Some(Value::I64(i)) => i as usize,
                        Some(Value::F64(f)) => f as usize,
                        _ => 0,
                    };
                    let start = match stack.pop() {
                        Some(Value::I64(i)) => i as usize,
                        Some(Value::F64(f)) => f as usize,
                        _ => 0,
                    };
                    let s = match stack.pop() {
                        Some(Value::Str(s)) => s,
                        _ => String::new(),
                    };
                    let chars: Vec<char> = s.chars().collect();
                    let start = start.min(chars.len());
                    let end = end.min(chars.len());
                    let result: String = if start <= end {
                        chars[start..end].iter().collect()
                    } else {
                        chars[end..start].iter().collect()
                    };
                    stack.push(Value::Str(result));
                    ip += 1;
                } else if name == "s.includes" {
                    // Built-in string method: s.includes(search)
                    // Stack: [string, search] -> [bool as i64]
                    let search = match stack.pop() {
                        Some(Value::Str(s)) => s,
                        _ => String::new(),
                    };
                    let s = match stack.pop() {
                        Some(Value::Str(s)) => s,
                        _ => String::new(),
                    };
                    let result = if s.contains(&search) { 1i64 } else { 0i64 };
                    stack.push(Value::I64(result));
                    ip += 1;
                } else if name == "s.startsWith" {
                    // Built-in string method: s.startsWith(prefix)
                    // Stack: [string, prefix] -> [bool as i64]
                    let prefix = match stack.pop() {
                        Some(Value::Str(s)) => s,
                        _ => String::new(),
                    };
                    let s = match stack.pop() {
                        Some(Value::Str(s)) => s,
                        _ => String::new(),
                    };
                    let result = if s.starts_with(&prefix) { 1i64 } else { 0i64 };
                    stack.push(Value::I64(result));
                    ip += 1;
                } else if name == "s.endsWith" {
                    // Built-in string method: s.endsWith(suffix)
                    // Stack: [string, suffix] -> [bool as i64]
                    let suffix = match stack.pop() {
                        Some(Value::Str(s)) => s,
                        _ => String::new(),
                    };
                    let s = match stack.pop() {
                        Some(Value::Str(s)) => s,
                        _ => String::new(),
                    };
                    let result = if s.ends_with(&suffix) { 1i64 } else { 0i64 };
                    stack.push(Value::I64(result));
                    ip += 1;
                } else {
                    // DEBUG: Trace CallSymbol lookup for key functions
                    if name.contains("findById") || name.contains("getStaticThreadList") || ip >= 1080 && ip <= 1090 {
                        eprintln!("[VM_DEBUG] CallSymbol at ip={}: looking up symbol '{}' (string index {})", ip, name, sym);
                    }
                    // Look up the symbol in the linked symbol table
                    match lookup_symbol(name) {
                        Some(offset) => {
                            if name.contains("findById") || name.contains("getStaticThreadList") || ip >= 1080 && ip <= 1090 {
                                eprintln!("[VM_DEBUG] CallSymbol: found '{}' at offset {}", name, offset);
                            }
                            ret_stack.push(ip + 1);
                            frames.push(Vec::new());
                            ip = offset as usize;
                        }
                        None => {
                            eprintln!("[VM_DEBUG] CallSymbol: symbol '{}' NOT FOUND in linked_symbol_table", name);
                            panic_with_message(format!(
                                "CallSymbol: undefined symbol '{}' - ensure the library is linked",
                                name
                            ));
                            return InterpreterResult::Failed(2);
                        }
                    }
                }
            }
            Op::Ret => {
                // In export call mode, capture the return value
                let return_value = if is_export_call { stack.pop() } else { None };

                // Pop frame and return to caller
                let _ = frames.pop();
                if let Some(rip) = ret_stack.pop() {
                    // Push return value back onto stack for caller
                    if let Some(v) = return_value {
                        stack.push(v);
                    }
                    ip = rip;
                } else {
                    // No return address - we're returning from the top-level function
                    if is_export_call {
                        // Extract i64 return value if present
                        let i64_return = return_value.and_then(|v| match v {
                            Value::I64(n) => Some(n),
                            _ => None,
                        });
                        return InterpreterResult::ReturnValue(i64_return);
                    }
                    break;
                }
            }
            Op::ExternCall {
                sym,
                argc,
                float_mask,
                ret_kind,
            } => {
                let name = match p.strings.get(sym as usize) {
                    Some(s) => s.as_str(),
                    None => {
                        panic_with_message(format!(
                            "extern call: invalid symbol string index {}",
                            sym
                        ));
                        return InterpreterResult::Failed(2);
                    }
                };
                let argc_usize = argc as usize;
                if argc_usize > VM_EXTERN_CALL_MAX_ARGS {
                    panic_with_message(format!(
                        "extern call '{}' has {} args, but VM supports at most {}",
                        name, argc_usize, VM_EXTERN_CALL_MAX_ARGS
                    ));
                    return InterpreterResult::Failed(2);
                }

                let ptr = match vm_resolve_extern_symbol(name) {
                    Ok(p) => p,
                    Err(e) => {
                        panic_with_message(e);
                        return InterpreterResult::Failed(2);
                    }
                };

                let mut raw_args: Vec<Value> = Vec::with_capacity(argc_usize);
                for _ in 0..argc_usize {
                    let Some(v) = stack.pop() else {
                        panic_with_message(format!(
                            "extern call '{}' expected {} args on stack",
                            name, argc_usize
                        ));
                        return InterpreterResult::Failed(2);
                    };
                    raw_args.push(v);
                }
                raw_args.reverse();

                let mut iargs = [0i64; VM_EXTERN_CALL_MAX_ARGS];
                let mut fargs = [0f64; VM_EXTERN_CALL_MAX_ARGS];
                for (i, v) in raw_args.iter().enumerate() {
                    iargs[i] = match vm_value_to_i64(v) {
                        Some(n) => n,
                        None => {
                            panic_with_message(format!(
                                "extern call '{}' argument {} is not an integer-like value",
                                name,
                                i + 1
                            ));
                            return InterpreterResult::Failed(2);
                        }
                    };
                    fargs[i] = match vm_value_to_f64(v) {
                        Some(n) => n,
                        None => {
                            panic_with_message(format!(
                                "extern call '{}' argument {} is not a numeric value",
                                name,
                                i + 1
                            ));
                            return InterpreterResult::Failed(2);
                        }
                    };
                }

                let out = unsafe {
                    match ret_kind {
                        0 => Value::I64(vm_call_extern_i64(
                            ptr,
                            argc_usize,
                            float_mask,
                            &iargs,
                            &fargs,
                        )),
                        1 => Value::F64(vm_call_extern_f64(
                            ptr,
                            argc_usize,
                            float_mask,
                            &iargs,
                            &fargs,
                        )),
                        2 => {
                            vm_call_extern_void(ptr, argc_usize, float_mask, &iargs, &fargs);
                            Value::I64(0)
                        }
                        _ => {
                            panic_with_message(format!(
                                "extern call '{}' has unknown ret_kind {}",
                                name, ret_kind
                            ));
                            return InterpreterResult::Failed(2);
                        }
                    }
                };
                stack.push(out);
                ip += 1;
            }

            // =========================================================================
            // Concurrent Runtime Operations
            // =========================================================================
            Op::ExecutorInit => {
                // Stack: num_threads -> success (1 if initialized, 0 if already initialized)
                let num_threads = match stack.pop() {
                    Some(Value::I64(n)) => n as usize,
                    _ => 4, // Default to 4 threads
                };
                let success = if init_global_concurrent_pool(num_threads) { 1 } else { 0 };
                stack.push(Value::I64(success));
                ip += 1;
            }
            Op::ExecutorThreadCount => {
                // Stack: -> thread_count
                let count = global_concurrent_pool_thread_count() as i64;
                stack.push(Value::I64(count));
                ip += 1;
            }
            Op::ExecutorActiveWorkers => {
                // Stack: -> active_count
                let count = global_concurrent_pool_active_workers() as i64;
                stack.push(Value::I64(count));
                ip += 1;
            }
            Op::ExecutorSpawn => {
                // Stack: fn_id -> task_id
                let fn_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let task_id = spawn_concurrent_task(fn_id, vec![]);
                stack.push(Value::I64(task_id as i64));
                ip += 1;
            }
            Op::ExecutorJoin => {
                // Stack: task_id -> result
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n as u64,
                    _ => 0,
                };
                let result = join_concurrent_task(task_id);
                stack.push(Value::I64(result));
                ip += 1;
            }
            Op::ExecutorSpawnWithArg => {
                // Stack: fn_id, arg -> task_id
                // Spawns a task with fn_id as work type and arg as parameter
                let arg = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let fn_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let task_id = spawn_concurrent_task_with_args(fn_id, vec![arg]);
                stack.push(Value::I64(task_id as i64));
                ip += 1;
            }
            Op::ExecutorActiveExecutorCount => {
                // Stack: -> count
                // Returns how many workers executed at least one task
                let count = global_active_executor_count() as i64;
                stack.push(Value::I64(count));
                ip += 1;
            }
            Op::ExecutorWorkerTaskCount => {
                // Stack: worker_idx -> count
                // Returns task count for specific worker
                let worker_idx = match stack.pop() {
                    Some(Value::I64(n)) => n as usize,
                    _ => 0,
                };
                let count = global_worker_task_count(worker_idx) as i64;
                stack.push(Value::I64(count));
                ip += 1;
            }
            Op::ExecutorResetStats => {
                // Stack: -> 0
                // Resets all worker task counters
                global_reset_worker_task_counts();
                stack.push(Value::I64(0));
                ip += 1;
            }
            Op::ExecutorSpawnAwait => {
                // Stack: sub_fn_id, sub_arg, local_accumulator -> task_id
                // Spawns work type 3 (spawn-and-await) task
                let local_accumulator = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let sub_arg = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let sub_fn_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 2, // default to fibonacci
                };
                // Work type 3 = spawn-and-await
                // args: [sub_fn_id, sub_arg, local_accumulator]
                let task_id = spawn_concurrent_task_with_args(3, vec![sub_fn_id, sub_arg, local_accumulator]);
                stack.push(Value::I64(task_id as i64));
                ip += 1;
            }

            // =========================================================================
            // MPMC Channel Operations (C06)
            // =========================================================================
            Op::MpmcChanCreate => {
                // Stack: capacity -> channel_handle
                let capacity = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = __arth_mpmc_chan_create(capacity);
                stack.push(Value::I64(handle));
                ip += 1;
            }
            Op::MpmcChanSend => {
                // Stack: channel_handle, value -> status
                let value = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let status = __arth_mpmc_chan_send(handle, value);
                stack.push(Value::I64(status as i64));
                ip += 1;
            }
            Op::MpmcChanSendBlocking => {
                // Stack: channel_handle, value -> status
                let value = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let status = __arth_mpmc_chan_send_blocking(handle, value);
                stack.push(Value::I64(status as i64));
                ip += 1;
            }
            Op::MpmcChanRecv => {
                // Stack: channel_handle -> value
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let value = __arth_mpmc_chan_recv(handle);
                stack.push(Value::I64(value));
                ip += 1;
            }
            Op::MpmcChanRecvBlocking => {
                // Stack: channel_handle -> value
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let value = __arth_mpmc_chan_recv_blocking(handle);
                stack.push(Value::I64(value));
                ip += 1;
            }
            Op::MpmcChanClose => {
                // Stack: channel_handle -> status
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let status = __arth_mpmc_chan_close(handle);
                stack.push(Value::I64(status as i64));
                ip += 1;
            }
            Op::MpmcChanLen => {
                // Stack: channel_handle -> length
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let len = __arth_mpmc_chan_len(handle);
                stack.push(Value::I64(len));
                ip += 1;
            }
            Op::MpmcChanIsEmpty => {
                // Stack: channel_handle -> is_empty
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let is_empty = __arth_mpmc_chan_is_empty(handle);
                stack.push(Value::I64(is_empty as i64));
                ip += 1;
            }
            Op::MpmcChanIsFull => {
                // Stack: channel_handle -> is_full
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let is_full = __arth_mpmc_chan_is_full(handle);
                stack.push(Value::I64(is_full as i64));
                ip += 1;
            }
            Op::MpmcChanIsClosed => {
                // Stack: channel_handle -> is_closed
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let is_closed = __arth_mpmc_chan_is_closed(handle);
                stack.push(Value::I64(is_closed as i64));
                ip += 1;
            }
            Op::MpmcChanCapacity => {
                // Stack: channel_handle -> capacity
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let capacity = __arth_mpmc_chan_capacity(handle);
                stack.push(Value::I64(capacity));
                ip += 1;
            }

            // =========================================================================
            // C07: Executor-Integrated MPMC Channel Operations
            // =========================================================================
            Op::MpmcChanSendWithTask => {
                // Stack: channel_handle, value, task_id -> status
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n as u64,
                    _ => 0,
                };
                let value = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let status = __arth_mpmc_chan_send_with_task(handle, value, task_id);
                stack.push(Value::I64(status as i64));
                ip += 1;
            }
            Op::MpmcChanRecvWithTask => {
                // Stack: channel_handle, task_id -> value
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n as u64,
                    _ => 0,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let value = __arth_mpmc_chan_recv_with_task(handle, task_id);
                stack.push(Value::I64(value));
                ip += 1;
            }
            Op::MpmcChanRecvAndWake => {
                // Stack: channel_handle -> value
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let value = __arth_mpmc_chan_recv_and_wake(handle);
                stack.push(Value::I64(value));
                ip += 1;
            }
            Op::MpmcChanPopWaitingSender => {
                // Stack: channel_handle -> task_id
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let task_id = __arth_mpmc_chan_pop_waiting_sender(handle);
                stack.push(Value::I64(task_id as i64));
                ip += 1;
            }
            Op::MpmcChanGetWaitingSenderValue => {
                // Stack: -> value
                let value = __arth_mpmc_chan_get_waiting_sender_value();
                stack.push(Value::I64(value));
                ip += 1;
            }
            Op::MpmcChanPopWaitingReceiver => {
                // Stack: channel_handle -> task_id
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let task_id = __arth_mpmc_chan_pop_waiting_receiver(handle);
                stack.push(Value::I64(task_id as i64));
                ip += 1;
            }
            Op::MpmcChanWaitingSenderCount => {
                // Stack: channel_handle -> count
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let count = __arth_mpmc_chan_waiting_sender_count(handle);
                stack.push(Value::I64(count));
                ip += 1;
            }
            Op::MpmcChanWaitingReceiverCount => {
                // Stack: channel_handle -> count
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let count = __arth_mpmc_chan_waiting_receiver_count(handle);
                stack.push(Value::I64(count));
                ip += 1;
            }
            Op::MpmcChanGetWokenSender => {
                // Stack: -> task_id
                let task_id = __arth_mpmc_chan_get_woken_sender();
                stack.push(Value::I64(task_id as i64));
                ip += 1;
            }

            // =========================================================================
            // C08: Blocking Receive Operations
            // =========================================================================
            Op::MpmcChanSendAndWake => {
                // Stack: channel_handle, value -> status
                let value = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let status = __arth_mpmc_chan_send_and_wake(handle, value);
                stack.push(Value::I64(status as i64));
                ip += 1;
            }
            Op::MpmcChanGetWokenReceiver => {
                // Stack: -> task_id
                let task_id = __arth_mpmc_chan_get_woken_receiver();
                stack.push(Value::I64(task_id as i64));
                ip += 1;
            }

            // =========================================================================
            // C09: Channel Select Operations
            // =========================================================================
            Op::MpmcChanSelectClear => {
                // Stack: -> (no result)
                __arth_mpmc_chan_select_clear();
                ip += 1;
            }
            Op::MpmcChanSelectAdd => {
                // Stack: channel_handle -> index
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let index = __arth_mpmc_chan_select_add(handle);
                stack.push(Value::I64(index));
                ip += 1;
            }
            Op::MpmcChanSelectCount => {
                // Stack: -> count
                let count = __arth_mpmc_chan_select_count();
                stack.push(Value::I64(count));
                ip += 1;
            }
            Op::MpmcChanTrySelectRecv => {
                // Stack: -> status
                let status = __arth_mpmc_chan_try_select_recv();
                stack.push(Value::I64(status as i64));
                ip += 1;
            }
            Op::MpmcChanSelectRecvBlocking => {
                // Stack: -> status
                let status = __arth_mpmc_chan_select_recv_blocking();
                stack.push(Value::I64(status as i64));
                ip += 1;
            }
            Op::MpmcChanSelectRecvWithTask => {
                // Stack: task_id -> status
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n as u64,
                    _ => 0,
                };
                let status = __arth_mpmc_chan_select_recv_with_task(task_id);
                stack.push(Value::I64(status as i64));
                ip += 1;
            }
            Op::MpmcChanSelectGetReadyIndex => {
                // Stack: -> index
                let index = __arth_mpmc_chan_select_get_ready_index();
                stack.push(Value::I64(index));
                ip += 1;
            }
            Op::MpmcChanSelectGetValue => {
                // Stack: -> value
                let value = __arth_mpmc_chan_select_get_value();
                stack.push(Value::I64(value));
                ip += 1;
            }
            Op::MpmcChanSelectDeregister => {
                // Stack: task_id, except_index -> (no result)
                let except_index = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n as u64,
                    _ => 0,
                };
                __arth_mpmc_chan_select_deregister(task_id, except_index);
                ip += 1;
            }
            Op::MpmcChanSelectGetHandle => {
                // Stack: index -> handle
                let index = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let handle = __arth_mpmc_chan_select_get_handle(index);
                stack.push(Value::I64(handle));
                ip += 1;
            }

            // =====================================================================
            // C11: Actor Operations (Actor = Task + Channel)
            // =====================================================================

            Op::ActorCreate => {
                // Stack: capacity -> actor_handle
                let capacity = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 10, // default capacity
                };
                let handle = __arth_actor_create(capacity);
                stack.push(Value::I64(handle));
                ip += 1;
            }

            Op::ActorSpawn => {
                // Stack: capacity, task_handle -> actor_handle
                let task_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let capacity = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 10, // default capacity
                };
                let handle = __arth_actor_spawn(capacity, task_handle);
                stack.push(Value::I64(handle));
                ip += 1;
            }

            Op::ActorSend => {
                // Stack: actor_handle, message -> status
                let message = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_actor_send(actor_handle, message);
                stack.push(Value::I64(status as i64));
                ip += 1;
            }

            Op::ActorSendBlocking => {
                // Stack: actor_handle, message -> status
                let message = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_actor_send_blocking(actor_handle, message);
                stack.push(Value::I64(status as i64));
                ip += 1;
            }

            Op::ActorRecv => {
                // Stack: actor_handle -> message
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let message = __arth_actor_recv(actor_handle);
                stack.push(Value::I64(message));
                ip += 1;
            }

            Op::ActorRecvBlocking => {
                // Stack: actor_handle -> message
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let message = __arth_actor_recv_blocking(actor_handle);
                stack.push(Value::I64(message));
                ip += 1;
            }

            Op::ActorClose => {
                // Stack: actor_handle -> status
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_actor_close(actor_handle);
                stack.push(Value::I64(status as i64));
                ip += 1;
            }

            Op::ActorStop => {
                // Stack: actor_handle -> status
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_actor_stop(actor_handle);
                stack.push(Value::I64(status as i64));
                ip += 1;
            }

            Op::ActorGetTask => {
                // Stack: actor_handle -> task_handle
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let task_handle = __arth_actor_get_task(actor_handle);
                stack.push(Value::I64(task_handle));
                ip += 1;
            }

            Op::ActorGetMailbox => {
                // Stack: actor_handle -> mailbox_handle
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let mailbox_handle = __arth_actor_get_mailbox(actor_handle);
                stack.push(Value::I64(mailbox_handle));
                ip += 1;
            }

            Op::ActorIsRunning => {
                // Stack: actor_handle -> is_running
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let is_running = __arth_actor_is_running(actor_handle);
                stack.push(Value::I64(is_running as i64));
                ip += 1;
            }

            Op::ActorGetState => {
                // Stack: actor_handle -> state
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let state = __arth_actor_get_state(actor_handle);
                stack.push(Value::I64(state as i64));
                ip += 1;
            }

            Op::ActorMessageCount => {
                // Stack: actor_handle -> count
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let count = __arth_actor_message_count(actor_handle);
                stack.push(Value::I64(count));
                ip += 1;
            }

            Op::ActorMailboxEmpty => {
                // Stack: actor_handle -> is_empty
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let is_empty = __arth_actor_mailbox_empty(actor_handle);
                stack.push(Value::I64(is_empty as i64));
                ip += 1;
            }

            Op::ActorMailboxLen => {
                // Stack: actor_handle -> length
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let length = __arth_actor_mailbox_len(actor_handle);
                stack.push(Value::I64(length));
                ip += 1;
            }

            Op::ActorSetTask => {
                // Stack: actor_handle, task_handle -> status
                let task_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_actor_set_task(actor_handle, task_handle);
                stack.push(Value::I64(status as i64));
                ip += 1;
            }

            Op::ActorMarkStopped => {
                // Stack: actor_handle -> status
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_actor_mark_stopped(actor_handle);
                stack.push(Value::I64(status as i64));
                ip += 1;
            }

            Op::ActorMarkFailed => {
                // Stack: actor_handle -> status
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_actor_mark_failed(actor_handle);
                stack.push(Value::I64(status as i64));
                ip += 1;
            }

            Op::ActorIsFailed => {
                // Stack: actor_handle -> result
                let actor_handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let result = __arth_actor_is_failed(actor_handle);
                stack.push(Value::I64(result as i64));
                ip += 1;
            }

            // ================================================================
            // Phase 4: Atomic<T> Operations (C19)
            // ================================================================

            Op::AtomicCreate => {
                // Stack: initial_value -> atomic_handle
                let initial_value = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = __arth_atomic_create(initial_value);
                stack.push(Value::I64(handle));
                ip += 1;
            }

            Op::AtomicLoad => {
                // Stack: atomic_handle -> value
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let value = __arth_atomic_load(handle);
                stack.push(Value::I64(value));
                ip += 1;
            }

            Op::AtomicStore => {
                // Stack: atomic_handle, value -> old_value
                let value = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let old_value = __arth_atomic_store(handle, value);
                stack.push(Value::I64(old_value));
                ip += 1;
            }

            Op::AtomicCas => {
                // Stack: atomic_handle, expected, new_value -> success (1 or 0)
                let new_value = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let expected = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let success = __arth_atomic_cas(handle, expected, new_value);
                stack.push(Value::I64(success));
                ip += 1;
            }

            Op::AtomicFetchAdd => {
                // Stack: atomic_handle, delta -> old_value
                let delta = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let old_value = __arth_atomic_fetch_add(handle, delta);
                stack.push(Value::I64(old_value));
                ip += 1;
            }

            Op::AtomicFetchSub => {
                // Stack: atomic_handle, delta -> old_value
                let delta = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let old_value = __arth_atomic_fetch_sub(handle, delta);
                stack.push(Value::I64(old_value));
                ip += 1;
            }

            Op::AtomicSwap => {
                // Stack: atomic_handle, new_value -> old_value
                let new_value = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let old_value = __arth_atomic_swap(handle, new_value);
                stack.push(Value::I64(old_value));
                ip += 1;
            }

            Op::AtomicGet => {
                // Stack: atomic_handle -> value (alias for load)
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let value = __arth_atomic_get(handle);
                stack.push(Value::I64(value));
                ip += 1;
            }

            Op::AtomicSet => {
                // Stack: atomic_handle, value -> old_value (alias for store)
                let value = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let old_value = __arth_atomic_set(handle, value);
                stack.push(Value::I64(old_value));
                ip += 1;
            }

            Op::AtomicInc => {
                // Stack: atomic_handle -> old_value (increment by 1)
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let old_value = __arth_atomic_inc(handle);
                stack.push(Value::I64(old_value));
                ip += 1;
            }

            Op::AtomicDec => {
                // Stack: atomic_handle -> old_value (decrement by 1)
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let old_value = __arth_atomic_dec(handle);
                stack.push(Value::I64(old_value));
                ip += 1;
            }

            // =========================================================================
            // C21: Event Loop Operations
            // =========================================================================

            Op::EventLoopCreate => {
                // Stack: -> loop_handle
                let handle = __arth_event_loop_create();
                stack.push(Value::I64(handle));
                ip += 1;
            }

            Op::EventLoopRegisterTimer => {
                // Stack: loop_handle, timeout_ms -> token
                let timeout_ms = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let token = __arth_event_loop_register_timer(handle, timeout_ms);
                stack.push(Value::I64(token));
                ip += 1;
            }

            Op::EventLoopRegisterFd => {
                // Stack: loop_handle, fd, interest -> token
                let interest = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let fd = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let token = __arth_event_loop_register_fd(handle, fd, interest);
                stack.push(Value::I64(token));
                ip += 1;
            }

            Op::EventLoopDeregister => {
                // Stack: loop_handle, token -> status
                let token = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_event_loop_deregister(handle, token);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::EventLoopPoll => {
                // Stack: loop_handle, timeout_ms -> num_events
                let timeout_ms = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let count = __arth_event_loop_poll(handle, timeout_ms);
                stack.push(Value::I64(count));
                ip += 1;
            }

            Op::EventLoopGetEvent => {
                // Stack: index -> token
                let index = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let token = __arth_event_loop_get_event(index);
                stack.push(Value::I64(token));
                ip += 1;
            }

            Op::EventLoopGetEventType => {
                // Stack: index -> event_type
                let index = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let event_type = __arth_event_loop_get_event_type(index);
                stack.push(Value::I64(event_type));
                ip += 1;
            }

            Op::EventLoopClose => {
                // Stack: loop_handle -> status
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_event_loop_close(handle);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::EventLoopPipeCreate => {
                // Stack: -> read_fd
                let read_fd = __arth_event_loop_pipe_create();
                stack.push(Value::I64(read_fd));
                ip += 1;
            }

            Op::EventLoopPipeGetWriteFd => {
                // Stack: -> write_fd
                let write_fd = __arth_event_loop_pipe_get_write_fd();
                stack.push(Value::I64(write_fd));
                ip += 1;
            }

            Op::EventLoopPipeWrite => {
                // Stack: write_fd, value -> bytes_written
                let value = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let write_fd = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let result = __arth_event_loop_pipe_write(write_fd, value);
                stack.push(Value::I64(result));
                ip += 1;
            }

            Op::EventLoopPipeRead => {
                // Stack: read_fd -> value
                let read_fd = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let value = __arth_event_loop_pipe_read(read_fd);
                stack.push(Value::I64(value));
                ip += 1;
            }

            Op::EventLoopPipeClose => {
                // Stack: fd -> status
                let fd = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_event_loop_pipe_close(fd);
                stack.push(Value::I64(status));
                ip += 1;
            }

            // =========================================================================
            // C22: Timer Operations
            // =========================================================================

            Op::TimerSleep => {
                // Stack: ms -> ()
                let ms = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                __arth_timer_sleep(ms);
                ip += 1;
            }

            Op::TimerSleepAsync => {
                // Stack: ms, task_id -> timer_id
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let ms = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let timer_id = __arth_timer_sleep_async(ms, task_id);
                stack.push(Value::I64(timer_id));
                ip += 1;
            }

            Op::TimerCheckExpired => {
                // Stack: timer_id -> status
                let timer_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_timer_check_expired(timer_id);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::TimerGetWaitingTask => {
                // Stack: timer_id -> task_id
                let timer_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let task_id = __arth_timer_get_waiting_task(timer_id);
                stack.push(Value::I64(task_id));
                ip += 1;
            }

            Op::TimerCancel => {
                // Stack: timer_id -> status
                let timer_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_timer_cancel(timer_id);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::TimerPollExpired => {
                // Stack: -> timer_id
                let timer_id = __arth_timer_poll_expired();
                stack.push(Value::I64(timer_id));
                ip += 1;
            }

            Op::TimerNow => {
                // Stack: -> ms
                let ms = __arth_timer_now();
                stack.push(Value::I64(ms));
                ip += 1;
            }

            Op::TimerElapsed => {
                // Stack: start_ms -> elapsed_ms
                let start_ms = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let elapsed = __arth_timer_elapsed(start_ms);
                stack.push(Value::I64(elapsed));
                ip += 1;
            }

            Op::TimerRemove => {
                // Stack: timer_id -> status
                let timer_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_timer_remove(timer_id);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::TimerRemaining => {
                // Stack: timer_id -> remaining_ms
                let timer_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let remaining = __arth_timer_remaining(timer_id);
                stack.push(Value::I64(remaining));
                ip += 1;
            }

            // ================================================================
            // C23: TCP Socket Operations
            // ================================================================

            Op::TcpListenerBind => {
                // Stack: port -> listener_handle
                let port = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let handle = __arth_tcp_listener_bind(port);
                stack.push(Value::I64(handle));
                ip += 1;
            }

            Op::TcpListenerAccept => {
                // Stack: listener_handle -> stream_handle
                let listener = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let handle = __arth_tcp_listener_accept(listener);
                stack.push(Value::I64(handle));
                ip += 1;
            }

            Op::TcpListenerAcceptAsync => {
                // Stack: listener_handle, task_id -> request_id
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let listener = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let request_id = __arth_tcp_listener_accept_async(listener, task_id);
                stack.push(Value::I64(request_id));
                ip += 1;
            }

            Op::TcpListenerClose => {
                // Stack: listener_handle -> status
                let listener = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_tcp_listener_close(listener);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::TcpListenerLocalPort => {
                // Stack: listener_handle -> port
                let listener = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let port = __arth_tcp_listener_local_port(listener);
                stack.push(Value::I64(port));
                ip += 1;
            }

            Op::TcpStreamConnect => {
                // Stack: host_str_idx, port -> stream_handle
                let port = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let host_idx = match stack.pop() {
                    Some(Value::I64(n)) => n as usize,
                    _ => 0,
                };
                let host = p.strings.get(host_idx).cloned().unwrap_or_default();
                let handle = __arth_tcp_stream_connect(host.as_ptr(), host.len() as i64, port);
                stack.push(Value::I64(handle));
                ip += 1;
            }

            Op::TcpStreamConnectAsync => {
                // Stack: host_str_idx, port, task_id -> request_id
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let port = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let host_idx = match stack.pop() {
                    Some(Value::I64(n)) => n as usize,
                    _ => 0,
                };
                let host = p.strings.get(host_idx).cloned().unwrap_or_default();
                let request_id = __arth_tcp_stream_connect_async(host.as_ptr(), host.len() as i64, port, task_id);
                stack.push(Value::I64(request_id));
                ip += 1;
            }

            Op::TcpStreamRead => {
                // Stack: stream_handle, max_bytes -> bytes_read
                let max_bytes = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let stream = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let bytes_read = __arth_tcp_stream_read(stream, max_bytes);
                stack.push(Value::I64(bytes_read));
                ip += 1;
            }

            Op::TcpStreamReadAsync => {
                // Stack: stream_handle, max_bytes, task_id -> request_id
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let max_bytes = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let stream = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let request_id = __arth_tcp_stream_read_async(stream, max_bytes, task_id);
                stack.push(Value::I64(request_id));
                ip += 1;
            }

            Op::TcpStreamWrite => {
                // Stack: stream_handle, data_str_idx -> bytes_written
                let data_idx = match stack.pop() {
                    Some(Value::I64(n)) => n as usize,
                    _ => 0,
                };
                let stream = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let data = p.strings.get(data_idx).cloned().unwrap_or_default();
                let bytes_written = __arth_tcp_stream_write(stream, data.as_ptr(), data.len() as i64);
                stack.push(Value::I64(bytes_written));
                ip += 1;
            }

            Op::TcpStreamWriteAsync => {
                // Stack: stream_handle, data_str_idx, task_id -> request_id
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let data_idx = match stack.pop() {
                    Some(Value::I64(n)) => n as usize,
                    _ => 0,
                };
                let stream = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let data = p.strings.get(data_idx).cloned().unwrap_or_default();
                let request_id = __arth_tcp_stream_write_async(stream, data.as_ptr(), data.len() as i64, task_id);
                stack.push(Value::I64(request_id));
                ip += 1;
            }

            Op::TcpStreamClose => {
                // Stack: stream_handle -> status
                let stream = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_tcp_stream_close(stream);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::TcpStreamGetLastRead => {
                // Stack: -> str_idx
                // Get the last read data and add it to strings
                let data = LAST_TCP_READ_DATA.with(|cell| cell.borrow().clone());
                // For now, return the length as a placeholder
                // Real implementation would need to add string to program
                stack.push(Value::I64(data.len() as i64));
                ip += 1;
            }

            Op::TcpStreamSetTimeout => {
                // Stack: stream_handle, timeout_ms -> status
                let timeout = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let stream = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_tcp_stream_set_timeout(stream, timeout);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::TcpCheckReady => {
                // Stack: request_id -> status
                let request_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_tcp_check_ready(request_id);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::TcpGetResult => {
                // Stack: request_id -> result
                let request_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let result = __arth_tcp_get_result(request_id);
                stack.push(Value::I64(result));
                ip += 1;
            }

            Op::TcpPollReady => {
                // Stack: -> request_id
                let request_id = __arth_tcp_poll_ready();
                stack.push(Value::I64(request_id));
                ip += 1;
            }

            Op::TcpRemoveRequest => {
                // Stack: request_id -> status
                let request_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_tcp_remove_request(request_id);
                stack.push(Value::I64(status));
                ip += 1;
            }

            // ================================================================
            // C24: HTTP Client Operations
            // ================================================================

            Op::HttpGet => {
                // Stack: url_str_idx, timeout_ms -> response_handle
                let timeout_ms = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let url = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let handle = __arth_http_get(url.as_ptr(), url.len() as i64, timeout_ms);
                stack.push(Value::I64(handle));
                ip += 1;
            }

            Op::HttpPost => {
                // Stack: url_str_idx, body_str_idx, timeout_ms -> response_handle
                let timeout_ms = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let body = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let url = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let handle = __arth_http_post(
                    url.as_ptr(), url.len() as i64,
                    body.as_ptr(), body.len() as i64,
                    timeout_ms
                );
                stack.push(Value::I64(handle));
                ip += 1;
            }

            Op::HttpGetAsync => {
                // Stack: url_str_idx, timeout_ms, task_id -> request_id
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let timeout_ms = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let url = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let request_id = __arth_http_get_async(url.as_ptr(), url.len() as i64, timeout_ms, task_id);
                stack.push(Value::I64(request_id));
                ip += 1;
            }

            Op::HttpPostAsync => {
                // Stack: url_str_idx, body_str_idx, timeout_ms, task_id -> request_id
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let timeout_ms = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 0,
                };
                let body = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let url = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let request_id = __arth_http_post_async(
                    url.as_ptr(), url.len() as i64,
                    body.as_ptr(), body.len() as i64,
                    timeout_ms, task_id
                );
                stack.push(Value::I64(request_id));
                ip += 1;
            }

            Op::HttpResponseStatus => {
                // Stack: response_handle -> status_code
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_http_response_status(handle);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::HttpResponseHeader => {
                // Stack: response_handle, header_key_str_idx -> value_str_idx
                // Returns string in TLS, need to fetch it
                let key = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let found = __arth_http_response_header(handle, key.as_ptr(), key.len() as i64);
                if found == 1 {
                    let mut len: i64 = 0;
                    let ptr = __arth_http_get_last_header(&mut len);
                    let header_value = unsafe {
                        std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len as usize))
                    }.to_string();
                    stack.push(Value::Str(header_value));
                } else {
                    stack.push(Value::Str(String::new()));
                }
                ip += 1;
            }

            Op::HttpResponseBody => {
                // Stack: response_handle -> body_str_idx
                // Returns body in TLS, need to fetch it
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let success = __arth_http_response_body(handle);
                if success == 1 {
                    let mut len: i64 = 0;
                    let ptr = __arth_http_get_last_body(&mut len);
                    let body = unsafe {
                        std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len as usize))
                    }.to_string();
                    stack.push(Value::Str(body));
                } else {
                    stack.push(Value::Str(String::new()));
                }
                ip += 1;
            }

            Op::HttpResponseClose => {
                // Stack: response_handle -> status
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_http_response_close(handle);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::HttpCheckReady => {
                // Stack: request_id -> status
                let request_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_http_check_ready(request_id);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::HttpGetResult => {
                // Stack: request_id -> response_handle
                let request_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let result = __arth_http_get_result(request_id);
                stack.push(Value::I64(result));
                ip += 1;
            }

            Op::HttpPollReady => {
                // Stack: -> request_id
                let request_id = __arth_http_poll_ready();
                stack.push(Value::I64(request_id));
                ip += 1;
            }

            Op::HttpRemoveRequest => {
                // Stack: request_id -> status
                let request_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_http_remove_request(request_id);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::HttpGetBodyLength => {
                // Stack: response_handle -> length
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let length = __arth_http_get_body_length(handle);
                stack.push(Value::I64(length));
                ip += 1;
            }

            Op::HttpGetHeaderCount => {
                // Stack: response_handle -> count
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let count = __arth_http_get_header_count(handle);
                stack.push(Value::I64(count));
                ip += 1;
            }

            // ================================================================
            // HTTP Server Operations (C25)
            // ================================================================

            Op::HttpServerCreate => {
                // Stack: port -> server_handle
                let port = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let handle = __arth_http_server_create(port);
                stack.push(Value::I64(handle));
                ip += 1;
            }

            Op::HttpServerClose => {
                // Stack: server_handle -> status
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_http_server_close(handle);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::HttpServerGetPort => {
                // Stack: server_handle -> port
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let port = __arth_http_server_get_port(handle);
                stack.push(Value::I64(port));
                ip += 1;
            }

            Op::HttpServerAccept => {
                // Stack: server_handle -> conn_handle
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let conn = __arth_http_server_accept(handle);
                stack.push(Value::I64(conn));
                ip += 1;
            }

            Op::HttpServerAcceptAsync => {
                // Stack: server_handle, task_id -> request_id
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let request_id = __arth_http_server_accept_async(handle, task_id);
                stack.push(Value::I64(request_id));
                ip += 1;
            }

            Op::HttpRequestMethod => {
                // Stack: conn_handle -> method_str
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let result = __arth_http_request_method(handle);
                if result == 1 {
                    // Get the method string from TLS
                    let mut len: i64 = 0;
                    let ptr = __arth_http_request_get_method(&mut len);
                    if !ptr.is_null() && len > 0 {
                        let method = unsafe {
                            let slice = std::slice::from_raw_parts(ptr, len as usize);
                            String::from_utf8_lossy(slice).to_string()
                        };
                        stack.push(Value::Str(method));
                    } else {
                        stack.push(Value::Str(String::new()));
                    }
                } else {
                    stack.push(Value::Str(String::new()));
                }
                ip += 1;
            }

            Op::HttpRequestPath => {
                // Stack: conn_handle -> path_str
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let result = __arth_http_request_path(handle);
                if result == 1 {
                    let mut len: i64 = 0;
                    let ptr = __arth_http_request_get_path(&mut len);
                    if !ptr.is_null() && len > 0 {
                        let path = unsafe {
                            let slice = std::slice::from_raw_parts(ptr, len as usize);
                            String::from_utf8_lossy(slice).to_string()
                        };
                        stack.push(Value::Str(path));
                    } else {
                        stack.push(Value::Str(String::new()));
                    }
                } else {
                    stack.push(Value::Str(String::new()));
                }
                ip += 1;
            }

            Op::HttpRequestHeader => {
                // Stack: conn_handle, header_name -> header_value
                let name = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let result = __arth_http_request_header(handle, name.as_ptr(), name.len() as i64);
                if result == 1 {
                    let mut len: i64 = 0;
                    let ptr = __arth_http_request_get_header(&mut len);
                    if !ptr.is_null() && len > 0 {
                        let header = unsafe {
                            let slice = std::slice::from_raw_parts(ptr, len as usize);
                            String::from_utf8_lossy(slice).to_string()
                        };
                        stack.push(Value::Str(header));
                    } else {
                        stack.push(Value::Str(String::new()));
                    }
                } else {
                    stack.push(Value::Str(String::new()));
                }
                ip += 1;
            }

            Op::HttpRequestBody => {
                // Stack: conn_handle -> body_str
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let result = __arth_http_request_body(handle);
                if result == 1 {
                    let mut len: i64 = 0;
                    let ptr = __arth_http_request_get_body(&mut len);
                    if !ptr.is_null() && len > 0 {
                        let body = unsafe {
                            let slice = std::slice::from_raw_parts(ptr, len as usize);
                            String::from_utf8_lossy(slice).to_string()
                        };
                        stack.push(Value::Str(body));
                    } else {
                        stack.push(Value::Str(String::new()));
                    }
                } else {
                    stack.push(Value::Str(String::new()));
                }
                ip += 1;
            }

            Op::HttpRequestHeaderCount => {
                // Stack: conn_handle -> count
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let count = __arth_http_request_header_count(handle);
                stack.push(Value::I64(count));
                ip += 1;
            }

            Op::HttpRequestBodyLength => {
                // Stack: conn_handle -> length
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let length = __arth_http_request_body_length(handle);
                stack.push(Value::I64(length));
                ip += 1;
            }

            Op::HttpWriterStatus => {
                // Stack: conn_handle, status_code -> status
                let status_code = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => 200,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_http_writer_status(handle, status_code);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::HttpWriterHeader => {
                // Stack: conn_handle, name, value -> status
                let value = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let name = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_http_writer_header(
                    handle,
                    name.as_ptr(), name.len() as i64,
                    value.as_ptr(), value.len() as i64
                );
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::HttpWriterBody => {
                // Stack: conn_handle, body -> status
                let body = match stack.pop() {
                    Some(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_http_writer_body(handle, body.as_ptr(), body.len() as i64);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::HttpWriterSend => {
                // Stack: conn_handle -> status
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_http_writer_send(handle);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::HttpWriterSendAsync => {
                // Stack: conn_handle, task_id -> request_id
                let task_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let handle = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let request_id = __arth_http_writer_send_async(handle, task_id);
                stack.push(Value::I64(request_id));
                ip += 1;
            }

            Op::HttpServerCheckReady => {
                // Stack: request_id -> status
                let request_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_http_server_check_ready(request_id);
                stack.push(Value::I64(status));
                ip += 1;
            }

            Op::HttpServerGetResult => {
                // Stack: request_id -> result
                let request_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let result = __arth_http_server_get_result(request_id);
                stack.push(Value::I64(result));
                ip += 1;
            }

            Op::HttpServerPollReady => {
                // Stack: -> request_id
                let request_id = __arth_http_server_poll_ready();
                stack.push(Value::I64(request_id));
                ip += 1;
            }

            Op::HttpServerRemoveRequest => {
                // Stack: request_id -> status
                let request_id = match stack.pop() {
                    Some(Value::I64(n)) => n,
                    _ => -1,
                };
                let status = __arth_http_server_remove_request(request_id);
                stack.push(Value::I64(status));
                ip += 1;
            }
        }
    }
    // Normal exit - return exit code for program mode, return value for export mode
    if is_export_call {
        // Extract i64 return value if present
        let i64_return = stack.pop().and_then(|v| match v {
            Value::I64(n) => Some(n),
            _ => None,
        });
        InterpreterResult::ReturnValue(i64_return)
    } else {
        InterpreterResult::ExitCode(0)
    }
}

// ============================================================================
// Direct Export Calling API
// ============================================================================

/// Configuration for running the interpreter in "export call" mode.
/// When provided, the interpreter starts at a specific offset and returns
/// the function's return value instead of just an exit code.
#[derive(Debug, Clone)]
pub struct ExportCallConfig<'a> {
    /// The bytecode offset to start execution at
    pub start_offset: usize,
    /// Arguments to pass to the function (as strings)
    pub args: &'a [&'a str],
}

/// Result of running the interpreter.
/// Used internally to support both program execution and export calls.
#[derive(Debug, Clone)]
pub enum InterpreterResult {
    /// Program completed normally with exit code (for run_program)
    ExitCode(i32),
    /// Export call completed with optional i64 return value (for call_export)
    ReturnValue(Option<i64>),
    /// Execution failed with error
    Failed(i32),
}

/// Result of calling an export function directly.
#[derive(Debug, Clone)]
pub enum CallExportResult {
    /// Successful execution with optional return value
    Success(Option<i64>),
    /// Execution failed with exit code
    Failed(i32),
    /// Export not found at the given offset
    ExportNotFound,
    /// Invalid argument type
    InvalidArgument(String),
}

/// Call an export function directly without creating a trampoline program.
///
/// This is the preferred way to call exports from embedders like Rune, as it:
/// - Avoids creating a new Program for each call
/// - Avoids linking overhead
/// - Directly jumps to the export offset
///
/// # Arguments
///
/// * `p` - The program containing the export
/// * `offset` - The bytecode offset of the export function
/// * `arity` - The expected number of arguments
/// * `args` - The arguments to pass (as strings, which is the Arth convention for JSON payloads)
/// * `ctx` - Optional host context for capability enforcement
///
/// # Returns
///
/// * `CallExportResult::Success(Some(value))` - Function returned a value
/// * `CallExportResult::Success(None)` - Function completed without a value
/// * `CallExportResult::Failed(code)` - Execution failed with exit code
pub fn call_export(
    p: &Program,
    offset: u32,
    arity: u32,
    args: &[&str],
    ctx: Option<&HostContext>,
) -> CallExportResult {
    // Validate offset is within bounds
    if offset as usize >= p.code.len() {
        return CallExportResult::ExportNotFound;
    }

    // Validate arity matches arguments
    if args.len() != arity as usize {
        return CallExportResult::InvalidArgument(format!(
            "expected {} arguments, got {}",
            arity,
            args.len()
        ));
    }

    // Run the export using internal implementation
    call_export_internal(p, offset, args, ctx)
}

/// Call an export with a host context for capability enforcement.
pub fn call_export_with_host(
    p: &Program,
    offset: u32,
    arity: u32,
    args: &[&str],
    ctx: &HostContext,
) -> CallExportResult {
    call_export(p, offset, arity, args, Some(ctx))
}

/// Call an export with a symbol table for CallSymbol resolution.
///
/// This is the recommended API for calling exports that may internally call
/// other functions via CallSymbol opcodes. The symbol table is set before
/// execution and allows the VM to resolve qualified function names like
/// "HostHelpers.loadPartial" to their bytecode offsets.
///
/// # Arguments
///
/// * `p` - The program containing the export
/// * `offset` - The bytecode offset of the export function
/// * `arity` - The expected number of arguments
/// * `args` - The arguments to pass
/// * `ctx` - Optional host context for capability enforcement
/// * `symbols` - Symbol table mapping function names to bytecode offsets
pub fn call_export_with_symbols(
    p: &Program,
    offset: u32,
    arity: u32,
    args: &[&str],
    ctx: Option<&HostContext>,
    symbols: std::collections::HashMap<String, u32>,
) -> CallExportResult {
    // Set the symbol table before execution
    set_linked_symbol_table(symbols);

    // Call the export
    call_export(p, offset, arity, args, ctx)
}

/// Internal implementation of direct export calling.
///
/// This function uses the unified interpreter in export call mode, which supports
/// ALL opcodes (unlike the previous simplified implementation).
fn call_export_internal(
    p: &Program,
    offset: u32,
    args: &[&str],
    ctx: Option<&HostContext>,
) -> CallExportResult {
    // Use the unified interpreter in export call mode
    let config = ExportCallConfig {
        start_offset: offset as usize,
        args,
    };

    match run_program_internal(p, ctx, Some(config)) {
        InterpreterResult::ReturnValue(opt_n) => CallExportResult::Success(opt_n),
        InterpreterResult::ExitCode(_) => CallExportResult::Success(None), // Shouldn't happen
        InterpreterResult::Failed(code) => CallExportResult::Failed(code),
    }
}
