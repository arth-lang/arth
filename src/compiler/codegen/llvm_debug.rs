//! DWARF Debug Information for LLVM IR Text Emission
//!
//! This module provides `SourceLineTable` for resolving byte-offset spans to
//! (file, line, col) triples, and `DebugInfoBuilder` for generating LLVM
//! metadata nodes (DICompileUnit, DIFile, DISubprogram, DILocation, etc.)
//! that clang/LLVM encode as DWARF sections in the final binary.

use std::collections::HashMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use crate::compiler::ir::{Span, Ty};

// =============================================================================
// Source Line Table
// =============================================================================

/// Resolved source location from a byte-offset span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLoc {
    pub file: PathBuf,
    pub line: u32,
    pub col: u32,
}

/// Maps source file paths to their line-start byte offsets, enabling
/// conversion from `Span` byte offsets to (line, col) pairs.
pub struct SourceLineTable {
    files: HashMap<PathBuf, Vec<u32>>,
}

impl SourceLineTable {
    /// Build from loaded source files. Call once before codegen.
    pub fn from_sources(sources: &[(PathBuf, &str)]) -> Self {
        let mut files = HashMap::new();
        for (path, text) in sources {
            let line_starts = compute_line_starts(text);
            files.insert(path.clone(), line_starts);
        }
        Self { files }
    }

    /// Convert a `Span` (byte offsets) to a `SourceLoc` (line, col).
    /// Returns `None` if the file is not in the table.
    pub fn resolve(&self, span: &Span) -> Option<SourceLoc> {
        let starts = self.files.get(span.file.as_ref())?;
        let (line, col) = byte_offset_to_line_col(starts, span.start);
        Some(SourceLoc {
            file: span.file.as_ref().clone(),
            line,
            col,
        })
    }
}

/// Compute sorted byte offsets where each line begins.
fn compute_line_starts(text: &str) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (i, ch) in text.char_indices() {
        if ch == '\n' {
            starts.push((i + 1) as u32);
        }
    }
    starts
}

/// Binary search to convert a byte offset to 1-based (line, col).
fn byte_offset_to_line_col(line_starts: &[u32], offset: u32) -> (u32, u32) {
    let line_idx = match line_starts.binary_search(&offset) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    };
    let col = offset.saturating_sub(line_starts[line_idx]) + 1;
    (line_idx as u32 + 1, col)
}

// =============================================================================
// Debug Info Builder
// =============================================================================

/// Builds LLVM debug metadata nodes during IR emission. After all functions
/// are emitted, call `finish()` to get the serialized metadata section.
pub struct DebugInfoBuilder {
    next_id: u32,
    nodes: Vec<String>,
    file_ids: HashMap<PathBuf, u32>,
    type_ids: HashMap<String, u32>,
    cu_id: u32,
    line_table: SourceLineTable,
    /// Metadata IDs that must appear in !llvm.dbg.cu
    subprogram_ids: Vec<u32>,
    /// The source directory used for DIFile directory fields
    source_dir: String,
    /// Track module flag IDs
    dwarf_version_id: u32,
    debug_info_version_id: u32,
}

impl DebugInfoBuilder {
    /// Create a new debug info builder.
    ///
    /// `producer` is the compiler identification string (e.g. "arth 0.1.0").
    /// `source_dir` is the project root directory.
    pub fn new(producer: &str, source_dir: &str, line_table: SourceLineTable) -> Self {
        let mut builder = Self {
            next_id: 0,
            nodes: Vec::new(),
            file_ids: HashMap::new(),
            type_ids: HashMap::new(),
            cu_id: 0,
            line_table,
            subprogram_ids: Vec::new(),
            source_dir: source_dir.to_string(),
            dwarf_version_id: 0,
            debug_info_version_id: 0,
        };

        // Allocate module flag nodes first
        builder.dwarf_version_id = builder.alloc_id();
        builder.nodes.push(format!(
            "!{} = !{{i32 2, !\"Dwarf Version\", i32 4}}",
            builder.dwarf_version_id
        ));

        builder.debug_info_version_id = builder.alloc_id();
        builder.nodes.push(format!(
            "!{} = !{{i32 2, !\"Debug Info Version\", i32 3}}",
            builder.debug_info_version_id
        ));

        // Create an empty file for the compile unit (will reference real files later)
        let cu_file_id = builder.alloc_id();
        builder.nodes.push(format!(
            "!{} = !DIFile(filename: \"<module>\", directory: \"{}\")",
            cu_file_id,
            escape_metadata_string(source_dir)
        ));

        // Create DICompileUnit
        let cu_id = builder.alloc_id();
        builder.nodes.push(format!(
            "!{} = distinct !DICompileUnit(language: DW_LANG_C, file: !{}, \
             producer: \"{}\", isOptimized: false, runtimeVersion: 0, \
             emissionKind: FullDebug)",
            cu_id,
            cu_file_id,
            escape_metadata_string(producer)
        ));
        builder.cu_id = cu_id;

        builder
    }

