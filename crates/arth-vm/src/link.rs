//! Bytecode Linking
//!
//! Provides functionality to link multiple .abc bytecode files together at load time.
//! Libraries are merged into the main program, with string tables concatenated and
//! code offsets adjusted appropriately.

use std::collections::HashMap;
use std::path::Path;

use crate::bytecode::{
    DecodeError, DecodeErrorKind, DecodePhase, decode_program, decode_program_detailed,
    encode_program,
};
use crate::ops::Op;
use crate::program::Program;

/// Export entry in a library's symbol table
#[derive(Clone, Debug)]
pub struct ExportEntry {
    /// Fully qualified function name (e.g., "net.http.Client.fetch")
    pub name: String,
    /// Bytecode offset within the library
    pub offset: u32,
    /// Number of parameters
    pub arity: u8,
}

/// A library program with export information
#[derive(Clone, Debug)]
pub struct Library {
    /// The underlying bytecode program
    pub program: Program,
    /// Exported symbols (function_name -> offset)
    pub exports: HashMap<String, ExportEntry>,
    /// Library identifier (package name)
    pub id: String,
}

impl Library {
    /// Create a new library from a program with exports
    pub fn new(id: String, program: Program, exports: Vec<ExportEntry>) -> Self {
        let export_map: HashMap<String, ExportEntry> =
            exports.into_iter().map(|e| (e.name.clone(), e)).collect();
        Self {
            program,
            exports: export_map,
            id,
        }
    }

    /// Create a library from bytecode bytes with export table
    pub fn from_bytes(id: String, bytes: &[u8]) -> Result<Self, String> {
        // The .abc format has the program followed by an optional export table
        // For now, we use a simple format: program bytes + export table
        let (program, exports) = decode_library(bytes)?;
        Ok(Self::new(id, program, exports))
    }

    /// Get the offset for an exported symbol
    pub fn get_export(&self, name: &str) -> Option<&ExportEntry> {
        self.exports.get(name)
    }
}

/// Result of linking programs together
#[derive(Clone, Debug)]
pub struct LinkedProgram {
    /// The merged bytecode program
    pub program: Program,
    /// Map from original library function names to final offsets
    pub symbol_table: HashMap<String, u32>,
}

impl LinkedProgram {
    /// Get the bytecode offset for a symbol
    pub fn get_symbol(&self, name: &str) -> Option<u32> {
        self.symbol_table.get(name).copied()
    }
}

/// Link multiple libraries into a main program
///
/// This merges all bytecode together:
/// 1. Concatenates string tables (remaps string indices in ops)
/// 2. Concatenates code sections (adjusts jump/call targets)
/// 3. Builds a symbol table for cross-program references
/// 4. Merges debug entries (adjusts offsets)
pub fn link_programs(main: Program, libraries: Vec<Library>) -> LinkedProgram {
    if libraries.is_empty() {
        return LinkedProgram {
            program: main,
            symbol_table: HashMap::new(),
        };
    }

    let mut merged_strings = main.strings.clone();
    let mut merged_code = main.code.clone();
    let mut merged_async = main.async_dispatch.clone();
    let mut merged_debug = main.debug_entries.clone();
    let mut symbol_table = HashMap::new();

    let main_code_len = main.code.len() as u32;
    let mut current_code_offset = main_code_len;

    for lib in libraries {
        let string_base = merged_strings.len() as u32;
        let code_base = current_code_offset;

        // Add library strings to merged table
        merged_strings.extend(lib.program.strings.clone());

        // Remap and add library code
        for op in &lib.program.code {
            let remapped = remap_op(op, string_base, code_base);
            merged_code.push(remapped);
        }

        // Add library async dispatch entries (with adjusted offsets)
        for (fn_id, offset) in &lib.program.async_dispatch {
            merged_async.push((*fn_id, offset + code_base));
        }

        // Add library debug entries (with adjusted offsets)
        for entry in &lib.program.debug_entries {
            merged_debug.push(crate::program::DebugEntry {
                offset: entry.offset + code_base,
                function_name: entry.function_name.clone(),
                source_file: entry.source_file.clone(),
                line: entry.line,
            });
        }

        // Build symbol table entries for this library's exports
        for (name, entry) in &lib.exports {
            let final_offset = entry.offset + code_base;
            symbol_table.insert(name.clone(), final_offset);
        }

        current_code_offset += lib.program.code.len() as u32;
    }

    // Sort debug entries by offset for efficient lookup
    merged_debug.sort_by_key(|e| e.offset);

    LinkedProgram {
        program: Program {
            strings: merged_strings,
            code: merged_code,
            async_dispatch: merged_async,
            debug_entries: merged_debug,
        },
        symbol_table,
    }
}

