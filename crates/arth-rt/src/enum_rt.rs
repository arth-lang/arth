//! Dynamic enum runtime support for native LLVM calls.

use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::Mutex;

use crate::new_handle;

#[derive(Clone, Debug)]
struct EnumValue {
    #[allow(dead_code)]
    enum_name: String,
    #[allow(dead_code)]
    variant_name: String,
    tag: i64,
    payloads: Vec<i64>,
}

lazy_static::lazy_static! {
    static ref ENUMS: Mutex<HashMap<i64, EnumValue>> = Mutex::new(HashMap::new());
}

fn cstr_to_string(ptr: *const u8) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe {
        CStr::from_ptr(ptr as *const c_char)
            .to_string_lossy()
            .to_string()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_enum_new(
    enum_name_ptr: *const u8,
    variant_name_ptr: *const u8,
    tag: i64,
    payload_count: i64,
) -> i64 {
    let enum_name = cstr_to_string(enum_name_ptr);
    let variant_name = cstr_to_string(variant_name_ptr);
    let count = payload_count.max(0) as usize;
    let handle = new_handle();

    let mut map = ENUMS.lock().unwrap();
    map.insert(
        handle,
        EnumValue {
            enum_name,
            variant_name,
            tag,
            payloads: vec![0; count],
        },
    );
    handle
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_enum_set_payload(handle: i64, index: i64, value: i64) -> i64 {
    let idx = index.max(0) as usize;
    let mut map = ENUMS.lock().unwrap();
    if let Some(ev) = map.get_mut(&handle) {
        if idx < ev.payloads.len() {
            ev.payloads[idx] = value;
            return handle;
        }
    }
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_enum_get_payload(handle: i64, index: i64) -> i64 {
    let idx = index.max(0) as usize;
    let map = ENUMS.lock().unwrap();
    map.get(&handle)
        .and_then(|ev| ev.payloads.get(idx).copied())
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_enum_get_tag(handle: i64) -> i64 {
    let map = ENUMS.lock().unwrap();
    map.get(&handle).map(|ev| ev.tag).unwrap_or(0)
}