    /// Get or create a DIFile metadata node for the given path.
    pub fn get_or_create_file(&mut self, path: &Path) -> u32 {
        if let Some(&id) = self.file_ids.get(path) {
            return id;
        }

        let filename = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());

        let directory = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.source_dir.clone());

        let id = self.alloc_id();
        self.nodes.push(format!(
            "!{} = !DIFile(filename: \"{}\", directory: \"{}\")",
            id,
            escape_metadata_string(&filename),
            escape_metadata_string(&directory)
        ));
        self.file_ids.insert(path.to_path_buf(), id);
        id
    }

    /// Get or create a DIBasicType for an IR type.
    pub fn get_or_create_type(&mut self, ty: &Ty) -> u32 {
        let key = type_debug_key(ty);
        if let Some(&id) = self.type_ids.get(&key) {
            return id;
        }

        let id = self.alloc_id();
        let node = match ty {
            Ty::I64 => format!(
                "!{} = !DIBasicType(name: \"Int\", size: 64, encoding: DW_ATE_signed)",
                id
            ),
            Ty::F64 => format!(
                "!{} = !DIBasicType(name: \"Float\", size: 64, encoding: DW_ATE_float)",
                id
            ),
            Ty::I1 => format!(
                "!{} = !DIBasicType(name: \"Bool\", size: 8, encoding: DW_ATE_boolean)",
                id
            ),
            Ty::Ptr | Ty::String => format!(
                "!{} = !DIBasicType(name: \"Ptr\", size: 64, encoding: DW_ATE_address)",
                id
            ),
            Ty::Void => format!("!{} = !{{}}", id),
            // For struct/enum/optional, emit as opaque pointer-sized type
            _ => format!(
                "!{} = !DIBasicType(name: \"{}\", size: 64, encoding: DW_ATE_signed)",
                id,
                escape_metadata_string(&key)
            ),
        };
        self.nodes.push(node);
        self.type_ids.insert(key, id);
        id
    }

    /// Create a DISubprogram for a function. Returns the metadata ID.
    pub fn create_subprogram(
        &mut self,
        func_name: &str,
        linkage_name: &str,
        file_id: u32,
        line: u32,
        ret_ty: &Ty,
        param_tys: &[Ty],
    ) -> u32 {
        // Build the subroutine type
        let subroutine_type_id = self.create_subroutine_type(ret_ty, param_tys);

        let id = self.alloc_id();
        self.nodes.push(format!(
            "!{} = distinct !DISubprogram(name: \"{}\", linkageName: \"{}\", \
             scope: !{}, file: !{}, line: {}, type: !{}, scopeLine: {}, \
             spFlags: DISPFlagDefinition, unit: !{})",
            id,
            escape_metadata_string(func_name),
            escape_metadata_string(linkage_name),
            file_id,
            file_id,
            line,
            subroutine_type_id,
            line,
            self.cu_id
        ));
        self.subprogram_ids.push(id);
        id
    }

    /// Create a DILocation metadata node. Returns the metadata ID.
    pub fn create_location(&mut self, line: u32, col: u32, scope_id: u32) -> u32 {
        let id = self.alloc_id();
        self.nodes.push(format!(
            "!{} = !DILocation(line: {}, column: {}, scope: !{})",
            id, line, col, scope_id
        ));
        id
    }

    /// Create a DILocalVariable. Returns the metadata ID.
    pub fn create_local_variable(
        &mut self,
        name: &str,
        scope_id: u32,
        file_id: u32,
        line: u32,
        type_id: u32,
    ) -> u32 {
        let id = self.alloc_id();
        self.nodes.push(format!(
            "!{} = !DILocalVariable(name: \"{}\", scope: !{}, file: !{}, \
             line: {}, type: !{})",
            id,
            escape_metadata_string(name),
            scope_id,
            file_id,
            line,
            type_id
        ));
        id
    }

    /// Resolve a span to a source location using the line table.
    pub fn resolve_span(&self, span: &Span) -> Option<SourceLoc> {
        self.line_table.resolve(span)
    }

    /// Serialize all metadata nodes into an LLVM IR metadata section.
    /// This should be appended after all function definitions.
    pub fn finish(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out);
        let _ = writeln!(out, "; --- Debug metadata ---");

        // Named metadata
        let _ = writeln!(out, "!llvm.dbg.cu = !{{!{}}}", self.cu_id);
        let _ = writeln!(
            out,
            "!llvm.module.flags = !{{!{}, !{}}}",
            self.dwarf_version_id, self.debug_info_version_id
        );
        let _ = writeln!(out);

        // All numbered nodes
        for node in &self.nodes {
            let _ = writeln!(out, "{}", node);
        }

        out
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn create_subroutine_type(&mut self, ret_ty: &Ty, param_tys: &[Ty]) -> u32 {
        // Types list: first element is return type, rest are params
        let ret_id = self.get_or_create_type(ret_ty);
        let param_ids: Vec<u32> = param_tys
            .iter()
            .map(|t| self.get_or_create_type(t))
            .collect();

        let types_id = self.alloc_id();
        let mut types_list = format!("!{}", ret_id);
        for pid in &param_ids {
            let _ = write!(types_list, ", !{}", pid);
        }
        self.nodes
            .push(format!("!{} = !{{{}}}", types_id, types_list));

        let id = self.alloc_id();
        self.nodes
            .push(format!("!{} = !DISubroutineType(types: !{})", id, types_id));
        id
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Escape a string for LLVM metadata (double backslashes and quotes).
fn escape_metadata_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Produce a deduplication key for a type.
fn type_debug_key(ty: &Ty) -> String {
    match ty {
        Ty::I64 => "Int".to_string(),
        Ty::F64 => "Float".to_string(),
        Ty::I1 => "Bool".to_string(),
        Ty::Ptr => "Ptr".to_string(),
        Ty::String => "String".to_string(),
        Ty::Void => "Void".to_string(),
        Ty::Struct(name) => format!("Struct:{}", name),
        Ty::Enum(name) => format!("Enum:{}", name),
        Ty::Optional(inner) => format!("Optional:{}", type_debug_key(inner)),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn make_span(file: &str, start: u32, end: u32) -> Span {
        Span {
            file: Arc::new(PathBuf::from(file)),
            start,
            end,
        }
    }

    // ── SourceLineTable tests ───────────────────────────────────────

    #[test]
    fn source_line_table_resolves_byte_offsets_to_line_col() {
        // "hello\nworld\n" → line 1 starts at 0, line 2 starts at 6
        let src = "hello\nworld\n";
        let table = SourceLineTable::from_sources(&[(PathBuf::from("test.arth"), src)]);

        let span = make_span("test.arth", 0, 5);
        let loc = table.resolve(&span).unwrap();
        assert_eq!(loc.line, 1);
        assert_eq!(loc.col, 1);

        // 'w' in "world" is at byte offset 6
        let span = make_span("test.arth", 6, 11);
        let loc = table.resolve(&span).unwrap();
        assert_eq!(loc.line, 2);
        assert_eq!(loc.col, 1);

        // 'r' in "world" is at byte offset 8
        let span = make_span("test.arth", 8, 9);
        let loc = table.resolve(&span).unwrap();
        assert_eq!(loc.line, 2);
        assert_eq!(loc.col, 3);
    }

    #[test]
    fn source_line_table_handles_multi_file() {
        let table = SourceLineTable::from_sources(&[
            (PathBuf::from("a.arth"), "line1\nline2\n"),
            (PathBuf::from("b.arth"), "single_line"),
        ]);

        let loc_a = table.resolve(&make_span("a.arth", 6, 11)).unwrap();
        assert_eq!(loc_a.line, 2);
        assert_eq!(loc_a.col, 1);

        let loc_b = table.resolve(&make_span("b.arth", 7, 11)).unwrap();
        assert_eq!(loc_b.line, 1);
        assert_eq!(loc_b.col, 8);
    }

    #[test]
    fn source_line_table_returns_none_for_unknown_file() {
        let table = SourceLineTable::from_sources(&[(PathBuf::from("a.arth"), "hello")]);
        assert!(table.resolve(&make_span("unknown.arth", 0, 1)).is_none());
    }

    // ── DebugInfoBuilder tests ──────────────────────────────────────

    #[test]
    fn debug_builder_emits_compile_unit_and_file() {
        let table = SourceLineTable::from_sources(&[]);
        let builder = DebugInfoBuilder::new("arth 0.1.0", "/project", table);
        let output = builder.finish();

        assert!(output.contains("!llvm.dbg.cu"));
        assert!(output.contains("DICompileUnit"));
        assert!(output.contains("arth 0.1.0"));
        assert!(output.contains("DW_LANG_C"));
    }

    #[test]
    fn debug_builder_emits_subprogram_for_function() {
        let table = SourceLineTable::from_sources(&[]);
        let mut builder = DebugInfoBuilder::new("arth", "/project", table);

        let file_id = builder.get_or_create_file(Path::new("/project/main.arth"));
        let sp_id = builder.create_subprogram("main", "main", file_id, 1, &Ty::Void, &[]);
        let output = builder.finish();

        assert!(output.contains("DISubprogram"));
        assert!(output.contains("name: \"main\""));
        assert!(output.contains("DISPFlagDefinition"));
        assert!(sp_id > 0);
    }

    #[test]
    fn debug_builder_emits_location_metadata() {
        let table = SourceLineTable::from_sources(&[]);
        let mut builder = DebugInfoBuilder::new("arth", "/project", table);

        let file_id = builder.get_or_create_file(Path::new("/project/main.arth"));
        let sp_id = builder.create_subprogram("main", "main", file_id, 1, &Ty::Void, &[]);
        let loc_id = builder.create_location(10, 5, sp_id);
        let output = builder.finish();

        assert!(output.contains(&format!(
            "!{} = !DILocation(line: 10, column: 5, scope: !{})",
            loc_id, sp_id
        )));
    }

    #[test]
    fn debug_builder_emits_basic_types() {
        let table = SourceLineTable::from_sources(&[]);
        let mut builder = DebugInfoBuilder::new("arth", "/project", table);

        let int_id = builder.get_or_create_type(&Ty::I64);
        let float_id = builder.get_or_create_type(&Ty::F64);
        let bool_id = builder.get_or_create_type(&Ty::I1);
        let output = builder.finish();

        assert!(output.contains("DIBasicType(name: \"Int\", size: 64, encoding: DW_ATE_signed)"));
        assert!(output.contains("DIBasicType(name: \"Float\", size: 64, encoding: DW_ATE_float)"));
        assert!(output.contains("DIBasicType(name: \"Bool\", size: 8, encoding: DW_ATE_boolean)"));
        assert_ne!(int_id, float_id);
        assert_ne!(float_id, bool_id);
    }

    #[test]
    fn debug_builder_emits_local_variable() {
        let table = SourceLineTable::from_sources(&[]);
        let mut builder = DebugInfoBuilder::new("arth", "/project", table);

        let file_id = builder.get_or_create_file(Path::new("/project/main.arth"));
        let sp_id = builder.create_subprogram("main", "main", file_id, 1, &Ty::Void, &[]);
        let type_id = builder.get_or_create_type(&Ty::I64);
        let var_id = builder.create_local_variable("x", sp_id, file_id, 5, type_id);
        let output = builder.finish();

        assert!(output.contains("DILocalVariable"));
        assert!(output.contains("name: \"x\""));
        assert!(var_id > 0);
    }

    #[test]
    fn debug_builder_deduplicates_files_and_types() {
        let table = SourceLineTable::from_sources(&[]);
        let mut builder = DebugInfoBuilder::new("arth", "/project", table);

        let id1 = builder.get_or_create_file(Path::new("/project/a.arth"));
        let id2 = builder.get_or_create_file(Path::new("/project/a.arth"));
        assert_eq!(id1, id2);

        let t1 = builder.get_or_create_type(&Ty::I64);
        let t2 = builder.get_or_create_type(&Ty::I64);
        assert_eq!(t1, t2);
    }

    #[test]
    fn debug_builder_finish_includes_module_flags() {
        let table = SourceLineTable::from_sources(&[]);
        let builder = DebugInfoBuilder::new("arth", "/project", table);
        let output = builder.finish();

        assert!(output.contains("!llvm.module.flags"));
        assert!(output.contains("Dwarf Version"));
        assert!(output.contains("Debug Info Version"));
    }
}
