use crate::Op;

/// Debug entry for a function in the bytecode.
/// Maps bytecode offsets to function names and source locations.
#[derive(Clone, Debug, PartialEq)]
pub struct DebugEntry {
    /// Bytecode offset where this function starts
    pub offset: u32,
    /// Function name (fully qualified, e.g., "pkg.module.function")
    pub function_name: String,
    /// Source file path (optional)
    pub source_file: Option<String>,
    /// Line number where the function is defined (1-indexed, 0 = unknown)
    pub line: u32,
}

impl DebugEntry {
    /// Create a new debug entry for a function.
    pub fn new(offset: u32, function_name: String) -> Self {
        Self {
            offset,
            function_name,
            source_file: None,
            line: 0,
        }
    }

    /// Create a debug entry with source location.
    pub fn with_location(
        offset: u32,
        function_name: String,
        source_file: String,
        line: u32,
    ) -> Self {
        Self {
            offset,
            function_name,
            source_file: Some(source_file),
            line,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub strings: Vec<String>,
    pub code: Vec<Op>,
    /// Async function dispatch table: maps fn_id (hash) to bytecode offset
    /// Used by TaskAwait to dispatch to the correct body function for deferred execution
    pub async_dispatch: Vec<(i64, u32)>,
    /// Debug info: maps bytecode offsets to function names and source locations.
    /// Sorted by offset for efficient lookup via binary search.
    pub debug_entries: Vec<DebugEntry>,
}

impl Program {
    pub fn new(strings: Vec<String>, code: Vec<Op>) -> Self {
        Self {
            strings,
            code,
            async_dispatch: Vec::new(),
            debug_entries: Vec::new(),
        }
    }

    /// Create a program with an async dispatch table
    pub fn with_async_dispatch(
        strings: Vec<String>,
        code: Vec<Op>,
        async_dispatch: Vec<(i64, u32)>,
    ) -> Self {
        Self {
            strings,
            code,
            async_dispatch,
            debug_entries: Vec::new(),
        }
    }

    /// Create a program with debug info
    pub fn with_debug_info(
        strings: Vec<String>,
        code: Vec<Op>,
        async_dispatch: Vec<(i64, u32)>,
        debug_entries: Vec<DebugEntry>,
    ) -> Self {
        Self {
            strings,
            code,
            async_dispatch,
            debug_entries,
        }
    }

    /// Look up the bytecode offset for an async body function by its fn_id hash
    pub fn get_async_body_offset(&self, fn_id: i64) -> Option<u32> {
        self.async_dispatch
            .iter()
            .find(|(id, _)| *id == fn_id)
            .map(|(_, offset)| *offset)
    }

    /// Look up the function name for a given bytecode offset.
    /// Uses binary search since debug_entries is sorted by offset.
    /// Returns the function containing the offset (the one with the highest offset <= ip).
    pub fn lookup_function(&self, ip: u32) -> Option<&DebugEntry> {
        if self.debug_entries.is_empty() {
            return None;
        }

        // Binary search for the highest offset <= ip
        match self.debug_entries.binary_search_by_key(&ip, |e| e.offset) {
            Ok(idx) => Some(&self.debug_entries[idx]),
            Err(idx) => {
                // idx is where ip would be inserted
                // We want the entry before this position (if any)
                if idx > 0 {
                    Some(&self.debug_entries[idx - 1])
                } else {
                    None
                }
            }
        }
    }
}
