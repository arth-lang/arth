//! Parser fuzzer - tests the parser with arbitrary source code
//!
//! This fuzzer targets the parser to find:
//! - Panics on malformed input
//! - Stack overflows from deeply nested constructs
//! - Memory issues
//! - Parser recovery failures
//!
//! Run with: cargo +nightly fuzz run fuzz_parser

#![no_main]

use arth::compiler::diagnostics::Reporter;
use arth::compiler::parser::parse_file;
use arth::compiler::source::SourceFile;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Only process valid UTF-8
    if let Ok(source) = std::str::from_utf8(data) {
        // Limit input size to prevent timeout
        if source.len() > 10_000 {
            return;
        }

        let sf = SourceFile {
            path: std::path::PathBuf::from("fuzz.arth"),
            text: source.to_string(),
        };

        let mut reporter = Reporter::new();
        let _ast = parse_file(&sf, &mut reporter);
        // Errors are expected for malformed input, panics are not
    }
});
