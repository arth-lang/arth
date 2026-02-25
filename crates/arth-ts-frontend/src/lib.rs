//! TypeScript → Arth bytecode front-end.
//!
//! This crate uses SWC to parse a restricted TypeScript subset and
//! compiles it to Arth VM bytecode. It provides both low-level APIs
//! (TS→HIR lowering) and high-level APIs (TS→bytecode compilation).
//!
//! # Quick Start
//!
//! ```ignore
//! use arth_ts_frontend::compile_ts_file;
//!
//! let result = compile_ts_file(Path::new("controller.ts"))?;
//! std::fs::write("controller.abc", &result.bytecode)?;
//! ```

mod compile;
mod controller;
mod diagnostics;
mod emit;
mod lower;
mod package;
mod symbol;
mod validate;
mod vm_meta;

// Primary API: complete TS → bytecode compilation
pub use crate::compile::{
    TsCompileError, TsCompileResult, compile_ts_file, compile_ts_project, compile_ts_string,
};

// Controller metadata extraction (for WAID IR generation)
pub use crate::controller::{
    ControllerError, ControllerMeta, ControllerMethod, StateVar, extract_controller_meta,
    extract_controller_meta_from_source,
};

// Low-level API: TS → HIR only (for advanced use cases)
pub use crate::lower::{
    TsLoweringError, TsLoweringOptions, lower_ts_project_to_hir, lower_ts_str_to_hir,
};

// Package manifest types
pub use crate::package::{
    TsGuestExport, TsGuestImport, TsGuestImportKind, TsGuestManifest, load_ts_guest_package,
    write_ts_guest_package,
};

// Symbol table and export table types
pub use crate::symbol::{
    ArthImport, CrossFileResolver, ExportEntry, ExportTable, Symbol, SymbolKind, SymbolTable,
    generate_arth_imports,
};

// Validation API
pub use crate::validate::{
    ValidationResult, validate_ts_subset, validate_ts_subset_with_diagnostics,
};

// Diagnostics for rich error reporting
pub use crate::diagnostics::{
    Diagnostic, DiagnosticBag, DiagnosticCategory, DiagnosticLevel, SourceSpan,
};

// Arth source code emitter
pub use crate::emit::{
    ArthEmitter, EmitConfig, EmitResult, emit_arth_source, emit_arth_source_with_config,
};

// VM integration metadata
pub use crate::vm_meta::{
    ControllerInfo, ControllerRegistry, HandlerInfo, ParamInfo, ProviderFieldInfo, ProviderInfo,
    VM_META_SCHEMA_VERSION, deserialize_controller_registry, extract_controller_registry,
    serialize_controller_registry,
};

// Re-export Program for convenience (users don't need to depend on arth-vm)
pub use arth_vm::Program;
