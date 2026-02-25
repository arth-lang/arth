// Templating engine operations using the scraper crate.
// Provides attribute-based HTML templating with data-* directives.
// Templates are compiled once and rendered multiple times with different contexts.

/// Represents a compiled template stored by the runtime.
#[derive(Clone, Debug)]
struct CompiledTemplate {
    /// Original HTML source
    html: String,
}

/// Global store for compiled templates.
fn template_store() -> &'static Mutex<HashMap<i64, CompiledTemplate>> {
    static T: OnceLock<Mutex<HashMap<i64, CompiledTemplate>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Global store for registered partial templates (name -> handle).
fn partial_store() -> &'static Mutex<HashMap<String, i64>> {
    static P: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Handle counter for templates (130_000+ range).
static NEXT_TEMPLATE: AtomicI64 = AtomicI64::new(130_000);

/// Compile a template string and store it, returning a handle.
fn template_compile(html_str: &str) -> i64 {
    let handle = NEXT_TEMPLATE.fetch_add(1, Ordering::Relaxed);

    let template = CompiledTemplate {
        html: html_str.to_string(),
    };

    if let Ok(mut store) = template_store().lock() {
        store.insert(handle, template);
    }

    handle
}

/// Compile a template from a file path.
fn template_compile_file(path: &str) -> Result<i64, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(template_compile(&contents)),
        Err(e) => Err(format!("Failed to read template file '{}': {}", path, e)),
    }
}

/// Free a compiled template handle.
fn template_free(handle: i64) {
    if let Ok(mut store) = template_store().lock() {
        store.remove(&handle);
    }
}

/// Register a partial template by name.
fn template_register_partial(name: &str, handle: i64) {
    if let Ok(mut store) = partial_store().lock() {
        store.insert(name.to_string(), handle);
    }
}

/// Get a registered partial by name.
fn template_get_partial(name: &str) -> i64 {
    if let Ok(store) = partial_store().lock() {
        if let Some(&handle) = store.get(name) {
            return handle;
        }
    }
    0
}

/// Unregister a partial by name.
fn template_unregister_partial(name: &str) {
    if let Ok(mut store) = partial_store().lock() {
        store.remove(name);
    }
}

/// Escape HTML special characters.
fn template_escape_html(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            '\'' => result.push_str("&#x27;"),
            _ => result.push(c),
        }
    }
    result
}

/// Unescape HTML entities.
fn template_unescape_html(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

// ============================================================================
// Simple string-based parsers for data-bind attributes (replaces regex)
// ============================================================================

/// Represents an HTML element with data-bind-* attributes.
struct DataBindElement<'a> {
    full_match: &'a str,
    tag: &'a str,
    attrs_str: &'a str,
}

/// Parse a single data-bind-* attribute. Returns (attr_name, expr).
fn parse_data_bind_attr(s: &str) -> Option<(&str, &str)> {
    const PREFIX: &str = "data-bind-";
    let start = s.find(PREFIX)?;
    let after_prefix = &s[start + PREFIX.len()..];

    // Find the attribute name (word characters until =)
    let eq_pos = after_prefix.find('=')?;
    let attr_name = &after_prefix[..eq_pos];

    // Check attribute name is alphanumeric
    if !attr_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }

    // Find the quoted value
    let after_eq = &after_prefix[eq_pos + 1..];
    if !after_eq.starts_with('"') {
        return None;
    }
    let value_start = 1;
    let value_end = after_eq[value_start..].find('"')?;
    let expr = &after_eq[value_start..value_start + value_end];

    Some((attr_name, expr))
}

/// Find all data-bind-* attributes in an attribute string.
fn find_all_data_bind_attrs(attrs: &str) -> Vec<(&str, &str, &str)> {
    let mut results = Vec::new();
    let mut search_start = 0;

    while let Some(pos) = attrs[search_start..].find("data-bind-") {
        let abs_pos = search_start + pos;
        let remaining = &attrs[abs_pos..];

        // Find the end of this attribute (closing quote)
        if let Some(eq_pos) = remaining.find('=') {
            let attr_name = &remaining[11..eq_pos]; // "data-bind-".len() = 11
            if attr_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                let after_eq = &remaining[eq_pos + 1..];
                if after_eq.starts_with('"') {
                    if let Some(end_quote) = after_eq[1..].find('"') {
                        let expr = &after_eq[1..1 + end_quote];
                        let full_attr = &remaining[..eq_pos + 2 + end_quote + 1];
                        results.push((attr_name, expr, full_attr));
                        search_start = abs_pos + eq_pos + 2 + end_quote + 1;
                        continue;
                    }
                }
            }
        }
        search_start = abs_pos + 1;
    }

    results
}

