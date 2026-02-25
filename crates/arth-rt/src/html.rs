//! HTML parsing and CSS selector support via scraper.
//!
//! Provides C FFI wrappers for HTML parsing operations.
//! Uses scraper (html5ever + selectors) for robust HTML5 parsing.
//!
//! Note: scraper's Html type is not Sync (uses non-atomic tendrils), so we
//! store source strings and re-parse on demand. This is the same approach
//! the original VM code used.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

use scraper::{ElementRef, Html, Selector};

// -----------------------------------------------------------------------------
// Internal State
// -----------------------------------------------------------------------------

/// Represents a parsed HTML document (stores source, parses on demand)
struct ParsedDocument {
    source: String,
    is_fragment: bool,
}

/// Represents an element extracted from a query
struct ParsedElement {
    tag: String,
    attrs: Vec<(String, String)>,
    text_content: String,
    inner_html: String,
    outer_html: String,
}

/// Global store for parsed HTML documents
fn doc_store() -> &'static Mutex<HashMap<i64, ParsedDocument>> {
    static STORE: std::sync::OnceLock<Mutex<HashMap<i64, ParsedDocument>>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Global store for extracted elements
fn elem_store() -> &'static Mutex<HashMap<i64, ParsedElement>> {
    static STORE: std::sync::OnceLock<Mutex<HashMap<i64, ParsedElement>>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Helper to parse a document from store
fn parse_document(source: &str, is_fragment: bool) -> Html {
    if is_fragment {
        Html::parse_fragment(source)
    } else {
        Html::parse_document(source)
    }
}

/// Handle counter for documents (1_000_000+ range)
static NEXT_DOC_HANDLE: AtomicI64 = AtomicI64::new(1_000_000);

/// Handle counter for elements (2_000_000+ range)
static NEXT_ELEM_HANDLE: AtomicI64 = AtomicI64::new(2_000_000);

// -----------------------------------------------------------------------------
// Parsing
// -----------------------------------------------------------------------------

/// Parse an HTML string and return a document handle.
///
/// # Arguments
/// * `html` - Pointer to UTF-8 HTML string
/// * `html_len` - Length of HTML string in bytes
///
/// # Returns
/// * Document handle (>= 1_000_000) on success
/// * -1 on error (null pointer)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_parse(html: *const u8, html_len: usize) -> i64 {
    if html.is_null() {
        return -1;
    }

    let html_str = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(html, html_len)) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    let handle = NEXT_DOC_HANDLE.fetch_add(1, Ordering::Relaxed);

    if let Ok(mut store) = doc_store().lock() {
        store.insert(
            handle,
            ParsedDocument {
                source: html_str.to_string(),
                is_fragment: false,
            },
        );
    }

    handle
}

/// Parse an HTML fragment (partial HTML without doctype/html/body).
///
/// # Arguments
/// * `html` - Pointer to UTF-8 HTML fragment string
/// * `html_len` - Length of HTML string in bytes
///
/// # Returns
/// * Document handle on success
/// * -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_parse_fragment(html: *const u8, html_len: usize) -> i64 {
    if html.is_null() {
        return -1;
    }

    let html_str = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(html, html_len)) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    let handle = NEXT_DOC_HANDLE.fetch_add(1, Ordering::Relaxed);

    if let Ok(mut store) = doc_store().lock() {
        store.insert(
            handle,
            ParsedDocument {
                source: html_str.to_string(),
                is_fragment: true,
            },
        );
    }

    handle
}

/// Free a document handle and all associated resources.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_free(handle: i64) {
    // Check if it's a document handle
    if (1_000_000..2_000_000).contains(&handle) {
        if let Ok(mut store) = doc_store().lock() {
            store.remove(&handle);
        }
    }
    // Check if it's an element handle
    else if handle >= 2_000_000 {
        if let Ok(mut store) = elem_store().lock() {
            store.remove(&handle);
        }
    }
}

// -----------------------------------------------------------------------------
// Serialization
// -----------------------------------------------------------------------------

/// Get the length of the serialized HTML for a document or element.
///
/// # Returns
/// * Length in bytes on success
/// * 0 if handle not found
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_stringify_len(handle: i64) -> usize {
    // Document handle
    if (1_000_000..2_000_000).contains(&handle) {
        if let Ok(store) = doc_store().lock() {
            if let Some(doc) = store.get(&handle) {
                return doc.source.len();
            }
        }
    }
    // Element handle
    else if handle >= 2_000_000 {
        if let Ok(store) = elem_store().lock() {
            if let Some(elem) = store.get(&handle) {
                return elem.outer_html.len();
            }
        }
    }
    0
}

