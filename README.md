# Arth

Compiler infrastructure built around a typed HIR substrate. Multiple frontends, multiple backends, one IR.

> **Status:** Developer preview — core pipeline works, expanding frontend/backend coverage.

## What is Arth

Arth is a compiler platform centered on a high-level intermediate representation (HIR) that serves as a shared substrate for multiple source languages and compilation targets. The Arth language itself — a systems language with deterministic memory management and no garbage collector — is one frontend. TypeScript is another. Code from any frontend lowers through the same HIR and SSA-based IR before being emitted to any supported backend.

## Architecture

```
                 ┌─────────────────┐
  Frontends      │   Arth (.arth)  │    TypeScript (.ts)
                 └────────┬────────┘    ─────────┬───────
                          │                      │
                          ▼                      ▼
                 ┌─────────────────────────────────────┐
                 │          AST → HIR Lowering         │
                 │  (desugar, normalize, assign HirIds) │
                 └──────────────────┬──────────────────┘
                                    │
                          ┌─────────▼─────────┐
                          │   Name Resolution  │
                          │   Type Checking    │
                          │   Ownership/Borrow │
                          └─────────┬─────────┘
                                    │
                          ┌─────────▼─────────┐
                          │   HIR → IR (SSA)   │
                          │   CFG, Dominance   │
                          │   Optimizations    │
                          └─────────┬─────────┘
                                    │
                 ┌──────────────────┼──────────────────┐
                 ▼                  ▼                   ▼
              ┌────────────┐         ┌──────────────┐
   Backends  │  VM (.abc)  │         │  LLVM (AOT)  │
              │  Portable   │         │  Native bin  │
              │  bytecode   │         │  + runtime   │
              │  + JIT tier │         │              │
              └────────────┘         └──────────────┘
```

## Frontends

### Arth Language

Systems programming language combining familiar syntax with Rust-grade memory safety. Ownership and borrowing with inferred lifetimes — no explicit lifetime annotations. Deterministic memory reclamation, no garbage collector.

- **Modules** for behavior, not methods on structs
- **Providers** for long-lived shared state instead of globals
- **Typed exceptions** with checked `throws` clauses
- **`Optional<T>`** instead of null
- **Actors and channels** for concurrency

### TypeScript (Guest)

TypeScript files lower through a dedicated frontend (`arth-ts-frontend`) into the same HIR. This enables mixed Arth + TS codebases sharing a single compilation pipeline and runtime.

## Backends

| Backend | Crate | Use Case |
|---------|-------|----------|
| **VM** | `arth-vm` | Portable bytecode (`.abc` files), fast iteration. Optional Cranelift JIT tier for hot functions. |
| **LLVM** | built-in | Native AOT compilation via LLVM IR emission + `arth-rt` runtime linking. |

## Key Concepts

**HIR (High-level IR)** — The shared substrate. Source-language-agnostic, desugared, with stable `HirId`s for cross-referencing. All analysis (name resolution, type checking, ownership) operates at this level.

**IR (SSA)** — Lower-level SSA form with CFG, dominance analysis, and optimization passes. Backend-agnostic — each codegen reads the same IR.

**Runtime (`arth-rt`)** — Native runtime library providing I/O, networking, crypto, database, async execution, and other host functions for natively compiled programs.

## Getting Started

```bash
# Build
cargo build

# Run a program (VM backend)
arth run examples/arth-sample/src/demo/Hello.arth

# Build with LLVM native backend
arth build --backend llvm examples/arth-sample/src/demo/Hello.arth

# Inspect the pipeline
arth lex examples/arth-sample/src/demo/Hello.arth       # tokens
arth parse examples/arth-sample/src/demo/Hello.arth      # AST
arth check --dump-hir examples/arth-sample/src/demo/Hello.arth  # HIR
arth check --dump-ir examples/arth-sample/src/demo/Hello.arth   # SSA IR
```

### Enable JIT Tier (Cranelift)

```bash
# Enables Cranelift JIT inside the VM for hot function compilation
cargo build --features cranelift
```

## Tooling

**LSP Server (`arth-lsp`)** — Language server providing diagnostics, completions, and go-to-definition for editors. VSCode extension included.

```bash
cargo build --bin arth-lsp
```

**Formatter (`arth fmt`)** — Code formatter for `.arth` files.

## Project Structure

```
src/compiler/
  lexer/          Tokenization
  parser/         AST construction
  hir/            High-level IR definitions
  lower/          AST→HIR and HIR→IR lowering
  resolve/        Name resolution, symbol tables
  typeck/         Type checking, ownership, effects
  ir/             SSA IR, CFG, dominance, optimizations
  codegen/        LLVM, Cranelift backends
  driver/         CLI, compilation orchestration

crates/
  arth-vm/            Bytecode VM runtime
  arth-rt/            Native runtime library
  arth-ts-frontend/   TypeScript frontend
  arth-cli/           CLI binary

src/bin/
  arth_lsp.rs       LSP server binary

stdlib/           Standard library (.arth sources)
examples/         Sample projects
editors/vscode/   VSCode syntax highlighting
tests/            Conformance, e2e, integration, benchmarks
fuzz/             Fuzz testing targets
```

## Contributing

We welcome contributions across the stack — new frontends, backend improvements, language features, optimizations, and tooling.

```bash
# Run tests
cargo test

# Format and lint (must pass)
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `docs:`, `refactor:`.

## License

Licensed under the [Apache License 2.0](LICENSE).
