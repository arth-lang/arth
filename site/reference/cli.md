# CLI Reference

## Commands

### `arth run`

Run an Arth program using the VM backend:

```bash
arth run <path>
```

`<path>` can be a `.arth` file, a directory, or a compiled `.abc` bytecode file.

### `arth build`

Compile an Arth program:

```bash
arth build <path>
arth build --backend vm <path>       # VM bytecode (default)
arth build --backend llvm <path>     # Native binary via LLVM
arth build --backend cranelift <path> # Cranelift JIT
```

### `arth check`

Run frontend analysis (parsing, type checking) without code generation:

```bash
arth check <path>
arth check --dump-hir <path>    # Print HIR (desugared IR)
arth check --dump-ir <path>     # Print SSA IR
```

### `arth lex`

Tokenize a file and print the token stream:

```bash
arth lex <path>
```

### `arth parse`

Parse a file and print the AST structure:

```bash
arth parse <path>
```

### `arth fmt`

Format `.arth` source files:

```bash
arth fmt <path>
```

### `arth emit-llvm`

Emit LLVM IR text:

```bash
arth emit-llvm [output-path]
```

## Configuration

### `arth.toml`

Project configuration file:

```toml
[package]
name = "myapp"
version = "0.1.0"
entry = "src/main.arth"

[dependencies]
# future: package dependencies
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Set log level (`debug`, `trace`, etc.) |

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Compilation error |
| 2 | Runtime error |