/// Serialize a document or element to HTML string.
///
/// # Arguments
/// * `handle` - Document or element handle
/// * `out` - Output buffer
/// * `out_len` - Size of output buffer
///
/// # Returns
/// * Number of bytes written on success
/// * -1 if buffer too small
/// * -2 if handle not found
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_stringify(handle: i64, out: *mut u8, out_len: usize) -> i64 {
    if out.is_null() {
        return -1;
    }

    let html_str: Option<String>;

    // Document handle
    if (1_000_000..2_000_000).contains(&handle) {
        if let Ok(store) = doc_store().lock() {
            if let Some(doc) = store.get(&handle) {
                html_str = Some(doc.source.clone());
            } else {
                return -2;
            }
        } else {
            return -2;
        }
    }
    // Element handle
    else if handle >= 2_000_000 {
        if let Ok(store) = elem_store().lock() {
            if let Some(elem) = store.get(&handle) {
                html_str = Some(elem.outer_html.clone());
            } else {
                return -2;
            }
        } else {
            return -2;
        }
    } else {
        return -2;
    }

    if let Some(s) = html_str {
        if s.len() > out_len {
            return -1;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), out, s.len());
        }
        return s.len() as i64;
    }

    -2
}

// -----------------------------------------------------------------------------
// Query Selectors
// -----------------------------------------------------------------------------

/// Query for the first matching element using a CSS selector.
///
/// # Arguments
/// * `doc_handle` - Document handle
/// * `selector` - CSS selector string
/// * `selector_len` - Length of selector string
///
/// # Returns
/// * Element handle (>= 2_000_000) on success
/// * 0 if no match found
/// * -1 on error (invalid selector, null pointer, etc.)
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_query(
    doc_handle: i64,
    selector: *const u8,
    selector_len: usize,
) -> i64 {
    if selector.is_null() {
        return -1;
    }

    let selector_str = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(selector, selector_len)) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    let sel = match Selector::parse(selector_str) {
        Ok(s) => s,
        Err(_) => return -1,
    };

    // Get the document source
    let (source, is_fragment) = {
        let store = match doc_store().lock() {
            Ok(s) => s,
            Err(_) => return -1,
        };
        match store.get(&doc_handle) {
            Some(d) => (d.source.clone(), d.is_fragment),
            None => return -1,
        }
    };

    // Parse and find first matching element
    let html = parse_document(&source, is_fragment);
    if let Some(element) = html.select(&sel).next() {
        return store_element(&element);
    }

    0 // No match
}

/// Count the number of elements matching a CSS selector.
///
/// # Returns
/// * Count of matching elements
/// * -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_query_count(
    doc_handle: i64,
    selector: *const u8,
    selector_len: usize,
) -> i64 {
    if selector.is_null() {
        return -1;
    }

    let selector_str = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(selector, selector_len)) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    let sel = match Selector::parse(selector_str) {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let (source, is_fragment) = {
        let store = match doc_store().lock() {
            Ok(s) => s,
            Err(_) => return -1,
        };
        match store.get(&doc_handle) {
            Some(d) => (d.source.clone(), d.is_fragment),
            None => return -1,
        }
    };

    let html = parse_document(&source, is_fragment);
    html.select(&sel).count() as i64
}

/// Query for the Nth matching element (0-indexed).
///
/// # Returns
/// * Element handle on success
/// * 0 if index out of bounds
/// * -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_query_nth(
    doc_handle: i64,
    selector: *const u8,
    selector_len: usize,
    index: usize,
) -> i64 {
    if selector.is_null() {
        return -1;
    }

    let selector_str = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(selector, selector_len)) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    let sel = match Selector::parse(selector_str) {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let (source, is_fragment) = {
        let store = match doc_store().lock() {
            Ok(s) => s,
            Err(_) => return -1,
        };
        match store.get(&doc_handle) {
            Some(d) => (d.source.clone(), d.is_fragment),
            None => return -1,
        }
    };

    let html = parse_document(&source, is_fragment);
    if let Some(element) = html.select(&sel).nth(index) {
        return store_element(&element);
    }

    0 // Index out of bounds
}