/// Remap a single opcode's string indices and jump targets
fn remap_op(op: &Op, string_base: u32, code_base: u32) -> Op {
    match op {
        // String index remapping
        Op::Print(ix) => Op::Print(*ix + string_base),
        Op::PrintStrVal(ix) => Op::PrintStrVal(*ix + string_base),
        Op::PrintRaw(ix) => Op::PrintRaw(*ix + string_base),
        Op::PrintRawStrVal(ix) => Op::PrintRawStrVal(*ix + string_base),
        Op::PushStr(ix) => Op::PushStr(*ix + string_base),
        Op::SharedGetByName(ix) => Op::SharedGetByName(*ix + string_base),
        Op::Panic(ix) => Op::Panic(*ix + string_base),
        Op::CallSymbol(ix) => Op::CallSymbol(*ix + string_base),
        Op::ExternCall {
            sym,
            argc,
            float_mask,
            ret_kind,
        } => Op::ExternCall {
            sym: *sym + string_base,
            argc: *argc,
            float_mask: *float_mask,
            ret_kind: *ret_kind,
        },

        // Jump/call target remapping (within-library jumps)
        Op::Jump(tgt) => Op::Jump(*tgt + code_base),
        Op::JumpIfFalse(tgt) => Op::JumpIfFalse(*tgt + code_base),
        Op::Call(tgt) => Op::Call(*tgt + code_base),
        Op::SetUnwindHandler(tgt) => Op::SetUnwindHandler(*tgt + code_base),
        Op::TaskRunBody(tgt) => Op::TaskRunBody(*tgt + code_base),
        Op::RcDecWithDeinit(tgt) => Op::RcDecWithDeinit(*tgt + code_base),
        Op::ClosureNew(func_id, captures) => Op::ClosureNew(*func_id + code_base, *captures),

        // Opcodes that don't need remapping
        _ => op.clone(),
    }
}

// ============================================================================
// Library Bytecode Version Constants
// ============================================================================

/// Library magic header prefix (7 bytes)
const LIB_MAGIC_PREFIX: &[u8; 7] = b"ARTHLIB";

/// Current library format version
pub const LIBRARY_VERSION: u8 = 1;

/// Minimum supported library version
pub const MIN_SUPPORTED_LIB_VERSION: u8 = 1;

/// Maximum supported library version
pub const MAX_SUPPORTED_LIB_VERSION: u8 = 1;

// Library header format is ARTHLIBN where N is decimal 0..9.
const _: () = {
    assert!(MIN_SUPPORTED_LIB_VERSION <= LIBRARY_VERSION);
    assert!(LIBRARY_VERSION <= MAX_SUPPORTED_LIB_VERSION);
    assert!(MAX_SUPPORTED_LIB_VERSION <= 9);
};

const fn make_library_magic(version: u8) -> [u8; 8] {
    [b'A', b'R', b'T', b'H', b'L', b'I', b'B', b'0' + version]
}

/// Full library magic header including version (8 bytes)
const LIB_MAGIC: [u8; 8] = make_library_magic(LIBRARY_VERSION);

/// Library version information
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibraryVersionInfo {
    /// The version number extracted from library
    pub version: u8,
    /// Whether this version is supported by the current VM
    pub supported: bool,
    /// Minimum version this VM supports
    pub min_supported: u8,
    /// Maximum version this VM supports
    pub max_supported: u8,
}

impl LibraryVersionInfo {
    /// Get version info for the current VM
    pub fn current() -> Self {
        Self {
            version: LIBRARY_VERSION,
            supported: true,
            min_supported: MIN_SUPPORTED_LIB_VERSION,
            max_supported: MAX_SUPPORTED_LIB_VERSION,
        }
    }
}

/// Extract version from library magic header
fn extract_lib_version(header: &[u8]) -> Option<u8> {
    if header.len() < 8 {
        return None;
    }
    if &header[..7] != LIB_MAGIC_PREFIX {
        return None;
    }
    let v = header[7];
    if v.is_ascii_digit() {
        Some(v - b'0')
    } else {
        None
    }
}

/// Check if a library version is supported
fn is_lib_version_supported(version: u8) -> bool {
    version >= MIN_SUPPORTED_LIB_VERSION && version <= MAX_SUPPORTED_LIB_VERSION
}

