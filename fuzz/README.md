# Arth Fuzzing Infrastructure

This directory contains fuzzing targets for the Arth compiler using `cargo-fuzz` and `libfuzzer`.

## Prerequisites

1. Install cargo-fuzz (requires nightly Rust):
   ```bash
   cargo install cargo-fuzz
   ```

2. Switch to nightly Rust:
   ```bash
   rustup default nightly
   # or use rustup override for this directory
   rustup override set nightly
   ```

## Fuzz Targets

### fuzz_lexer
Tests the lexer with arbitrary byte inputs. Finds panics, memory issues, and infinite loops.

```bash
cargo +nightly fuzz run fuzz_lexer
```

### fuzz_parser
Tests the parser with arbitrary source code. Finds stack overflows, parser recovery failures.

```bash
cargo +nightly fuzz run fuzz_parser
```

### fuzz_typeck
Tests the full frontend pipeline (lexer → parser → HIR → resolver → typechecker).

```bash
cargo +nightly fuzz run fuzz_typeck
```

### fuzz_structured
Uses `arbitrary` crate to generate syntactically plausible Arth programs.

```bash
cargo +nightly fuzz run fuzz_structured
```

## Corpus

Initial seed inputs are in `corpus/`:
- `corpus/lexer/`: Basic token sequences
- `corpus/parser/`: Valid program structures
- `corpus/typeck/`: Programs for type checking

The fuzzer will automatically expand the corpus with interesting inputs.

## Running Fuzzing

Run for a specific duration:
```bash
cargo +nightly fuzz run fuzz_lexer -- -max_total_time=3600  # 1 hour
```

Use multiple cores:
```bash
cargo +nightly fuzz run fuzz_lexer --jobs 4
```

Check coverage:
```bash
cargo +nightly fuzz coverage fuzz_lexer
```

## Triage Crashes

Crashes are saved in `artifacts/fuzz_<target>/`. To reproduce:
```bash
cargo +nightly fuzz run fuzz_lexer artifacts/fuzz_lexer/crash-<hash>
```

To minimize a crash:
```bash
cargo +nightly fuzz tmin fuzz_lexer artifacts/fuzz_lexer/crash-<hash>
```

## CI Integration

Add to CI workflow:
```yaml
- name: Fuzz for regressions
  run: |
    cargo +nightly fuzz run fuzz_lexer -- -max_total_time=60
    cargo +nightly fuzz run fuzz_parser -- -max_total_time=60
```

## Performance Targets

The fuzzer should process:
- Lexer: ~10,000 inputs/second for small inputs
- Parser: ~1,000 inputs/second for small programs
- Typechecker: ~100 inputs/second for valid programs

Track these metrics to detect performance regressions.
