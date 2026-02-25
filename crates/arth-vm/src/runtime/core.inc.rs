// Note: Tokio runtime removed - WebSocket/SSE now use synchronous arth_rt sockets

#[derive(Clone, Debug)]
enum Value {
    I64(i64),
    F64(f64),
    Bool(bool),
    Str(String),
}

// Wrapper for Value that can be used as HashMap key
// F64 values use bit representation for hashing (NaN-safe)
#[derive(Clone, Debug)]
struct HashableValue(Value);

impl std::hash::Hash for HashableValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match &self.0 {
            Value::I64(n) => {
                0u8.hash(state);
                n.hash(state);
            }
            Value::F64(f) => {
                1u8.hash(state);
                f.to_bits().hash(state);
            }
            Value::Bool(b) => {
                2u8.hash(state);
                b.hash(state);
            }
            Value::Str(s) => {
                3u8.hash(state);
                s.hash(state);
            }
        }
    }
}

impl PartialEq for HashableValue {
    fn eq(&self, other: &Self) -> bool {
        values_equal(&self.0, &other.0)
    }
}

impl Eq for HashableValue {}

fn max_list_len() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var("ARTH_VM_MAX_LIST_LEN")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1_000_000)
    })
}

fn max_map_len() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var("ARTH_VM_MAX_MAP_LEN")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1_000_000)
    })
}

// --- HTTP Response Infrastructure (used by http_fetch) ---

/// HTTP response from fetch
#[derive(Clone, Debug)]
#[allow(dead_code)]
struct HttpResponse {
    status: i64,
    reason: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn http_response_store() -> &'static Mutex<HashMap<i64, HttpResponse>> {
    static MAP: OnceLock<Mutex<HashMap<i64, HttpResponse>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_HTTP_RESPONSE: AtomicI64 = AtomicI64::new(40_000);

// Note: WebSocket/SSE server infrastructure moved to ffi.inc.rs (synchronous implementation)

// --- Simple global stores for List/Map handles ---
fn list_store() -> &'static Mutex<HashMap<i64, Vec<Value>>> {
    static L: OnceLock<Mutex<HashMap<i64, Vec<Value>>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}
static NEXT_LIST: AtomicI64 = AtomicI64::new(50_000);
fn list_new() -> i64 {
    let h = NEXT_LIST.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut m) = list_store().lock() {
        m.insert(h, Vec::new());
    }
    h
}
fn list_push(h: i64, v: Value) -> usize {
    if let Ok(mut m) = list_store().lock() {
        if let Some(vec) = m.get_mut(&h) {
            if vec.len() >= max_list_len() {
                return vec.len();
            }
            vec.push(v);
            return vec.len();
        }
    }
    0
}
fn list_get(h: i64, idx: usize) -> Option<Value> {
    if let Ok(m) = list_store().lock() {
        if let Some(vec) = m.get(&h) {
            return vec.get(idx).cloned();
        }
    }
    None
}
fn list_len(h: i64) -> usize {
    if let Ok(m) = list_store().lock() {
        if let Some(vec) = m.get(&h) {
            return vec.len();
        }
    }
    0
}

// Removed: list_index_of, list_contains, list_insert - now pure Arth code in stdlib/src/arth/array.arth

fn list_remove(h: i64, idx: usize) -> Option<Value> {
    if let Ok(mut m) = list_store().lock() {
        if let Some(vec) = m.get_mut(&h) {
            if idx < vec.len() {
                return Some(vec.remove(idx));
            }
        }
    }
    None
}

// Removed: list_clear, list_reverse, list_concat, list_slice - now pure Arth code in stdlib/src/arth/array.arth

fn list_sort(h: i64) {
    if let Ok(mut m) = list_store().lock() {
        if let Some(vec) = m.get_mut(&h) {
            vec.sort_by(|a, b| {
                match (a, b) {
                    (Value::I64(x), Value::I64(y)) => x.cmp(y),
                    (Value::F64(x), Value::F64(y)) => {
                        // NaN handling: treat NaN as less than any number
                        if x.is_nan() && y.is_nan() {
                            std::cmp::Ordering::Equal
                        } else if x.is_nan() {
                            std::cmp::Ordering::Less
                        } else if y.is_nan() {
                            std::cmp::Ordering::Greater
                        } else {
                            x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal)
                        }
                    }
                    (Value::Str(x), Value::Str(y)) => x.cmp(y),
                    (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
                    // Mixed types: order by type then value
                    (Value::I64(_), _) => std::cmp::Ordering::Less,
                    (_, Value::I64(_)) => std::cmp::Ordering::Greater,
                    (Value::F64(_), Value::Bool(_)) | (Value::F64(_), Value::Str(_)) => {
                        std::cmp::Ordering::Less
                    }
                    (Value::Bool(_), Value::F64(_)) | (Value::Str(_), Value::F64(_)) => {
                        std::cmp::Ordering::Greater
                    }
                    (Value::Bool(_), Value::Str(_)) => std::cmp::Ordering::Less,
                    (Value::Str(_), Value::Bool(_)) => std::cmp::Ordering::Greater,
                }
            });
        }
    }
}

// Removed: list_unique - now pure Arth code in stdlib/src/arth/array.arth

// Helper function to compare values for equality (used by HashableValue and map operations)
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::I64(x), Value::I64(y)) => x == y,
        (Value::F64(x), Value::F64(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        _ => false,
    }
}

fn map_store() -> &'static Mutex<HashMap<i64, HashMap<HashableValue, Value>>> {
    static M: OnceLock<Mutex<HashMap<i64, HashMap<HashableValue, Value>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}
static NEXT_MAP: AtomicI64 = AtomicI64::new(60_000);
fn map_new() -> i64 {
    let h = NEXT_MAP.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut m) = map_store().lock() {
        m.insert(h, HashMap::new());
    }
    h
}
fn map_put(h: i64, k: Value, v: Value) -> bool {
    if let Ok(mut m) = map_store().lock() {
        if let Some(mm) = m.get_mut(&h) {
            if mm.len() >= max_map_len() && !mm.contains_key(&HashableValue(k.clone())) {
                return false;
            }
            mm.insert(HashableValue(k), v);
            return true;
        }
    }
    false
}
fn map_get(h: i64, k: &Value) -> Option<Value> {
    if let Ok(m) = map_store().lock() {
        if let Some(mm) = m.get(&h) {
            return mm.get(&HashableValue(k.clone())).cloned();
        }
    }
    None
}
fn map_len(h: i64) -> usize {
    if let Ok(m) = map_store().lock() {
        if let Some(mm) = m.get(&h) {
            return mm.len();
        }
    }
    0
}

fn map_contains_key(h: i64, k: &Value) -> bool {
    if let Ok(m) = map_store().lock() {
        if let Some(mm) = m.get(&h) {
            return mm.contains_key(&HashableValue(k.clone()));
        }
    }
    false
}

// Removed: map_contains_value - now pure Arth code in stdlib/src/arth/map.arth

fn map_remove(h: i64, k: &Value) -> Option<Value> {
    if let Ok(mut m) = map_store().lock() {
        if let Some(mm) = m.get_mut(&h) {
            return mm.remove(&HashableValue(k.clone()));
        }
    }
    None
}

// Removed: map_clear, map_is_empty, map_get_or_default - now pure Arth code in stdlib/src/arth/map.arth

fn map_keys(h: i64) -> i64 {
    let list_handle = list_new();
    if let Ok(m) = map_store().lock() {
        if let Some(mm) = m.get(&h) {
            // Collect keys from the map
            let keys: Vec<Value> = mm.keys().map(|k| k.0.clone()).collect();
            drop(m); // Release map lock before acquiring list lock

            if let Ok(mut lst) = list_store().lock() {
                if let Some(vec) = lst.get_mut(&list_handle) {
                    *vec = keys;
                }
            }
        }
    }
    list_handle
}

// Removed: map_values - now pure Arth code in stdlib/src/arth/map.arth

fn map_merge(dest: i64, src: i64) -> i64 {
    // Copy all entries from src map into dest map
    if let Ok(mut m) = map_store().lock() {
        // Get entries from source map
        let entries: Vec<(HashableValue, Value)> = m
            .get(&src)
            .map(|src_map| src_map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();

        // Insert entries into dest map
        if let Some(dest_map) = m.get_mut(&dest) {
            for (k, v) in entries {
                dest_map.insert(k, v);
            }
        }
    }
    dest
}

// --- Optional<T>: represents an optional value (Some or None) ---
fn opt_store() -> &'static Mutex<HashMap<i64, Option<Value>>> {
    static O: OnceLock<Mutex<HashMap<i64, Option<Value>>>> = OnceLock::new();
    O.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_OPT: AtomicI64 = AtomicI64::new(70_000);

fn opt_some(v: Value) -> i64 {
    let h = NEXT_OPT.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut m) = opt_store().lock() {
        m.insert(h, Some(v));
    }
    h
}

fn opt_none() -> i64 {
    let h = NEXT_OPT.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut m) = opt_store().lock() {
        m.insert(h, None);
    }
    h
}

fn opt_is_some(h: i64) -> bool {
    if let Ok(m) = opt_store().lock() {
        if let Some(opt) = m.get(&h) {
            return opt.is_some();
        }
    }
    false
}

fn opt_unwrap(h: i64) -> Value {
    if let Ok(m) = opt_store().lock() {
        if let Some(Some(v)) = m.get(&h) {
            return v.clone();
        }
    }
    Value::I64(0) // Return 0 if None or invalid handle
}