/// Find HTML elements with data-bind-* attributes (no regex).
fn find_data_bind_elements(html: &str) -> Vec<DataBindElement<'_>> {
    let mut results = Vec::new();
    let mut search_start = 0;

    while search_start < html.len() {
        // Find opening tag
        let Some(tag_start) = html[search_start..].find('<') else {
            break;
        };
        let tag_start = search_start + tag_start;

        // Skip closing tags and comments
        if html[tag_start + 1..].starts_with('/') || html[tag_start + 1..].starts_with('!') {
            search_start = tag_start + 1;
            continue;
        }

        // Find the end of the opening tag
        let Some(tag_end) = html[tag_start..].find('>') else {
            break;
        };
        let tag_end = tag_start + tag_end;

        let element = &html[tag_start..=tag_end];

        // Check if it contains data-bind- but NOT just data-bind=
        if element.contains("data-bind-") {
            // Parse tag name
            let tag_content = &element[1..element.len() - 1]; // Remove < and >
            let tag_name_end = tag_content
                .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
                .unwrap_or(tag_content.len());
            let tag_name = &tag_content[..tag_name_end];

            if !tag_name.is_empty() && tag_name.chars().all(|c| c.is_ascii_alphanumeric()) {
                let attrs_str = if tag_name_end < tag_content.len() {
                    tag_content[tag_name_end..].trim_end_matches('/')
                } else {
                    ""
                };

                results.push(DataBindElement {
                    full_match: element,
                    tag: tag_name,
                    attrs_str,
                });
            }
        }

        search_start = tag_end + 1;
    }

    results
}

/// Remove all data-* attributes from HTML (no regex).
fn remove_data_attrs(html: &str) -> String {
    let mut result = html.to_string();
    let mut changed = true;

    // Keep removing until no more changes (handles multiple attrs)
    while changed {
        changed = false;
        let mut new_result = String::with_capacity(result.len());
        let mut i = 0;
        let bytes = result.as_bytes();

        while i < bytes.len() {
            // Look for whitespace followed by "data-"
            if bytes[i].is_ascii_whitespace() {
                let ws_start = i;
                // Skip whitespace
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }

                // Check for "data-"
                if i + 5 <= bytes.len() && &result[i..i + 5] == "data-" {
                    // Find the = and quoted value
                    let attr_start = i;
                    if let Some(eq_pos) = result[i..].find('=') {
                        let after_eq = i + eq_pos + 1;
                        if after_eq < result.len() && bytes[after_eq] == b'"' {
                            if let Some(end_quote) = result[after_eq + 1..].find('"') {
                                // Skip this entire attribute (including leading whitespace)
                                i = after_eq + 1 + end_quote + 1;
                                changed = true;
                                continue;
                            }
                        }
                    }
                    // If we couldn't parse it properly, keep the whitespace
                    new_result.push_str(&result[ws_start..attr_start]);
                } else {
                    // Not a data- attribute, keep the whitespace
                    new_result.push_str(&result[ws_start..i]);
                }
            } else {
                new_result.push(bytes[i] as char);
                i += 1;
            }
        }

        if changed {
            result = new_result;
        }
    }

    result
}

