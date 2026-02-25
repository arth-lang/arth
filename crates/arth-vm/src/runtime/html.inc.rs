// HTML parsing operations via arth-rt C FFI.
// Provides lenient DOM-like HTML parsing with CSS selector support.
// All state is managed by arth-rt; VM just calls through to C FFI functions.

// -----------------------------------------------------------------------------
// C FFI Declarations
// -----------------------------------------------------------------------------

unsafe extern "C" {
    fn arth_rt_html_parse(html: *const u8, html_len: usize) -> i64;
    fn arth_rt_html_parse_fragment(html: *const u8, html_len: usize) -> i64;
    fn arth_rt_html_free(handle: i64);
    fn arth_rt_html_stringify_len(handle: i64) -> usize;
    fn arth_rt_html_stringify(handle: i64, out: *mut u8, out_len: usize) -> i64;
    fn arth_rt_html_query(doc_handle: i64, selector: *const u8, selector_len: usize) -> i64;
    fn arth_rt_html_query_count(doc_handle: i64, selector: *const u8, selector_len: usize) -> i64;
    fn arth_rt_html_query_nth(doc_handle: i64, selector: *const u8, selector_len: usize, index: usize) -> i64;
    fn arth_rt_html_query_all(doc_handle: i64, selector: *const u8, selector_len: usize, out_handles: *mut i64, max_count: usize) -> i64;
    fn arth_rt_html_text_len(elem_handle: i64) -> usize;
    fn arth_rt_html_text(elem_handle: i64, out: *mut u8, out_len: usize) -> i64;
    fn arth_rt_html_tag_len(elem_handle: i64) -> usize;
    fn arth_rt_html_tag(elem_handle: i64, out: *mut u8, out_len: usize) -> i64;
    fn arth_rt_html_inner_len(elem_handle: i64) -> usize;
    fn arth_rt_html_inner(elem_handle: i64, out: *mut u8, out_len: usize) -> i64;
    fn arth_rt_html_outer_len(elem_handle: i64) -> usize;
    fn arth_rt_html_outer(elem_handle: i64, out: *mut u8, out_len: usize) -> i64;
    fn arth_rt_html_attr_len(elem_handle: i64, name: *const u8, name_len: usize) -> usize;
    fn arth_rt_html_attr(elem_handle: i64, name: *const u8, name_len: usize, out: *mut u8, out_len: usize) -> i64;
    fn arth_rt_html_has_attr(elem_handle: i64, name: *const u8, name_len: usize) -> i32;
    fn arth_rt_html_attr_count(elem_handle: i64) -> i64;
    fn arth_rt_html_attr_name_at(elem_handle: i64, index: usize, out: *mut u8, out_len: usize) -> i64;
    fn arth_rt_html_has_class(elem_handle: i64, class_name: *const u8, class_len: usize) -> i32;
}

// -----------------------------------------------------------------------------
// VM-Level Wrapper Functions
// -----------------------------------------------------------------------------

/// Parse an HTML string and store the document, returning a handle.
fn html_parse(html_str: &str) -> i64 {
    unsafe { arth_rt_html_parse(html_str.as_ptr(), html_str.len()) }
}

/// Parse an HTML fragment.
fn html_parse_fragment(html_str: &str) -> i64 {
    unsafe { arth_rt_html_parse_fragment(html_str.as_ptr(), html_str.len()) }
}

/// Serialize an HTML document/node back to string.
fn html_stringify(handle: i64) -> String {
    let len = unsafe { arth_rt_html_stringify_len(handle) };
    if len == 0 {
        return String::new();
    }
    let mut buf = vec![0u8; len];
    let written = unsafe { arth_rt_html_stringify(handle, buf.as_mut_ptr(), buf.len()) };
    if written < 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..written as usize]).to_string()
}

/// Pretty-print HTML with indentation.
fn html_stringify_pretty(handle: i64, _indent: i64) -> String {
    // For now, just return the regular stringify
    // Full implementation would add proper indentation
    html_stringify(handle)
}

/// Free an HTML document handle.
fn html_free(handle: i64) {
    unsafe { arth_rt_html_free(handle) }
}

/// Get the node type (1=element, 3=text, 8=comment, 9=document).
fn html_node_type(handle: i64) -> i64 {
    // Document handles are in 1_000_000+ range, element handles in 2_000_000+ range
    if handle >= 2_000_000 {
        1 // Element
    } else if handle >= 1_000_000 {
        9 // Document
    } else {
        0 // Unknown
    }
}

/// Get the tag name of an element.
fn html_tag_name(handle: i64) -> String {
    let len = unsafe { arth_rt_html_tag_len(handle) };
    if len == 0 {
        return String::new();
    }
    let mut buf = vec![0u8; len];
    let written = unsafe { arth_rt_html_tag(handle, buf.as_mut_ptr(), buf.len()) };
    if written < 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..written as usize]).to_string()
}

