# Installation

## Prerequisites

- **Rust toolchain** (1.85+ recommended) — [rustup.rs](https://rustup.rs)
- **LLVM 18+** (optional, for native compilation) — see [LLVM releases](https://releases.llvm.org)
- **Clang + LLD** (optional, for native linking on Linux)

## Build from Source

```bash
# Clone the repository
git clone https://github.com/splentainc/arth.git
cd arth

# Build the compiler
cargo build --release

# Verify installation
./target/release/arth --help
```

## Optional Features

### Cranelift JIT

Enables JIT compilation for hot functions in the VM backend:

```bash
cargo build --release --features cranelift
```

### LLVM Native Backend

For AOT native binaries (requires LLVM 18+ installed):

```bash
arth build --backend llvm your_program.arth
```

On Linux, ensure `clang` and `lld` are available:

```bash
sudo apt install clang lld
```

## Editor Support

### VSCode

A syntax highlighting extension is included in the repository:

```
editors/vscode/
```

Install it by copying to your VSCode extensions directory or opening it as a workspace extension.

### LSP Server

Build the language server for diagnostics, completions, and go-to-definition:

```bash
cargo build --release --bin arth-lsp
```

Configure your editor to use `./target/release/arth-lsp` as the language server for `.arth` files.

## Verify Your Setup

```bash
# Create a test file
cat > hello.arth << 'EOF'
package hello;

public void main() {
    println("Hello from Arth!");
}
EOF

# Run it
arth run hello.arth
```

You should see: `Hello from Arth!`