/// Evaluate a simple expression against a context map.
/// Supports: variable names, dot notation (user.name), negation (!var), array index (items[0])
fn eval_expr(expr: &str, ctx_handle: i64) -> Option<Value> {
    let expr = expr.trim();

    // Handle negation
    if let Some(inner) = expr.strip_prefix('!') {
        let val = eval_expr(inner.trim(), ctx_handle)?;
        return Some(Value::Bool(!value_is_truthy(&val)));
    }

    // Handle dot notation: user.name -> get "user" map, then get "name" from it
    if expr.contains('.') {
        let parts: Vec<&str> = expr.splitn(2, '.').collect();
        if parts.len() == 2 {
            let root = parts[0].trim();
            let rest = parts[1].trim();

            // Get the root value
            let root_val = map_get(ctx_handle, &Value::Str(root.to_string()))?;

            // If root is a map handle, recursively evaluate
            if let Value::I64(nested_handle) = root_val {
                return eval_expr(rest, nested_handle);
            }
        }
        return None;
    }

    // Handle array indexing: items[0]
    if expr.contains('[') && expr.ends_with(']') {
        if let Some(bracket_pos) = expr.find('[') {
            let var_name = &expr[..bracket_pos];
            let idx_str = &expr[bracket_pos + 1..expr.len() - 1];

            if let Ok(idx) = idx_str.parse::<usize>() {
                let list_val = map_get(ctx_handle, &Value::Str(var_name.to_string()))?;
                if let Value::I64(list_handle) = list_val {
                    return list_get(list_handle, idx);
                }
            }
        }
        return None;
    }

    // Simple variable lookup
    map_get(ctx_handle, &Value::Str(expr.to_string()))
}

