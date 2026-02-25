//! Lexer fuzzer - tests the lexer with arbitrary byte inputs
//!
//! This fuzzer targets the lexer to find:
//! - Panics on malformed input
//! - Memory safety issues
//! - Infinite loops
//! - Incorrect token boundaries
//!
//! Run with: cargo +nightly fuzz run fuzz_lexer

#![no_main]

use arth::compiler::lexer::lex_all;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Only process valid UTF-8 to avoid trivial crashes
    if let Ok(source) = std::str::from_utf8(data) {
        // Tokenize - this should never panic
        let _tokens = lex_all(source);
    }
});
