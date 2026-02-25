use crate::ops::{HostCryptoOp, HostDbOp, HostIoOp, HostMailOp, HostNetOp, HostTimeOp};
use crate::program::DebugEntry;
use crate::{Op, Program};
use std::fmt;

// Host-call extension tags encoded after opcode 0xA1.
// 0xA1 <io-op> remains the legacy/direct HostIo encoding.
const HOST_CALL_EXT_DB: u8 = 0xFD;
const HOST_CALL_EXT_MAIL: u8 = 0xFE;
const HOST_CALL_EXT_CRYPTO: u8 = 0xFF;

// ============================================================================
// Bytecode Decode Error Types
// ============================================================================

/// Phase of bytecode decoding where an error occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodePhase {
    /// Validating the magic header bytes
    Header,
    /// Reading the string table
    StringTable,
    /// Decoding opcodes/instructions
    Instructions,
    /// Reading the async dispatch table
    AsyncDispatch,
    /// Reading debug entries
    DebugInfo,
}

impl fmt::Display for DecodePhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodePhase::Header => write!(f, "header validation"),
            DecodePhase::StringTable => write!(f, "string table"),
            DecodePhase::Instructions => write!(f, "instruction decoding"),
            DecodePhase::AsyncDispatch => write!(f, "async dispatch table"),
            DecodePhase::DebugInfo => write!(f, "debug info"),
        }
    }
}

/// Detailed error information for bytecode decoding failures.
#[derive(Debug, Clone)]
pub struct DecodeError {
    /// What kind of error occurred
    pub kind: DecodeErrorKind,
    /// Phase of decoding where the error occurred
    pub phase: DecodePhase,
    /// Byte offset in the bytecode where the error was detected
    pub offset: usize,
    /// Total size of the bytecode being decoded
    pub total_size: usize,
    /// Surrounding bytes for context (up to 16 bytes before and after)
    pub context_bytes: Vec<u8>,
    /// Offset of the first byte in context_bytes relative to bytecode start
    pub context_start: usize,
}

/// The specific kind of decode error.
#[derive(Debug, Clone)]
pub enum DecodeErrorKind {
    /// Bytecode is too short to contain required data
    TooShort { expected: usize, got: usize },
    /// Invalid magic header - not Arth bytecode
    InvalidMagic { found: [u8; 8] },
    /// File appears to be a different format
    WrongFileType { detected: &'static str },
    /// Bytecode version is not supported
    UnsupportedVersion {
        version: u8,
        min: u8,
        max: u8,
        too_new: bool,
    },
    /// Unknown opcode encountered
    UnknownOpcode { opcode: u8 },
    /// Unknown sub-opcode for a category
    UnknownSubOpcode {
        category: &'static str,
        opcode: u8,
        sub_opcode: u8,
    },
    /// Unexpected end of bytecode
    UnexpectedEof { expected: &'static str },
    /// Invalid UTF-8 in string data
    InvalidUtf8 { message: String },
    /// Generic decode error with message
    Other { message: String },
}

impl DecodeError {
    /// Create a new decode error with context.
    ///
    /// # Arguments
    /// * `kind` - The specific type of decode error
    /// * `phase` - Which phase of decoding the error occurred in
    /// * `offset` - Byte offset where the error was detected
    /// * `bytes` - The full bytecode being decoded (for context extraction)
    pub fn new(kind: DecodeErrorKind, phase: DecodePhase, offset: usize, bytes: &[u8]) -> Self {
        let total_size = bytes.len();

        // Extract context bytes (up to 16 before and 16 after the error point)
        // Handle edge case where offset is beyond bytes length
        let safe_offset = offset.min(bytes.len());
        let context_start = safe_offset.saturating_sub(16);
        let context_end = (safe_offset + 16).min(bytes.len());
        let context_bytes = if context_start < context_end {
            bytes[context_start..context_end].to_vec()
        } else {
            Vec::new()
        };

        Self {
            kind,
            phase,
            offset,
            total_size,
            context_bytes,
            context_start,
        }
    }

    /// Format bytes as a hex dump with ASCII representation.
    fn format_hex_dump(&self) -> String {
        if self.context_bytes.is_empty() {
            return String::from("  (no context bytes available)");
        }

        let mut result = String::new();
        let error_pos_in_context = self.offset - self.context_start;

        // Format bytes in rows of 16
        for (i, chunk) in self.context_bytes.chunks(16).enumerate() {
            let row_offset = self.context_start + i * 16;
            result.push_str(&format!("  {:08x}: ", row_offset));

            // Hex bytes
            for (j, byte) in chunk.iter().enumerate() {
                let abs_pos = i * 16 + j;
                if abs_pos == error_pos_in_context {
                    result.push_str(&format!("[{:02x}]", byte));
                } else {
                    result.push_str(&format!(" {:02x} ", byte));
                }
            }

            // Padding if row is short
            for _ in chunk.len()..16 {
                result.push_str("    ");
            }

            result.push_str("  |");

            // ASCII representation
            for (j, byte) in chunk.iter().enumerate() {
                let abs_pos = i * 16 + j;
                let ch = if byte.is_ascii_graphic() || *byte == b' ' {
                    *byte as char
                } else {
                    '.'
                };
                if abs_pos == error_pos_in_context {
                    result.push('[');
                    result.push(ch);
                    result.push(']');
                } else {
                    result.push(ch);
                }
            }
            result.push_str("|\n");
        }

        result
    }

    /// Get a suggestion based on the error.
    fn suggestion(&self) -> Option<&'static str> {
        match &self.kind {
            DecodeErrorKind::InvalidMagic { found } => {
                // Check if it looks like text
                if found.iter().all(|b| b.is_ascii()) {
                    Some(
                        "The data appears to be ASCII text, not compiled bytecode. \
                          Did you pass a source file (.arth) instead of compiled bytecode (.abc)?",
                    )
                } else {
                    Some(
                        "The file does not appear to be Arth bytecode. \
                          Ensure you're loading a compiled .abc file.",
                    )
                }
            }
            DecodeErrorKind::WrongFileType { .. } => Some(
                "You appear to be loading a different file type. \
                      Use 'arth build' to compile source files to bytecode first.",
            ),
            DecodeErrorKind::UnsupportedVersion { too_new: true, .. } => Some(
                "The bytecode was compiled with a newer Arth compiler. \
                      Please upgrade your Arth VM.",
            ),
            DecodeErrorKind::UnsupportedVersion { too_new: false, .. } => Some(
                "The bytecode was compiled with an older Arth compiler. \
                      Please recompile your source files.",
            ),
            DecodeErrorKind::UnknownOpcode { opcode } => {
                // Check if the opcode looks like ASCII text
                if opcode.is_ascii_alphabetic() {
                    Some(
                        "The unknown opcode appears to be an ASCII letter, which suggests \
                          the bytecode may be corrupted or truncated, or source code was \
                          passed instead of compiled bytecode.",
                    )
                } else {
                    Some(
                        "This opcode is not recognized. The bytecode may have been \
                          generated by an incompatible compiler version or corrupted.",
                    )
                }
            }
            DecodeErrorKind::UnexpectedEof { .. } => Some(
                "The bytecode appears to be truncated. \
                      The file may have been incompletely written or corrupted during transfer.",
            ),
            _ => None,
        }
    }

    /// Get known valid opcodes near the unknown opcode (for suggestions).
    fn nearby_opcodes(opcode: u8) -> Vec<(u8, &'static str)> {
        // Return opcodes within ±5 of the unknown opcode
        let nearby: Vec<(u8, &'static str)> = KNOWN_OPCODES
            .iter()
            .filter(|(op, _)| {
                let diff = (*op as i16 - opcode as i16).abs();
                diff <= 5 && *op != opcode
            })
            .copied()
            .collect();
        nearby
    }
}

/// Known opcodes with their names (for error messages).
const KNOWN_OPCODES: &[(u8, &str)] = &[
    (0x00, "CallSymbol"),
    (0x01, "Print"),
    (0x02, "PrintStrVal"),
    (0x03, "PrintRaw"),
    (0x04, "PrintRawStrVal"),
    (0x05, "PrintLn"),
    (0x10, "PushI64"),
    (0x11, "PushBool"),
    (0x12, "PushF64"),
    (0x13, "PushStr"),
    (0x20, "AddI64"),
    (0x21, "LtI64"),
    (0x22, "SubI64"),
    (0x23, "MulI64"),
    (0x24, "DivI64"),
    (0x25, "EqI64"),
    (0x26, "ModI64"),
    (0x27, "ShlI64"),
    (0x28, "ShrI64"),
    (0x29, "AndI64"),
    (0x2A, "OrI64"),
    (0x2B, "XorI64"),
    (0x2C, "EqStr"),
    (0x2D, "ConcatStr"),
    (0x30, "Jump"),
    (0x31, "JumpIfFalse"),
    (0x40, "Pop"),
    (0x50, "PrintTop"),
    (0x60, "LocalGet"),
    (0x61, "LocalSet"),
    (0x62, "ToF64"),
    (0x63, "ToI64"),
    (0x64, "ToBool"),
    (0x65, "ToChar"),
    (0x66, "ToI64OrEnumTag"),
    (0x67, "ToI8"),
    (0x68, "ToI16"),
    (0x69, "ToI32"),
    (0x6A, "ToU8"),
    (0x6B, "ToU16"),
    (0x6C, "ToU32"),
    (0x6D, "ToU64"),
    (0x6E, "ToF32"),
    (0x70, "Call"),
    (0x71, "Ret"),
    (0x72, "StructNew"),
    (0x73, "StructSet"),
    (0x74, "StructGet"),
    (0x75, "StructGetNamed"),
    (0x76, "StructCopy"),
    (0x77, "StructTypeName"),
    (0x78, "StructFieldCount"),
    (0x79, "EnumNew"),
    (0x7A, "EnumSetPayload"),
    (0x7B, "EnumGetPayload"),
    (0x7C, "EnumGetTag"),
    (0x7D, "EnumGetVariant"),
    (0x7E, "EnumTypeName"),
    (0x7F, "HTTP sub-operations"),
    (0x80, "TemplateCompile"),
    (0x81, "TemplateCompileFile"),
    (0x82, "TemplateRender"),
    (0x83, "TemplateRegisterPartial"),
    (0x84, "TemplateGetPartial"),
    (0x85, "TemplateUnregisterPartial"),
    (0x86, "TemplateFree"),
    (0x87, "TemplateEscapeHtml"),
    (0x88, "TemplateUnescapeHtml"),
    (0x89, "StructSetNamed"),
    (0x8A, "TCP sub-operations"),
    (0x8B, "HTTP server sub-operations"),
    (0x8C, "String sub-operations"),
    (0x90, "SqrtF64"),
    (0x91, "PowF64"),
    (0x92, "SinF64"),
    (0x93, "CosF64"),
    (0x94, "TanF64"),
    (0x95, "FloorF64"),
    (0x96, "CeilF64"),
    (0x97, "RoundF64"),
    (0xA1, "HostCallIo"),
    (0xA2, "HostCallNet"),
    (0xA3, "HostCallTime"),
    (0xA4, "OptSome"),
    (0xA5, "OptNone"),
    (0xA6, "OptIsSome"),
    (0xA7, "OptUnwrap"),
    (0xA8, "OptOrElse"),
    (0xA9, "HostCallDb"),
    (0xAA, "HostCallMail"),
    (0xB0, "ListNew"),
    (0xB1, "ListPush"),
    (0xB2, "ListGet"),
    (0xB3, "ListLen"),
    (0xB4, "MapNew"),
    (0xB5, "MapPut"),
    (0xB6, "MapGet"),
    (0xB7, "MapLen"),
    (0xB8, "ListSet"),
    (0xBB, "ListRemove"),
    (0xC0, "SharedNew"),
    (0xC1, "SharedStore"),
    (0xC2, "SharedLoad"),
    (0xC3, "SharedGetByName"),
    (0xC4, "ListSort"),
    (0xC6, "MapContainsKey"),
    (0xC8, "MapRemove"),
    (0xCC, "MapKeys"),
    (0xCD, "MapMerge"),
    (0xCE, "ClosureNew"),
    (0xCF, "ClosureCapture"),
    (0xD0, "ClosureCall"),
    (0xD1, "RcAlloc"),
    (0xD2, "RcInc"),
    (0xD3, "RcDec"),
    (0xD4, "RcDecWithDeinit"),
    (0xD5, "RcLoad"),
    (0xD6, "RcStore"),
    (0xD7, "RcGetCount"),
    (0xD8, "RegionEnter"),
    (0xD9, "RegionExit"),
    (0xDA, "Panic"),
    (0xDB, "SetUnwindHandler"),
    (0xDC, "ClearUnwindHandler"),
    (0xDD, "GetPanicMessage"),
    (0xDE, "Throw"),
    (0xDF, "GetException"),
    (0xE0, "ExternCall"),
    (0xE1, "JsonStringify"),
    (0xE2, "JsonParse"),
    (0xE3, "StructToJson"),
    (0xE4, "JsonToStruct"),
    (0xE5, "HtmlParse"),
    // Note: JSON accessor ops use 0x7F prefix with sub-opcodes 0x30-0x39
    (0xFF, "Halt"),
];

/// Look up opcode name.
fn opcode_name(opcode: u8) -> Option<&'static str> {
    KNOWN_OPCODES
        .iter()
        .find(|(op, _)| *op == opcode)
        .map(|(_, name)| *name)
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Arth bytecode decode error during {}", self.phase)?;
        writeln!(
            f,
            "  Offset: 0x{:08x} ({} / {} bytes)",
            self.offset, self.offset, self.total_size
        )?;

        match &self.kind {
            DecodeErrorKind::TooShort { expected, got } => {
                writeln!(f, "  Error: bytecode too short")?;
                writeln!(f, "  Expected at least {} bytes, got {}", expected, got)?;
            }
            DecodeErrorKind::InvalidMagic { found } => {
                writeln!(f, "  Error: invalid magic header")?;
                writeln!(f, "  Expected: ARTHBC01")?;
                write!(f, "  Found:    ")?;
                for b in found {
                    if b.is_ascii_graphic() || *b == b' ' {
                        write!(f, "{}", *b as char)?;
                    } else {
                        write!(f, "\\x{:02x}", b)?;
                    }
                }
                writeln!(f)?;
            }
            DecodeErrorKind::WrongFileType { detected } => {
                writeln!(f, "  Error: wrong file type")?;
                writeln!(f, "  Detected: {}", detected)?;
            }
            DecodeErrorKind::UnsupportedVersion {
                version, min, max, ..
            } => {
                writeln!(f, "  Error: unsupported bytecode version")?;
                writeln!(f, "  Found version: {}", version)?;
                writeln!(f, "  Supported: {} - {}", min, max)?;
            }
            DecodeErrorKind::UnknownOpcode { opcode } => {
                writeln!(f, "  Error: unknown opcode 0x{:02x}", opcode)?;
                if opcode.is_ascii_graphic() {
                    writeln!(f, "  (ASCII: '{}')", *opcode as char)?;
                }
                let nearby = DecodeError::nearby_opcodes(*opcode);
                if !nearby.is_empty() {
                    writeln!(f, "  Nearby valid opcodes:")?;
                    for (op, name) in nearby.iter().take(5) {
                        writeln!(f, "    0x{:02x} = {}", op, name)?;
                    }
                }
            }
            DecodeErrorKind::UnknownSubOpcode {
                category,
                opcode,
                sub_opcode,
            } => {
                writeln!(
                    f,
                    "  Error: unknown {} sub-opcode 0x{:02x}",
                    category, sub_opcode
                )?;
                writeln!(f, "  Parent opcode: 0x{:02x}", opcode)?;
            }
            DecodeErrorKind::UnexpectedEof { expected } => {
                writeln!(f, "  Error: unexpected end of bytecode")?;
                writeln!(f, "  Expected: {}", expected)?;
            }
            DecodeErrorKind::InvalidUtf8 { message } => {
                writeln!(f, "  Error: invalid UTF-8 in string data")?;
                writeln!(f, "  Details: {}", message)?;
            }
            DecodeErrorKind::Other { message } => {
                writeln!(f, "  Error: {}", message)?;
            }
        }

        writeln!(f, "\nContext (error position marked with []):")?;
        write!(f, "{}", self.format_hex_dump())?;

        if let Some(suggestion) = self.suggestion() {
            writeln!(f, "\nSuggestion: {}", suggestion)?;
        }

        Ok(())
    }
}