/// Check if a value is truthy.
fn value_is_truthy(val: &Value) -> bool {
    match val {
        Value::Bool(b) => *b,
        Value::I64(n) => *n != 0,
        Value::F64(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
    }
}

/// Convert a value to a string for rendering.
fn value_to_string(val: &Value) -> String {
    match val {
        Value::Bool(b) => b.to_string(),
        Value::I64(n) => n.to_string(),
        Value::F64(f) => f.to_string(),
        Value::Str(s) => s.clone(),
    }
}

/// Render a compiled template with a context map.
fn template_render(handle: i64, ctx_handle: i64) -> String {
    let html = {
        let store = match template_store().lock() {
            Ok(s) => s,
            Err(_) => return String::new(),
        };
        match store.get(&handle) {
            Some(tpl) => tpl.html.clone(),
            None => return String::new(),
        }
    };

    // Parse the HTML (used for validation/future enhancements)
    let _doc = html_parse(&html);

    // We'll use a simple approach: serialize to string and do text-based replacements
    // This is not ideal for complex templates but works for the MVP
    let mut output = html.clone();

    // Process data-include first (so included content can have other directives)
    output = process_includes(&output, ctx_handle);

    // Process data-for loops (creates multiple copies of elements)
    output = process_for_loops(&output, ctx_handle);

    // Process data-if conditionals (removes elements if false)
    output = process_conditionals(&output, ctx_handle);

    // Process data-bind and data-bind-* (replaces content/attributes)
    output = process_bindings(&output, ctx_handle);

    // Final cleanup: remove all data-* template attributes
    output = cleanup_data_attrs(&output);

    output
}

/// Process data-include directives.
fn process_includes(html: &str, ctx_handle: i64) -> String {
    let doc_handle = html_parse(html);
    let mut output = html.to_string();

    let elements = html_query_selector_all(doc_handle, "[data-include]");
    let count = list_len(elements);

    for i in 0..count {
        let elem_handle = match list_get(elements, i) {
            Some(Value::I64(h)) => h,
            _ => continue,
        };

        let include_name = html_get_attr(elem_handle, "data-include");
        if include_name.is_empty() {
            html_free(elem_handle);
            continue;
        }

        let partial_handle = template_get_partial(&include_name);
        if partial_handle == 0 {
            html_free(elem_handle);
            continue;
        }

        // Render the partial with the same context
        let partial_content = template_render(partial_handle, ctx_handle);

        // Replace the element's inner HTML with the partial content
        let outer = html_outer_html(elem_handle);
        let tag = html_tag_name(elem_handle);

        // Get all attributes except data-include
        let attrs = get_attrs_except(elem_handle, &["data-include"]);

        let replacement = format!("<{}{}>{}</{}>", tag, attrs, partial_content, tag);
        output = output.replace(&outer, &replacement);

        html_free(elem_handle);
    }

    // Note: element list handles are managed by global list store - no explicit free needed
    html_free(doc_handle);
    output
}

/// Helper to get all attributes as a string, except specified ones
fn get_attrs_except(elem_handle: i64, except: &[&str]) -> String {
    let attr_names_list = html_attr_names(elem_handle);
    let count = list_len(attr_names_list);
    let mut result = String::new();

    for i in 0..count {
        if let Some(Value::Str(name)) = list_get(attr_names_list, i) {
            if !except.iter().any(|e| e.eq_ignore_ascii_case(&name)) && !name.starts_with("data-bind-") {
                let value = html_get_attr(elem_handle, &name);
                result.push_str(&format!(" {}=\"{}\"", name, template_escape_html(&value)));
            }
        }
    }

    result
}

/// Process data-for directives.
fn process_for_loops(html: &str, ctx_handle: i64) -> String {
    let doc_handle = html_parse(html);
    let mut output = html.to_string();

    let elements = html_query_selector_all(doc_handle, "[data-for]");
    let elem_count = list_len(elements);

    for elem_idx in 0..elem_count {
        let elem_handle = match list_get(elements, elem_idx) {
            Some(Value::I64(h)) => h,
            _ => continue,
        };

        let for_expr = html_get_attr(elem_handle, "data-for");
        if for_expr.is_empty() {
            html_free(elem_handle);
            continue;
        }

        // Parse "item in items" syntax
        let parts: Vec<&str> = for_expr.split(" in ").collect();
        if parts.len() != 2 {
            html_free(elem_handle);
            continue;
        }

        let var_name = parts[0].trim();
        let collection_expr = parts[1].trim();

        // Get the collection from context
        let collection_val = match eval_expr(collection_expr, ctx_handle) {
            Some(v) => v,
            None => {
                html_free(elem_handle);
                continue;
            }
        };

        // Collection should be a list handle
        let list_handle = match collection_val {
            Value::I64(h) => h,
            _ => {
                html_free(elem_handle);
                continue;
            }
        };

        let len = list_len(list_handle);
        let outer = html_outer_html(elem_handle);
        let tag = html_tag_name(elem_handle);
        let inner = html_inner_html(elem_handle);
        let attrs = get_attrs_except(elem_handle, &["data-for"]);

        // Build replacement by iterating over the list
        let mut replacement = String::new();

        for i in 0..len {
            if let Some(item_val) = list_get(list_handle, i) {
                // Create a nested context with the loop variable
                let nested_ctx = map_new();

                // Copy all keys from parent context
                if let Ok(map_s) = map_store().lock() {
                    if let Some(_parent_map) = map_s.get(&ctx_handle) {
                        drop(map_s); // Release lock before modifying
                        if let Ok(mut map_s) = map_store().lock() {
                            if let Some(nested_map) = map_s.get_mut(&nested_ctx) {
                                if let Ok(parent_s) = map_store().lock() {
                                    if let Some(pm) = parent_s.get(&ctx_handle) {
                                        for (k, v) in pm.iter() {
                                            nested_map.insert(k.clone(), v.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Add the loop variable
                map_put(nested_ctx, Value::Str(var_name.to_string()), item_val);

                // Process bindings in the inner HTML with nested context
                let processed_inner = process_bindings(&inner, nested_ctx);

                replacement.push_str(&format!("<{}{}>{}</{}>", tag, attrs, processed_inner, tag));

                // Note: nested_ctx stays in map_store - acceptable for MVP
            }
        }

        output = output.replace(&outer, &replacement);
        html_free(elem_handle);
    }

    // Note: element list handles are managed globally - no explicit free needed
    html_free(doc_handle);
    output
}

/// Process data-if conditionals.
fn process_conditionals(html: &str, ctx_handle: i64) -> String {
    let doc_handle = html_parse(html);
    let mut output = html.to_string();

    let elements = html_query_selector_all(doc_handle, "[data-if]");
    let count = list_len(elements);

    for i in 0..count {
        let elem_handle = match list_get(elements, i) {
            Some(Value::I64(h)) => h,
            _ => continue,
        };

        let condition_expr = html_get_attr(elem_handle, "data-if");
        if condition_expr.is_empty() {
            html_free(elem_handle);
            continue;
        }

        let condition_val = eval_expr(&condition_expr, ctx_handle);
        let is_truthy = condition_val.map(|v| value_is_truthy(&v)).unwrap_or(false);

        let outer = html_outer_html(elem_handle);

        if is_truthy {
            // Keep the element but remove the data-if attribute
            let tag = html_tag_name(elem_handle);
            let attrs = get_attrs_except(elem_handle, &["data-if"]);
            let inner = html_inner_html(elem_handle);

            let replacement = format!("<{}{}>{}</{}>", tag, attrs, inner, tag);
            output = output.replace(&outer, &replacement);
        } else {
            // Remove the entire element
            output = output.replace(&outer, "");
        }

        html_free(elem_handle);
    }

    // Note: element list handles are managed globally - no explicit free needed
    html_free(doc_handle);
    output
}

/// Process data-bind and data-bind-* directives.
fn process_bindings(html: &str, ctx_handle: i64) -> String {
    let doc_handle = html_parse(html);
    let mut output = html.to_string();

    // Process data-bind (text content)
    let elements = html_query_selector_all(doc_handle, "[data-bind]");
    let count = list_len(elements);

    for i in 0..count {
        let elem_handle = match list_get(elements, i) {
            Some(Value::I64(h)) => h,
            _ => continue,
        };

        let bind_expr = html_get_attr(elem_handle, "data-bind");
        if bind_expr.is_empty() {
            html_free(elem_handle);
            continue;
        }

        let value = eval_expr(&bind_expr, ctx_handle);
        let text = value.map(|v| value_to_string(&v)).unwrap_or_default();

        let outer = html_outer_html(elem_handle);
        let tag = html_tag_name(elem_handle);

        // Rebuild element with new text content, keeping attributes except data-bind/data-bind-*
        let attrs = get_attrs_except(elem_handle, &["data-bind"]);

        // Process data-bind-* attributes
        let bind_attrs = get_data_bind_attrs(elem_handle, ctx_handle);

        let replacement = format!("<{}{}{}>{}</{}>", tag, attrs, bind_attrs, template_escape_html(&text), tag);
        output = output.replace(&outer, &replacement);

        html_free(elem_handle);
    }

    // Note: element list handles are managed globally - no explicit free needed
    html_free(doc_handle);

    // Process elements with only data-bind-* (no data-bind)
    // Find elements that have data-bind-* but not data-bind
    let html_clone = output.clone();
    for elem in find_data_bind_elements(&html_clone) {
        // Skip if it has data-bind= (already processed above)
        if elem.attrs_str.contains("data-bind=") && !elem.attrs_str.contains("data-bind-") {
            continue;
        }

        // Parse all data-bind-* attributes
        let mut new_attrs = elem.attrs_str.to_string();
        for (target_attr, expr, old_attr) in find_all_data_bind_attrs(elem.attrs_str) {
            if let Some(val) = eval_expr(expr, ctx_handle) {
                let val_str = value_to_string(&val);
                new_attrs = new_attrs.replace(old_attr, &format!("{}=\"{}\"", target_attr, template_escape_html(&val_str)));
            }
        }

        let replacement = format!("<{}{}>", elem.tag, new_attrs);
        output = output.replace(elem.full_match, &replacement);
    }

    output
}

/// Helper to process data-bind-* attributes and return the computed attributes string
fn get_data_bind_attrs(elem_handle: i64, ctx_handle: i64) -> String {
    let attr_names_list = html_attr_names(elem_handle);
    let count = list_len(attr_names_list);
    let mut result = String::new();

    for i in 0..count {
        if let Some(Value::Str(name)) = list_get(attr_names_list, i) {
            if let Some(target_attr) = name.strip_prefix("data-bind-") {
                let expr = html_get_attr(elem_handle, &name);
                if let Some(val) = eval_expr(&expr, ctx_handle) {
                    let val_str = value_to_string(&val);
                    result.push_str(&format!(" {}=\"{}\"", target_attr, template_escape_html(&val_str)));
                }
            }
        }
    }

    result
}

/// Remove all remaining data-* attributes from the output.
fn cleanup_data_attrs(html: &str) -> String {
    remove_data_attrs(html)
}