fn opt_or_else(h: i64, default: Value) -> Value {
    if let Ok(m) = opt_store().lock() {
        if let Some(opt) = m.get(&h) {
            return match opt {
                Some(v) => v.clone(),
                None => default,
            };
        }
    }
    default
}

// --- Native Structs: typed, indexed field storage ---
// Structs store type metadata and fields in an indexed array for efficient access
#[derive(Clone, Debug)]
struct NativeStruct {
    type_name: String,
    fields: Vec<Value>,
    // Maps field names to indices for named access
    field_names: Vec<String>,
}

fn struct_store() -> &'static Mutex<HashMap<i64, NativeStruct>> {
    static S: OnceLock<Mutex<HashMap<i64, NativeStruct>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_STRUCT: AtomicI64 = AtomicI64::new(90_000);

fn struct_new(type_name: String, field_count: usize) -> i64 {
    let h = NEXT_STRUCT.fetch_add(1, Ordering::Relaxed);
    let s = NativeStruct {
        type_name,
        fields: vec![Value::I64(0); field_count],
        field_names: vec![String::new(); field_count],
    };
    if let Ok(mut m) = struct_store().lock() {
        m.insert(h, s);
    }
    h
}

fn struct_set(h: i64, idx: usize, val: Value, field_name: String) {
    if let Ok(mut m) = struct_store().lock() {
        if let Some(s) = m.get_mut(&h) {
            if idx < s.fields.len() {
                s.fields[idx] = val;
                if !field_name.is_empty() {
                    s.field_names[idx] = field_name;
                }
            }
        }
    }
}

fn struct_get(h: i64, idx: usize) -> Value {
    if let Ok(m) = struct_store().lock() {
        if let Some(s) = m.get(&h) {
            if idx < s.fields.len() {
                return s.fields[idx].clone();
            }
        }
    }
    Value::I64(0)
}

/// Check if handle is a JSON handle (110_000+ range).
fn is_json_handle(h: i64) -> bool {
    h >= 110_000
}

/// Get a field from a JSON object and convert to a VM Value.
/// For nested objects/arrays, returns the handle.
/// For primitives (string, number, bool, null), returns the actual value.
fn json_get_field_as_value(handle: i64, field_name: &str) -> Value {
    let is_interesting = field_name == "subject" || field_name == "sender" || field_name == "body" || field_name == "preview" || field_name == "date" || field_name == "id";

    let store = json_store();
    let Ok(guard) = store.lock() else {
        if is_interesting {
            eprintln!("[VM_DEBUG] json_get_field_as_value: failed to lock store");
        }
        return Value::I64(0);
    };
    let Some(val) = guard.get(&handle) else {
        if is_interesting {
            eprintln!("[VM_DEBUG] json_get_field_as_value: handle {} NOT FOUND in json_store", handle);
        }
        return Value::I64(0);
    };

    let JsonVal::Object(pairs) = val else {
        if is_interesting {
            eprintln!("[VM_DEBUG] json_get_field_as_value: handle {} is not an Object", handle);
        }
        return Value::I64(0);
    };

    if is_interesting {
        eprintln!("[VM_DEBUG] json_get_field_as_value: handle {} is Object with {} pairs, looking for '{}'", handle, pairs.len(), field_name);
        let keys: Vec<_> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        eprintln!("[VM_DEBUG] json_get_field_as_value: available keys = {:?}", keys);
    }

    let Some((_, field_val)) = pairs.iter().find(|(k, _)| k == field_name) else {
        if is_interesting {
            eprintln!("[VM_DEBUG] json_get_field_as_value: field '{}' NOT FOUND", field_name);
        }
        return Value::I64(0);
    };

    // Convert JsonVal to Value
    match field_val {
        JsonVal::String(s) => {
            if is_interesting {
                eprintln!("[VM_DEBUG] json_get_field_as_value: field '{}' = String('{}')", field_name, s);
            }
            Value::Str(s.clone())
        }
        JsonVal::Number(n) => {
            if n.fract() == 0.0 {
                Value::I64(*n as i64)
            } else {
                Value::F64(*n)
            }
        }
        // Return I64 for booleans (1 for true, 0 for false), consistent with json_get_bool.
        // This allows comparison operations (EqI64, etc.) to work correctly since
        // they don't handle Bool/I64 type mixing.
        JsonVal::Bool(b) => Value::I64(if *b { 1 } else { 0 }),
        JsonVal::Null => Value::I64(0),
        // For nested objects/arrays, store and return a handle
        JsonVal::Object(_) | JsonVal::Array(_) => {
            let cloned = field_val.clone();
            let h = NEXT_JSON.fetch_add(1, Ordering::Relaxed);
            drop(guard); // Release lock before re-acquiring
            if let Ok(mut m) = store.lock() {
                m.insert(h, cloned);
            }
            Value::I64(h)
        }
    }
}

/// Check if handle is in the Optional range (70_000..90_000).
fn is_opt_handle(h: i64) -> bool {
    (70_000..90_000).contains(&h)
}

fn struct_get_named(h: i64, field_name: &str) -> Value {
    let is_interesting = field_name == "subject" || field_name == "sender" || field_name == "body" || field_name == "preview" || field_name == "date" || field_name == "id" || field_name == "length";

    // Check if this is a list handle - support .length property for TypeScript compatibility
    if is_list_handle(h) {
        if field_name == "length" {
            let len = list_len(h);
            if is_interesting {
                eprintln!("[VM_DEBUG] struct_get_named: list handle {} .length = {}", h, len);
            }
            return Value::I64(len as i64);
        }
        // Other fields on lists are not supported
        return Value::I64(0);
    }

    // Check if this is an Optional handle - if so, unwrap and recurse
    if is_opt_handle(h) {
        if is_interesting {
            eprintln!("[VM_DEBUG] struct_get_named: handle {} is Optional handle, unwrapping for field '{}'", h, field_name);
        }
        // Unwrap the Optional to get the inner value
        let inner = opt_unwrap(h);
        if is_interesting {
            eprintln!("[VM_DEBUG] struct_get_named: unwrapped Optional to {:?}", &inner);
        }
        // If inner is a handle (I64), recurse on that handle
        if let Value::I64(inner_h) = inner {
            if inner_h != 0 {
                return struct_get_named(inner_h, field_name);
            }
        }
        // If inner is not a handle or is 0 (None), return 0
        return Value::I64(0);
    }

    // Check if this is a JSON handle - if so, delegate to JSON field access
    if is_json_handle(h) {
        if is_interesting {
            eprintln!("[VM_DEBUG] struct_get_named: handle {} is JSON handle, delegating to json_get_field_as_value for field '{}'", h, field_name);
        }
        return json_get_field_as_value(h, field_name);
    }

    // Otherwise, use struct storage
    if let Ok(m) = struct_store().lock() {
        if let Some(s) = m.get(&h) {
            // DEBUG: Trace struct lookups for interesting fields
            let is_interesting = field_name == "subject" || field_name == "sender" || field_name == "body" || field_name == "preview" || field_name == "date" || field_name == "id";
            if is_interesting {
                eprintln!("[VM_DEBUG] struct_get_named: handle={}, looking for field '{}' in struct with {} fields", h, field_name, s.fields.len());
                eprintln!("[VM_DEBUG] struct_get_named: available field_names = {:?}", s.field_names);
            }
            for (idx, name) in s.field_names.iter().enumerate() {
                if name == field_name {
                    let val = s.fields[idx].clone();
                    if is_interesting {
                        eprintln!("[VM_DEBUG] struct_get_named: found field '{}' at idx {}, value = {:?}", field_name, idx, &val);
                    }
                    return val;
                }
            }
            if is_interesting {
                eprintln!("[VM_DEBUG] struct_get_named: field '{}' NOT FOUND in struct", field_name);
            }
        } else {
            let is_interesting = field_name == "subject" || field_name == "sender" || field_name == "body" || field_name == "preview" || field_name == "date" || field_name == "id";
            if is_interesting {
                eprintln!("[VM_DEBUG] struct_get_named: handle {} NOT FOUND in struct_store", h);
            }
        }
    }
    Value::I64(0)
}

fn struct_set_named(h: i64, field_name: &str, val: Value) {
    if let Ok(mut m) = struct_store().lock() {
        if let Some(s) = m.get_mut(&h) {
            for (idx, name) in s.field_names.iter().enumerate() {
                if name == field_name {
                    s.fields[idx] = val;
                    return;
                }
            }
        }
    }
}

fn struct_copy(dest: i64, src: i64) {
    if let Ok(mut m) = struct_store().lock() {
        // Get source fields
        let fields: Vec<(Value, String)> = m
            .get(&src)
            .map(|s| {
                s.fields
                    .iter()
                    .zip(s.field_names.iter())
                    .map(|(v, n)| (v.clone(), n.clone()))
                    .collect()
            })
            .unwrap_or_default();

        // Copy to dest
        if let Some(dest_s) = m.get_mut(&dest) {
            for (idx, (val, name)) in fields.into_iter().enumerate() {
                if idx < dest_s.fields.len() {
                    dest_s.fields[idx] = val;
                    dest_s.field_names[idx] = name;
                }
            }
        }
    }
}

fn struct_type_name(h: i64) -> String {
    if let Ok(m) = struct_store().lock() {
        if let Some(s) = m.get(&h) {
            return s.type_name.clone();
        }
    }
    String::new()
}

fn struct_field_count(h: i64) -> usize {
    if let Ok(m) = struct_store().lock() {
        if let Some(s) = m.get(&h) {
            return s.fields.len();
        }
    }
    0
}