/// Query all matching elements and store handles in output array.
///
/// # Arguments
/// * `doc_handle` - Document handle
/// * `selector` - CSS selector string
/// * `selector_len` - Length of selector string
/// * `out_handles` - Output array for element handles
/// * `max_count` - Maximum number of handles to store
///
/// # Returns
/// * Number of handles stored on success
/// * -1 on error
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_query_all(
    doc_handle: i64,
    selector: *const u8,
    selector_len: usize,
    out_handles: *mut i64,
    max_count: usize,
) -> i64 {
    if selector.is_null() || out_handles.is_null() {
        return -1;
    }

    let selector_str = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(selector, selector_len)) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    let sel = match Selector::parse(selector_str) {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let (source, is_fragment) = {
        let store = match doc_store().lock() {
            Ok(s) => s,
            Err(_) => return -1,
        };
        match store.get(&doc_handle) {
            Some(d) => (d.source.clone(), d.is_fragment),
            None => return -1,
        }
    };

    let html = parse_document(&source, is_fragment);
    let mut count = 0usize;
    for element in html.select(&sel) {
        if count >= max_count {
            break;
        }
        let elem_handle = store_element(&element);
        unsafe {
            *out_handles.add(count) = elem_handle;
        }
        count += 1;
    }

    count as i64
}

/// Helper function to store an element and return its handle
fn store_element(element: &ElementRef) -> i64 {
    let handle = NEXT_ELEM_HANDLE.fetch_add(1, Ordering::Relaxed);

    let parsed = ParsedElement {
        tag: element.value().name().to_string(),
        attrs: element
            .value()
            .attrs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        text_content: element.text().collect::<Vec<_>>().join(""),
        inner_html: element.inner_html(),
        outer_html: element.html(),
    };

    if let Ok(mut store) = elem_store().lock() {
        store.insert(handle, parsed);
    }

    handle
}

// -----------------------------------------------------------------------------
// Element Data Accessors
// -----------------------------------------------------------------------------

/// Get the length of an element's text content.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_text_len(elem_handle: i64) -> usize {
    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            return elem.text_content.len();
        }
    }
    0
}

/// Get an element's text content.
///
/// # Returns
/// * Number of bytes written on success
/// * -1 if buffer too small
/// * -2 if handle not found
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_text(elem_handle: i64, out: *mut u8, out_len: usize) -> i64 {
    if out.is_null() {
        return -1;
    }

    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            if elem.text_content.len() > out_len {
                return -1;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(
                    elem.text_content.as_ptr(),
                    out,
                    elem.text_content.len(),
                );
            }
            return elem.text_content.len() as i64;
        }
    }
    -2
}

/// Get the length of an element's tag name.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_tag_len(elem_handle: i64) -> usize {
    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            return elem.tag.len();
        }
    }
    0
}

/// Get an element's tag name.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_tag(elem_handle: i64, out: *mut u8, out_len: usize) -> i64 {
    if out.is_null() {
        return -1;
    }

    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            if elem.tag.len() > out_len {
                return -1;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(elem.tag.as_ptr(), out, elem.tag.len());
            }
            return elem.tag.len() as i64;
        }
    }
    -2
}

/// Get the length of an element's inner HTML.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_inner_len(elem_handle: i64) -> usize {
    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            return elem.inner_html.len();
        }
    }
    0
}

/// Get an element's inner HTML.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_inner(elem_handle: i64, out: *mut u8, out_len: usize) -> i64 {
    if out.is_null() {
        return -1;
    }

    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            if elem.inner_html.len() > out_len {
                return -1;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(elem.inner_html.as_ptr(), out, elem.inner_html.len());
            }
            return elem.inner_html.len() as i64;
        }
    }
    -2
}

/// Get the length of an element's outer HTML.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_outer_len(elem_handle: i64) -> usize {
    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            return elem.outer_html.len();
        }
    }
    0
}

/// Get an element's outer HTML.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_outer(elem_handle: i64, out: *mut u8, out_len: usize) -> i64 {
    if out.is_null() {
        return -1;
    }

    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            if elem.outer_html.len() > out_len {
                return -1;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(elem.outer_html.as_ptr(), out, elem.outer_html.len());
            }
            return elem.outer_html.len() as i64;
        }
    }
    -2
}

// -----------------------------------------------------------------------------
// Attribute Access
// -----------------------------------------------------------------------------

/// Get the length of an attribute value.
///
/// # Returns
/// * Length in bytes if attribute exists
/// * 0 if attribute not found or handle invalid
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_attr_len(
    elem_handle: i64,
    name: *const u8,
    name_len: usize,
) -> usize {
    if name.is_null() {
        return 0;
    }

    let attr_name = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(name, name_len)) {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            for (k, v) in &elem.attrs {
                if k.eq_ignore_ascii_case(attr_name) {
                    return v.len();
                }
            }
        }
    }
    0
}

