# Arth Single-File Demo

This minimal sample contains a single `.arth` file to verify single-file checks/builds and the new auto-borrow behavior for named types (e.g., Logger).

## Files
- `QuickDemo.arth` — one file with a `module Single { public void main() { ... } }`

## Run just this file
- Check only:
  - `cargo run -- check examples/arth-single-file/QuickDemo.arth`
- Build and run with the VM backend:
  - `cargo run -- build --backend vm examples/arth-single-file/QuickDemo.arth`

Notes
- Use the file path (not the directory) to leverage single-file mode (package↔path mapping is relaxed only for a single-file run).
- The program logs via `log.Logger` and calls a helper that takes `Logger` without moving it; you'll see INFO/DEBUG lines.