/// Validate library header and return version
fn validate_library_header(bytes: &[u8]) -> Result<u8, String> {
    if bytes.len() < 8 {
        return Err(format!(
            "library too short: expected at least 8 bytes, got {}",
            bytes.len()
        ));
    }

    let header = &bytes[..8];

    // Check if it looks like an Arth library
    if &header[..7] != LIB_MAGIC_PREFIX {
        // Check for common misidentifications
        if header.starts_with(b"ARTHBC") {
            return Err(
                "invalid library: file appears to be an Arth program (.abc), not a library (.alib). \
                 Use 'arth run' to execute programs directly.".into()
            );
        }
        return Err(format!(
            "invalid library magic: expected 'ARTHLIB', got '{}'",
            String::from_utf8_lossy(&header[..7])
        ));
    }

    // Extract and validate version
    let version = extract_lib_version(header).ok_or_else(|| {
        format!(
            "invalid library version format: expected digit, got '{}'",
            header[7] as char
        )
    })?;

    // Check version compatibility
    if !is_lib_version_supported(version) {
        if version > MAX_SUPPORTED_LIB_VERSION {
            return Err(format!(
                "library version {} is too new: this VM supports versions {}-{}. \
                 Please upgrade your Arth VM to load this library.",
                version, MIN_SUPPORTED_LIB_VERSION, MAX_SUPPORTED_LIB_VERSION
            ));
        } else {
            return Err(format!(
                "library version {} is too old: this VM supports versions {}-{}. \
                 Please recompile the library with a newer Arth compiler.",
                version, MIN_SUPPORTED_LIB_VERSION, MAX_SUPPORTED_LIB_VERSION
            ));
        }
    }

    Ok(version)
}

/// Get information about a library without fully decoding it
pub fn get_library_info(bytes: &[u8]) -> Result<LibraryVersionInfo, String> {
    let version = validate_library_header(bytes)?;
    Ok(LibraryVersionInfo {
        version,
        supported: is_lib_version_supported(version),
        min_supported: MIN_SUPPORTED_LIB_VERSION,
        max_supported: MAX_SUPPORTED_LIB_VERSION,
    })
}

/// Check if bytes represent a valid Arth library
pub fn is_valid_library(bytes: &[u8]) -> bool {
    validate_library_header(bytes).is_ok()
}