// --- Native Enums: tagged values with payload array ---
// Enums store type info, variant tag, and payload values
#[derive(Clone, Debug)]
struct NativeEnum {
    enum_name: String,
    variant_name: String,
    tag: i64,
    payload: Vec<Value>,
}

fn enum_store() -> &'static Mutex<HashMap<i64, NativeEnum>> {
    static E: OnceLock<Mutex<HashMap<i64, NativeEnum>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_ENUM: AtomicI64 = AtomicI64::new(100_000);

fn enum_new(enum_name: String, variant_name: String, tag: i64, payload_count: usize) -> i64 {
    let h = NEXT_ENUM.fetch_add(1, Ordering::Relaxed);
    let e = NativeEnum {
        enum_name,
        variant_name,
        tag,
        payload: vec![Value::I64(0); payload_count],
    };
    if let Ok(mut m) = enum_store().lock() {
        m.insert(h, e);
    }
    h
}

fn enum_set_payload(h: i64, idx: usize, val: Value) {
    if let Ok(mut m) = enum_store().lock() {
        if let Some(e) = m.get_mut(&h) {
            if idx < e.payload.len() {
                e.payload[idx] = val;
            }
        }
    }
}

fn enum_get_payload(h: i64, idx: usize) -> Value {
    if let Ok(m) = enum_store().lock() {
        if let Some(e) = m.get(&h) {
            if idx < e.payload.len() {
                return e.payload[idx].clone();
            }
        }
    }
    Value::I64(0)
}

fn enum_get_tag(h: i64) -> i64 {
    if let Ok(m) = enum_store().lock() {
        if let Some(e) = m.get(&h) {
            return e.tag;
        }
    }
    -1
}

fn enum_get_variant(h: i64) -> String {
    if let Ok(m) = enum_store().lock() {
        if let Some(e) = m.get(&h) {
            return e.variant_name.clone();
        }
    }
    String::new()
}

fn enum_type_name(h: i64) -> String {
    if let Ok(m) = enum_store().lock() {
        if let Some(e) = m.get(&h) {
            return e.enum_name.clone();
        }
    }
    String::new()
}

// --- Closures: first-class functions with captured variables ---
#[derive(Clone, Debug)]
struct Closure {
    func_id: u32,
    captures: Vec<Value>,
}

fn closure_store() -> &'static Mutex<HashMap<i64, Closure>> {
    static C: OnceLock<Mutex<HashMap<i64, Closure>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_CLOSURE: AtomicI64 = AtomicI64::new(80_000);

fn closure_new(func_id: u32, _num_captures: u32) -> i64 {
    let h = NEXT_CLOSURE.fetch_add(1, Ordering::Relaxed);
    let closure = Closure {
        func_id,
        captures: Vec::new(),
    };
    if let Ok(mut m) = closure_store().lock() {
        m.insert(h, closure);
    }
    h
}

fn closure_capture(h: i64, value: Value) {
    if let Ok(mut m) = closure_store().lock() {
        if let Some(closure) = m.get_mut(&h) {
            closure.captures.push(value);
        }
    }
}

fn closure_get_func_id(h: i64) -> Option<u32> {
    if let Ok(m) = closure_store().lock() {
        if let Some(closure) = m.get(&h) {
            return Some(closure.func_id);
        }
    }
    None
}

fn closure_get_captures(h: i64) -> Vec<Value> {
    if let Ok(m) = closure_store().lock() {
        if let Some(closure) = m.get(&h) {
            return closure.captures.clone();
        }
    }
    Vec::new()
}

// --- Shared cells: simple global handle -> Value store ---
fn shared_store() -> &'static Mutex<HashMap<i64, Value>> {
    static S: OnceLock<Mutex<HashMap<i64, Value>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}
static NEXT_SHARED: AtomicI64 = AtomicI64::new(70_000);
fn shared_new() -> i64 {
    let h = NEXT_SHARED.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut m) = shared_store().lock() {
        m.insert(h, Value::I64(0));
    }
    h
}
fn shared_store_val(h: i64, v: Value) {
    if let Ok(mut m) = shared_store().lock() {
        m.insert(h, v);
    }
}
fn shared_load_val(h: i64) -> Value {
    if let Ok(m) = shared_store().lock() {
        if let Some(v) = m.get(&h) {
            return v.clone();
        }
    }
    Value::I64(0)
}
fn named_shared_map() -> &'static Mutex<HashMap<String, i64>> {
    static N: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    N.get_or_init(|| Mutex::new(HashMap::new()))
}

// --- Reference Counting: heap-allocated values with automatic cleanup ---
#[derive(Clone, Debug)]
struct RcCell {
    value: Value,
    count: u64,
}

fn rc_store() -> &'static Mutex<HashMap<i64, RcCell>> {
    static R: OnceLock<Mutex<HashMap<i64, RcCell>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_RC: AtomicI64 = AtomicI64::new(90_000);

/// Allocate a new RC cell with the given value, starting with count=1.
/// Returns a handle to the cell.
fn rc_alloc(value: Value) -> i64 {
    let h = NEXT_RC.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut m) = rc_store().lock() {
        m.insert(h, RcCell { value, count: 1 });
    }
    h
}

/// Increment the reference count for an RC cell.
/// Returns true if successful, false if handle not found.
fn rc_inc(h: i64) -> bool {
    if let Ok(mut m) = rc_store().lock() {
        if let Some(cell) = m.get_mut(&h) {
            cell.count = cell.count.saturating_add(1);
            return true;
        }
    }
    false
}

/// Decrement the reference count for an RC cell.
/// Returns Some(value) if count reached 0 and cell was deallocated,
/// None if count > 0 or handle not found.
fn rc_dec(h: i64) -> Option<Value> {
    if let Ok(mut m) = rc_store().lock() {
        if let Some(cell) = m.get_mut(&h) {
            cell.count = cell.count.saturating_sub(1);
            if cell.count == 0 {
                // Remove and return the value for potential cleanup
                return m.remove(&h).map(|c| c.value);
            }
        }
    }
    None
}

/// Load the value from an RC cell. Returns None if handle not found.
fn rc_load(h: i64) -> Option<Value> {
    if let Ok(m) = rc_store().lock() {
        if let Some(cell) = m.get(&h) {
            return Some(cell.value.clone());
        }
    }
    None
}

/// Store a new value into an RC cell. Returns true if successful.
fn rc_store_value(h: i64, value: Value) -> bool {
    if let Ok(mut m) = rc_store().lock() {
        if let Some(cell) = m.get_mut(&h) {
            cell.value = value;
            return true;
        }
    }
    false
}

/// Get the current reference count for an RC cell. Returns 0 if not found.
fn rc_get_count(h: i64) -> u64 {
    if let Ok(m) = rc_store().lock() {
        if let Some(cell) = m.get(&h) {
            return cell.count;
        }
    }
    0
}

// --- Region-based allocation: bulk deallocation for loop-local values ---

/// A region contains values that will be bulk-deallocated together.
#[derive(Clone, Debug, Default)]
struct Region {
    /// Values allocated in this region (will be deallocated on region exit)
    allocations: Vec<Value>,
}

fn region_store() -> &'static Mutex<HashMap<u32, Region>> {
    static R: OnceLock<Mutex<HashMap<u32, Region>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Stack of active regions (for LIFO semantics)
fn region_stack() -> &'static Mutex<Vec<u32>> {
    static S: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

/// Enter a new region - creates a region for bulk deallocation.
fn region_enter(region_id: u32) {
    if let (Ok(mut store), Ok(mut stack)) = (region_store().lock(), region_stack().lock()) {
        store.insert(region_id, Region::default());
        stack.push(region_id);
    }
}

/// Exit a region - removes the region and deallocates all values.
/// Returns the allocations that were in the region (for potential deinit calls).
fn region_exit(region_id: u32) -> Vec<Value> {
    let mut allocations = Vec::new();
    if let (Ok(mut store), Ok(mut stack)) = (region_store().lock(), region_stack().lock()) {
        // Remove from stack if it's the current region
        if stack.last() == Some(&region_id) {
            stack.pop();
        }
        // Remove and collect allocations
        if let Some(region) = store.remove(&region_id) {
            allocations = region.allocations;
        }
    }
    allocations
}

/// Add a value to the current region (optional: for explicit tracking).
#[allow(dead_code)]
fn region_alloc(value: Value) {
    if let (Ok(mut store), Ok(stack)) = (region_store().lock(), region_stack().lock()) {
        if let Some(&region_id) = stack.last() {
            if let Some(region) = store.get_mut(&region_id) {
                region.allocations.push(value);
            }
        }
    }
}

/// Check if currently inside a region.
#[allow(dead_code)]
fn is_in_region() -> bool {
    if let Ok(stack) = region_stack().lock() {
        !stack.is_empty()
    } else {
        false
    }
}

// --- File I/O: handle-based file operations ---

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Read as IoRead, Seek, SeekFrom};
use std::path::Path;

fn file_store() -> &'static Mutex<HashMap<i64, File>> {
    static F: OnceLock<Mutex<HashMap<i64, File>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_FILE: AtomicI64 = AtomicI64::new(100_000);