/// Get an attribute value.
///
/// # Returns
/// * Number of bytes written on success
/// * 0 if attribute not found
/// * -1 if buffer too small
/// * -2 if handle not found
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_attr(
    elem_handle: i64,
    name: *const u8,
    name_len: usize,
    out: *mut u8,
    out_len: usize,
) -> i64 {
    if name.is_null() || out.is_null() {
        return -1;
    }

    let attr_name = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(name, name_len)) {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };

    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            for (k, v) in &elem.attrs {
                if k.eq_ignore_ascii_case(attr_name) {
                    if v.len() > out_len {
                        return -1;
                    }
                    unsafe {
                        std::ptr::copy_nonoverlapping(v.as_ptr(), out, v.len());
                    }
                    return v.len() as i64;
                }
            }
            return 0; // Attribute not found
        }
    }
    -2
}

/// Check if an element has a specific attribute.
///
/// # Returns
/// * 1 if attribute exists
/// * 0 if attribute does not exist or handle invalid
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_has_attr(elem_handle: i64, name: *const u8, name_len: usize) -> i32 {
    if name.is_null() {
        return 0;
    }

    let attr_name = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(name, name_len)) {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            for (k, _) in &elem.attrs {
                if k.eq_ignore_ascii_case(attr_name) {
                    return 1;
                }
            }
        }
    }
    0
}

/// Get the number of attributes on an element.
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_attr_count(elem_handle: i64) -> i64 {
    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            return elem.attrs.len() as i64;
        }
    }
    0
}

/// Get an attribute name by index.
///
/// # Returns
/// * Number of bytes written on success
/// * -1 if buffer too small or index out of bounds
/// * -2 if handle not found
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_attr_name_at(
    elem_handle: i64,
    index: usize,
    out: *mut u8,
    out_len: usize,
) -> i64 {
    if out.is_null() {
        return -1;
    }

    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            if index < elem.attrs.len() {
                let name = &elem.attrs[index].0;
                if name.len() > out_len {
                    return -1;
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(name.as_ptr(), out, name.len());
                }
                return name.len() as i64;
            }
            return -1; // Index out of bounds
        }
    }
    -2
}

/// Get an attribute value by index.
///
/// # Returns
/// * Number of bytes written on success
/// * -1 if buffer too small or index out of bounds
/// * -2 if handle not found
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_attr_value_at(
    elem_handle: i64,
    index: usize,
    out: *mut u8,
    out_len: usize,
) -> i64 {
    if out.is_null() {
        return -1;
    }

    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            if index < elem.attrs.len() {
                let value = &elem.attrs[index].1;
                if value.len() > out_len {
                    return -1;
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(value.as_ptr(), out, value.len());
                }
                return value.len() as i64;
            }
            return -1; // Index out of bounds
        }
    }
    -2
}

// -----------------------------------------------------------------------------
// Convenience Functions
// -----------------------------------------------------------------------------

