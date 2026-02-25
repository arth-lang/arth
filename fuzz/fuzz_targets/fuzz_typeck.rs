//! Type checker fuzzer - tests the full frontend pipeline
//!
//! This fuzzer targets the type checker to find:
//! - Panics during type checking
//! - Infinite loops in type inference
//! - Memory issues
//! - Incorrect error handling
//!
//! Run with: cargo +nightly fuzz run fuzz_typeck

#![no_main]

use arth::compiler::diagnostics::Reporter;
use arth::compiler::parser::parse_file;
use arth::compiler::resolve::resolve_project;
use arth::compiler::source::SourceFile;
use arth::compiler::typeck::typecheck_project;
use libfuzzer_sys::fuzz_target;
use std::path::Path;

fuzz_target!(|data: &[u8]| {
    // Only process valid UTF-8
    if let Ok(source) = std::str::from_utf8(data) {
        // Limit input size to prevent timeout
        if source.len() > 5_000 {
            return;
        }

        // Add package declaration if missing to make it valid for resolution
        let source = if !source.contains("package ") {
            format!("package fuzz;\n{}", source)
        } else {
            source.to_string()
        };

        let sf = SourceFile {
            path: std::path::PathBuf::from("fuzz.arth"),
            text: source,
        };

        let mut rep = Reporter::new();
        let ast = parse_file(&sf, &mut rep);

        // Skip if there were parse errors
        if rep.has_errors() {
            return;
        }

        let files = vec![(sf, ast)];
        let mut resolve_rep = Reporter::new();
        let resolved = resolve_project(Path::new("/fuzz"), &files, &mut resolve_rep);

        let mut type_rep = Reporter::new();
        typecheck_project(Path::new("/fuzz"), &files, &resolved, &mut type_rep);
        // Type errors are expected for fuzzed input, panics are not
    }
});