/// Open a file. Mode: 0=read, 1=write, 2=append, 3=read_write
/// Returns file handle or -1 on error.
fn file_open(path: &str, mode: i64) -> i64 {
    let result = match mode {
        0 => File::open(path),
        1 => File::create(path),
        2 => OpenOptions::new().append(true).create(true).open(path),
        3 => OpenOptions::new().read(true).write(true).open(path),
        _ => return -1,
    };

    match result {
        Ok(file) => {
            let h = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut m) = file_store().lock() {
                m.insert(h, file);
            }
            h
        }
        Err(_) => -1,
    }
}

/// Close a file handle. Returns 0 on success, -1 on error.
fn file_close(h: i64) -> i64 {
    if let Ok(mut m) = file_store().lock() {
        if m.remove(&h).is_some() {
            return 0;
        }
    }
    -1
}

/// Read up to max_bytes from a file into a new string.
/// Returns the string content (empty on error).
fn file_read(h: i64, max_bytes: usize) -> String {
    if let Ok(mut m) = file_store().lock() {
        if let Some(file) = m.get_mut(&h) {
            let mut buffer = vec![0u8; max_bytes];
            if let Ok(n) = file.read(&mut buffer) {
                buffer.truncate(n);
                return String::from_utf8_lossy(&buffer).to_string();
            }
        }
    }
    String::new()
}

// Removed: file_read_all - now pure Arth in stdlib/src/io/File.arth

/// Write string data to file. Returns bytes written or -1 on error.
fn file_write(h: i64, data: &str) -> i64 {
    if let Ok(mut m) = file_store().lock() {
        if let Some(file) = m.get_mut(&h) {
            match file.write(data.as_bytes()) {
                Ok(n) => return n as i64,
                Err(_) => return -1,
            }
        }
    }
    -1
}

/// Flush file buffers. Returns 0 on success, -1 on error.
fn file_flush(h: i64) -> i64 {
    if let Ok(mut m) = file_store().lock() {
        if let Some(file) = m.get_mut(&h) {
            if file.flush().is_ok() {
                return 0;
            }
        }
    }
    -1
}

/// Seek in file. Whence: 0=start, 1=current, 2=end. Returns new position or -1.
fn file_seek(h: i64, offset: i64, whence: i64) -> i64 {
    let seek_from = match whence {
        0 => SeekFrom::Start(offset as u64),
        1 => SeekFrom::Current(offset),
        2 => SeekFrom::End(offset),
        _ => return -1,
    };

    if let Ok(mut m) = file_store().lock() {
        if let Some(file) = m.get_mut(&h) {
            match file.seek(seek_from) {
                Ok(pos) => return pos as i64,
                Err(_) => return -1,
            }
        }
    }
    -1
}

/// Get file size. Returns size or -1 on error.
fn file_size(h: i64) -> i64 {
    if let Ok(mut m) = file_store().lock() {
        if let Some(file) = m.get_mut(&h) {
            if let Ok(metadata) = file.metadata() {
                return metadata.len() as i64;
            }
        }
    }
    -1
}

/// Check if a file exists.
fn file_exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// Delete a file. Returns 0 on success, -1 on error.
fn file_delete(path: &str) -> i64 {
    if fs::remove_file(path).is_ok() { 0 } else { -1 }
}

/// Copy a file. Returns 0 on success, -1 on error.
fn file_copy(src: &str, dst: &str) -> i64 {
    if fs::copy(src, dst).is_ok() { 0 } else { -1 }
}

/// Move/rename a file. Returns 0 on success, -1 on error.
fn file_move(src: &str, dst: &str) -> i64 {
    if fs::rename(src, dst).is_ok() { 0 } else { -1 }
}

// --- Directory operations ---

/// Create a directory. Returns 0 on success, -1 on error.
fn dir_create(path: &str) -> i64 {
    if fs::create_dir(path).is_ok() { 0 } else { -1 }
}

/// Create a directory and all parent directories. Returns 0 on success, -1 on error.
fn dir_create_all(path: &str) -> i64 {
    if fs::create_dir_all(path).is_ok() {
        0
    } else {
        -1
    }
}

/// Delete a directory. Returns 0 on success, -1 on error.
fn dir_delete(path: &str) -> i64 {
    if fs::remove_dir(path).is_ok() { 0 } else { -1 }
}

/// List directory contents. Returns a list handle of entry names.
fn dir_list(path: &str) -> i64 {
    let h = list_new();
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                list_push(h, Value::Str(name.to_string()));
            }
        }
    }
    h
}

/// Check if path exists and is a directory.
fn dir_exists(path: &str) -> bool {
    Path::new(path).is_dir()
}

/// Check if path is a directory.
fn is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

/// Check if path is a regular file.
fn is_file(path: &str) -> bool {
    Path::new(path).is_file()
}

// --- Path operations ---
// Removed: path_join, path_parent, path_filename, path_extension
// These are now pure Arth string operations in stdlib/src/io/Path.arth

/// Convert to absolute path (requires OS access for cwd).
fn path_absolute(path: &str) -> String {
    match fs::canonicalize(path) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => path.to_string(),
    }
}

// --- Console I/O ---

/// Read a line from stdin. Returns the line (without newline).
fn console_read_line() -> String {
    let stdin = std::io::stdin();
    let mut line = String::new();
    if stdin.lock().read_line(&mut line).is_ok() {
        // Remove trailing newline
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
    }
    line
}

/// Write to stdout (no newline).
fn console_write(s: &str) {
    let _ = std::io::stdout().write_all(s.as_bytes());
    let _ = std::io::stdout().flush();
}

/// Write to stderr (no newline).
fn console_write_err(s: &str) {
    let _ = std::io::stderr().write_all(s.as_bytes());
    let _ = std::io::stderr().flush();
}

// --- JSON serialization ---
// Store for parsed JSON values (represented as recursive enum)

#[derive(Clone, Debug)]
#[allow(dead_code)] // Fields will be used when JsonValue accessors are implemented
enum JsonVal {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonVal>),
    Object(Vec<(String, JsonVal)>), // Use Vec for order preservation
}

fn json_store() -> &'static Mutex<HashMap<i64, JsonVal>> {
    static J: OnceLock<Mutex<HashMap<i64, JsonVal>>> = OnceLock::new();
    J.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_JSON: AtomicI64 = AtomicI64::new(110_000);

/// Escape a string for JSON output.
fn json_escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Stringify a VM Value to JSON string.
fn json_stringify_value(v: &Value) -> String {
    match v {
        Value::I64(n) => n.to_string(),
        Value::F64(f) => {
            if f.is_nan() || f.is_infinite() {
                "null".to_string()
            } else {
                f.to_string()
            }
        }
        Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Str(s) => json_escape_string(s),
    }
}

/// Stringify a list handle to JSON array.
fn json_stringify_list(h: i64) -> String {
    if let Ok(m) = list_store().lock() {
        if let Some(vec) = m.get(&h) {
            let parts: Vec<String> = vec.iter().map(json_stringify_value).collect();
            return format!("[{}]", parts.join(","));
        }
    }
    "[]".to_string()
}

/// Stringify a map handle to JSON object.
fn json_stringify_map(h: i64) -> String {
    if let Ok(m) = map_store().lock() {
        if let Some(mm) = m.get(&h) {
            let parts: Vec<String> = mm
                .iter()
                .map(|(k, v)| {
                    let key_str = match &k.0 {
                        Value::Str(s) => json_escape_string(s),
                        other => json_escape_string(&json_stringify_value(other)),
                    };
                    format!("{}:{}", key_str, json_stringify_value(v))
                })
                .collect();
            return format!("{{{}}}", parts.join(","));
        }
    }
    "{}".to_string()
}

/// Check if handle is a list handle (50_000 - 59_999 range).
fn is_list_handle(h: i64) -> bool {
    (50_000..60_000).contains(&h)
}

/// Check if handle is a map handle (60_000 - 69_999 range).
fn is_map_handle(h: i64) -> bool {
    (60_000..70_000).contains(&h)
}

/// Check if handle is a struct handle (90_000+ range, below 100_000 which is enums).
fn is_struct_handle(h: i64) -> bool {
    (90_000..100_000).contains(&h)
}

/// Stringify a struct handle to JSON object using its field names.
/// Note: Must copy data before releasing lock to avoid deadlock on nested structs.
fn json_stringify_struct(h: i64) -> String {
    // Copy field data while holding the lock, then release before recursing
    let field_data: Vec<(String, Value)> = {
        if let Ok(m) = struct_store().lock() {
            if let Some(s) = m.get(&h) {
                s.field_names
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, name)| {
                        if idx < s.fields.len() && !name.is_empty() {
                            Some((name.clone(), s.fields[idx].clone()))
                        } else {
                            None
                        }
                    })
                    .collect()
            } else {
                return "{}".to_string();
            }
        } else {
            return "{}".to_string();
        }
    };
    // Lock is released, now safe to recurse
    let parts: Vec<String> = field_data
        .iter()
        .map(|(name, val)| {
            let key_str = json_escape_string(name);
            let val_str = json_stringify_recursive(val);
            format!("{}:{}", key_str, val_str)
        })
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// Recursive JSON stringify that handles nested structs/lists/maps.
fn json_stringify_recursive(v: &Value) -> String {
    match v {
        Value::I64(h) if is_list_handle(*h) => json_stringify_list_recursive(*h),
        Value::I64(h) if is_map_handle(*h) => json_stringify_map_recursive(*h),
        Value::I64(h) if is_struct_handle(*h) => json_stringify_struct(*h),
        other => json_stringify_value(other),
    }
}