/// Check if an element has a specific CSS class.
///
/// # Returns
/// * 1 if element has the class
/// * 0 if not or handle invalid
#[unsafe(no_mangle)]
pub extern "C" fn arth_rt_html_has_class(
    elem_handle: i64,
    class_name: *const u8,
    class_len: usize,
) -> i32 {
    if class_name.is_null() {
        return 0;
    }

    let class_str = unsafe {
        match std::str::from_utf8(std::slice::from_raw_parts(class_name, class_len)) {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    if let Ok(store) = elem_store().lock() {
        if let Some(elem) = store.get(&elem_handle) {
            for (k, v) in &elem.attrs {
                if k.eq_ignore_ascii_case("class") {
                    return if v.split_whitespace().any(|c| c == class_str) {
                        1
                    } else {
                        0
                    };
                }
            }
        }
    }
    0
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_and_free() {
        let html = b"<html><body><p>Hello</p></body></html>";
        let handle = arth_rt_html_parse(html.as_ptr(), html.len());
        assert!(handle >= 1_000_000);
        arth_rt_html_free(handle);
    }

    #[test]
    fn test_query_selector() {
        let html = b"<html><body><p class=\"test\">Hello</p></body></html>";
        let doc = arth_rt_html_parse(html.as_ptr(), html.len());
        assert!(doc >= 1_000_000);

        let selector = b"p.test";
        let elem = arth_rt_html_query(doc, selector.as_ptr(), selector.len());
        assert!(elem >= 2_000_000);

        // Get text content
        let text_len = arth_rt_html_text_len(elem);
        assert_eq!(text_len, 5); // "Hello"

        let mut buf = vec![0u8; text_len];
        let written = arth_rt_html_text(elem, buf.as_mut_ptr(), buf.len());
        assert_eq!(written, 5);
        assert_eq!(&buf, b"Hello");

        arth_rt_html_free(elem);
        arth_rt_html_free(doc);
    }

    #[test]
    fn test_query_all() {
        let html = b"<html><body><p>One</p><p>Two</p><p>Three</p></body></html>";
        let doc = arth_rt_html_parse(html.as_ptr(), html.len());

        let selector = b"p";
        let count = arth_rt_html_query_count(doc, selector.as_ptr(), selector.len());
        assert_eq!(count, 3);

        let mut handles = vec![0i64; 10];
        let stored = arth_rt_html_query_all(
            doc,
            selector.as_ptr(),
            selector.len(),
            handles.as_mut_ptr(),
            handles.len(),
        );
        assert_eq!(stored, 3);

        // Clean up
        for i in 0..stored as usize {
            arth_rt_html_free(handles[i]);
        }
        arth_rt_html_free(doc);
    }

    #[test]
    fn test_attributes() {
        let html = b"<div id=\"main\" class=\"container active\" data-value=\"42\"></div>";
        let doc = arth_rt_html_parse(html.as_ptr(), html.len());

        let selector = b"div";
        let elem = arth_rt_html_query(doc, selector.as_ptr(), selector.len());
        assert!(elem >= 2_000_000);

        // Check has_attr
        let id_name = b"id";
        assert_eq!(
            arth_rt_html_has_attr(elem, id_name.as_ptr(), id_name.len()),
            1
        );

        let missing = b"missing";
        assert_eq!(
            arth_rt_html_has_attr(elem, missing.as_ptr(), missing.len()),
            0
        );

        // Get attribute value
        let attr_len = arth_rt_html_attr_len(elem, id_name.as_ptr(), id_name.len());
        assert_eq!(attr_len, 4); // "main"

        let mut buf = vec![0u8; attr_len];
        let written = arth_rt_html_attr(
            elem,
            id_name.as_ptr(),
            id_name.len(),
            buf.as_mut_ptr(),
            buf.len(),
        );
        assert_eq!(written, 4);
        assert_eq!(&buf, b"main");

        // Check has_class
        let class = b"active";
        assert_eq!(arth_rt_html_has_class(elem, class.as_ptr(), class.len()), 1);

        let no_class = b"inactive";
        assert_eq!(
            arth_rt_html_has_class(elem, no_class.as_ptr(), no_class.len()),
            0
        );

        arth_rt_html_free(elem);
        arth_rt_html_free(doc);
    }

    #[test]
    fn test_tag_name() {
        let html = b"<article>Content</article>";
        let doc = arth_rt_html_parse(html.as_ptr(), html.len());

        let selector = b"article";
        let elem = arth_rt_html_query(doc, selector.as_ptr(), selector.len());

        let tag_len = arth_rt_html_tag_len(elem);
        assert_eq!(tag_len, 7); // "article"

        let mut buf = vec![0u8; tag_len];
        let written = arth_rt_html_tag(elem, buf.as_mut_ptr(), buf.len());
        assert_eq!(written, 7);
        assert_eq!(&buf, b"article");

        arth_rt_html_free(elem);
        arth_rt_html_free(doc);
    }

    #[test]
    fn test_inner_outer_html() {
        let html = b"<div><span>Inner</span></div>";
        let doc = arth_rt_html_parse(html.as_ptr(), html.len());

        let selector = b"div";
        let elem = arth_rt_html_query(doc, selector.as_ptr(), selector.len());

        // Inner HTML
        let inner_len = arth_rt_html_inner_len(elem);
        let mut inner_buf = vec![0u8; inner_len];
        arth_rt_html_inner(elem, inner_buf.as_mut_ptr(), inner_buf.len());
        assert_eq!(
            std::str::from_utf8(&inner_buf).unwrap(),
            "<span>Inner</span>"
        );

        // Outer HTML
        let outer_len = arth_rt_html_outer_len(elem);
        let mut outer_buf = vec![0u8; outer_len];
        arth_rt_html_outer(elem, outer_buf.as_mut_ptr(), outer_buf.len());
        assert_eq!(
            std::str::from_utf8(&outer_buf).unwrap(),
            "<div><span>Inner</span></div>"
        );

        arth_rt_html_free(elem);
        arth_rt_html_free(doc);
    }
}