/// Encode a library to bytes (program + export table)
pub fn encode_library(lib: &Library) -> Vec<u8> {
    let mut out = Vec::new();

    // Magic header for library format
    out.extend_from_slice(&LIB_MAGIC);

    // Library ID
    let id_bytes = lib.id.as_bytes();
    out.extend_from_slice(&(id_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(id_bytes);

    // Program bytes
    let program_bytes = encode_program(&lib.program);
    out.extend_from_slice(&(program_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&program_bytes);

    // Export table
    out.extend_from_slice(&(lib.exports.len() as u32).to_le_bytes());
    for (name, entry) in &lib.exports {
        // Name
        let name_bytes = name.as_bytes();
        out.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(name_bytes);
        // Offset
        out.extend_from_slice(&entry.offset.to_le_bytes());
        // Arity
        out.push(entry.arity);
    }

    out
}

/// Decode a library from bytes
pub fn decode_library(bytes: &[u8]) -> Result<(Program, Vec<ExportEntry>), String> {
    decode_library_detailed(bytes).map_err(|e| e.to_string())
}

/// Decode a library from bytes with detailed error information.
///
/// Returns a structured `DecodeError` on failure, providing:
/// - Exact byte offset where the error occurred
/// - Hex dump of surrounding bytes for debugging
/// - The phase of decoding (header, library metadata, program, exports)
/// - Helpful suggestions for common errors
pub fn decode_library_detailed(bytes: &[u8]) -> Result<(Program, Vec<ExportEntry>), DecodeError> {
    if bytes.len() < 8 {
        return Err(DecodeError::new(
            DecodeErrorKind::TooShort {
                expected: 8,
                got: bytes.len(),
            },
            DecodePhase::Header,
            0,
            bytes,
        ));
    }

    // Check if this starts with library magic prefix
    if bytes.starts_with(LIB_MAGIC_PREFIX) {
        // Validate library header and version compatibility
        let _version = validate_library_header_detailed(bytes)?;
        decode_library_format_detailed(bytes)
    } else {
        // Fall back to plain program (no exports)
        // This path uses decode_program_detailed() which has its own version validation
        let program = decode_program_detailed(bytes)?;
        Ok((program, Vec::new()))
    }
}

/// Validate library header with detailed error reporting.
fn validate_library_header_detailed(bytes: &[u8]) -> Result<u8, DecodeError> {
    validate_library_header(bytes).map_err(|e| {
        DecodeError::new(
            DecodeErrorKind::Other { message: e },
            DecodePhase::Header,
            0,
            bytes,
        )
    })
}

/// Decode library format with detailed error reporting.
fn decode_library_format_detailed(
    bytes: &[u8],
) -> Result<(Program, Vec<ExportEntry>), DecodeError> {
    let mut off = LIB_MAGIC.len();

    // Helper for EOF errors in library metadata
    let lib_eof = |off: usize, expected: &'static str| -> DecodeError {
        DecodeError::new(
            DecodeErrorKind::UnexpectedEof { expected },
            DecodePhase::Header, // Library metadata is part of header phase
            off,
            bytes,
        )
    };

    // Helper to read u32
    let read_u32 = |off: &mut usize| -> Result<u32, DecodeError> {
        if *off + 4 > bytes.len() {
            return Err(lib_eof(*off, "u32 value"));
        }
        let mut tmp = [0u8; 4];
        tmp.copy_from_slice(&bytes[*off..*off + 4]);
        *off += 4;
        Ok(u32::from_le_bytes(tmp))
    };

    // Read library ID
    let id_len = read_u32(&mut off)? as usize;
    if off + id_len > bytes.len() {
        return Err(lib_eof(off, "library ID"));
    }
    let _id = String::from_utf8(bytes[off..off + id_len].to_vec()).map_err(|e| {
        DecodeError::new(
            DecodeErrorKind::InvalidUtf8 {
                message: format!("invalid library ID: {}", e),
            },
            DecodePhase::Header,
            off,
            bytes,
        )
    })?;
    off += id_len;

    // Read program bytes
    let program_len = read_u32(&mut off)? as usize;
    if off + program_len > bytes.len() {
        return Err(lib_eof(off, "program bytes"));
    }
    // Use decode_program (complete implementation) and wrap errors
    let program_bytes = &bytes[off..off + program_len];
    let program = decode_program(program_bytes).map_err(|e| {
        DecodeError::new(
            DecodeErrorKind::Other { message: e },
            DecodePhase::Instructions,
            off,
            program_bytes,
        )
    })?;
    off += program_len;

    // Read export table
    let export_count = read_u32(&mut off)? as usize;
    let mut exports = Vec::with_capacity(export_count);

    for i in 0..export_count {
        // Name
        let name_len = read_u32(&mut off)? as usize;
        if off + name_len > bytes.len() {
            return Err(DecodeError::new(
                DecodeErrorKind::UnexpectedEof {
                    expected: "export name",
                },
                DecodePhase::Header,
                off,
                bytes,
            ));
        }
        let name = String::from_utf8(bytes[off..off + name_len].to_vec()).map_err(|e| {
            DecodeError::new(
                DecodeErrorKind::InvalidUtf8 {
                    message: format!("invalid export name #{}: {}", i, e),
                },
                DecodePhase::Header,
                off,
                bytes,
            )
        })?;
        off += name_len;

        // Offset
        let offset = read_u32(&mut off)?;

        // Arity
        if off >= bytes.len() {
            return Err(DecodeError::new(
                DecodeErrorKind::UnexpectedEof {
                    expected: "export arity",
                },
                DecodePhase::Header,
                off,
                bytes,
            ));
        }
        let arity = bytes[off];
        off += 1;

        exports.push(ExportEntry {
            name,
            offset,
            arity,
        });
    }

    Ok((program, exports))
}

fn decode_library_format(bytes: &[u8]) -> Result<(Program, Vec<ExportEntry>), String> {
    let mut off = LIB_MAGIC.len();

    // Read library ID
    let id_len = read_u32_le(bytes, &mut off)? as usize;
    if off + id_len > bytes.len() {
        return Err("unexpected EOF reading library ID".into());
    }
    let _id = String::from_utf8(bytes[off..off + id_len].to_vec())
        .map_err(|e| format!("invalid library ID: {}", e))?;
    off += id_len;

    // Read program bytes
    let program_len = read_u32_le(bytes, &mut off)? as usize;
    if off + program_len > bytes.len() {
        return Err("unexpected EOF reading program bytes".into());
    }
    let program = decode_program(&bytes[off..off + program_len])?;
    off += program_len;

    // Read export table
    let export_count = read_u32_le(bytes, &mut off)? as usize;
    let mut exports = Vec::with_capacity(export_count);

    for _ in 0..export_count {
        // Name
        let name_len = read_u32_le(bytes, &mut off)? as usize;
        if off + name_len > bytes.len() {
            return Err("unexpected EOF reading export name".into());
        }
        let name = String::from_utf8(bytes[off..off + name_len].to_vec())
            .map_err(|e| format!("invalid export name: {}", e))?;
        off += name_len;

        // Offset
        let offset = read_u32_le(bytes, &mut off)?;

        // Arity
        if off >= bytes.len() {
            return Err("unexpected EOF reading export arity".into());
        }
        let arity = bytes[off];
        off += 1;

        exports.push(ExportEntry {
            name,
            offset,
            arity,
        });
    }

    Ok((program, exports))
}

fn read_u32_le(bytes: &[u8], off: &mut usize) -> Result<u32, String> {
    if *off + 4 > bytes.len() {
        return Err("unexpected EOF reading u32".into());
    }
    let mut tmp = [0u8; 4];
    tmp.copy_from_slice(&bytes[*off..*off + 4]);
    *off += 4;
    Ok(u32::from_le_bytes(tmp))
}

/// Load a library from a file path
pub fn load_library(path: &Path, id: &str) -> Result<Library, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
    Library::from_bytes(id.to_string(), &bytes)
}

/// Load multiple libraries and link them with a main program
pub fn load_and_link(
    main: Program,
    library_paths: &[(String, &Path)],
) -> Result<LinkedProgram, String> {
    let mut libraries = Vec::new();

    for (id, path) in library_paths {
        let lib = load_library(path, id)?;
        libraries.push(lib);
    }

    Ok(link_programs(main, libraries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_empty_libraries() {
        let main = Program::new(vec!["hello".into()], vec![Op::Print(0), Op::Halt]);
        let linked = link_programs(main.clone(), vec![]);

        assert_eq!(linked.program.strings, main.strings);
        assert_eq!(linked.program.code, main.code);
        assert!(linked.symbol_table.is_empty());
    }

    #[test]
    fn test_link_single_library() {
        let main = Program::new(
            vec!["main".into()],
            vec![Op::Print(0), Op::Call(100), Op::Halt], // Call placeholder
        );

        let lib_program = Program::new(vec!["lib".into()], vec![Op::Print(0), Op::Ret]);

        let lib = Library::new(
            "test.lib".into(),
            lib_program,
            vec![ExportEntry {
                name: "test.lib.hello".into(),
                offset: 0,
                arity: 0,
            }],
        );

        let linked = link_programs(main, vec![lib]);

        // Check merged strings
        assert_eq!(linked.program.strings.len(), 2);
        assert_eq!(linked.program.strings[0], "main");
        assert_eq!(linked.program.strings[1], "lib");

        // Check merged code (main + lib)
        assert_eq!(linked.program.code.len(), 5);

        // Check symbol table
        assert_eq!(linked.get_symbol("test.lib.hello"), Some(3)); // Offset 3 (after main's 3 ops)
    }

    #[test]
    fn test_string_remapping() {
        let main = Program::new(
            vec!["a".into(), "b".into()],
            vec![Op::PushStr(0), Op::PushStr(1)],
        );

        let lib_program = Program::new(
            vec!["x".into(), "y".into()],
            vec![Op::PushStr(0), Op::PushStr(1)],
        );

        let lib = Library::new("lib".into(), lib_program, vec![]);

        let linked = link_programs(main, vec![lib]);

        // Strings: ["a", "b", "x", "y"]
        assert_eq!(linked.program.strings.len(), 4);

        // Library ops should reference remapped indices
        match &linked.program.code[2] {
            Op::PushStr(ix) => assert_eq!(*ix, 2), // "x" at index 2
            _ => panic!("expected PushStr"),
        }
        match &linked.program.code[3] {
            Op::PushStr(ix) => assert_eq!(*ix, 3), // "y" at index 3
            _ => panic!("expected PushStr"),
        }
    }

    #[test]
    fn test_jump_remapping() {
        let main = Program::new(vec![], vec![Op::Jump(1), Op::Halt]);

        let lib_program = Program::new(vec![], vec![Op::Jump(1), Op::Ret]);

        let lib = Library::new("lib".into(), lib_program, vec![]);

        let linked = link_programs(main, vec![lib]);

        // Main jump should stay at 1
        match &linked.program.code[0] {
            Op::Jump(tgt) => assert_eq!(*tgt, 1),
            _ => panic!("expected Jump"),
        }

        // Library jump should be remapped to 3 (original 1 + main code length 2)
        match &linked.program.code[2] {
            Op::Jump(tgt) => assert_eq!(*tgt, 3),
            _ => panic!("expected Jump"),
        }
    }

    #[test]
    fn test_library_encode_decode_roundtrip() {
        let program = Program::new(vec!["test".into()], vec![Op::Print(0), Op::Ret]);
        let exports = vec![ExportEntry {
            name: "hello".into(),
            offset: 0,
            arity: 2,
        }];
        let lib = Library::new("mylib".into(), program, exports);

        let bytes = encode_library(&lib);
        let (decoded_program, decoded_exports) = decode_library(&bytes).unwrap();

        assert_eq!(decoded_program.strings, lib.program.strings);
        assert_eq!(decoded_program.code, lib.program.code);
        assert_eq!(decoded_exports.len(), 1);
        assert_eq!(decoded_exports[0].name, "hello");
        assert_eq!(decoded_exports[0].offset, 0);
        assert_eq!(decoded_exports[0].arity, 2);
    }

    #[test]
    fn test_plain_program_as_library() {
        // A plain .abc file (no export table) should load as a library with no exports
        let program = Program::new(vec!["hello".into()], vec![Op::Print(0), Op::Halt]);
        let bytes = encode_program(&program);

        let (decoded, exports) = decode_library(&bytes).unwrap();
        assert_eq!(decoded.strings, program.strings);
        assert!(exports.is_empty());
    }

    #[test]
    fn test_library_version_validation() {
        // Create a valid library
        let program = Program::new(vec!["test".into()], vec![Op::Ret]);
        let lib = Library::new("test".into(), program, vec![]);
        let bytes = encode_library(&lib);

        // Should decode successfully
        let result = decode_library(&bytes);
        assert!(result.is_ok());
    }

    #[test]
    fn test_library_too_short() {
        let bytes = b"ARTH";
        let result = decode_library(bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too short"));
    }

    #[test]
    fn test_library_program_file_rejected() {
        // A program file should be detected but still decoded (fallback path)
        let program = Program::new(vec!["test".into()], vec![Op::Halt]);
        let bytes = encode_program(&program);

        // Should succeed via the fallback path
        let result = decode_library(&bytes);
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_library_info() {
        let program = Program::new(vec!["test".into()], vec![Op::Ret]);
        let lib = Library::new("test".into(), program, vec![]);
        let bytes = encode_library(&lib);

        let info = get_library_info(&bytes).unwrap();
        assert_eq!(info.version, LIBRARY_VERSION);
        assert!(info.supported);
        assert_eq!(info.min_supported, MIN_SUPPORTED_LIB_VERSION);
        assert_eq!(info.max_supported, MAX_SUPPORTED_LIB_VERSION);
    }

    #[test]
    fn test_is_valid_library() {
        let program = Program::new(vec!["test".into()], vec![Op::Ret]);
        let lib = Library::new("test".into(), program, vec![]);
        let bytes = encode_library(&lib);

        assert!(is_valid_library(&bytes));
        assert!(!is_valid_library(b"INVALID0"));
        assert!(!is_valid_library(b"ARTH")); // too short
    }

    #[test]
    fn test_library_version_info_current() {
        let info = LibraryVersionInfo::current();
        assert_eq!(info.version, LIBRARY_VERSION);
        assert!(info.supported);
    }

    #[test]
    fn test_library_magic_tracks_version() {
        let expected = [
            b'A',
            b'R',
            b'T',
            b'H',
            b'L',
            b'I',
            b'B',
            b'0' + LIBRARY_VERSION,
        ];
        assert_eq!(LIB_MAGIC, expected);
        assert_eq!(extract_lib_version(&LIB_MAGIC), Some(LIBRARY_VERSION));
    }

    #[test]
    fn test_library_version_too_new_rejected() {
        let mut bytes = b"ARTHLIB2".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0]); // minimal trailing bytes
        let result = decode_library(&bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too new"));
    }

    #[test]
    fn test_library_version_too_old_rejected() {
        let mut bytes = b"ARTHLIB0".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0]); // minimal trailing bytes
        let result = decode_library(&bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too old"));
    }

    #[test]
    fn test_validate_library_header_program_rejected() {
        // A program header should be rejected by validate_library_header
        let bytes = b"ARTHBC01\x00\x00\x00\x00";
        let result = validate_library_header(bytes);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("program"));
        assert!(err.contains("not a library"));
    }
}