/// Recursive list stringify that handles nested values.
/// Note: Must copy data before releasing lock to avoid deadlock on nested lists.
fn json_stringify_list_recursive(h: i64) -> String {
    // Copy values while holding the lock, then release before recursing
    let values: Vec<Value> = {
        if let Ok(m) = list_store().lock() {
            if let Some(vec) = m.get(&h) {
                vec.clone()
            } else {
                return "[]".to_string();
            }
        } else {
            return "[]".to_string();
        }
    };
    // Lock is released, now safe to recurse
    let parts: Vec<String> = values.iter().map(json_stringify_recursive).collect();
    format!("[{}]", parts.join(","))
}

/// Recursive map stringify that handles nested values.
/// Note: Must copy data before releasing lock to avoid deadlock on nested maps.
fn json_stringify_map_recursive(h: i64) -> String {
    // Copy entries while holding the lock, then release before recursing
    let entries: Vec<(Value, Value)> = {
        if let Ok(m) = map_store().lock() {
            if let Some(mm) = m.get(&h) {
                mm.iter().map(|(k, v)| (k.0.clone(), v.clone())).collect()
            } else {
                return "{}".to_string();
            }
        } else {
            return "{}".to_string();
        }
    };
    // Lock is released, now safe to recurse
    let parts: Vec<String> = entries
        .iter()
        .map(|(k, v)| {
            let key_str = match k {
                Value::Str(s) => json_escape_string(s),
                other => json_escape_string(&json_stringify_value(other)),
            };
            format!("{}:{}", key_str, json_stringify_recursive(v))
        })
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// Stringify any value (primitive or handle) to JSON.
fn json_stringify(v: &Value) -> String {
    match v {
        Value::I64(h) if is_list_handle(*h) => json_stringify_list_recursive(*h),
        Value::I64(h) if is_map_handle(*h) => json_stringify_map_recursive(*h),
        Value::I64(h) if is_struct_handle(*h) => json_stringify_struct(*h),
        other => json_stringify_value(other),
    }
}

/// Stringify a JsonVal to JSON string.
fn json_stringify_jsonval(v: &JsonVal) -> String {
    match v {
        JsonVal::Null => "null".to_string(),
        JsonVal::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        JsonVal::Number(n) => {
            if n.is_nan() || n.is_infinite() {
                "null".to_string()
            } else {
                n.to_string()
            }
        }
        JsonVal::String(s) => json_escape_string(s),
        JsonVal::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(json_stringify_jsonval).collect();
            format!("[{}]", parts.join(","))
        }
        JsonVal::Object(pairs) => {
            let parts: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}:{}", json_escape_string(k), json_stringify_jsonval(v)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

/// Simple recursive descent JSON parser.
/// Returns (parsed value, remaining input) or None on error.
fn parse_json(s: &str) -> Option<(JsonVal, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }

    match s.chars().next()? {
        'n' if s.starts_with("null") => Some((JsonVal::Null, &s[4..])),
        't' if s.starts_with("true") => Some((JsonVal::Bool(true), &s[4..])),
        'f' if s.starts_with("false") => Some((JsonVal::Bool(false), &s[5..])),
        '"' => parse_json_string(s),
        '[' => parse_json_array(s),
        '{' => parse_json_object(s),
        c if c == '-' || c.is_ascii_digit() => parse_json_number(s),
        _ => None,
    }
}

/// Parse a JSON string literal.
fn parse_json_string(s: &str) -> Option<(JsonVal, &str)> {
    if !s.starts_with('"') {
        return None;
    }
    let s = &s[1..];
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    let mut consumed = 1; // Opening quote

    while let Some(c) = chars.next() {
        consumed += c.len_utf8();
        match c {
            '"' => {
                return Some((JsonVal::String(result), &s[consumed - 1..]));
            }
            '\\' => {
                if let Some(esc) = chars.next() {
                    consumed += esc.len_utf8();
                    match esc {
                        '"' => result.push('"'),
                        '\\' => result.push('\\'),
                        '/' => result.push('/'),
                        'n' => result.push('\n'),
                        'r' => result.push('\r'),
                        't' => result.push('\t'),
                        'b' => result.push('\x08'),
                        'f' => result.push('\x0C'),
                        'u' => {
                            // Parse 4 hex digits
                            let mut hex = String::new();
                            for _ in 0..4 {
                                if let Some(h) = chars.next() {
                                    consumed += h.len_utf8();
                                    hex.push(h);
                                }
                            }
                            if let Ok(code) = u32::from_str_radix(&hex, 16) {
                                if let Some(ch) = char::from_u32(code) {
                                    result.push(ch);
                                }
                            }
                        }
                        _ => result.push(esc),
                    }
                }
            }
            _ => result.push(c),
        }
    }
    None // Unterminated string
}

/// Parse a JSON number.
fn parse_json_number(s: &str) -> Option<(JsonVal, &str)> {
    let mut end = 0;
    let chars: Vec<char> = s.chars().collect();

    // Optional leading minus
    if end < chars.len() && chars[end] == '-' {
        end += 1;
    }

    // Integer part
    while end < chars.len() && chars[end].is_ascii_digit() {
        end += 1;
    }

    // Fractional part
    if end < chars.len() && chars[end] == '.' {
        end += 1;
        while end < chars.len() && chars[end].is_ascii_digit() {
            end += 1;
        }
    }

    // Exponent part
    if end < chars.len() && (chars[end] == 'e' || chars[end] == 'E') {
        end += 1;
        if end < chars.len() && (chars[end] == '+' || chars[end] == '-') {
            end += 1;
        }
        while end < chars.len() && chars[end].is_ascii_digit() {
            end += 1;
        }
    }

    if end == 0 {
        return None;
    }

    let num_str: String = chars[..end].iter().collect();
    let byte_len: usize = num_str.len();

    if let Ok(n) = num_str.parse::<f64>() {
        Some((JsonVal::Number(n), &s[byte_len..]))
    } else {
        None
    }
}

/// Parse a JSON array.
fn parse_json_array(s: &str) -> Option<(JsonVal, &str)> {
    if !s.starts_with('[') {
        return None;
    }
    let mut s = s[1..].trim_start();
    let mut items = Vec::new();

    if s.starts_with(']') {
        return Some((JsonVal::Array(items), &s[1..]));
    }

    loop {
        let (val, rest) = parse_json(s)?;
        items.push(val);
        s = rest.trim_start();

        if s.starts_with(']') {
            return Some((JsonVal::Array(items), &s[1..]));
        } else if s.starts_with(',') {
            s = s[1..].trim_start();
        } else {
            return None;
        }
    }
}

/// Parse a JSON object.
fn parse_json_object(s: &str) -> Option<(JsonVal, &str)> {
    if !s.starts_with('{') {
        return None;
    }
    let mut s = s[1..].trim_start();
    let mut items = Vec::new();

    if s.starts_with('}') {
        return Some((JsonVal::Object(items), &s[1..]));
    }

    loop {
        // Parse key (must be string)
        let (key_val, rest) = parse_json_string(s)?;
        let key = match key_val {
            JsonVal::String(k) => k,
            _ => return None,
        };
        s = rest.trim_start();

        // Expect colon
        if !s.starts_with(':') {
            return None;
        }
        s = s[1..].trim_start();

        // Parse value
        let (val, rest) = parse_json(s)?;
        items.push((key, val));
        s = rest.trim_start();

        if s.starts_with('}') {
            return Some((JsonVal::Object(items), &s[1..]));
        } else if s.starts_with(',') {
            s = s[1..].trim_start();
        } else {
            return None;
        }
    }
}

/// Parse a JSON string and store the result, returning a handle.
/// Returns -1 on parse error.
fn json_parse_and_store(s: &str) -> i64 {
    let is_event_related = s.contains("has_value") || s.contains("payload") || s.contains("intent");
    match parse_json(s.trim()) {
        Some((val, rest)) if rest.trim().is_empty() => {
            let h = NEXT_JSON.fetch_add(1, Ordering::Relaxed);
            if is_event_related {
                eprintln!("[VM_DEBUG] json_parse_and_store: parsed successfully, handle={}, val={:?}", h, &format!("{:?}", val).chars().take(300).collect::<String>());
            }
            if let Ok(mut m) = json_store().lock() {
                m.insert(h, val);
            }
            h
        }
        Some((_, rest)) => {
            if is_event_related {
                eprintln!("[VM_DEBUG] json_parse_and_store: parse succeeded but trailing content: '{}'", rest);
            }
            -1
        }
        None => {
            if is_event_related {
                eprintln!("[VM_DEBUG] json_parse_and_store: parse failed for input: '{}'", &s.chars().take(100).collect::<String>());
            }
            -1
        }
    }
}

// --- JSON value accessor functions ---

/// Get a field from a JSON object by key.
/// Returns a new handle to the field value, or -1 if not found or not an object.
fn json_get_field(handle: i64, key: &str) -> i64 {
    let is_interesting = key == "value" || key == "payload" || key == "intent" || key == "intentArgs" || key == "has_value";
    let store = json_store();
    let field_val = {
        let Ok(guard) = store.lock() else {
            if is_interesting {
                eprintln!("[VM_DEBUG] json_get_field: failed to lock store");
            }
            return -1;
        };
        let Some(val) = guard.get(&handle) else {
            if is_interesting {
                eprintln!("[VM_DEBUG] json_get_field: handle {} not found", handle);
            }
            return -1;
        };

        if let JsonVal::Object(pairs) = val {
            if is_interesting {
                let keys: Vec<_> = pairs.iter().map(|(k, _)| k.as_str()).collect();
                eprintln!("[VM_DEBUG] json_get_field: handle={}, looking for '{}', available keys={:?}", handle, key, keys);
            }
            pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        } else {
            if is_interesting {
                eprintln!("[VM_DEBUG] json_get_field: handle {} is not an Object", handle);
            }
            None
        }
    };

    match field_val {
        Some(v) => {
            let h = NEXT_JSON.fetch_add(1, Ordering::Relaxed);
            if is_interesting {
                eprintln!("[VM_DEBUG] json_get_field: found '{}', stored as handle {}, val={:?}", key, h, &format!("{:?}", v).chars().take(100).collect::<String>());
            }
            if let Ok(mut m) = store.lock() {
                m.insert(h, v);
            }
            h
        }
        None => {
            if is_interesting {
                eprintln!("[VM_DEBUG] json_get_field: '{}' NOT FOUND", key);
            }
            -1
        }
    }
}

/// Get an element from a JSON array by index.
/// Returns a new handle to the element, or -1 if out of bounds or not an array.
fn json_get_index(handle: i64, index: i64) -> i64 {
    if index < 0 {
        return -1;
    }
    let store = json_store();
    let elem_val = {
        let Ok(guard) = store.lock() else { return -1 };
        let Some(val) = guard.get(&handle) else { return -1 };

        if let JsonVal::Array(arr) = val {
            arr.get(index as usize).cloned()
        } else {
            None
        }
    };

    match elem_val {
        Some(v) => {
            let h = NEXT_JSON.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut m) = store.lock() {
                m.insert(h, v);
            }
            h
        }
        None => -1,
    }
}