/// Get the text content of a node.
fn html_text_content(handle: i64) -> String {
    let len = unsafe { arth_rt_html_text_len(handle) };
    if len == 0 {
        return String::new();
    }
    let mut buf = vec![0u8; len];
    let written = unsafe { arth_rt_html_text(handle, buf.as_mut_ptr(), buf.len()) };
    if written < 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..written as usize]).to_string()
}

/// Get the inner HTML of an element.
fn html_inner_html(handle: i64) -> String {
    let len = unsafe { arth_rt_html_inner_len(handle) };
    if len == 0 {
        return String::new();
    }
    let mut buf = vec![0u8; len];
    let written = unsafe { arth_rt_html_inner(handle, buf.as_mut_ptr(), buf.len()) };
    if written < 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..written as usize]).to_string()
}

/// Get the outer HTML of an element.
fn html_outer_html(handle: i64) -> String {
    let len = unsafe { arth_rt_html_outer_len(handle) };
    if len == 0 {
        return String::new();
    }
    let mut buf = vec![0u8; len];
    let written = unsafe { arth_rt_html_outer(handle, buf.as_mut_ptr(), buf.len()) };
    if written < 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..written as usize]).to_string()
}

/// Get an attribute value.
fn html_get_attr(handle: i64, attr_name: &str) -> String {
    let len = unsafe { arth_rt_html_attr_len(handle, attr_name.as_ptr(), attr_name.len()) };
    if len == 0 {
        return String::new();
    }
    let mut buf = vec![0u8; len];
    let written = unsafe {
        arth_rt_html_attr(handle, attr_name.as_ptr(), attr_name.len(), buf.as_mut_ptr(), buf.len())
    };
    if written <= 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..written as usize]).to_string()
}

/// Check if an element has an attribute.
fn html_has_attr(handle: i64, attr_name: &str) -> bool {
    unsafe { arth_rt_html_has_attr(handle, attr_name.as_ptr(), attr_name.len()) != 0 }
}

/// Get all attribute names as a list handle.
fn html_attr_names(handle: i64) -> i64 {
    let list = list_new();
    let count = unsafe { arth_rt_html_attr_count(handle) };
    if count <= 0 {
        return list;
    }

    for i in 0..count as usize {
        // Get attr name - use a reasonable buffer size
        let mut buf = vec![0u8; 256];
        let len = unsafe { arth_rt_html_attr_name_at(handle, i, buf.as_mut_ptr(), buf.len()) };
        if len > 0 {
            let name = String::from_utf8_lossy(&buf[..len as usize]).to_string();
            list_push(list, Value::Str(name));
        }
    }
    list
}

/// Get parent node (returns 0 for document).
fn html_parent(_handle: i64) -> i64 {
    // Parent tracking not implemented in MVP
    0
}

/// Get child nodes as a list handle.
fn html_children(_handle: i64) -> i64 {
    // Child tracking not implemented in MVP
    list_new()
}

/// Get element children only.
fn html_element_children(_handle: i64) -> i64 {
    list_new()
}

/// Get first child (returns 0 if none).
fn html_first_child(_handle: i64) -> i64 {
    0
}

/// Get last child (returns 0 if none).
fn html_last_child(_handle: i64) -> i64 {
    0
}

/// Get next sibling (returns 0 if none).
fn html_next_sibling(_handle: i64) -> i64 {
    0
}

/// Get previous sibling (returns 0 if none).
fn html_prev_sibling(_handle: i64) -> i64 {
    0
}

/// Query selector - find first matching element.
fn html_query_selector(handle: i64, selector: &str) -> i64 {
    unsafe { arth_rt_html_query(handle, selector.as_ptr(), selector.len()) }
}

/// Query selector all - find all matching elements.
fn html_query_selector_all(handle: i64, selector: &str) -> i64 {
    let result_list = list_new();

    // First get count
    let count = unsafe { arth_rt_html_query_count(handle, selector.as_ptr(), selector.len()) };
    if count <= 0 {
        return result_list;
    }

    // Allocate handles array and get all matches
    let mut handles = vec![0i64; count as usize];
    let stored = unsafe {
        arth_rt_html_query_all(
            handle,
            selector.as_ptr(),
            selector.len(),
            handles.as_mut_ptr(),
            handles.len(),
        )
    };

    // Add handles to result list
    for i in 0..stored as usize {
        list_push(result_list, Value::I64(handles[i]));
    }

    result_list
}

/// Get element by ID.
fn html_get_by_id(handle: i64, id: &str) -> i64 {
    let selector = format!("#{}", id);
    html_query_selector(handle, &selector)
}

/// Get elements by tag name.
fn html_get_by_tag(handle: i64, tag: &str) -> i64 {
    html_query_selector_all(handle, tag)
}

/// Get elements by class name.
fn html_get_by_class(handle: i64, class_name: &str) -> i64 {
    let selector = format!(".{}", class_name);
    html_query_selector_all(handle, &selector)
}

/// Check if element has a specific class.
fn html_has_class(handle: i64, class_name: &str) -> bool {
    unsafe { arth_rt_html_has_class(handle, class_name.as_ptr(), class_name.len()) != 0 }
}
