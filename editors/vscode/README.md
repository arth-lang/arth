Arth VS Code Extension (Local Scaffold)

Quick start
- Prerequisite: build the LSP server at repo root
  - `cargo build` (produces `target/debug/arth_lsp`)
- Install dependencies and build the extension
  - `cd editors/vscode`
  - `npm install`
  - `npm run watch` (optional, for live rebuilds)
- Run the extension in a VS Code Extension Host
  - Open the `editors/vscode` folder in VS Code
  - Press F5 (Run Extension)
  - Open a `.arth` file to activate the client (syntax highlighting + LSP)

Configuration
- Setting: `Arth › Lsp: Server Path` (`arth-lsp.serverPath`)
  - Leave empty to auto-detect `target/debug/arth_lsp` from the repo root, or use `arth_lsp` from PATH.
  - Set an absolute path if detection fails (e.g., on Windows `…\\target\\debug\\arth_lsp.exe`).

Notes
- Syntax highlighting follows the initial draft in `docs/spec.md` (keywords, comments, literals, attributes, primitives).
- The client launches `arth_lsp` over stdio. Future versions can download a packaged binary on install.
- Document selector activates on `*.arth` files.