/// Get the string value from a JSON value handle.
/// Returns empty string if not a string.
fn json_get_string(handle: i64) -> String {
    let Ok(guard) = json_store().lock() else {
        return String::new();
    };
    match guard.get(&handle) {
        Some(JsonVal::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Get the number value from a JSON value handle.
/// Returns 0.0 if not a number.
fn json_get_number(handle: i64) -> f64 {
    let Ok(guard) = json_store().lock() else {
        return 0.0;
    };
    match guard.get(&handle) {
        Some(JsonVal::Number(n)) => *n,
        _ => 0.0,
    }
}

/// Get the boolean value from a JSON value handle.
/// Returns 0 if not a boolean (1 for true, 0 for false).
fn json_get_bool(handle: i64) -> i64 {
    let Ok(guard) = json_store().lock() else {
        return 0;
    };
    match guard.get(&handle) {
        Some(JsonVal::Bool(b)) => if *b { 1 } else { 0 },
        _ => 0,
    }
}

/// Check if a JSON value is null.
fn json_is_null(handle: i64) -> i64 {
    let Ok(guard) = json_store().lock() else {
        return 0;
    };
    match guard.get(&handle) {
        Some(JsonVal::Null) => 1,
        _ => 0,
    }
}

/// Check if a JSON value is an object.
fn json_is_object(handle: i64) -> i64 {
    let Ok(guard) = json_store().lock() else {
        return 0;
    };
    match guard.get(&handle) {
        Some(JsonVal::Object(_)) => 1,
        _ => 0,
    }
}

/// Check if a JSON value is an array.
fn json_is_array(handle: i64) -> i64 {
    let Ok(guard) = json_store().lock() else {
        return 0;
    };
    match guard.get(&handle) {
        Some(JsonVal::Array(_)) => 1,
        _ => 0,
    }
}

/// Get the length of a JSON array.
/// Returns -1 if not an array.
fn json_array_len(handle: i64) -> i64 {
    let Ok(guard) = json_store().lock() else {
        return -1;
    };
    match guard.get(&handle) {
        Some(JsonVal::Array(arr)) => arr.len() as i64,
        _ => -1,
    }
}

/// Get all keys from a JSON object as a list of strings.
/// Returns an empty list handle if not an object.
fn json_keys(handle: i64) -> i64 {
    let Ok(guard) = json_store().lock() else {
        return list_new();
    };

    let keys: Vec<String> = match guard.get(&handle) {
        Some(JsonVal::Object(pairs)) => pairs.iter().map(|(k, _)| k.clone()).collect(),
        _ => Vec::new(),
    };
    drop(guard);

    // Create a list and push all keys
    let list_h = list_new();
    for key in keys {
        list_push(list_h, Value::Str(key));
    }
    list_h
}

/// Struct JSON metadata format:
/// Enhanced: "name:idx,name:idx,...;flags" where:
///   - name:idx pairs map JSON field name to struct field index
///   - fields with @JsonIgnore are omitted (index skipped)
///   - flags after ';' can include: 'I' for ignoreUnknown
/// Legacy (backwards compat): "x,y,z" (comma-separated names, sequential indices)
struct StructJsonMeta {
    /// (json_name, struct_index) pairs for serialization/deserialization
    fields: Vec<(String, usize)>,
    /// Total number of fields in the struct (including ignored ones)
    total_fields: usize,
    /// Whether to ignore unknown fields during deserialization
    ignore_unknown: bool,
}

fn parse_struct_json_meta(meta_str: &str) -> StructJsonMeta {
    // Check for enhanced format (contains ':' or ';')
    if meta_str.contains(':') || meta_str.contains(';') {
        let (fields_part, flags_part) = if let Some(idx) = meta_str.find(';') {
            (&meta_str[..idx], &meta_str[idx + 1..])
        } else {
            (meta_str, "")
        };

        let ignore_unknown = flags_part.contains('I');
        let mut fields = Vec::new();
        let mut max_idx = 0usize;

        for pair in fields_part.split(',') {
            if pair.is_empty() {
                continue;
            }
            if let Some(colon_idx) = pair.find(':') {
                let name = pair[..colon_idx].to_string();
                if let Ok(idx) = pair[colon_idx + 1..].parse::<usize>() {
                    max_idx = max_idx.max(idx + 1);
                    fields.push((name, idx));
                }
            }
        }

        StructJsonMeta {
            fields,
            total_fields: max_idx,
            ignore_unknown,
        }
    } else {
        // Legacy format: "x,y,z" -> sequential indices
        let fields: Vec<(String, usize)> = meta_str
            .split(',')
            .enumerate()
            .filter(|(_, s)| !s.is_empty())
            .map(|(i, s)| (s.to_string(), i))
            .collect();
        let total = fields.len();
        StructJsonMeta {
            fields,
            total_fields: total,
            ignore_unknown: false,
        }
    }
}

/// Result type for struct serialization (to handle cycle errors)
#[derive(Debug)]
enum StructToJsonResult {
    Ok(String),
    CycleDetected,
}

/// Serialize a struct (represented as list handle) to JSON with cycle detection.
/// Field metadata format: "name:idx,name:idx,...;flags" or legacy "x,y,z"
fn struct_to_json(struct_handle: i64, field_meta: &str) -> StructToJsonResult {
    let mut visited = std::collections::HashSet::new();
    struct_to_json_with_cycle_check(struct_handle, field_meta, &mut visited)
}

fn struct_to_json_with_cycle_check(
    struct_handle: i64,
    field_meta: &str,
    visited: &mut std::collections::HashSet<i64>,
) -> StructToJsonResult {
    // Check for cycles
    if visited.contains(&struct_handle) {
        return StructToJsonResult::CycleDetected;
    }
    visited.insert(struct_handle);

    let meta = parse_struct_json_meta(field_meta);

    let result = if let Ok(m) = list_store().lock() {
        if let Some(values) = m.get(&struct_handle) {
            let mut parts = Vec::new();
            for (name, idx) in &meta.fields {
                if let Some(val) = values.get(*idx) {
                    // Check if value is a struct handle (list handle) that could cause cycles
                    let json_val = match val {
                        Value::I64(h) if *h >= 100_000 && *h < 110_000 => {
                            // This is a list handle, could be a nested struct
                            // For MVP, we stringify it as a nested object if it looks like a struct
                            // In full impl, we'd need type info to know the nested field names
                            json_stringify_value(val)
                        }
                        _ => json_stringify_value(val),
                    };
                    parts.push(format!("\"{}\":{}", name, json_val));
                }
            }
            format!("{{{}}}", parts.join(","))
        } else {
            "{}".to_string()
        }
    } else {
        "{}".to_string()
    };

    visited.remove(&struct_handle);
    StructToJsonResult::Ok(result)
}

/// Deserialize JSON to a struct (returns list handle).
/// Field metadata format: "name:idx,name:idx,...;flags" or legacy "x,y,z"
/// Returns -1 on parse error, -2 on unknown field error (when !ignoreUnknown)
fn json_to_struct(json_str: &str, field_meta: &str) -> i64 {
    let meta = parse_struct_json_meta(field_meta);

    // Parse JSON
    let json_handle = json_parse_and_store(json_str);
    if json_handle < 0 {
        return -1;
    }

    // Create struct as list with all fields (including ignored ones)
    let struct_handle = list_new();

    // Pre-fill with default values for all fields
    for _ in 0..meta.total_fields {
        list_push(struct_handle, Value::I64(0));
    }

    if let Ok(jstore) = json_store().lock() {
        if let Some(JsonVal::Object(obj)) = jstore.get(&json_handle) {
            // Build a set of known field names for unknown field checking
            let known_fields: std::collections::HashSet<&str> =
                meta.fields.iter().map(|(n, _)| n.as_str()).collect();

            // Check for unknown fields if not ignoring them
            if !meta.ignore_unknown {
                for (json_key, _) in obj {
                    if !known_fields.contains(json_key.as_str()) {
                        // Unknown field error
                        return -2;
                    }
                }
            }

            // Set values at their correct indices
            for (name, idx) in &meta.fields {
                if let Some((_, jval)) = obj.iter().find(|(k, _)| k == name) {
                    let val = json_val_to_vm_value(jval);
                    list_set(struct_handle, *idx, val);
                }
            }
        }
    }
    struct_handle
}

/// Set a value at a specific index in a list.
/// Returns `true` if successful, `false` if index is out of bounds.
fn list_set(handle: i64, idx: usize, val: Value) -> bool {
    if let Ok(mut m) = list_store().lock() {
        if let Some(list) = m.get_mut(&handle) {
            if idx < list.len() {
                list[idx] = val;
                return true;
            }
        }
    }
    false
}

/// Convert a JsonVal to a VM Value.
fn json_val_to_vm_value(jv: &JsonVal) -> Value {
    match jv {
        JsonVal::Null => Value::I64(0),
        // Return I64 for booleans (1 for true, 0 for false), consistent with json_get_bool
        JsonVal::Bool(b) => Value::I64(if *b { 1 } else { 0 }),
        JsonVal::Number(n) => {
            // If it's a whole number, return as I64
            if n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64 {
                Value::I64(*n as i64)
            } else {
                Value::F64(*n)
            }
        }
        JsonVal::String(s) => Value::Str(s.clone()),
        JsonVal::Array(_) | JsonVal::Object(_) => Value::I64(0), // nested not supported in MVP
    }
}

// --- DateTime operations ---
// DateTime is represented as i64 milliseconds since Unix epoch

use std::time::{Instant as StdInstant, SystemTime, UNIX_EPOCH};

/// Get current time as milliseconds since Unix epoch.
fn datetime_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// Removed: datetime_year, datetime_month, datetime_day, datetime_hour, datetime_minute,
//          datetime_second, datetime_day_of_week, datetime_day_of_year
//          - now pure Arth in stdlib/src/time/DateTime.arth
// Kept helpers below for datetime_format/datetime_parse which still need native impl

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Helper to convert millis to year, month, day.
fn millis_to_ymd(millis: i64) -> (i64, i64, i64) {
    let secs = millis / 1000;
    let days = secs / 86400;

    let mut year = 1970i64;
    let mut remaining_days = days;

    // Find year
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    // Days in each month
    let days_in_months: [i64; 12] = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    // Find month and day
    let mut month = 1i64;
    for &days_in_month in &days_in_months {
        if remaining_days < days_in_month {
            break;
        }
        remaining_days -= days_in_month;
        month += 1;
    }

    let day = remaining_days + 1; // Days are 1-indexed
    (year, month, day)
}

/// Format datetime to string. Simple ISO-like format for MVP.
fn datetime_format(millis: i64, _format: &str) -> String {
    let (year, month, day) = millis_to_ymd(millis);
    let secs = millis / 1000;
    let hour = (secs % 86400) / 3600;
    let minute = (secs % 3600) / 60;
    let second = secs % 60;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, minute, second
    )
}

/// Parse datetime from string. Returns millis or -1 on error.
/// Supports simple ISO format: YYYY-MM-DDTHH:MM:SSZ
fn datetime_parse(_format: &str, input: &str) -> i64 {
    // Simple ISO format parsing
    let parts: Vec<&str> = input.split('T').collect();
    if parts.len() != 2 {
        return -1;
    }

    let date_parts: Vec<&str> = parts[0].split('-').collect();
    if date_parts.len() != 3 {
        return -1;
    }

    let time_str = parts[1].trim_end_matches('Z');
    let time_parts: Vec<&str> = time_str.split(':').collect();
    if time_parts.len() != 3 {
        return -1;
    }

    let year: i64 = date_parts[0].parse().unwrap_or(-1);
    let month: i64 = date_parts[1].parse().unwrap_or(-1);
    let day: i64 = date_parts[2].parse().unwrap_or(-1);
    let hour: i64 = time_parts[0].parse().unwrap_or(-1);
    let minute: i64 = time_parts[1].parse().unwrap_or(-1);
    let second: i64 = time_parts[2].parse().unwrap_or(-1);

    if year < 0 || month < 1 || month > 12 || day < 1 || day > 31 {
        return -1;
    }
    if hour < 0 || hour > 23 || minute < 0 || minute > 59 || second < 0 || second > 59 {
        return -1;
    }

    ymd_hms_to_millis(year, month, day, hour, minute, second)
}

/// Convert year, month, day, hour, minute, second to millis since epoch.
fn ymd_hms_to_millis(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> i64 {
    // Count days from 1970 to year
    let mut days: i64 = 0;
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }

    // Add days for months
    let days_in_months: [i64; 12] = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    for i in 0..(month - 1) as usize {
        days += days_in_months[i];
    }

    // Add day of month (1-indexed, so subtract 1)
    days += day - 1;

    // Convert to milliseconds
    let secs = days * 86400 + hour * 3600 + minute * 60 + second;
    secs * 1000
}

// --- Instant operations (monotonic clock) ---

fn instant_store() -> &'static Mutex<HashMap<i64, StdInstant>> {
    static I: OnceLock<Mutex<HashMap<i64, StdInstant>>> = OnceLock::new();
    I.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_INSTANT: AtomicI64 = AtomicI64::new(200_000);

/// Create a new instant (captures current monotonic time).
fn instant_now() -> i64 {
    let h = NEXT_INSTANT.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut m) = instant_store().lock() {
        m.insert(h, StdInstant::now());
    }
    h
}

/// Get elapsed time in milliseconds since instant was created.
fn instant_elapsed(h: i64) -> i64 {
    if let Ok(m) = instant_store().lock() {
        if let Some(instant) = m.get(&h) {
            return instant.elapsed().as_millis() as i64;
        }
    }
    0
}

// --- BigDecimal operations ---
// BigDecimal is stored as a string for arbitrary precision.
// Handle maps to string representation.

fn bigdecimal_store() -> &'static Mutex<HashMap<i64, String>> {
    static BD: OnceLock<Mutex<HashMap<i64, String>>> = OnceLock::new();
    BD.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_BIGDECIMAL: AtomicI64 = AtomicI64::new(300_000);

/// Create a BigDecimal from a string representation.
fn bigdecimal_new(s: &str) -> i64 {
    // Validate it looks like a decimal number
    let s = s.trim();
    if s.is_empty() {
        return -1;
    }

    // Simple validation: allow optional sign, digits, optional decimal point, digits
    let mut chars = s.chars().peekable();
    if chars.peek() == Some(&'-') || chars.peek() == Some(&'+') {
        chars.next();
    }

    let mut has_digit = false;
    let mut has_dot = false;
    for c in chars {
        if c.is_ascii_digit() {
            has_digit = true;
        } else if c == '.' && !has_dot {
            has_dot = true;
        } else {
            return -1; // Invalid character
        }
    }

    if !has_digit {
        return -1;
    }

    let h = NEXT_BIGDECIMAL.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut m) = bigdecimal_store().lock() {
        m.insert(h, normalize_decimal(s));
    }
    h
}