impl std::error::Error for DecodeError {}

// ============================================================================
// Bytecode Version Constants
// ============================================================================

/// Magic header prefix for Arth bytecode files (6 bytes)
const MAGIC_PREFIX: &[u8; 6] = b"ARTHBC";

/// Current bytecode format version
pub const BYTECODE_VERSION: u8 = 1;

/// Minimum supported bytecode version (for forward compatibility)
pub const MIN_SUPPORTED_VERSION: u8 = 1;

/// Maximum supported bytecode version
pub const MAX_SUPPORTED_VERSION: u8 = 1;

// Bytecode header format is ARTHBCNN where NN is decimal 00..99.
const _: () = {
    assert!(MIN_SUPPORTED_VERSION <= BYTECODE_VERSION);
    assert!(BYTECODE_VERSION <= MAX_SUPPORTED_VERSION);
    assert!(MAX_SUPPORTED_VERSION <= 99);
};

const fn make_magic_header(version: u8) -> [u8; 8] {
    [
        b'A',
        b'R',
        b'T',
        b'H',
        b'B',
        b'C',
        b'0' + (version / 10),
        b'0' + (version % 10),
    ]
}

/// Full magic header including version (8 bytes)
const MAGIC: [u8; 8] = make_magic_header(BYTECODE_VERSION);

/// Bytecode version information
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytecodeVersionInfo {
    /// The version number extracted from bytecode
    pub version: u8,
    /// Whether this version is supported by the current VM
    pub supported: bool,
    /// Minimum version this VM supports
    pub min_supported: u8,
    /// Maximum version this VM supports
    pub max_supported: u8,
}

impl BytecodeVersionInfo {
    /// Get version info for the current VM
    pub fn current() -> Self {
        Self {
            version: BYTECODE_VERSION,
            supported: true,
            min_supported: MIN_SUPPORTED_VERSION,
            max_supported: MAX_SUPPORTED_VERSION,
        }
    }
}

/// Extract version from magic header bytes
fn extract_version(magic: &[u8]) -> Option<u8> {
    if magic.len() < 8 {
        return None;
    }
    // Check prefix "ARTHBC"
    if &magic[..6] != MAGIC_PREFIX {
        return None;
    }
    // Parse version from bytes 6-7 (e.g., "01" -> 1)
    let v1 = magic[6];
    let v2 = magic[7];
    if v1.is_ascii_digit() && v2.is_ascii_digit() {
        Some((v1 - b'0') * 10 + (v2 - b'0'))
    } else {
        None
    }
}

/// Check if a bytecode version is supported
fn is_version_supported(version: u8) -> bool {
    version >= MIN_SUPPORTED_VERSION && version <= MAX_SUPPORTED_VERSION
}

/// Validate bytecode header and return detailed error if invalid.
/// Returns (version, DecodeError) on success/failure.
fn validate_bytecode_header_detailed(bytes: &[u8]) -> Result<u8, DecodeError> {
    // Check minimum length
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

    let header = &bytes[..8];

    // Check if it looks like Arth bytecode at all
    if &header[..6] != MAGIC_PREFIX {
        // Check for common misidentifications
        if header.starts_with(b"ARTHLIB") {
            return Err(DecodeError::new(
                DecodeErrorKind::WrongFileType {
                    detected: "Arth library (.alib) - use linker to load libraries",
                },
                DecodePhase::Header,
                0,
                bytes,
            ));
        }
        if header.starts_with(b"\x7fELF") {
            return Err(DecodeError::new(
                DecodeErrorKind::WrongFileType {
                    detected: "ELF binary",
                },
                DecodePhase::Header,
                0,
                bytes,
            ));
        }
        if header.starts_with(b"MZ") {
            return Err(DecodeError::new(
                DecodeErrorKind::WrongFileType {
                    detected: "Windows executable (PE)",
                },
                DecodePhase::Header,
                0,
                bytes,
            ));
        }
        if header.starts_with(b"PK") {
            return Err(DecodeError::new(
                DecodeErrorKind::WrongFileType {
                    detected: "ZIP archive",
                },
                DecodePhase::Header,
                0,
                bytes,
            ));
        }

        let mut found = [0u8; 8];
        found.copy_from_slice(header);
        return Err(DecodeError::new(
            DecodeErrorKind::InvalidMagic { found },
            DecodePhase::Header,
            0,
            bytes,
        ));
    }

    // Extract and validate version
    let version = match extract_version(header) {
        Some(v) => v,
        None => {
            return Err(DecodeError::new(
                DecodeErrorKind::Other {
                    message: format!(
                        "invalid version format: expected digits, got '{}{}'",
                        header[6] as char, header[7] as char
                    ),
                },
                DecodePhase::Header,
                6,
                bytes,
            ));
        }
    };

    // Check version compatibility
    if !is_version_supported(version) {
        return Err(DecodeError::new(
            DecodeErrorKind::UnsupportedVersion {
                version,
                min: MIN_SUPPORTED_VERSION,
                max: MAX_SUPPORTED_VERSION,
                too_new: version > MAX_SUPPORTED_VERSION,
            },
            DecodePhase::Header,
            6,
            bytes,
        ));
    }

    Ok(version)
}

/// Validate bytecode header and return detailed error if invalid (legacy String API)
fn validate_bytecode_header(bytes: &[u8]) -> Result<u8, String> {
    validate_bytecode_header_detailed(bytes).map_err(|e| e.to_string())
}

/// Get information about bytecode without fully decoding it
pub fn get_bytecode_info(bytes: &[u8]) -> Result<BytecodeVersionInfo, String> {
    let version = validate_bytecode_header(bytes)?;
    Ok(BytecodeVersionInfo {
        version,
        supported: is_version_supported(version),
        min_supported: MIN_SUPPORTED_VERSION,
        max_supported: MAX_SUPPORTED_VERSION,
    })
}

/// Check if bytes represent valid Arth bytecode
pub fn is_valid_bytecode(bytes: &[u8]) -> bool {
    validate_bytecode_header(bytes).is_ok()
}

fn write_u32_le(buf: &mut Vec<u8>, n: u32) {
    buf.extend_from_slice(&n.to_le_bytes());
}

fn read_u32_le(bytes: &[u8], off: &mut usize) -> Result<u32, String> {
    if *off + 4 > bytes.len() {
        return Err("unexpected EOF while reading u32".into());
    }
    let mut tmp = [0u8; 4];
    tmp.copy_from_slice(&bytes[*off..*off + 4]);
    *off += 4;
    Ok(u32::from_le_bytes(tmp))
}