/// Normalize a decimal string (remove leading zeros, trailing zeros after decimal).
fn normalize_decimal(s: &str) -> String {
    let s = s.trim();
    let (sign, rest) = if s.starts_with('-') {
        ("-", &s[1..])
    } else if s.starts_with('+') {
        ("", &s[1..])
    } else {
        ("", s)
    };

    let rest = rest.trim_start_matches('0');
    let rest = if rest.is_empty() || rest.starts_with('.') {
        format!("0{}", rest)
    } else {
        rest.to_string()
    };

    // Remove trailing zeros after decimal point
    let result = if rest.contains('.') {
        let trimmed = rest.trim_end_matches('0').trim_end_matches('.');
        if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.to_string()
        }
    } else {
        rest
    };

    if result == "0" || result == "-0" {
        "0".to_string()
    } else {
        format!("{}{}", sign, result)
    }
}

/// Create BigDecimal from i64.
fn bigdecimal_from_int(n: i64) -> i64 {
    bigdecimal_new(&n.to_string())
}

/// Create BigDecimal from f64.
fn bigdecimal_from_float(f: f64) -> i64 {
    bigdecimal_new(&f.to_string())
}

/// Get BigDecimal value as string.
fn bigdecimal_get(h: i64) -> Option<String> {
    if let Ok(m) = bigdecimal_store().lock() {
        return m.get(&h).cloned();
    }
    None
}

/// Add two BigDecimals. Returns new handle.
fn bigdecimal_add(h1: i64, h2: i64) -> i64 {
    let (s1, s2) = match (bigdecimal_get(h1), bigdecimal_get(h2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return -1,
    };

    // For MVP, use f64 arithmetic (loses precision for very large numbers)
    let v1: f64 = s1.parse().unwrap_or(0.0);
    let v2: f64 = s2.parse().unwrap_or(0.0);
    bigdecimal_new(&(v1 + v2).to_string())
}

/// Subtract two BigDecimals. Returns new handle.
fn bigdecimal_sub(h1: i64, h2: i64) -> i64 {
    let (s1, s2) = match (bigdecimal_get(h1), bigdecimal_get(h2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return -1,
    };

    let v1: f64 = s1.parse().unwrap_or(0.0);
    let v2: f64 = s2.parse().unwrap_or(0.0);
    bigdecimal_new(&(v1 - v2).to_string())
}

/// Multiply two BigDecimals. Returns new handle.
fn bigdecimal_mul(h1: i64, h2: i64) -> i64 {
    let (s1, s2) = match (bigdecimal_get(h1), bigdecimal_get(h2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return -1,
    };

    let v1: f64 = s1.parse().unwrap_or(0.0);
    let v2: f64 = s2.parse().unwrap_or(0.0);
    bigdecimal_new(&(v1 * v2).to_string())
}

/// Divide two BigDecimals with specified scale. Returns new handle.
fn bigdecimal_div(h1: i64, h2: i64, scale: i64) -> i64 {
    let (s1, s2) = match (bigdecimal_get(h1), bigdecimal_get(h2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return -1,
    };

    let v1: f64 = s1.parse().unwrap_or(0.0);
    let v2: f64 = s2.parse().unwrap_or(0.0);

    if v2 == 0.0 {
        return -1; // Division by zero
    }

    let result = v1 / v2;
    let formatted = format!("{:.prec$}", result, prec = scale as usize);
    bigdecimal_new(&formatted)
}

/// Remainder of two BigDecimals. Returns new handle.
fn bigdecimal_rem(h1: i64, h2: i64) -> i64 {
    let (s1, s2) = match (bigdecimal_get(h1), bigdecimal_get(h2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return -1,
    };

    let v1: f64 = s1.parse().unwrap_or(0.0);
    let v2: f64 = s2.parse().unwrap_or(0.0);

    if v2 == 0.0 {
        return -1;
    }

    bigdecimal_new(&(v1 % v2).to_string())
}

/// Power of BigDecimal. Returns new handle.
fn bigdecimal_pow(h: i64, exp: i64) -> i64 {
    let s = match bigdecimal_get(h) {
        Some(a) => a,
        _ => return -1,
    };

    let v: f64 = s.parse().unwrap_or(0.0);
    bigdecimal_new(&v.powi(exp as i32).to_string())
}

// Removed: bigdecimal_abs - now pure Arth in stdlib/src/numeric/BigDecimal.arth

/// Negate BigDecimal. Returns new handle.
fn bigdecimal_negate(h: i64) -> i64 {
    let s = match bigdecimal_get(h) {
        Some(a) => a,
        _ => return -1,
    };

    let v: f64 = s.parse().unwrap_or(0.0);
    bigdecimal_new(&(-v).to_string())
}

/// Compare two BigDecimals. Returns -1, 0, or 1.
fn bigdecimal_compare(h1: i64, h2: i64) -> i64 {
    let (s1, s2) = match (bigdecimal_get(h1), bigdecimal_get(h2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return 0,
    };

    let v1: f64 = s1.parse().unwrap_or(0.0);
    let v2: f64 = s2.parse().unwrap_or(0.0);

    if v1 < v2 {
        -1
    } else if v1 > v2 {
        1
    } else {
        0
    }
}

/// Convert BigDecimal to i64 (truncated).
fn bigdecimal_to_int(h: i64) -> i64 {
    let s = match bigdecimal_get(h) {
        Some(a) => a,
        _ => return 0,
    };

    let v: f64 = s.parse().unwrap_or(0.0);
    v as i64
}

/// Convert BigDecimal to f64.
fn bigdecimal_to_float(h: i64) -> f64 {
    let s = match bigdecimal_get(h) {
        Some(a) => a,
        _ => return 0.0,
    };

    s.parse().unwrap_or(0.0)
}

/// Get scale (number of decimal places) of BigDecimal.
fn bigdecimal_scale(h: i64) -> i64 {
    let s = match bigdecimal_get(h) {
        Some(a) => a,
        _ => return 0,
    };

    if let Some(pos) = s.find('.') {
        (s.len() - pos - 1) as i64
    } else {
        0
    }
}

/// Round BigDecimal to specified scale.
fn bigdecimal_round(h: i64, scale: i64, _rounding_mode: i64) -> i64 {
    let s = match bigdecimal_get(h) {
        Some(a) => a,
        _ => return -1,
    };

    let v: f64 = s.parse().unwrap_or(0.0);
    let multiplier = 10f64.powi(scale as i32);
    let rounded = (v * multiplier).round() / multiplier;
    bigdecimal_new(&format!("{:.prec$}", rounded, prec = scale as usize))
}

// --- BigInt operations ---
// BigInt is stored as a string for arbitrary precision.

fn bigint_store() -> &'static Mutex<HashMap<i64, String>> {
    static BI: OnceLock<Mutex<HashMap<i64, String>>> = OnceLock::new();
    BI.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_BIGINT: AtomicI64 = AtomicI64::new(400_000);

/// Create a BigInt from a string representation.
fn bigint_new(s: &str) -> i64 {
    let s = s.trim();
    if s.is_empty() {
        return -1;
    }

    // Validate: optional sign followed by digits only
    let mut chars = s.chars().peekable();
    if chars.peek() == Some(&'-') || chars.peek() == Some(&'+') {
        chars.next();
    }

    let mut has_digit = false;
    for c in chars {
        if c.is_ascii_digit() {
            has_digit = true;
        } else {
            return -1;
        }
    }

    if !has_digit {
        return -1;
    }

    let h = NEXT_BIGINT.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut m) = bigint_store().lock() {
        m.insert(h, normalize_bigint(s));
    }
    h
}

/// Normalize a big integer string (remove leading zeros).
fn normalize_bigint(s: &str) -> String {
    let s = s.trim();
    let (sign, rest) = if s.starts_with('-') {
        ("-", &s[1..])
    } else if s.starts_with('+') {
        ("", &s[1..])
    } else {
        ("", s)
    };

    let rest = rest.trim_start_matches('0');
    let rest = if rest.is_empty() { "0" } else { rest };

    if rest == "0" {
        "0".to_string()
    } else {
        format!("{}{}", sign, rest)
    }
}

/// Create BigInt from i64.
fn bigint_from_int(n: i64) -> i64 {
    bigint_new(&n.to_string())
}

/// Get BigInt value as string.
fn bigint_get(h: i64) -> Option<String> {
    if let Ok(m) = bigint_store().lock() {
        return m.get(&h).cloned();
    }
    None
}

/// Add two BigInts. Returns new handle.
fn bigint_add(h1: i64, h2: i64) -> i64 {
    let (s1, s2) = match (bigint_get(h1), bigint_get(h2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return -1,
    };

    // For MVP, use i128 arithmetic
    let v1: i128 = s1.parse().unwrap_or(0);
    let v2: i128 = s2.parse().unwrap_or(0);
    bigint_new(&(v1 + v2).to_string())
}

/// Subtract two BigInts. Returns new handle.
fn bigint_sub(h1: i64, h2: i64) -> i64 {
    let (s1, s2) = match (bigint_get(h1), bigint_get(h2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return -1,
    };

    let v1: i128 = s1.parse().unwrap_or(0);
    let v2: i128 = s2.parse().unwrap_or(0);
    bigint_new(&(v1 - v2).to_string())
}

/// Multiply two BigInts. Returns new handle.
fn bigint_mul(h1: i64, h2: i64) -> i64 {
    let (s1, s2) = match (bigint_get(h1), bigint_get(h2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return -1,
    };

    let v1: i128 = s1.parse().unwrap_or(0);
    let v2: i128 = s2.parse().unwrap_or(0);
    bigint_new(&(v1 * v2).to_string())
}

/// Divide two BigInts. Returns new handle.
fn bigint_div(h1: i64, h2: i64) -> i64 {
    let (s1, s2) = match (bigint_get(h1), bigint_get(h2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return -1,
    };

    let v1: i128 = s1.parse().unwrap_or(0);
    let v2: i128 = s2.parse().unwrap_or(0);

    if v2 == 0 {
        return -1;
    }

    bigint_new(&(v1 / v2).to_string())
}

/// Remainder of two BigInts. Returns new handle.
fn bigint_rem(h1: i64, h2: i64) -> i64 {
    let (s1, s2) = match (bigint_get(h1), bigint_get(h2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return -1,
    };

    let v1: i128 = s1.parse().unwrap_or(0);
    let v2: i128 = s2.parse().unwrap_or(0);

    if v2 == 0 {
        return -1;
    }

    bigint_new(&(v1 % v2).to_string())
}

/// Power of BigInt. Returns new handle.
fn bigint_pow(h: i64, exp: i64) -> i64 {
    let s = match bigint_get(h) {
        Some(a) => a,
        _ => return -1,
    };

    let v: i128 = s.parse().unwrap_or(0);
    let result = v.pow(exp as u32);
    bigint_new(&result.to_string())
}

// Removed: bigint_abs - now pure Arth in stdlib/src/numeric/BigInt.arth

/// Negate BigInt. Returns new handle.
fn bigint_negate(h: i64) -> i64 {
    let s = match bigint_get(h) {
        Some(a) => a,
        _ => return -1,
    };

    let v: i128 = s.parse().unwrap_or(0);
    bigint_new(&(-v).to_string())
}

/// Compare two BigInts. Returns -1, 0, or 1.
fn bigint_compare(h1: i64, h2: i64) -> i64 {
    let (s1, s2) = match (bigint_get(h1), bigint_get(h2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return 0,
    };

    let v1: i128 = s1.parse().unwrap_or(0);
    let v2: i128 = s2.parse().unwrap_or(0);

    match v1.cmp(&v2) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// Convert BigInt to i64 (truncated/clamped).
fn bigint_to_int(h: i64) -> i64 {
    let s = match bigint_get(h) {
        Some(a) => a,
        _ => return 0,
    };

    let v: i128 = s.parse().unwrap_or(0);
    v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

// Removed: bigint_gcd, bigint_mod_pow - now pure Arth in stdlib/src/numeric/BigInt.arth