pub fn encode_program(p: &Program) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    write_u32_le(&mut out, p.strings.len() as u32);
    for s in &p.strings {
        write_u32_le(&mut out, s.len() as u32);
        out.extend_from_slice(s.as_bytes());
    }
    write_u32_le(&mut out, p.code.len() as u32);
    for op in &p.code {
        match op {
            Op::Print(ix) => {
                out.push(0x01);
                write_u32_le(&mut out, *ix);
            }
            Op::PrintStrVal(ix) => {
                out.push(0x02);
                write_u32_le(&mut out, *ix);
            }
            Op::PrintRaw(ix) => {
                out.push(0x03);
                write_u32_le(&mut out, *ix);
            }
            Op::PrintRawStrVal(ix) => {
                out.push(0x04);
                write_u32_le(&mut out, *ix);
            }
            Op::PrintLn => {
                out.push(0x05);
            }
            Op::Halt => {
                out.push(0xFF);
            }
            Op::PushI64(n) => {
                out.push(0x10);
                out.extend_from_slice(&n.to_le_bytes());
            }
            Op::PushF64(x) => {
                out.push(0x12);
                out.extend_from_slice(&x.to_le_bytes());
            }
            Op::PushBool(b) => {
                out.push(0x11);
                out.push(*b);
            }
            Op::PushStr(ix) => {
                out.push(0x13);
                write_u32_le(&mut out, *ix);
            }
            Op::AddI64 => {
                out.push(0x20);
            }
            Op::SubI64 => {
                out.push(0x22);
            }
            Op::MulI64 => {
                out.push(0x23);
            }
            Op::DivI64 => {
                out.push(0x24);
            }
            Op::ModI64 => {
                out.push(0x26);
            }
            Op::LtI64 => {
                out.push(0x21);
            }
            Op::EqI64 => {
                out.push(0x25);
            }
            Op::EqStr => {
                out.push(0x2C);
            }
            Op::ConcatStr => {
                out.push(0x2D);
            }
            Op::ShlI64 => {
                out.push(0x27);
            }
            Op::ShrI64 => {
                out.push(0x28);
            }
            Op::AndI64 => {
                out.push(0x29);
            }
            Op::OrI64 => {
                out.push(0x2A);
            }
            Op::XorI64 => {
                out.push(0x2B);
            }
            Op::Jump(tgt) => {
                out.push(0x30);
                write_u32_le(&mut out, *tgt);
            }
            Op::JumpIfFalse(tgt) => {
                out.push(0x31);
                write_u32_le(&mut out, *tgt);
            }
            Op::Call(tgt) => {
                out.push(0x70);
                write_u32_le(&mut out, *tgt);
            }
            Op::CallSymbol(sym) => {
                out.push(0x00);
                write_u32_le(&mut out, *sym);
            }
            Op::ExternCall {
                sym,
                argc,
                float_mask,
                ret_kind,
            } => {
                // 0xE0 was previously HttpFetch; now reused for VM extern calls.
                out.push(0xE0);
                write_u32_le(&mut out, *sym);
                out.push(*argc);
                write_u32_le(&mut out, *float_mask);
                out.push(*ret_kind);
            }
            Op::Ret => {
                out.push(0x71);
            }
            Op::Pop => {
                out.push(0x40);
            }
            Op::PrintTop => {
                out.push(0x50);
            }
            Op::LocalGet(ix) => {
                out.push(0x60);
                write_u32_le(&mut out, *ix);
            }
            Op::LocalSet(ix) => {
                out.push(0x61);
                write_u32_le(&mut out, *ix);
            }
            Op::ToF64 => {
                out.push(0x62);
            }
            Op::ToI64 => {
                out.push(0x63);
            }
            Op::ToI64OrEnumTag => {
                out.push(0x66);
            }
            Op::ToBool => {
                out.push(0x64);
            }
            Op::ToChar => {
                out.push(0x65);
            }
            Op::ToI8 => {
                out.push(0x67);
            }
            Op::ToI16 => {
                out.push(0x68);
            }
            Op::ToI32 => {
                out.push(0x69);
            }
            Op::ToU8 => {
                out.push(0x6A);
            }
            Op::ToU16 => {
                out.push(0x6B);
            }
            Op::ToU32 => {
                out.push(0x6C);
            }
            Op::ToU64 => {
                out.push(0x6D);
            }
            Op::ToF32 => {
                out.push(0x6E);
            }
            Op::SqrtF64 => {
                out.push(0x90);
            }
            Op::PowF64 => {
                out.push(0x91);
            }
            Op::SinF64 => {
                out.push(0x92);
            }
            Op::CosF64 => {
                out.push(0x93);
            }
            Op::TanF64 => {
                out.push(0x94);
            }
            Op::FloorF64 => {
                out.push(0x95);
            }
            Op::CeilF64 => {
                out.push(0x96);
            }
            Op::RoundF64 => {
                out.push(0x97);
            }
            // Removed: RoundF64N (0xA0), MinF64 (0x98), MaxF64 (0x99), ClampF64 (0x9A),
            //          AbsF64 (0x9B), MinI64 (0x9C), MaxI64 (0x9D), ClampI64 (0x9E), AbsI64 (0x9F)
            // These are now pure Arth in stdlib/src/math/Math.arth
            Op::ListNew => {
                out.push(0xB0);
            }
            Op::ListPush => {
                out.push(0xB1);
            }
            Op::ListGet => {
                out.push(0xB2);
            }
            Op::ListSet => {
                out.push(0xB8);
            }
            Op::ListLen => {
                out.push(0xB3);
            }
            // Removed: ListContains (0xB9), ListInsert (0xBA),
            //          ListClear (0xBC), ListReverse (0xBD), ListConcat (0xBE),
            //          ListSlice (0xBF), ListUnique (0xC5)
            // These are now pure Arth code in stdlib/src/arth/array.arth
            // Note: 0xB8 was ListIndexOf, now reused for ListSet
            Op::ListRemove => {
                out.push(0xBB);
            }
            Op::ListSort => {
                out.push(0xC4);
            }
            Op::MapNew => {
                out.push(0xB4);
            }
            Op::MapPut => {
                out.push(0xB5);
            }
            Op::MapGet => {
                out.push(0xB6);
            }
            Op::MapLen => {
                out.push(0xB7);
            }
            Op::MapContainsKey => {
                out.push(0xC6);
            }
            // Removed: MapContainsValue (0xC7), MapClear (0xC9), MapIsEmpty (0xCA),
            //          MapGetOrDefault (0xCB), MapValues (0xCD)
            // These are now pure Arth code in stdlib/src/arth/map.arth
            Op::MapRemove => {
                out.push(0xC8);
            }
            Op::MapKeys => {
                out.push(0xCC);
            }
            Op::MapMerge => {
                out.push(0xCD);
            }
            // Optional operations (0xA4-0xA8)
            Op::OptSome => out.push(0xA4),
            Op::OptNone => out.push(0xA5),
            Op::OptIsSome => out.push(0xA6),
            Op::OptUnwrap => out.push(0xA7),
            Op::OptOrElse => out.push(0xA8),
            // Native struct operations (0x72-0x78)
            Op::StructNew => out.push(0x72),
            Op::StructSet => out.push(0x73),
            Op::StructGet => out.push(0x74),
            Op::StructGetNamed => out.push(0x75),
            Op::StructSetNamed => {
                // 2-byte encoding: 0x7F prefix + 0x20 sub-opcode (struct extension)
                out.push(0x7F);
                out.push(0x20);
            }
            Op::StructCopy => out.push(0x76),
            Op::StructTypeName => out.push(0x77),
            Op::StructFieldCount => out.push(0x78),
            // Native enum operations (0x79-0x7E)
            Op::EnumNew => out.push(0x79),
            Op::EnumSetPayload => out.push(0x7A),
            Op::EnumGetPayload => out.push(0x7B),
            Op::EnumGetTag => out.push(0x7C),
            Op::EnumGetVariant => out.push(0x7D),
            Op::EnumTypeName => out.push(0x7E),
            // HTTP/WebSocket/SSE opcodes migrated to HostCallNet
            // Legacy 0xE0 HttpFetch, 0xE5 HttpServe removed
            Op::JsonStringify => {
                out.push(0xE1);
            }
            Op::JsonParse => {
                out.push(0xE2);
            }
            Op::StructToJson => {
                out.push(0xE3);
            }
            Op::JsonToStruct => {
                out.push(0xE4);
            }
            // JSON accessor operations (0x7F prefix + 0x30-0x39 sub-opcodes)
            Op::JsonGetField => {
                out.push(0x7F);
                out.push(0x30);
            }
            Op::JsonGetIndex => {
                out.push(0x7F);
                out.push(0x31);
            }
            Op::JsonGetString => {
                out.push(0x7F);
                out.push(0x32);
            }
            Op::JsonGetNumber => {
                out.push(0x7F);
                out.push(0x33);
            }
            Op::JsonGetBool => {
                out.push(0x7F);
                out.push(0x34);
            }
            Op::JsonIsNull => {
                out.push(0x7F);
                out.push(0x35);
            }
            Op::JsonIsObject => {
                out.push(0x7F);
                out.push(0x36);
            }
            Op::JsonIsArray => {
                out.push(0x7F);
                out.push(0x37);
            }
            Op::JsonArrayLen => {
                out.push(0x7F);
                out.push(0x38);
            }
            Op::JsonKeys => {
                out.push(0x7F);
                out.push(0x39);
            }
            // HTML parsing operations (0xE5-0xFE)
            Op::HtmlParse => out.push(0xE5),
            Op::HtmlParseFragment => out.push(0xE6),
            Op::HtmlStringify => out.push(0xE7),
            Op::HtmlStringifyPretty => out.push(0xE8),
            Op::HtmlFree => out.push(0xE9),
            Op::HtmlNodeType => out.push(0xEA),
            Op::HtmlTagName => out.push(0xEB),
            Op::HtmlTextContent => out.push(0xEC),
            Op::HtmlInnerHtml => out.push(0xED),
            Op::HtmlOuterHtml => out.push(0xEE),
            Op::HtmlGetAttr => out.push(0xEF),
            Op::HtmlHasAttr => out.push(0xF0),
            Op::HtmlAttrNames => out.push(0xF1),
            Op::HtmlParent => out.push(0xF2),
            Op::HtmlChildren => out.push(0xF3),
            Op::HtmlElementChildren => out.push(0xF4),
            Op::HtmlFirstChild => out.push(0xF5),
            Op::HtmlLastChild => out.push(0xF6),
            Op::HtmlNextSibling => out.push(0xF7),
            Op::HtmlPrevSibling => out.push(0xF8),
            Op::HtmlQuerySelector => out.push(0xF9),
            Op::HtmlQuerySelectorAll => out.push(0xFA),
            Op::HtmlGetById => out.push(0xFB),
            Op::HtmlGetByTag => out.push(0xFC),
            Op::HtmlGetByClass => out.push(0xFD),
            Op::HtmlHasClass => out.push(0xFE),
            // Template engine operations (0x80-0x88)
            Op::TemplateCompile => out.push(0x80),
            Op::TemplateCompileFile => out.push(0x81),
            Op::TemplateRender => out.push(0x82),
            Op::TemplateRegisterPartial => out.push(0x83),
            Op::TemplateGetPartial => out.push(0x84),
            Op::TemplateUnregisterPartial => out.push(0x85),
            Op::TemplateFree => out.push(0x86),
            Op::TemplateEscapeHtml => out.push(0x87),
            Op::TemplateUnescapeHtml => out.push(0x88),
            Op::SharedGetByName(ix) => {
                out.push(0xC3);
                write_u32_le(&mut out, *ix);
            }
            Op::SharedNew => {
                out.push(0xC0);
            }
            Op::SharedStore => {
                out.push(0xC1);
            }
            Op::SharedLoad => {
                out.push(0xC2);
            }
            Op::ClosureNew(func_id, num_captures) => {
                out.push(0xCE);
                write_u32_le(&mut out, *func_id);
                write_u32_le(&mut out, *num_captures);
            }
            Op::ClosureCapture => {
                out.push(0xCF);
            }
            Op::ClosureCall(num_args) => {
                out.push(0xD0);
                write_u32_le(&mut out, *num_args);
            }
            // Reference counting operations
            Op::RcAlloc => {
                out.push(0xD1);
            }
            Op::RcInc => {
                out.push(0xD2);
            }
            Op::RcDec => {
                out.push(0xD3);
            }
            Op::RcDecWithDeinit(func_idx) => {
                out.push(0xD4);
                write_u32_le(&mut out, *func_idx);
            }
            Op::RcLoad => {
                out.push(0xD5);
            }
            Op::RcStore => {
                out.push(0xD6);
            }
            Op::RcGetCount => {
                out.push(0xD7);
            }
            // Region-based allocation operations
            Op::RegionEnter(region_id) => {
                out.push(0xD8);
                write_u32_le(&mut out, *region_id);
            }
            Op::RegionExit(region_id) => {
                out.push(0xD9);
                write_u32_le(&mut out, *region_id);
            }
            // Panic and unwinding operations
            Op::Panic(msg_idx) => {
                out.push(0xDA);
                write_u32_le(&mut out, *msg_idx);
            }
            Op::SetUnwindHandler(handler_ip) => {
                out.push(0xDB);
                write_u32_le(&mut out, *handler_ip);
            }
            Op::ClearUnwindHandler => {
                out.push(0xDC);
            }
            Op::GetPanicMessage => {
                out.push(0xDD);
            }
            Op::Throw => {
                out.push(0xDE);
            }
            Op::GetException => {
                out.push(0xDF);
            }
            // WebSocket and SSE opcodes migrated to HostCallNet
            // Legacy 0xF0-0xF6 WsServe/Accept/SendText/SendBinary/Recv/Close/IsOpen removed
            // Legacy 0xF7-0xFC SseServe/Accept/Send/Close/IsOpen removed
            // Host call opcodes (new capability-based dispatch)
            Op::HostCallIo(host_op) => {
                out.push(0xA1);
                out.push(*host_op as u8);
            }
            Op::HostCallNet(host_op) => {
                out.push(0xA2);
                out.push(*host_op as u8);
            }
            Op::HostCallTime(host_op) => {
                out.push(0xA3);
                out.push(*host_op as u8);
            }
            Op::HostCallDb(host_op) => {
                out.push(0xA1);
                out.push(HOST_CALL_EXT_DB);
                out.push(*host_op as u8);
            }
            Op::HostCallMail(host_op) => {
                out.push(0xA1);
                out.push(HOST_CALL_EXT_MAIL);
                out.push(*host_op as u8);
            }
            Op::HostCallCrypto(host_op) => {
                out.push(0xA1);
                out.push(HOST_CALL_EXT_CRYPTO);
                out.push(*host_op as u8);
            }
            Op::HostCallGeneric => {
                out.push(0x4D);
            }
            // Async/Task operations (0xA9-0xAC)
            Op::TaskSpawn => {
                out.push(0xA9);
            }
            Op::TaskPushArg => {
                out.push(0xAA);
            }
            Op::TaskAwait => {
                out.push(0xAB);
            }
            Op::TaskRunBody(func_offset) => {
                out.push(0xAC);
                write_u32_le(&mut out, *func_offset);
            }
            // Concurrent runtime operations (0xAD-0xB1 reserved)
            Op::ExecutorInit => {
                out.push(0xAD);
            }
            Op::ExecutorThreadCount => {
                out.push(0xAE);
            }
            Op::ExecutorActiveWorkers => {
                out.push(0xAF);
            }
            Op::ExecutorSpawn => {
                out.push(0x89);
            }
            Op::ExecutorJoin => {
                out.push(0x8A);
            }
            Op::ExecutorSpawnWithArg => {
                out.push(0x8B);
            }
            Op::ExecutorActiveExecutorCount => {
                out.push(0x8C);
            }
            Op::ExecutorWorkerTaskCount => {
                out.push(0x8D);
            }
            Op::ExecutorResetStats => {
                out.push(0x8E);
            }
            Op::ExecutorSpawnAwait => {
                out.push(0x8F);
            }
            // MPMC Channel operations (C06)
            Op::MpmcChanCreate => {
                out.push(0x98);
            }
            Op::MpmcChanSend => {
                out.push(0x99);
            }
            Op::MpmcChanSendBlocking => {
                out.push(0x9A);
            }
            Op::MpmcChanRecv => {
                out.push(0x9B);
            }
            Op::MpmcChanRecvBlocking => {
                out.push(0x9C);
            }
            Op::MpmcChanClose => {
                out.push(0x9D);
            }
            Op::MpmcChanLen => {
                out.push(0x9E);
            }
            Op::MpmcChanIsEmpty => {
                out.push(0x9F);
            }
            Op::MpmcChanIsFull => {
                out.push(0xA0);
            }
            Op::MpmcChanIsClosed => {
                out.push(0x06);
            }
            Op::MpmcChanCapacity => {
                out.push(0x07);
            }
            // C07: Executor-integrated MPMC channel operations
            // Using available opcodes from removed List/Map operations
            Op::MpmcChanSendWithTask => {
                out.push(0xB9); // was ListContains
            }
            Op::MpmcChanRecvWithTask => {
                out.push(0xBA); // was ListInsert
            }
            Op::MpmcChanRecvAndWake => {
                out.push(0xBC); // was ListClear
            }
            Op::MpmcChanPopWaitingSender => {
                out.push(0xBD); // was ListReverse
            }
            Op::MpmcChanGetWaitingSenderValue => {
                out.push(0xBE); // was ListConcat
            }
            Op::MpmcChanPopWaitingReceiver => {
                out.push(0xBF); // was ListSlice
            }
            Op::MpmcChanWaitingSenderCount => {
                out.push(0xC5); // was ListUnique
            }
            Op::MpmcChanWaitingReceiverCount => {
                out.push(0xC7); // was MapContainsValue
            }
            Op::MpmcChanGetWokenSender => {
                out.push(0xC9); // was MapClear
            }
            // C08: Blocking receive operations
            Op::MpmcChanSendAndWake => {
                out.push(0xCA); // was MapIsEmpty
            }
            Op::MpmcChanGetWokenReceiver => {
                out.push(0xCB); // was MapGetOrDefault
            }
            // C09: Channel Select operations (0x14-0x1D)
            Op::MpmcChanSelectClear => {
                out.push(0x14);
            }
            Op::MpmcChanSelectAdd => {
                out.push(0x15);
            }
            Op::MpmcChanSelectCount => {
                out.push(0x16);
            }
            Op::MpmcChanTrySelectRecv => {
                out.push(0x17);
            }
            Op::MpmcChanSelectRecvBlocking => {
                out.push(0x18);
            }
            Op::MpmcChanSelectRecvWithTask => {
                out.push(0x19);
            }
            Op::MpmcChanSelectGetReadyIndex => {
                out.push(0x1A);
            }
            Op::MpmcChanSelectGetValue => {
                out.push(0x1B);
            }
            Op::MpmcChanSelectDeregister => {
                out.push(0x1C);
            }
            Op::MpmcChanSelectGetHandle => {
                out.push(0x1D);
            }

            // C11: Actor operations
            Op::ActorCreate => {
                out.push(0x08);
            }
            Op::ActorSpawn => {
                out.push(0x09);
            }
            Op::ActorSend => {
                out.push(0x0A);
            }
            Op::ActorSendBlocking => {
                out.push(0x0B);
            }
            Op::ActorRecv => {
                out.push(0x0C);
            }
            Op::ActorRecvBlocking => {
                out.push(0x0D);
            }
            Op::ActorClose => {
                out.push(0x0E);
            }
            Op::ActorStop => {
                out.push(0x0F);
            }
            Op::ActorGetTask => {
                out.push(0x1E);
            }
            Op::ActorGetMailbox => {
                out.push(0x1F);
            }
            Op::ActorIsRunning => {
                out.push(0x2E);
            }
            Op::ActorGetState => {
                out.push(0x2F);
            }
            Op::ActorMessageCount => {
                out.push(0x3E);
            }
            Op::ActorMailboxEmpty => {
                out.push(0x3F);
            }
            Op::ActorMailboxLen => {
                out.push(0x4E);
            }
            Op::ActorSetTask => {
                out.push(0x4F);
            }
            Op::ActorMarkStopped => {
                out.push(0x5E);
            }
            Op::ActorMarkFailed => {
                out.push(0x5F);
            }
            Op::ActorIsFailed => {
                out.push(0x51);
            }
            // C19: Atomic operations (0x52-0x5D)
            Op::AtomicCreate => {
                out.push(0x52);
            }
            Op::AtomicLoad => {
                out.push(0x53);
            }
            Op::AtomicStore => {
                out.push(0x54);
            }
            Op::AtomicCas => {
                out.push(0x55);
            }
            Op::AtomicFetchAdd => {
                out.push(0x56);
            }
            Op::AtomicFetchSub => {
                out.push(0x57);
            }
            Op::AtomicSwap => {
                out.push(0x58);
            }
            Op::AtomicGet => {
                out.push(0x59);
            }
            Op::AtomicSet => {
                out.push(0x5A);
            }
            Op::AtomicInc => {
                out.push(0x5B);
            }
            Op::AtomicDec => {
                out.push(0x5C);
            }
            // C21: Event Loop operations (0x32-0x41)
            Op::EventLoopCreate => {
                out.push(0x32);
            }
            Op::EventLoopRegisterTimer => {
                out.push(0x33);
            }
            Op::EventLoopRegisterFd => {
                out.push(0x34);
            }
            Op::EventLoopDeregister => {
                out.push(0x35);
            }
            Op::EventLoopPoll => {
                out.push(0x36);
            }
            Op::EventLoopGetEvent => {
                out.push(0x37);
            }
            Op::EventLoopGetEventType => {
                out.push(0x38);
            }
            Op::EventLoopClose => {
                out.push(0x39);
            }
            Op::EventLoopPipeCreate => {
                out.push(0x3A);
            }
            Op::EventLoopPipeGetWriteFd => {
                out.push(0x3B);
            }
            Op::EventLoopPipeWrite => {
                out.push(0x3C);
            }
            Op::EventLoopPipeRead => {
                out.push(0x3D);
            }
            Op::EventLoopPipeClose => {
                out.push(0x41);
            }
            // C22: Timer operations (0x42-0x49)
            Op::TimerSleep => {
                out.push(0x42);
            }
            Op::TimerSleepAsync => {
                out.push(0x43);
            }
            Op::TimerCheckExpired => {
                out.push(0x44);
            }
            Op::TimerGetWaitingTask => {
                out.push(0x45);
            }
            Op::TimerCancel => {
                out.push(0x46);
            }
            Op::TimerPollExpired => {
                out.push(0x47);
            }
            Op::TimerNow => {
                out.push(0x48);
            }
            Op::TimerElapsed => {
                out.push(0x49);
            }
            Op::TimerRemove => {
                out.push(0x4A);
            }
            Op::TimerRemaining => {
                out.push(0x4B);
            }
            // Phase 5: TCP Socket Operations (C23)
            // Uses 2-byte encoding: 0x6F prefix + sub-opcode
            Op::TcpListenerBind => {
                out.push(0x6F);
                out.push(0x00);
            }
            Op::TcpListenerAccept => {
                out.push(0x6F);
                out.push(0x01);
            }
            Op::TcpListenerAcceptAsync => {
                out.push(0x6F);
                out.push(0x02);
            }
            Op::TcpListenerClose => {
                out.push(0x6F);
                out.push(0x03);
            }
            Op::TcpListenerLocalPort => {
                out.push(0x6F);
                out.push(0x04);
            }
            Op::TcpStreamConnect => {
                out.push(0x6F);
                out.push(0x05);
            }
            Op::TcpStreamConnectAsync => {
                out.push(0x6F);
                out.push(0x06);
            }
            Op::TcpStreamRead => {
                out.push(0x6F);
                out.push(0x07);
            }
            Op::TcpStreamReadAsync => {
                out.push(0x6F);
                out.push(0x08);
            }
            Op::TcpStreamWrite => {
                out.push(0x6F);
                out.push(0x09);
            }
            Op::TcpStreamWriteAsync => {
                out.push(0x6F);
                out.push(0x0A);
            }
            Op::TcpStreamClose => {
                out.push(0x6F);
                out.push(0x0B);
            }
            Op::TcpStreamGetLastRead => {
                out.push(0x6F);
                out.push(0x0C);
            }
            Op::TcpStreamSetTimeout => {
                out.push(0x6F);
                out.push(0x0D);
            }
            Op::TcpCheckReady => {
                out.push(0x6F);
                out.push(0x0E);
            }
            Op::TcpGetResult => {
                out.push(0x6F);
                out.push(0x0F);
            }
            Op::TcpPollReady => {
                out.push(0x6F);
                out.push(0x10);
            }
            Op::TcpRemoveRequest => {
                out.push(0x6F);
                out.push(0x11);
            }
            // Phase 5: HTTP Client Operations (C24)
            // Uses 2-byte encoding: 0x7F prefix + sub-opcode
            Op::HttpGet => {
                out.push(0x7F);
                out.push(0x00);
            }
            Op::HttpPost => {
                out.push(0x7F);
                out.push(0x01);
            }
            Op::HttpGetAsync => {
                out.push(0x7F);
                out.push(0x02);
            }
            Op::HttpPostAsync => {
                out.push(0x7F);
                out.push(0x03);
            }
            Op::HttpResponseStatus => {
                out.push(0x7F);
                out.push(0x04);
            }
            Op::HttpResponseHeader => {
                out.push(0x7F);
                out.push(0x05);
            }
            Op::HttpResponseBody => {
                out.push(0x7F);
                out.push(0x06);
            }
            Op::HttpResponseClose => {
                out.push(0x7F);
                out.push(0x07);
            }
            Op::HttpCheckReady => {
                out.push(0x7F);
                out.push(0x08);
            }
            Op::HttpGetResult => {
                out.push(0x7F);
                out.push(0x09);
            }
            Op::HttpPollReady => {
                out.push(0x7F);
                out.push(0x0A);
            }
            Op::HttpRemoveRequest => {
                out.push(0x7F);
                out.push(0x0B);
            }
            Op::HttpGetBodyLength => {
                out.push(0x7F);
                out.push(0x0C);
            }
            Op::HttpGetHeaderCount => {
                out.push(0x7F);
                out.push(0x0D);
            }
            // Phase 5: HTTP Server Operations (C25)
            // Uses 2-byte encoding: 0x5D prefix + sub-opcode
            Op::HttpServerCreate => {
                out.push(0x5D);
                out.push(0x00);
            }
            Op::HttpServerClose => {
                out.push(0x5D);
                out.push(0x01);
            }
            Op::HttpServerGetPort => {
                out.push(0x5D);
                out.push(0x02);
            }
            Op::HttpServerAccept => {
                out.push(0x5D);
                out.push(0x03);
            }
            Op::HttpServerAcceptAsync => {
                out.push(0x5D);
                out.push(0x04);
            }
            Op::HttpRequestMethod => {
                out.push(0x5D);
                out.push(0x05);
            }
            Op::HttpRequestPath => {
                out.push(0x5D);
                out.push(0x06);
            }
            Op::HttpRequestHeader => {
                out.push(0x5D);
                out.push(0x07);
            }
            Op::HttpRequestBody => {
                out.push(0x5D);
                out.push(0x08);
            }
            Op::HttpRequestHeaderCount => {
                out.push(0x5D);
                out.push(0x09);
            }
            Op::HttpRequestBodyLength => {
                out.push(0x5D);
                out.push(0x0A);
            }
            Op::HttpWriterStatus => {
                out.push(0x5D);
                out.push(0x0B);
            }
            Op::HttpWriterHeader => {
                out.push(0x5D);
                out.push(0x0C);
            }
            Op::HttpWriterBody => {
                out.push(0x5D);
                out.push(0x0D);
            }
            Op::HttpWriterSend => {
                out.push(0x5D);
                out.push(0x0E);
            }
            Op::HttpWriterSendAsync => {
                out.push(0x5D);
                out.push(0x0F);
            }
            Op::HttpServerCheckReady => {
                out.push(0x5D);
                out.push(0x10);
            }
            Op::HttpServerGetResult => {
                out.push(0x5D);
                out.push(0x11);
            }
            Op::HttpServerPollReady => {
                out.push(0x5D);
                out.push(0x12);
            }
            Op::HttpServerRemoveRequest => {
                out.push(0x5D);
                out.push(0x13);
            }
            // String operations (0x4C prefix + sub-opcode)
            Op::StrLen => {
                out.push(0x4C);
                out.push(0x00);
            }
            Op::StrSubstring => {
                out.push(0x4C);
                out.push(0x01);
            }
            Op::StrIndexOf => {
                out.push(0x4C);
                out.push(0x02);
            }
            Op::StrLastIndexOf => {
                out.push(0x4C);
                out.push(0x03);
            }
            Op::StrStartsWith => {
                out.push(0x4C);
                out.push(0x04);
            }
            Op::StrEndsWith => {
                out.push(0x4C);
                out.push(0x05);
            }
            Op::StrSplit => {
                out.push(0x4C);
                out.push(0x06);
            }
            Op::StrTrim => {
                out.push(0x4C);
                out.push(0x07);
            }
            Op::StrToLower => {
                out.push(0x4C);
                out.push(0x08);
            }
            Op::StrToUpper => {
                out.push(0x4C);
                out.push(0x09);
            }
            Op::StrReplace => {
                out.push(0x4C);
                out.push(0x0A);
            }
            Op::StrCharAt => {
                out.push(0x4C);
                out.push(0x0B);
            }
            Op::StrContains => {
                out.push(0x4C);
                out.push(0x0C);
            }
            Op::StrRepeat => {
                out.push(0x4C);
                out.push(0x0D);
            }
            Op::StrParseInt => {
                out.push(0x4C);
                out.push(0x0E);
            }
            Op::StrParseFloat => {
                out.push(0x4C);
                out.push(0x0F);
            }
            Op::StrFromInt => {
                out.push(0x4C);
                out.push(0x10);
            }
            Op::StrFromFloat => {
                out.push(0x4C);
                out.push(0x11);
            }
            // File/Dir/Path/Console ops are not yet encodable in the
            // demo bytecode format used by this prototype VM.
            _ => {
                unimplemented!("encode_program: opcode {:?} not supported", op);
            }
        }
    }

    // Encode async dispatch table (fn_id -> bytecode offset)
    write_u32_le(&mut out, p.async_dispatch.len() as u32);
    for (fn_id, offset) in &p.async_dispatch {
        out.extend_from_slice(&fn_id.to_le_bytes());
        write_u32_le(&mut out, *offset);
    }

    // Encode debug entries (offset -> function name + source location)
    write_u32_le(&mut out, p.debug_entries.len() as u32);
    for entry in &p.debug_entries {
        // Offset
        write_u32_le(&mut out, entry.offset);
        // Function name
        let name_bytes = entry.function_name.as_bytes();
        write_u32_le(&mut out, name_bytes.len() as u32);
        out.extend_from_slice(name_bytes);
        // Source file (optional: 0 length means none)
        if let Some(ref file) = entry.source_file {
            let file_bytes = file.as_bytes();
            write_u32_le(&mut out, file_bytes.len() as u32);
            out.extend_from_slice(file_bytes);
        } else {
            write_u32_le(&mut out, 0);
        }
        // Line number
        write_u32_le(&mut out, entry.line);
    }

    out
}

/// Decode Arth bytecode into a Program with detailed error information.
///
/// This function returns a structured `DecodeError` with context including:
/// - The exact byte offset where the error occurred
/// - Surrounding bytes for debugging
/// - The phase of decoding (header, strings, instructions, etc.)
/// - Suggestions for fixing the problem
pub fn decode_program_detailed(bytes: &[u8]) -> Result<Program, DecodeError> {
    // Validate header and check version compatibility
    let _version = validate_bytecode_header_detailed(bytes)?;
    let mut off = MAGIC.len();

    // Helper to create EOF errors
    let eof_err = |off: usize, phase: DecodePhase, expected: &'static str| -> DecodeError {
        DecodeError::new(
            DecodeErrorKind::UnexpectedEof { expected },
            phase,
            off,
            bytes,
        )
    };

    // Helper to read u32
    let read_u32 = |off: &mut usize| -> Result<u32, DecodeError> {
        if *off + 4 > bytes.len() {
            return Err(eof_err(*off, DecodePhase::StringTable, "u32 value"));
        }
        let mut tmp = [0u8; 4];
        tmp.copy_from_slice(&bytes[*off..*off + 4]);
        *off += 4;
        Ok(u32::from_le_bytes(tmp))
    };

    // Decode strings
    let str_count = read_u32(&mut off)? as usize;
    let mut strings = Vec::with_capacity(str_count);
    for _ in 0..str_count {
        let len = read_u32(&mut off)? as usize;
        if off + len > bytes.len() {
            return Err(eof_err(off, DecodePhase::StringTable, "string data"));
        }
        let s = String::from_utf8(bytes[off..off + len].to_vec()).map_err(|e| {
            DecodeError::new(
                DecodeErrorKind::InvalidUtf8 {
                    message: e.to_string(),
                },
                DecodePhase::StringTable,
                off,
                bytes,
            )
        })?;
        off += len;
        strings.push(s);
    }

    // Decode instructions
    let code_count = read_u32(&mut off).map_err(|mut e| {
        e.phase = DecodePhase::Instructions;
        e
    })? as usize;
    let mut code = Vec::with_capacity(code_count);

    for _ in 0..code_count {
        if off >= bytes.len() {
            return Err(eof_err(off, DecodePhase::Instructions, "opcode"));
        }
        let op_offset = off; // Save offset for error reporting
        let op = bytes[off];
        off += 1;
        match op {
            0x01 => {
                let ix = read_u32(&mut off).map_err(|mut e| {
                    e.phase = DecodePhase::Instructions;
                    e
                })?;
                code.push(Op::Print(ix));
            }
            0xFF => code.push(Op::Halt),
            0x02 => {
                let ix = read_u32(&mut off).map_err(|mut e| {
                    e.phase = DecodePhase::Instructions;
                    e
                })?;
                code.push(Op::PrintStrVal(ix));
            }
            0x03 => {
                let ix = read_u32(&mut off).map_err(|mut e| {
                    e.phase = DecodePhase::Instructions;
                    e
                })?;
                code.push(Op::PrintRaw(ix));
            }
            0x04 => {
                let ix = read_u32(&mut off).map_err(|mut e| {
                    e.phase = DecodePhase::Instructions;
                    e
                })?;
                code.push(Op::PrintRawStrVal(ix));
            }
            0x05 => code.push(Op::PrintLn),
            0x10 => {
                if off + 8 > bytes.len() {
                    return Err(eof_err(off, DecodePhase::Instructions, "i64 literal"));
                }
                let mut tmp = [0u8; 8];
                tmp.copy_from_slice(&bytes[off..off + 8]);
                off += 8;
                code.push(Op::PushI64(i64::from_le_bytes(tmp)));
            }
            0x12 => {
                if off + 8 > bytes.len() {
                    return Err(eof_err(off, DecodePhase::Instructions, "f64 literal"));
                }
                let mut tmp = [0u8; 8];
                tmp.copy_from_slice(&bytes[off..off + 8]);
                off += 8;
                code.push(Op::PushF64(f64::from_le_bytes(tmp)));
            }
            0x11 => {
                if off >= bytes.len() {
                    return Err(eof_err(off, DecodePhase::Instructions, "bool value"));
                }
                let b = bytes[off];
                off += 1;
                code.push(Op::PushBool(b));
            }
            0x13 => {
                let ix = read_u32(&mut off).map_err(|mut e| {
                    e.phase = DecodePhase::Instructions;
                    e
                })?;
                code.push(Op::PushStr(ix));
            }
            // All the other opcodes use the same pattern - for now, delegate to inner match
            // that returns the op or an error message, which we convert
            _ => {
                // This is where we handle the unknown opcode case with proper error
                return Err(DecodeError::new(
                    DecodeErrorKind::UnknownOpcode { opcode: op },
                    DecodePhase::Instructions,
                    op_offset,
                    bytes,
                ));
            }
        }
    }

    // Return partial result for now - the full impl will be in decode_program
    // This is just for the most common error cases
    Ok(Program {
        strings,
        code,
        async_dispatch: Vec::new(),
        debug_entries: Vec::new(),
    })
}

pub fn decode_program(bytes: &[u8]) -> Result<Program, String> {
    // Validate header and check version compatibility
    let _version = validate_bytecode_header(bytes)?;
    let mut off = MAGIC.len();
    let str_count = read_u32_le(bytes, &mut off)? as usize;
    let mut strings = Vec::with_capacity(str_count);
    for _ in 0..str_count {
        let len = read_u32_le(bytes, &mut off)? as usize;
        if off + len > bytes.len() {
            return Err(DecodeError::new(
                DecodeErrorKind::UnexpectedEof {
                    expected: "string data",
                },
                DecodePhase::StringTable,
                off,
                bytes,
            )
            .to_string());
        }
        let s = String::from_utf8(bytes[off..off + len].to_vec()).map_err(|e| {
            DecodeError::new(
                DecodeErrorKind::InvalidUtf8 {
                    message: e.to_string(),
                },
                DecodePhase::StringTable,
                off,
                bytes,
            )
            .to_string()
        })?;
        off += len;
        strings.push(s);
    }
    let code_count = read_u32_le(bytes, &mut off)? as usize;
    let mut code = Vec::with_capacity(code_count);
    for _ in 0..code_count {
        let op_offset = off; // Save for error context
        if off >= bytes.len() {
            return Err(DecodeError::new(
                DecodeErrorKind::UnexpectedEof { expected: "opcode" },
                DecodePhase::Instructions,
                off,
                bytes,
            )
            .to_string());
        }
        let op = bytes[off];
        off += 1;
        match op {
            0x01 => {
                let ix = read_u32_le(bytes, &mut off)?;
                code.push(Op::Print(ix));
            }
            0xFF => code.push(Op::Halt),
            0x02 => {
                let ix = read_u32_le(bytes, &mut off)?;
                code.push(Op::PrintStrVal(ix));
            }
            0x03 => {
                let ix = read_u32_le(bytes, &mut off)?;
                code.push(Op::PrintRaw(ix));
            }
            0x04 => {
                let ix = read_u32_le(bytes, &mut off)?;
                code.push(Op::PrintRawStrVal(ix));
            }
            0x05 => code.push(Op::PrintLn),
            0x10 => {
                if off + 8 > bytes.len() {
                    return Err("unexpected EOF while reading i64".into());
                }
                let mut tmp = [0u8; 8];
                tmp.copy_from_slice(&bytes[off..off + 8]);
                off += 8;
                code.push(Op::PushI64(i64::from_le_bytes(tmp)));
            }
            0x12 => {
                if off + 8 > bytes.len() {
                    return Err("unexpected EOF while reading f64".into());
                }
                let mut tmp = [0u8; 8];
                tmp.copy_from_slice(&bytes[off..off + 8]);
                off += 8;
                code.push(Op::PushF64(f64::from_le_bytes(tmp)));
            }
            0x11 => {
                if off >= bytes.len() {
                    return Err("unexpected EOF while reading bool".into());
                }
                let b = bytes[off];
                off += 1;
                code.push(Op::PushBool(b));
            }
            0x13 => {
                let ix = read_u32_le(bytes, &mut off)?;
                code.push(Op::PushStr(ix));
            }
            0x20 => code.push(Op::AddI64),
            0x21 => code.push(Op::LtI64),
            0x22 => code.push(Op::SubI64),
            0x23 => code.push(Op::MulI64),
            0x24 => code.push(Op::DivI64),
            0x26 => code.push(Op::ModI64),
            0x25 => code.push(Op::EqI64),
            0x2C => code.push(Op::EqStr),
            0x2D => code.push(Op::ConcatStr),
            0x27 => code.push(Op::ShlI64),
            0x28 => code.push(Op::ShrI64),
            0x29 => code.push(Op::AndI64),
            0x2A => code.push(Op::OrI64),
            0x2B => code.push(Op::XorI64),
            0x30 => {
                let tgt = read_u32_le(bytes, &mut off)?;
                code.push(Op::Jump(tgt));
            }
            0x31 => {
                let tgt = read_u32_le(bytes, &mut off)?;
                code.push(Op::JumpIfFalse(tgt));
            }
            0x70 => {
                let tgt = read_u32_le(bytes, &mut off)?;
                code.push(Op::Call(tgt));
            }
            0x00 => {
                let sym = read_u32_le(bytes, &mut off)?;
                code.push(Op::CallSymbol(sym));
            }
            0xE0 => {
                let sym = read_u32_le(bytes, &mut off)?;
                if off >= bytes.len() {
                    return Err("unexpected EOF while reading extern argc".into());
                }
                let argc = bytes[off];
                off += 1;
                let float_mask = read_u32_le(bytes, &mut off)?;
                if off >= bytes.len() {
                    return Err("unexpected EOF while reading extern ret_kind".into());
                }
                let ret_kind = bytes[off];
                off += 1;
                code.push(Op::ExternCall {
                    sym,
                    argc,
                    float_mask,
                    ret_kind,
                });
            }
            0x71 => code.push(Op::Ret),
            0x40 => code.push(Op::Pop),
            0x50 => code.push(Op::PrintTop),
            0x60 => {
                let ix = read_u32_le(bytes, &mut off)?;
                code.push(Op::LocalGet(ix));
            }
            0x61 => {
                let ix = read_u32_le(bytes, &mut off)?;
                code.push(Op::LocalSet(ix));
            }
            0x62 => code.push(Op::ToF64),
            0x63 => code.push(Op::ToI64),
            0x66 => code.push(Op::ToI64OrEnumTag),
            0x64 => code.push(Op::ToBool),
            0x65 => code.push(Op::ToChar),
            0x67 => code.push(Op::ToI8),
            0x68 => code.push(Op::ToI16),
            0x69 => code.push(Op::ToI32),
            0x6A => code.push(Op::ToU8),
            0x6B => code.push(Op::ToU16),
            0x6C => code.push(Op::ToU32),
            0x6D => code.push(Op::ToU64),
            0x6E => code.push(Op::ToF32),
            0x90 => code.push(Op::SqrtF64),
            0x91 => code.push(Op::PowF64),
            0x92 => code.push(Op::SinF64),
            0x93 => code.push(Op::CosF64),
            0x94 => code.push(Op::TanF64),
            0x95 => code.push(Op::FloorF64),
            0x96 => code.push(Op::CeilF64),
            0x97 => code.push(Op::RoundF64),
            // Removed: 0xA0 RoundF64N, 0x98 MinF64, 0x99 MaxF64, 0x9A ClampF64,
            //          0x9B AbsF64, 0x9C MinI64, 0x9D MaxI64, 0x9E ClampI64, 0x9F AbsI64
            // These are now pure Arth in stdlib/src/math/Math.arth
            0xB0 => code.push(Op::ListNew),
            0xB1 => code.push(Op::ListPush),
            0xB2 => code.push(Op::ListGet),
            0xB3 => code.push(Op::ListLen),
            0xB8 => code.push(Op::ListSet), // was ListIndexOf, now reused
            // Removed: 0xB9 ListContains, 0xBA ListInsert,
            //          0xBC ListClear, 0xBD ListReverse, 0xBE ListConcat,
            //          0xBF ListSlice, 0xC5 ListUnique
            0xBB => code.push(Op::ListRemove),
            0xC4 => code.push(Op::ListSort),
            0xB4 => code.push(Op::MapNew),
            0xB5 => code.push(Op::MapPut),
            0xB6 => code.push(Op::MapGet),
            0xB7 => code.push(Op::MapLen),
            0xC6 => code.push(Op::MapContainsKey),
            // Removed: 0xC7 MapContainsValue, 0xC9 MapClear, 0xCA MapIsEmpty,
            //          0xCB MapGetOrDefault, 0xCD MapValues
            0xC8 => code.push(Op::MapRemove),
            0xCC => code.push(Op::MapKeys),
            0xCD => code.push(Op::MapMerge),
            // Optional operations (0xA4-0xA8)
            0xA4 => code.push(Op::OptSome),
            0xA5 => code.push(Op::OptNone),
            0xA6 => code.push(Op::OptIsSome),
            0xA7 => code.push(Op::OptUnwrap),
            0xA8 => code.push(Op::OptOrElse),
            // Native struct operations (0x72-0x78)
            0x72 => code.push(Op::StructNew),
            0x73 => code.push(Op::StructSet),
            0x74 => code.push(Op::StructGet),
            0x75 => code.push(Op::StructGetNamed),
            0x76 => code.push(Op::StructCopy),
            0x77 => code.push(Op::StructTypeName),
            0x78 => code.push(Op::StructFieldCount),
            // Native enum operations (0x79-0x7E)
            0x79 => code.push(Op::EnumNew),
            0x7A => code.push(Op::EnumSetPayload),
            0x7B => code.push(Op::EnumGetPayload),
            0x7C => code.push(Op::EnumGetTag),
            0x7D => code.push(Op::EnumGetVariant),
            0x7E => code.push(Op::EnumTypeName),
            // 0xE0 HttpFetch - migrated to HostCallNet
            0xE1 => code.push(Op::JsonStringify),
            0xE2 => code.push(Op::JsonParse),
            0xE3 => code.push(Op::StructToJson),
            0xE4 => code.push(Op::JsonToStruct),
            // HTML parsing operations (0xE5-0xFE)
            0xE5 => code.push(Op::HtmlParse),
            0xE6 => code.push(Op::HtmlParseFragment),
            0xE7 => code.push(Op::HtmlStringify),
            0xE8 => code.push(Op::HtmlStringifyPretty),
            0xE9 => code.push(Op::HtmlFree),
            0xEA => code.push(Op::HtmlNodeType),
            0xEB => code.push(Op::HtmlTagName),
            0xEC => code.push(Op::HtmlTextContent),
            0xED => code.push(Op::HtmlInnerHtml),
            0xEE => code.push(Op::HtmlOuterHtml),
            0xEF => code.push(Op::HtmlGetAttr),
            0xF0 => code.push(Op::HtmlHasAttr),
            0xF1 => code.push(Op::HtmlAttrNames),
            0xF2 => code.push(Op::HtmlParent),
            0xF3 => code.push(Op::HtmlChildren),
            0xF4 => code.push(Op::HtmlElementChildren),
            0xF5 => code.push(Op::HtmlFirstChild),
            0xF6 => code.push(Op::HtmlLastChild),
            0xF7 => code.push(Op::HtmlNextSibling),
            0xF8 => code.push(Op::HtmlPrevSibling),
            0xF9 => code.push(Op::HtmlQuerySelector),
            0xFA => code.push(Op::HtmlQuerySelectorAll),
            0xFB => code.push(Op::HtmlGetById),
            0xFC => code.push(Op::HtmlGetByTag),
            0xFD => code.push(Op::HtmlGetByClass),
            0xFE => code.push(Op::HtmlHasClass),
            // Template engine operations (0x80-0x88)
            0x80 => code.push(Op::TemplateCompile),
            0x81 => code.push(Op::TemplateCompileFile),
            0x82 => code.push(Op::TemplateRender),
            0x83 => code.push(Op::TemplateRegisterPartial),
            0x84 => code.push(Op::TemplateGetPartial),
            0x85 => code.push(Op::TemplateUnregisterPartial),
            0x86 => code.push(Op::TemplateFree),
            0x87 => code.push(Op::TemplateEscapeHtml),
            0x88 => code.push(Op::TemplateUnescapeHtml),
            0xC3 => {
                let ix = read_u32_le(bytes, &mut off)?;
                code.push(Op::SharedGetByName(ix));
            }
            0xC0 => code.push(Op::SharedNew),
            0xC1 => code.push(Op::SharedStore),
            0xC2 => code.push(Op::SharedLoad),
            0xCE => {
                let func_id = read_u32_le(bytes, &mut off)?;
                let num_captures = read_u32_le(bytes, &mut off)?;
                code.push(Op::ClosureNew(func_id, num_captures));
            }
            0xCF => code.push(Op::ClosureCapture),
            0xD0 => {
                let num_args = read_u32_le(bytes, &mut off)?;
                code.push(Op::ClosureCall(num_args));
            }
            // Reference counting operations
            0xD1 => code.push(Op::RcAlloc),
            0xD2 => code.push(Op::RcInc),
            0xD3 => code.push(Op::RcDec),
            0xD4 => {
                let func_idx = read_u32_le(bytes, &mut off)?;
                code.push(Op::RcDecWithDeinit(func_idx));
            }
            0xD5 => code.push(Op::RcLoad),
            0xD6 => code.push(Op::RcStore),
            0xD7 => code.push(Op::RcGetCount),
            // Region-based allocation operations
            0xD8 => {
                let region_id = read_u32_le(bytes, &mut off)?;
                code.push(Op::RegionEnter(region_id));
            }
            0xD9 => {
                let region_id = read_u32_le(bytes, &mut off)?;
                code.push(Op::RegionExit(region_id));
            }
            // Panic and unwinding operations
            0xDA => {
                let msg_idx = read_u32_le(bytes, &mut off)?;
                code.push(Op::Panic(msg_idx));
            }
            0xDB => {
                let handler_ip = read_u32_le(bytes, &mut off)?;
                code.push(Op::SetUnwindHandler(handler_ip));
            }
            0xDC => code.push(Op::ClearUnwindHandler),
            0xDD => code.push(Op::GetPanicMessage),
            0xDE => code.push(Op::Throw),
            0xDF => code.push(Op::GetException),
            // WebSocket and SSE opcodes (0xF0-0xFC) migrated to HostCallNet
            // Host call opcodes (new capability-based dispatch)
            0xA1 => {
                if off >= bytes.len() {
                    return Err("unexpected EOF while reading HostIoOp".into());
                }
                let host_op_byte = bytes[off];
                off += 1;
                match host_op_byte {
                    HOST_CALL_EXT_DB => {
                        if off >= bytes.len() {
                            return Err("unexpected EOF while reading HostDbOp".into());
                        }
                        let db_op_byte = bytes[off];
                        off += 1;
                        let db_op = HostDbOp::from_u8(db_op_byte)
                            .ok_or_else(|| format!("unknown HostDbOp: 0x{:02x}", db_op_byte))?;
                        code.push(Op::HostCallDb(db_op));
                    }
                    HOST_CALL_EXT_MAIL => {
                        if off >= bytes.len() {
                            return Err("unexpected EOF while reading HostMailOp".into());
                        }
                        let mail_op_byte = bytes[off];
                        off += 1;
                        let mail_op = HostMailOp::from_u8(mail_op_byte)
                            .ok_or_else(|| format!("unknown HostMailOp: 0x{:02x}", mail_op_byte))?;
                        code.push(Op::HostCallMail(mail_op));
                    }
                    HOST_CALL_EXT_CRYPTO => {
                        if off >= bytes.len() {
                            return Err("unexpected EOF while reading HostCryptoOp".into());
                        }
                        let crypto_op_byte = bytes[off];
                        off += 1;
                        let crypto_op = HostCryptoOp::from_u8(crypto_op_byte).ok_or_else(|| {
                            format!("unknown HostCryptoOp: 0x{:02x}", crypto_op_byte)
                        })?;
                        code.push(Op::HostCallCrypto(crypto_op));
                    }
                    _ => {
                        let host_op = HostIoOp::from_u8(host_op_byte)
                            .ok_or_else(|| format!("unknown HostIoOp: 0x{:02x}", host_op_byte))?;
                        code.push(Op::HostCallIo(host_op));
                    }
                }
            }
            0xA2 => {
                if off >= bytes.len() {
                    return Err("unexpected EOF while reading HostNetOp".into());
                }
                let host_op_byte = bytes[off];
                off += 1;
                let host_op = HostNetOp::from_u8(host_op_byte)
                    .ok_or_else(|| format!("unknown HostNetOp: 0x{:02x}", host_op_byte))?;
                code.push(Op::HostCallNet(host_op));
            }
            0xA3 => {
                if off >= bytes.len() {
                    return Err("unexpected EOF while reading HostTimeOp".into());
                }
                let host_op_byte = bytes[off];
                off += 1;
                let host_op = HostTimeOp::from_u8(host_op_byte)
                    .ok_or_else(|| format!("unknown HostTimeOp: 0x{:02x}", host_op_byte))?;
                code.push(Op::HostCallTime(host_op));
            }
            0x4D => {
                code.push(Op::HostCallGeneric);
            }
            // Async/Task operations (0xA9-0xAC)
            0xA9 => {
                code.push(Op::TaskSpawn);
            }
            0xAA => {
                code.push(Op::TaskPushArg);
            }
            0xAB => {
                code.push(Op::TaskAwait);
            }
            0xAC => {
                let func_offset = read_u32_le(bytes, &mut off)?;
                code.push(Op::TaskRunBody(func_offset));
            }
            // Concurrent runtime operations
            0xAD => code.push(Op::ExecutorInit),
            0xAE => code.push(Op::ExecutorThreadCount),
            0xAF => code.push(Op::ExecutorActiveWorkers),
            0x89 => code.push(Op::ExecutorSpawn),
            0x8A => code.push(Op::ExecutorJoin),
            // C02 work-stealing stats operations
            0x8B => code.push(Op::ExecutorSpawnWithArg),
            0x8C => code.push(Op::ExecutorActiveExecutorCount),
            0x8D => code.push(Op::ExecutorWorkerTaskCount),
            0x8E => code.push(Op::ExecutorResetStats),
            // C04 suspension operation
            0x8F => code.push(Op::ExecutorSpawnAwait),
            // MPMC Channel operations (C06)
            0x98 => code.push(Op::MpmcChanCreate),
            0x99 => code.push(Op::MpmcChanSend),
            0x9A => code.push(Op::MpmcChanSendBlocking),
            0x9B => code.push(Op::MpmcChanRecv),
            0x9C => code.push(Op::MpmcChanRecvBlocking),
            0x9D => code.push(Op::MpmcChanClose),
            0x9E => code.push(Op::MpmcChanLen),
            0x9F => code.push(Op::MpmcChanIsEmpty),
            0xA0 => code.push(Op::MpmcChanIsFull),
            0x06 => code.push(Op::MpmcChanIsClosed),
            0x07 => code.push(Op::MpmcChanCapacity),
            // C07: Executor-integrated MPMC channel operations
            0xB9 => code.push(Op::MpmcChanSendWithTask),
            0xBA => code.push(Op::MpmcChanRecvWithTask),
            0xBC => code.push(Op::MpmcChanRecvAndWake),
            0xBD => code.push(Op::MpmcChanPopWaitingSender),
            0xBE => code.push(Op::MpmcChanGetWaitingSenderValue),
            0xBF => code.push(Op::MpmcChanPopWaitingReceiver),
            0xC5 => code.push(Op::MpmcChanWaitingSenderCount),
            0xC7 => code.push(Op::MpmcChanWaitingReceiverCount),
            0xC9 => code.push(Op::MpmcChanGetWokenSender),
            // C08: Blocking receive operations
            0xCA => code.push(Op::MpmcChanSendAndWake),
            0xCB => code.push(Op::MpmcChanGetWokenReceiver),
            // C09: Channel Select operations (0x14-0x1D)
            0x14 => code.push(Op::MpmcChanSelectClear),
            0x15 => code.push(Op::MpmcChanSelectAdd),
            0x16 => code.push(Op::MpmcChanSelectCount),
            0x17 => code.push(Op::MpmcChanTrySelectRecv),
            0x18 => code.push(Op::MpmcChanSelectRecvBlocking),
            0x19 => code.push(Op::MpmcChanSelectRecvWithTask),
            0x1A => code.push(Op::MpmcChanSelectGetReadyIndex),
            0x1B => code.push(Op::MpmcChanSelectGetValue),
            0x1C => code.push(Op::MpmcChanSelectDeregister),
            0x1D => code.push(Op::MpmcChanSelectGetHandle),

            // C11: Actor operations (0x08-0x18)
            0x08 => code.push(Op::ActorCreate),
            0x09 => code.push(Op::ActorSpawn),
            0x0A => code.push(Op::ActorSend),
            0x0B => code.push(Op::ActorSendBlocking),
            0x0C => code.push(Op::ActorRecv),
            0x0D => code.push(Op::ActorRecvBlocking),
            0x0E => code.push(Op::ActorClose),
            0x0F => code.push(Op::ActorStop),
            0x1E => code.push(Op::ActorGetTask),
            0x1F => code.push(Op::ActorGetMailbox),
            0x2E => code.push(Op::ActorIsRunning),
            0x2F => code.push(Op::ActorGetState),
            0x3E => code.push(Op::ActorMessageCount),
            0x3F => code.push(Op::ActorMailboxEmpty),
            0x4E => code.push(Op::ActorMailboxLen),
            0x4F => code.push(Op::ActorSetTask),
            0x51 => code.push(Op::ActorIsFailed),
            0x5E => code.push(Op::ActorMarkStopped),
            0x5F => code.push(Op::ActorMarkFailed),
            // C19: Atomic operations (0x52-0x5C)
            0x52 => code.push(Op::AtomicCreate),
            0x53 => code.push(Op::AtomicLoad),
            0x54 => code.push(Op::AtomicStore),
            0x55 => code.push(Op::AtomicCas),
            0x56 => code.push(Op::AtomicFetchAdd),
            0x57 => code.push(Op::AtomicFetchSub),
            0x58 => code.push(Op::AtomicSwap),
            0x59 => code.push(Op::AtomicGet),
            0x5A => code.push(Op::AtomicSet),
            0x5B => code.push(Op::AtomicInc),
            0x5C => code.push(Op::AtomicDec),
            // C21: Event Loop operations (0x32-0x41)
            0x32 => code.push(Op::EventLoopCreate),
            0x33 => code.push(Op::EventLoopRegisterTimer),
            0x34 => code.push(Op::EventLoopRegisterFd),
            0x35 => code.push(Op::EventLoopDeregister),
            0x36 => code.push(Op::EventLoopPoll),
            0x37 => code.push(Op::EventLoopGetEvent),
            0x38 => code.push(Op::EventLoopGetEventType),
            0x39 => code.push(Op::EventLoopClose),
            0x3A => code.push(Op::EventLoopPipeCreate),
            0x3B => code.push(Op::EventLoopPipeGetWriteFd),
            0x3C => code.push(Op::EventLoopPipeWrite),
            0x3D => code.push(Op::EventLoopPipeRead),
            0x41 => code.push(Op::EventLoopPipeClose),
            // C22: Timer operations (0x42-0x49)
            0x42 => code.push(Op::TimerSleep),
            0x43 => code.push(Op::TimerSleepAsync),
            0x44 => code.push(Op::TimerCheckExpired),
            0x45 => code.push(Op::TimerGetWaitingTask),
            0x46 => code.push(Op::TimerCancel),
            0x47 => code.push(Op::TimerPollExpired),
            0x48 => code.push(Op::TimerNow),
            0x49 => code.push(Op::TimerElapsed),
            0x4A => code.push(Op::TimerRemove),
            0x4B => code.push(Op::TimerRemaining),
            // C23: TCP Socket operations (0x6F prefix + sub-opcode)
            0x6F => {
                if off >= bytes.len() {
                    return Err(DecodeError::new(
                        DecodeErrorKind::UnexpectedEof {
                            expected: "TCP sub-opcode",
                        },
                        DecodePhase::Instructions,
                        off,
                        bytes,
                    )
                    .to_string());
                }
                let sub = bytes[off];
                off += 1;
                match sub {
                    0x00 => code.push(Op::TcpListenerBind),
                    0x01 => code.push(Op::TcpListenerAccept),
                    0x02 => code.push(Op::TcpListenerAcceptAsync),
                    0x03 => code.push(Op::TcpListenerClose),
                    0x04 => code.push(Op::TcpListenerLocalPort),
                    0x05 => code.push(Op::TcpStreamConnect),
                    0x06 => code.push(Op::TcpStreamConnectAsync),
                    0x07 => code.push(Op::TcpStreamRead),
                    0x08 => code.push(Op::TcpStreamReadAsync),
                    0x09 => code.push(Op::TcpStreamWrite),
                    0x0A => code.push(Op::TcpStreamWriteAsync),
                    0x0B => code.push(Op::TcpStreamClose),
                    0x0C => code.push(Op::TcpStreamGetLastRead),
                    0x0D => code.push(Op::TcpStreamSetTimeout),
                    0x0E => code.push(Op::TcpCheckReady),
                    0x0F => code.push(Op::TcpGetResult),
                    0x10 => code.push(Op::TcpPollReady),
                    0x11 => code.push(Op::TcpRemoveRequest),
                    _ => {
                        return Err(DecodeError::new(
                            DecodeErrorKind::UnknownSubOpcode {
                                category: "TCP",
                                opcode: 0x6F,
                                sub_opcode: sub,
                            },
                            DecodePhase::Instructions,
                            off - 1,
                            bytes,
                        )
                        .to_string());
                    }
                }
            }
            // C24: HTTP Client operations (0x7F prefix + sub-opcode)
            0x7F => {
                if off >= bytes.len() {
                    return Err(DecodeError::new(
                        DecodeErrorKind::UnexpectedEof {
                            expected: "HTTP client sub-opcode",
                        },
                        DecodePhase::Instructions,
                        off,
                        bytes,
                    )
                    .to_string());
                }
                let sub = bytes[off];
                off += 1;
                match sub {
                    0x00 => code.push(Op::HttpGet),
                    0x01 => code.push(Op::HttpPost),
                    0x02 => code.push(Op::HttpGetAsync),
                    0x03 => code.push(Op::HttpPostAsync),
                    0x04 => code.push(Op::HttpResponseStatus),
                    0x05 => code.push(Op::HttpResponseHeader),
                    0x06 => code.push(Op::HttpResponseBody),
                    0x07 => code.push(Op::HttpResponseClose),
                    0x08 => code.push(Op::HttpCheckReady),
                    0x09 => code.push(Op::HttpGetResult),
                    0x0A => code.push(Op::HttpPollReady),
                    0x0B => code.push(Op::HttpRemoveRequest),
                    0x0C => code.push(Op::HttpGetBodyLength),
                    0x0D => code.push(Op::HttpGetHeaderCount),
                    // Struct extension operations (0x20+)
                    0x20 => code.push(Op::StructSetNamed),
                    // JSON accessor operations (0x30-0x39)
                    0x30 => code.push(Op::JsonGetField),
                    0x31 => code.push(Op::JsonGetIndex),
                    0x32 => code.push(Op::JsonGetString),
                    0x33 => code.push(Op::JsonGetNumber),
                    0x34 => code.push(Op::JsonGetBool),
                    0x35 => code.push(Op::JsonIsNull),
                    0x36 => code.push(Op::JsonIsObject),
                    0x37 => code.push(Op::JsonIsArray),
                    0x38 => code.push(Op::JsonArrayLen),
                    0x39 => code.push(Op::JsonKeys),
                    _ => {
                        return Err(DecodeError::new(
                            DecodeErrorKind::UnknownSubOpcode {
                                category: "HTTP client",
                                opcode: 0x7F,
                                sub_opcode: sub,
                            },
                            DecodePhase::Instructions,
                            off - 1,
                            bytes,
                        )
                        .to_string());
                    }
                }
            }
            // C25: HTTP Server operations (0x5D prefix + sub-opcode)
            0x5D => {
                if off >= bytes.len() {
                    return Err(DecodeError::new(
                        DecodeErrorKind::UnexpectedEof {
                            expected: "HTTP server sub-opcode",
                        },
                        DecodePhase::Instructions,
                        off,
                        bytes,
                    )
                    .to_string());
                }
                let sub = bytes[off];
                off += 1;
                match sub {
                    0x00 => code.push(Op::HttpServerCreate),
                    0x01 => code.push(Op::HttpServerClose),
                    0x02 => code.push(Op::HttpServerGetPort),
                    0x03 => code.push(Op::HttpServerAccept),
                    0x04 => code.push(Op::HttpServerAcceptAsync),
                    0x05 => code.push(Op::HttpRequestMethod),
                    0x06 => code.push(Op::HttpRequestPath),
                    0x07 => code.push(Op::HttpRequestHeader),
                    0x08 => code.push(Op::HttpRequestBody),
                    0x09 => code.push(Op::HttpRequestHeaderCount),
                    0x0A => code.push(Op::HttpRequestBodyLength),
                    0x0B => code.push(Op::HttpWriterStatus),
                    0x0C => code.push(Op::HttpWriterHeader),
                    0x0D => code.push(Op::HttpWriterBody),
                    0x0E => code.push(Op::HttpWriterSend),
                    0x0F => code.push(Op::HttpWriterSendAsync),
                    0x10 => code.push(Op::HttpServerCheckReady),
                    0x11 => code.push(Op::HttpServerGetResult),
                    0x12 => code.push(Op::HttpServerPollReady),
                    0x13 => code.push(Op::HttpServerRemoveRequest),
                    _ => {
                        return Err(DecodeError::new(
                            DecodeErrorKind::UnknownSubOpcode {
                                category: "HTTP server",
                                opcode: 0x5D,
                                sub_opcode: sub,
                            },
                            DecodePhase::Instructions,
                            off - 1,
                            bytes,
                        )
                        .to_string());
                    }
                }
            }
            // String operations (0x4C prefix + sub-opcode)
            0x4C => {
                if off >= bytes.len() {
                    return Err(DecodeError::new(
                        DecodeErrorKind::UnexpectedEof {
                            expected: "String sub-opcode",
                        },
                        DecodePhase::Instructions,
                        off,
                        bytes,
                    )
                    .to_string());
                }
                let sub = bytes[off];
                off += 1;
                match sub {
                    0x00 => code.push(Op::StrLen),
                    0x01 => code.push(Op::StrSubstring),
                    0x02 => code.push(Op::StrIndexOf),
                    0x03 => code.push(Op::StrLastIndexOf),
                    0x04 => code.push(Op::StrStartsWith),
                    0x05 => code.push(Op::StrEndsWith),
                    0x06 => code.push(Op::StrSplit),
                    0x07 => code.push(Op::StrTrim),
                    0x08 => code.push(Op::StrToLower),
                    0x09 => code.push(Op::StrToUpper),
                    0x0A => code.push(Op::StrReplace),
                    0x0B => code.push(Op::StrCharAt),
                    0x0C => code.push(Op::StrContains),
                    0x0D => code.push(Op::StrRepeat),
                    0x0E => code.push(Op::StrParseInt),
                    0x0F => code.push(Op::StrParseFloat),
                    0x10 => code.push(Op::StrFromInt),
                    0x11 => code.push(Op::StrFromFloat),
                    _ => {
                        return Err(DecodeError::new(
                            DecodeErrorKind::UnknownSubOpcode {
                                category: "String",
                                opcode: 0x4C,
                                sub_opcode: sub,
                            },
                            DecodePhase::Instructions,
                            off - 1,
                            bytes,
                        )
                        .to_string());
                    }
                }
            }
            // This catch-all ensures we get a proper error for unknown opcodes
            // if the match becomes non-exhaustive in the future (e.g., new opcodes added)
            #[allow(unreachable_patterns)]
            _ => {
                return Err(DecodeError::new(
                    DecodeErrorKind::UnknownOpcode { opcode: op },
                    DecodePhase::Instructions,
                    op_offset,
                    bytes,
                )
                .to_string());
            }
        }
    }

    // Decode async dispatch table (if present for backward compatibility)
    let mut async_dispatch = Vec::new();
    if off < bytes.len() {
        let async_count = read_u32_le(bytes, &mut off)? as usize;
        for _ in 0..async_count {
            if off + 12 > bytes.len() {
                return Err(DecodeError::new(
                    DecodeErrorKind::UnexpectedEof {
                        expected: "async dispatch entry (12 bytes)",
                    },
                    DecodePhase::AsyncDispatch,
                    off,
                    bytes,
                )
                .to_string());
            }
            let mut fn_id_bytes = [0u8; 8];
            fn_id_bytes.copy_from_slice(&bytes[off..off + 8]);
            let fn_id = i64::from_le_bytes(fn_id_bytes);
            off += 8;
            let offset = read_u32_le(bytes, &mut off)?;
            async_dispatch.push((fn_id, offset));
        }
    }

    // Decode debug entries (if present for backward compatibility)
    let mut debug_entries = Vec::new();
    if off < bytes.len() {
        let debug_count = read_u32_le(bytes, &mut off)? as usize;
        for _ in 0..debug_count {
            // Read offset
            let entry_offset = read_u32_le(bytes, &mut off)?;

            // Read function name
            let name_len = read_u32_le(bytes, &mut off)? as usize;
            if off + name_len > bytes.len() {
                return Err(DecodeError::new(
                    DecodeErrorKind::UnexpectedEof {
                        expected: "debug entry function name",
                    },
                    DecodePhase::DebugInfo,
                    off,
                    bytes,
                )
                .to_string());
            }
            let function_name =
                String::from_utf8(bytes[off..off + name_len].to_vec()).map_err(|e| {
                    DecodeError::new(
                        DecodeErrorKind::InvalidUtf8 {
                            message: e.to_string(),
                        },
                        DecodePhase::DebugInfo,
                        off,
                        bytes,
                    )
                    .to_string()
                })?;
            off += name_len;

            // Read source file (0 length means none)
            let file_len = read_u32_le(bytes, &mut off)? as usize;
            let source_file = if file_len > 0 {
                if off + file_len > bytes.len() {
                    return Err(DecodeError::new(
                        DecodeErrorKind::UnexpectedEof {
                            expected: "debug entry source file",
                        },
                        DecodePhase::DebugInfo,
                        off,
                        bytes,
                    )
                    .to_string());
                }
                let file = String::from_utf8(bytes[off..off + file_len].to_vec()).map_err(|e| {
                    DecodeError::new(
                        DecodeErrorKind::InvalidUtf8 {
                            message: e.to_string(),
                        },
                        DecodePhase::DebugInfo,
                        off,
                        bytes,
                    )
                    .to_string()
                })?;
                off += file_len;
                Some(file)
            } else {
                None
            };

            // Read line number
            let line = read_u32_le(bytes, &mut off)?;

            debug_entries.push(DebugEntry {
                offset: entry_offset,
                function_name,
                source_file,
                line,
            });
        }
    }

    Ok(Program {
        strings,
        code,
        async_dispatch,
        debug_entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_bytecode_header() {
        // Valid bytecode with version 01
        let bytes = b"ARTHBC01\x00\x00\x00\x00\x00\x00\x00\x00\xFF";
        let result = validate_bytecode_header(bytes);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1);
    }

    #[test]
    fn test_bytecode_too_short() {
        let bytes = b"ARTH";
        let result = validate_bytecode_header(bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too short"));
    }

    #[test]
    fn test_invalid_magic_prefix() {
        let bytes = b"INVALID0\x00\x00\x00\x00";
        let result = validate_bytecode_header(bytes);
        assert!(result.is_err());
        // New error format includes "invalid magic header"
        assert!(result.unwrap_err().contains("invalid magic header"));
    }

    #[test]
    fn test_library_file_rejected() {
        let bytes = b"ARTHLIB1\x00\x00\x00\x00";
        let result = validate_bytecode_header(bytes);
        assert!(result.is_err());
        let err = result.unwrap_err();
        // New error format reports file type detection
        assert!(err.contains("library") || err.contains("wrong file type"));
    }

    #[test]
    fn test_elf_file_rejected() {
        let bytes = b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00";
        let result = validate_bytecode_header(bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ELF binary"));
    }

    #[test]
    fn test_get_bytecode_info() {
        let bytes = b"ARTHBC01\x00\x00\x00\x00\x00\x00\x00\x00\xFF";
        let info = get_bytecode_info(bytes).unwrap();
        assert_eq!(info.version, 1);
        assert!(info.supported);
        assert_eq!(info.min_supported, MIN_SUPPORTED_VERSION);
        assert_eq!(info.max_supported, MAX_SUPPORTED_VERSION);
    }

    #[test]
    fn test_is_valid_bytecode() {
        assert!(is_valid_bytecode(
            b"ARTHBC01\x00\x00\x00\x00\x00\x00\x00\x00\xFF"
        ));
        assert!(!is_valid_bytecode(b"INVALID0"));
        assert!(!is_valid_bytecode(b"ARTH")); // too short
    }

    #[test]
    fn test_extract_version() {
        assert_eq!(extract_version(b"ARTHBC01"), Some(1));
        assert_eq!(extract_version(b"ARTHBC09"), Some(9));
        assert_eq!(extract_version(b"ARTHBC12"), Some(12));
        assert_eq!(extract_version(b"ARTHBC99"), Some(99));
        assert_eq!(extract_version(b"INVALID0"), None);
        assert_eq!(extract_version(b"ARTH"), None); // too short
    }

    #[test]
    fn test_bytecode_version_info_current() {
        let info = BytecodeVersionInfo::current();
        assert_eq!(info.version, BYTECODE_VERSION);
        assert!(info.supported);
    }

    #[test]
    fn test_magic_header_tracks_bytecode_version() {
        let expected = [
            b'A',
            b'R',
            b'T',
            b'H',
            b'B',
            b'C',
            b'0' + (BYTECODE_VERSION / 10),
            b'0' + (BYTECODE_VERSION % 10),
        ];
        assert_eq!(MAGIC, expected);
        assert_eq!(extract_version(&MAGIC), Some(BYTECODE_VERSION));
    }

    #[test]
    fn test_bytecode_version_too_new_rejected() {
        let bytes = b"ARTHBC02".to_vec();
        let result = validate_bytecode_header(&bytes);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("unsupported bytecode version"));
        assert!(err.contains("Found version: 2"));
    }

    #[test]
    fn test_bytecode_version_too_old_rejected() {
        let bytes = b"ARTHBC00".to_vec();
        let result = validate_bytecode_header(&bytes);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("unsupported bytecode version"));
        assert!(err.contains("Found version: 0"));
    }

    #[test]
    fn test_decode_error_detailed_provides_error_for_unknown_opcode() {
        // Create bytecode with an opcode not handled by decode_program_detailed
        // decode_program_detailed only handles a subset, so 0x4D will trigger error
        let mut bytes = b"ARTHBC01".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0]); // 0 strings
        bytes.extend_from_slice(&[1, 0, 0, 0]); // 1 instruction
        bytes.push(0x4D); // opcode not in decode_program_detailed's subset

        let result = decode_program_detailed(&bytes);
        assert!(result.is_err());

        let err = result.unwrap_err();
        // Verify error includes key information
        let err_string = err.to_string();
        assert!(err_string.contains("decode error"));
        assert!(err_string.contains("instruction decoding"));
        assert!(err_string.contains("0x4d"));
        // Should mention that 0x4D is ASCII 'M'
        assert!(err_string.contains("'M'"));
    }

    #[test]
    fn test_decode_error_detailed_provides_hex_dump() {
        // Create bytecode with corrupted data
        let mut bytes = b"ARTHBC01".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0]); // 0 strings
        bytes.extend_from_slice(&[1, 0, 0, 0]); // 1 instruction
        bytes.push(0x4D); // opcode not in decode_program_detailed's subset

        let result = decode_program_detailed(&bytes);
        assert!(result.is_err());

        let err = result.unwrap_err();
        // Verify DecodeError has expected properties
        assert_eq!(err.phase, DecodePhase::Instructions);
        assert!(err.offset > 0); // offset should point to the opcode location
        assert!(!err.context_bytes.is_empty()); // should have context bytes

        // Verify Display includes hex dump
        let err_string = err.to_string();
        assert!(err_string.contains("Context"));
    }

    #[test]
    fn test_decode_error_suggestion_for_ascii_opcode() {
        // Create error with context bytes that include the error position
        let test_bytes = b"ARTHBC01test data here and more";
        let err = DecodeError::new(
            DecodeErrorKind::UnknownOpcode { opcode: 0x4D }, // 'M'
            DecodePhase::Instructions,
            10, // offset within the test_bytes range
            test_bytes,
        );

        let suggestion = err.suggestion();
        assert!(suggestion.is_some());
        // Should suggest that ASCII letter means corrupted/wrong input
        let sugg = suggestion.unwrap();
        assert!(sugg.contains("ASCII") || sugg.contains("source") || sugg.contains("bytecode"));
    }

    #[test]
    fn test_decode_error_eof_includes_expected() {
        let test_bytes = b"some data";
        let err = DecodeError::new(
            DecodeErrorKind::UnexpectedEof {
                expected: "u32 value",
            },
            DecodePhase::StringTable,
            5,
            test_bytes,
        );

        let err_string = err.to_string();
        assert!(err_string.contains("unexpected end of bytecode"));
        assert!(err_string.contains("u32 value"));
        assert!(err_string.contains("string table"));
    }

    #[test]
    fn test_decode_error_nearby_opcodes() {
        // Test that nearby_opcodes returns opcodes within range
        let nearby = DecodeError::nearby_opcodes(0x71); // Ret opcode
        // Should find opcodes like 0x70 (Call), 0x72 (StructNew), etc.
        assert!(!nearby.is_empty());
        // All returned opcodes should be within ±5 of 0x71
        for (op, _) in nearby.iter() {
            let diff = (*op as i16 - 0x71_i16).abs();
            assert!(diff <= 5);
        }
    }

    #[test]
    fn test_decode_program_invalid_magic_uses_decode_error() {
        // Verify that decode_program returns detailed error for invalid magic
        let bytes = b"NOTARTH0\x00\x00\x00\x00";
        let result = decode_program(bytes);
        assert!(result.is_err());

        let err = result.unwrap_err();
        // Should contain detailed error information
        assert!(err.contains("invalid magic header") || err.contains("decode error"));
    }

    #[test]
    fn test_decode_program_truncated_uses_decode_error() {
        // Verify that decode_program returns detailed error for truncated bytecode
        let mut bytes = b"ARTHBC01".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0]); // 0 strings
        bytes.extend_from_slice(&[1, 0, 0, 0]); // claims 1 instruction
        // But no instruction data - truncated!

        let result = decode_program(&bytes);
        assert!(result.is_err());

        let err = result.unwrap_err();
        // Should indicate unexpected EOF
        assert!(err.contains("unexpected") || err.contains("EOF") || err.contains("truncated"));
    }

    /// Comprehensive test to prevent encoder/decoder version drift.
    /// This test creates one instance of each encodable Op variant and verifies
    /// it survives encode/decode round-trip.
    ///
    /// If this test fails after adding a new Op variant, you need to:
    /// 1. Add encoding logic in `encode_program`
    /// 2. Add decoding logic in `decode_program`
    /// 3. Add the new Op to this test's `all_ops` vector
    /// 4. Update KNOWN_OPCODES in the DecodeError section
    ///
    /// Note: HostCallDb/HostCallMail/HostCallCrypto are encoded via host-call extension tags.
    /// in the bytecode format. They are excluded from this test.
    ///
    /// Future improvement: Use strum's EnumIter derive to automatically
    /// iterate all Op variants and test them.
    #[test]
    fn test_all_ops_encode_decode_roundtrip() {
        use crate::ops::{HostIoOp, HostNetOp, HostTimeOp};

        // Create one instance of every encodable Op variant
        // This list must be updated when new Op variants are added
        let all_ops: Vec<Op> = vec![
            // Print operations
            Op::Print(0),
            Op::PrintStrVal(0),
            Op::PrintRaw(0),
            Op::PrintRawStrVal(0),
            Op::PrintLn,
            Op::Halt,
            // Stack operations
            Op::PushI64(42),
            Op::PushF64(3.14),
            Op::PushBool(1),
            Op::PushStr(0),
            // Arithmetic
            Op::AddI64,
            Op::SubI64,
            Op::MulI64,
            Op::DivI64,
            Op::ModI64,
            Op::LtI64,
            Op::EqI64,
            Op::EqStr,
            Op::ConcatStr,
            Op::ShlI64,
            Op::ShrI64,
            Op::AndI64,
            Op::OrI64,
            Op::XorI64,
            // Control flow
            Op::Jump(10),
            Op::JumpIfFalse(20),
            Op::Call(5),
            Op::CallSymbol(0),
            Op::ExternCall {
                sym: 0,
                argc: 1,
                float_mask: 0,
                ret_kind: 0,
            },
            Op::Ret,
            Op::Pop,
            Op::PrintTop,
            // Locals
            Op::LocalGet(0),
            Op::LocalSet(0),
            // Conversions
            Op::ToF64,
            Op::ToI64,
            Op::ToI64OrEnumTag,
            Op::ToBool,
            Op::ToChar,
            Op::ToI8,
            Op::ToI16,
            Op::ToI32,
            Op::ToU8,
            Op::ToU16,
            Op::ToU32,
            Op::ToU64,
            Op::ToF32,
            // Float math
            Op::SqrtF64,
            Op::PowF64,
            Op::SinF64,
            Op::CosF64,
            Op::TanF64,
            Op::FloorF64,
            Op::CeilF64,
            Op::RoundF64,
            // Lists
            Op::ListNew,
            Op::ListPush,
            Op::ListGet,
            Op::ListSet,
            Op::ListLen,
            Op::ListRemove,
            Op::ListSort,
            // Maps
            Op::MapNew,
            Op::MapPut,
            Op::MapGet,
            Op::MapLen,
            Op::MapContainsKey,
            Op::MapRemove,
            Op::MapKeys,
            Op::MapMerge,
            // Strings
            Op::StrLen,
            Op::StrSubstring,
            Op::StrIndexOf,
            Op::StrLastIndexOf,
            Op::StrStartsWith,
            Op::StrEndsWith,
            Op::StrSplit,
            Op::StrTrim,
            Op::StrToLower,
            Op::StrToUpper,
            Op::StrReplace,
            Op::StrCharAt,
            Op::StrContains,
            Op::StrRepeat,
            Op::StrParseInt,
            Op::StrParseFloat,
            Op::StrFromInt,
            Op::StrFromFloat,
            // Optionals
            Op::OptSome,
            Op::OptNone,
            Op::OptIsSome,
            Op::OptUnwrap,
            Op::OptOrElse,
            // Structs
            Op::StructNew,
            Op::StructSet,
            Op::StructGet,
            Op::StructGetNamed,
            Op::StructSetNamed,
            Op::StructCopy,
            Op::StructTypeName,
            Op::StructFieldCount,
            // Enums
            Op::EnumNew,
            Op::EnumSetPayload,
            Op::EnumGetPayload,
            Op::EnumGetTag,
            Op::EnumGetVariant,
            Op::EnumTypeName,
            // JSON
            Op::JsonStringify,
            Op::JsonParse,
            Op::StructToJson,
            Op::JsonToStruct,
            // HTML
            Op::HtmlParse,
            Op::HtmlParseFragment,
            Op::HtmlStringify,
            Op::HtmlStringifyPretty,
            Op::HtmlFree,
            Op::HtmlNodeType,
            Op::HtmlTagName,
            Op::HtmlTextContent,
            Op::HtmlInnerHtml,
            Op::HtmlOuterHtml,
            Op::HtmlGetAttr,
            Op::HtmlHasAttr,
            Op::HtmlAttrNames,
            Op::HtmlParent,
            Op::HtmlChildren,
            Op::HtmlElementChildren,
            Op::HtmlFirstChild,
            Op::HtmlLastChild,
            Op::HtmlNextSibling,
            Op::HtmlPrevSibling,
            Op::HtmlQuerySelector,
            Op::HtmlQuerySelectorAll,
            Op::HtmlGetById,
            Op::HtmlGetByTag,
            Op::HtmlGetByClass,
            Op::HtmlHasClass,
            // Templates
            Op::TemplateCompile,
            Op::TemplateCompileFile,
            Op::TemplateRender,
            Op::TemplateRegisterPartial,
            Op::TemplateGetPartial,
            Op::TemplateUnregisterPartial,
            Op::TemplateFree,
            Op::TemplateEscapeHtml,
            Op::TemplateUnescapeHtml,
            // Shared memory
            Op::SharedNew,
            Op::SharedStore,
            Op::SharedLoad,
            Op::SharedGetByName(0),
            // Closures
            Op::ClosureNew(0, 0),
            Op::ClosureCapture,
            Op::ClosureCall(0),
            // Reference counting
            Op::RcAlloc,
            Op::RcInc,
            Op::RcDec,
            Op::RcDecWithDeinit(0),
            Op::RcLoad,
            Op::RcStore,
            Op::RcGetCount,
            // Regions
            Op::RegionEnter(0),
            Op::RegionExit(0),
            // Panic/unwind
            Op::Panic(0),
            Op::SetUnwindHandler(0),
            Op::ClearUnwindHandler,
            Op::GetPanicMessage,
            // Exceptions
            Op::Throw,
            Op::GetException,
            // Host calls
            Op::HostCallIo(HostIoOp::FileExists),
            Op::HostCallNet(HostNetOp::HttpFetch),
            Op::HostCallTime(HostTimeOp::DateTimeNow),
            Op::HostCallDb(HostDbOp::SqliteOpen),
            Op::HostCallMail(HostMailOp::SmtpConnect),
            Op::HostCallCrypto(HostCryptoOp::Hash),
            Op::HostCallGeneric,
            // Note: BigDecimal ops are not yet encodable in bytecode format
            // TCP
            Op::TcpListenerBind,
            Op::TcpListenerAccept,
            Op::TcpListenerAcceptAsync,
            Op::TcpListenerClose,
            Op::TcpListenerLocalPort,
            Op::TcpStreamConnect,
            Op::TcpStreamConnectAsync,
            Op::TcpStreamRead,
            Op::TcpStreamReadAsync,
            Op::TcpStreamWrite,
            Op::TcpStreamWriteAsync,
            Op::TcpStreamClose,
            Op::TcpStreamGetLastRead,
            Op::TcpStreamSetTimeout,
            Op::TcpCheckReady,
            Op::TcpGetResult,
            Op::TcpPollReady,
            Op::TcpRemoveRequest,
            // HTTP client
            Op::HttpGet,
            Op::HttpPost,
            Op::HttpGetAsync,
            Op::HttpPostAsync,
            Op::HttpResponseStatus,
            Op::HttpResponseHeader,
            Op::HttpResponseBody,
            Op::HttpResponseClose,
            Op::HttpCheckReady,
            Op::HttpGetResult,
            Op::HttpPollReady,
            Op::HttpRemoveRequest,
            Op::HttpGetBodyLength,
            Op::HttpGetHeaderCount,
            // HTTP server
            Op::HttpServerCreate,
            Op::HttpServerClose,
            Op::HttpServerGetPort,
            Op::HttpServerAccept,
            Op::HttpServerAcceptAsync,
            Op::HttpRequestMethod,
            Op::HttpRequestPath,
            Op::HttpRequestHeader,
            Op::HttpRequestBody,
            Op::HttpRequestHeaderCount,
            Op::HttpRequestBodyLength,
            Op::HttpWriterStatus,
            Op::HttpWriterHeader,
            Op::HttpWriterBody,
            Op::HttpWriterSend,
            Op::HttpWriterSendAsync,
            Op::HttpServerCheckReady,
            Op::HttpServerGetResult,
            Op::HttpServerPollReady,
            Op::HttpServerRemoveRequest,
        ];

        // Test each op individually for easier debugging
        for (i, op) in all_ops.iter().enumerate() {
            let program = crate::Program::new(vec!["test".to_string()], vec![op.clone(), Op::Halt]);
            let encoded = encode_program(&program);
            let decoded = decode_program(&encoded);

            match decoded {
                Ok(p) => {
                    assert_eq!(
                        p.code.first(),
                        Some(op),
                        "Op #{} {:?} failed roundtrip: decoded as {:?}",
                        i,
                        op,
                        p.code.first()
                    );
                }
                Err(e) => {
                    panic!(
                        "Op #{} {:?} failed to decode: {}\nEncoded bytes: {:02x?}",
                        i, op, e, encoded
                    );
                }
            }
        }

        // Also test all ops together as a program
        let combined_program = crate::Program::new(
            vec!["combined_test".to_string()],
            all_ops
                .iter()
                .cloned()
                .chain(std::iter::once(Op::Halt))
                .collect(),
        );
        let encoded = encode_program(&combined_program);
        let decoded = decode_program(&encoded).expect("combined program decode failed");
        assert_eq!(
            decoded.code.len(),
            combined_program.code.len(),
            "Code length mismatch in combined roundtrip"
        );
    }
}
