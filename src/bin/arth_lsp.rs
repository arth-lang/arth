#![allow(dead_code)]
#![allow(clippy::collapsible_if)]
#![allow(unused_variables)]

use arth::compiler::ast::{Decl, FileAst};
use arth::compiler::diagnostics::{Diagnostic, Reporter, Severity};
use arth::compiler::parser::parse_file;
use arth::compiler::source::SourceFile;
use arth::compiler::stdlib::StdlibIndex;
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types as lsp;
use tower_lsp::{Client, LanguageServer, LspService, Server};

/// Cached compilation state for a document.
#[derive(Clone)]
struct DocumentState {
    /// Original source text.
    text: String,
    /// Parsed AST (if parsing succeeded).
    ast: Option<FileAst>,
    /// Diagnostics from compilation.
    diagnostics: Vec<Diagnostic>,
}

struct Backend {
    client: Client,
    /// Latest text and compilation state for open documents.
    documents: Arc<RwLock<HashMap<lsp::Url, DocumentState>>>,
    /// Stdlib index for completions (loaded once at startup).
    stdlib: Arc<Option<StdlibIndex>>,
    /// Workspace root directories for cross-file navigation.
    workspace_roots: Arc<RwLock<Vec<PathBuf>>>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Completion Support
// ─────────────────────────────────────────────────────────────────────────────

/// All Arth keywords for completion.
const ARTH_KEYWORDS: &[&str] = &[
    "package",
    "module",
    "public",
    "import",
    "internal",
    "private",
    "export",
    "struct",
    "interface",
    "enum",
    "sealed",
    "provider",
    "shared",
    "type",
    "extends",
    "static",
    "final",
    "void",
    "async",
    "await",
    "throws",
    "var",
    "fn",
    "unsafe",
    "extern",
    "implements",
    "as",
    "true",
    "false",
    "if",
    "else",
    "while",
    "for",
    "switch",
    "case",
    "default",
    "try",
    "catch",
    "finally",
    "break",
    "continue",
    "return",
    "throw",
    "panic",
];

/// Primitive types in Arth.
const ARTH_PRIMITIVE_TYPES: &[&str] = &[
    // Standard names
    "int", "short", "long", "float", "double", "bool", "char", "string", "bytes", "void",
    // Sized integer types
    "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64", "u128", "int8", "int16", "int32",
    "int64", "int128", "uint8", "uint16", "uint32", "uint64", "uint128",
    // Sized float types
    "f32", "f64", "float32", "float64",
];

/// Common builtin types from stdlib.
const ARTH_BUILTIN_TYPES: &[&str] = &[
    "String", "Optional", "List", "Map", "Set", "Result", "Task", "Channel", "Mutex", "Atomic",
    "Watch", "Notify", "Duration", "Instant", "DateTime", "Request", "Response", "Headers", "Body",
    "Logger", "LogLevel",
];

/// Common exceptions.
const ARTH_EXCEPTIONS: &[&str] = &[
    "IoError",
    "TimeoutError",
    "HttpError",
    "CancelledError",
    "EncodeError",
    "DecodeError",
    "ParseError",
    "ValidationError",
];

/// Built-in functions.
const ARTH_BUILTINS: &[(&str, &str)] = &[
    ("println", "println($0)"),
    ("print", "print($0)"),
    ("assert", "assert($0)"),
    ("spawn", "spawn($0)"),
    ("spawnBlocking", "spawnBlocking($0)"),
    ("startTask", "startTask($0)"),
];

/// Attribute names (without @).
const ARTH_ATTRIBUTES: &[&str] = &[
    "derive",
    "test",
    "bench",
    "deprecated",
    "inline",
    "must_use",
    "allow",
    "intrinsic",
];

/// Derive macro arguments.
const ARTH_DERIVE_ARGS: &[&str] = &[
    "Eq",
    "Hash",
    "Show",
    "JsonCodec",
    "BinaryCodec",
    "Default",
    "Clone",
    "Copy",
];

/// Completion context to determine what kind of completions to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionContext {
    /// At the start of a line or after visibility modifiers - show keywords
    TopLevel,
    /// After a type keyword (: or in parameter list) - show types
    TypePosition,
    /// After a dot - show fields/methods (limited without type info)
    MemberAccess,
    /// After @ - show attributes
    Attribute,
    /// Inside @derive(...) - show derive arguments
    DeriveArgs,
    /// After throws ( - show exception types
    ThrowsClause,
    /// After extends/implements - show interfaces
    ExtendsClause,
    /// After import - show package paths
    ImportPath,
    /// General identifier context
    Identifier,
}

/// Determine the completion context from the current line and cursor position.
fn determine_context(line: &str, col: usize) -> CompletionContext {
    let before_cursor = if col <= line.len() {
        &line[..col]
    } else {
        line
    };
    let trimmed = before_cursor.trim_start();

    // Check for attribute context
    if let Some(pos) = trimmed.rfind('@') {
        let after_at = &trimmed[pos + 1..];
        if after_at.contains("derive(") && !after_at.contains(')') {
            return CompletionContext::DeriveArgs;
        }
        if !after_at.contains('(') && !after_at.contains(' ') {
            return CompletionContext::Attribute;
        }
    }

    // Check for dot context (member access)
    if before_cursor.ends_with('.') || before_cursor.ends_with("?.") {
        return CompletionContext::MemberAccess;
    }

    // Check for type position after colon
    if before_cursor.contains(':') {
        let after_colon = before_cursor.rsplit(':').next().unwrap_or("");
        if after_colon.trim().is_empty() || is_partial_type(after_colon.trim()) {
            return CompletionContext::TypePosition;
        }
    }

    // Check for throws clause
    if trimmed.contains("throws") && trimmed.contains('(') && !trimmed.ends_with(')') {
        return CompletionContext::ThrowsClause;
    }

    // Check for extends/implements
    if trimmed.ends_with("extends ") || trimmed.ends_with("implements ") {
        return CompletionContext::ExtendsClause;
    }
    if trimmed.contains("extends ") || trimmed.contains("implements ") {
        let last_part = trimmed.rsplit(' ').next().unwrap_or("");
        if is_partial_type(last_part) {
            return CompletionContext::ExtendsClause;
        }
    }

    // Check for import statement
    if trimmed.starts_with("import ") {
        return CompletionContext::ImportPath;
    }

    // Check for type position in various contexts
    // After < for generics, or in param list after comma
    if before_cursor.ends_with('<') || before_cursor.ends_with(", ") {
        // Could be generic type arg or param - check context
        if before_cursor.contains('<') && !before_cursor.contains('>') {
            return CompletionContext::TypePosition;
        }
    }

    // Top level keywords - at start of line or after visibility
    if trimmed.is_empty()
        || trimmed == "public"
        || trimmed == "private"
        || trimmed == "internal"
        || trimmed == "export"
        || trimmed.ends_with(' ')
            && (trimmed.starts_with("public ")
                || trimmed.starts_with("private ")
                || trimmed.starts_with("internal ")
                || trimmed.starts_with("export "))
    {
        return CompletionContext::TopLevel;
    }

    CompletionContext::Identifier
}

/// Check if a string looks like a partial type name.
fn is_partial_type(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .next()
            .map(|c| c.is_uppercase() || c.is_lowercase())
            .unwrap_or(false)
}

/// Extract user-defined symbols from the AST.
fn extract_user_symbols(ast: &FileAst) -> Vec<(String, lsp::CompletionItemKind)> {
    let mut symbols = Vec::new();

    for decl in &ast.decls {
        match decl {
            Decl::Module(m) => {
                symbols.push((m.name.0.clone(), lsp::CompletionItemKind::MODULE));
                // Add module functions
                for func in &m.items {
                    symbols.push((
                        format!("{}.{}", m.name.0, func.sig.name.0),
                        lsp::CompletionItemKind::FUNCTION,
                    ));
                }
            }
            Decl::Struct(s) => {
                symbols.push((s.name.0.clone(), lsp::CompletionItemKind::STRUCT));
            }
            Decl::Interface(i) => {
                symbols.push((i.name.0.clone(), lsp::CompletionItemKind::INTERFACE));
            }
            Decl::Enum(e) => {
                symbols.push((e.name.0.clone(), lsp::CompletionItemKind::ENUM));
                // Add enum variants
                for variant in &e.variants {
                    let name = variant.name();
                    symbols.push((
                        format!("{}.{}", e.name.0, name.0),
                        lsp::CompletionItemKind::ENUM_MEMBER,
                    ));
                }
            }
            Decl::Provider(p) => {
                symbols.push((p.name.0.clone(), lsp::CompletionItemKind::CLASS));
            }
            Decl::TypeAlias(t) => {
                symbols.push((t.name.0.clone(), lsp::CompletionItemKind::TYPE_PARAMETER));
            }
            Decl::Function(f) => {
                symbols.push((f.sig.name.0.clone(), lsp::CompletionItemKind::FUNCTION));
            }
            Decl::ExternFunc(e) => {
                symbols.push((e.name.0.clone(), lsp::CompletionItemKind::FUNCTION));
            }
        }
    }

    symbols
}

/// Try to find the stdlib directory relative to the executable or workspace.
fn find_stdlib_path() -> Option<PathBuf> {
    // Try relative to current dir (common in development)
    let candidates = [
        PathBuf::from("stdlib/src"),
        PathBuf::from("arth/stdlib/src"),
        PathBuf::from("../stdlib/src"),
    ];

    for path in &candidates {
        if path.exists() && path.is_dir() {
            return Some(path.clone());
        }
    }

    // Try relative to executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let stdlib_path = parent.join("stdlib/src");
            if stdlib_path.exists() {
                return Some(stdlib_path);
            }
        }
    }

    None
}

/// Extract the prefix being typed at the cursor position.
fn extract_prefix(line: &str, col: usize) -> String {
    if col == 0 || line.is_empty() {
        return String::new();
    }

    let before_cursor = if col <= line.len() {
        &line[..col]
    } else {
        line
    };
    let bytes = before_cursor.as_bytes();

    // Find the start of the current identifier
    let mut start = col;
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }

    if start < col {
        before_cursor[start..col].to_string()
    } else {
        String::new()
    }
}

/// Extract the receiver identifier before a dot.
fn extract_receiver(line: &str, col: usize) -> String {
    if col < 2 || line.is_empty() {
        return String::new();
    }

    // Find the position just before the dot
    let before_cursor = if col <= line.len() {
        &line[..col]
    } else {
        line
    };
    let trimmed = before_cursor.trim_end_matches('.').trim_end_matches("?.");
    let bytes = trimmed.as_bytes();

    if bytes.is_empty() {
        return String::new();
    }

    // Find the end of the receiver identifier
    let mut end = bytes.len();
    while end > 0 && !is_ident_char(bytes[end - 1]) {
        end -= 1;
    }

    if end == 0 {
        return String::new();
    }

    // Find the start of the receiver identifier
    let mut start = end;
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }

    if start < end {
        trimmed[start..end].to_string()
    } else {
        String::new()
    }
}

/// A symbol with its location information for go-to-definition.
#[derive(Debug, Clone)]
struct SymbolLocation {
    /// Simple name of the symbol
    name: String,
    /// Qualified name (e.g., "ModuleName.funcName" for functions in modules)
    qualified_name: Option<String>,
    /// The kind of symbol
    kind: lsp::SymbolKind,
    /// Range of the symbol name in the source
    name_range: lsp::Range,
    /// Full range of the declaration
    full_range: lsp::Range,
}

/// Extract all symbols from an AST with their locations.
fn extract_symbols_with_locations(ast: &FileAst, text: &str) -> Vec<SymbolLocation> {
    let line_offsets = compute_line_offsets(text);
    let mut symbols = Vec::new();

    for decl in &ast.decls {
        match decl {
            Decl::Module(m) => {
                let name_range = span_to_range(&m.span, &line_offsets);
                symbols.push(SymbolLocation {
                    name: m.name.0.clone(),
                    qualified_name: None,
                    kind: lsp::SymbolKind::MODULE,
                    name_range,
                    full_range: span_to_range(&m.span, &line_offsets),
                });

                // Add functions within the module
                for func in &m.items {
                    let func_range = span_to_range(&func.span, &line_offsets);
                    symbols.push(SymbolLocation {
                        name: func.sig.name.0.clone(),
                        qualified_name: Some(format!("{}.{}", m.name.0, func.sig.name.0)),
                        kind: lsp::SymbolKind::FUNCTION,
                        name_range: span_to_range(&func.sig.span, &line_offsets),
                        full_range: func_range,
                    });
                }
            }
            Decl::Struct(s) => {
                let range = span_to_range(&s.span, &line_offsets);
                symbols.push(SymbolLocation {
                    name: s.name.0.clone(),
                    qualified_name: None,
                    kind: lsp::SymbolKind::STRUCT,
                    name_range: range,
                    full_range: range,
                });

                // Add fields
                for field in &s.fields {
                    let field_range = span_to_range(&field.span, &line_offsets);
                    symbols.push(SymbolLocation {
                        name: field.name.0.clone(),
                        qualified_name: Some(format!("{}.{}", s.name.0, field.name.0)),
                        kind: lsp::SymbolKind::FIELD,
                        name_range: field_range,
                        full_range: field_range,
                    });
                }
            }
            Decl::Interface(i) => {
                let range = span_to_range(&i.span, &line_offsets);
                symbols.push(SymbolLocation {
                    name: i.name.0.clone(),
                    qualified_name: None,
                    kind: lsp::SymbolKind::INTERFACE,
                    name_range: range,
                    full_range: range,
                });

                // Add methods
                for method in &i.methods {
                    let method_range = span_to_range(&method.sig.span, &line_offsets);
                    symbols.push(SymbolLocation {
                        name: method.sig.name.0.clone(),
                        qualified_name: Some(format!("{}.{}", i.name.0, method.sig.name.0)),
                        kind: lsp::SymbolKind::METHOD,
                        name_range: method_range,
                        full_range: method_range,
                    });
                }
            }
            Decl::Enum(e) => {
                let range = span_to_range(&e.span, &line_offsets);
                symbols.push(SymbolLocation {
                    name: e.name.0.clone(),
                    qualified_name: None,
                    kind: lsp::SymbolKind::ENUM,
                    name_range: range,
                    full_range: range,
                });

                // Add variants
                for variant in &e.variants {
                    let variant_name = variant.name().0.clone();
                    symbols.push(SymbolLocation {
                        name: variant_name.clone(),
                        qualified_name: Some(format!("{}.{}", e.name.0, variant_name)),
                        kind: lsp::SymbolKind::ENUM_MEMBER,
                        name_range: range, // Variants don't have individual spans
                        full_range: range,
                    });
                }
            }
            Decl::Provider(p) => {
                let range = span_to_range(&p.span, &line_offsets);
                symbols.push(SymbolLocation {
                    name: p.name.0.clone(),
                    qualified_name: None,
                    kind: lsp::SymbolKind::CLASS,
                    name_range: range,
                    full_range: range,
                });
            }
            Decl::TypeAlias(t) => {
                let range = span_to_range(&t.span, &line_offsets);
                symbols.push(SymbolLocation {
                    name: t.name.0.clone(),
                    qualified_name: None,
                    kind: lsp::SymbolKind::TYPE_PARAMETER,
                    name_range: range,
                    full_range: range,
                });
            }
            Decl::Function(f) => {
                let range = span_to_range(&f.span, &line_offsets);
                symbols.push(SymbolLocation {
                    name: f.sig.name.0.clone(),
                    qualified_name: None,
                    kind: lsp::SymbolKind::FUNCTION,
                    name_range: span_to_range(&f.sig.span, &line_offsets),
                    full_range: range,
                });
            }
            Decl::ExternFunc(e) => {
                let range = span_to_range(&e.span, &line_offsets);
                symbols.push(SymbolLocation {
                    name: e.name.0.clone(),
                    qualified_name: None,
                    kind: lsp::SymbolKind::FUNCTION,
                    name_range: range,
                    full_range: range,
                });
            }
        }
    }

    symbols
}

/// Convert a compiler Span to an LSP Range.
fn span_to_range(span: &arth::compiler::source::Span, line_offsets: &[usize]) -> lsp::Range {
    let start = offset_to_position(span.start, line_offsets);
    let end = offset_to_position(span.end, line_offsets);
    lsp::Range { start, end }
}

/// Find all .arth files in a directory recursively.
fn find_arth_files_in_dir(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Skip hidden directories and common non-source directories
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if !name.starts_with('.') && name != "target" && name != "node_modules" {
                        files.extend(find_arth_files_in_dir(&path));
                    }
                }
            } else if path.extension().is_some_and(|ext| ext == "arth") {
                files.push(path);
            }
        }
    }
    files
}

/// Parse a file and extract symbols with locations.
fn parse_file_symbols(path: &std::path::Path) -> Option<(lsp::Url, Vec<SymbolLocation>)> {
    let text = std::fs::read_to_string(path).ok()?;
    let sf = SourceFile {
        path: path.to_path_buf(),
        text: text.clone(),
    };
    let mut reporter = Reporter::new();
    let ast = parse_file(&sf, &mut reporter);

    let uri = lsp::Url::from_file_path(path).ok()?;
    let symbols = extract_symbols_with_locations(&ast, &text);
    Some((uri, symbols))
}

/// Extract the qualified identifier at cursor (e.g., "Module.function" or just "function").
fn extract_qualified_name(line: &str, col: usize) -> (Option<String>, String) {
    let bytes = line.as_bytes();
    if col == 0 || col > bytes.len() {
        return (None, String::new());
    }

    // Find word boundaries
    let mut end = col;
    while end < bytes.len() && is_ident_char(bytes[end]) {
        end += 1;
    }

    let mut start = col.saturating_sub(1);
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }

    if start >= end {
        return (None, String::new());
    }

    let name = line[start..end].to_string();

    // Check if there's a dot before this identifier (qualified access)
    if start >= 2 {
        let before = &line[..start];
        if let Some(before_dot) = before.strip_suffix('.') {
            // Find the qualifier before the dot
            let qualifier_bytes = before_dot.as_bytes();
            let mut q_end = qualifier_bytes.len();
            while q_end > 0 && !is_ident_char(qualifier_bytes[q_end - 1]) {
                q_end -= 1;
            }
            let mut q_start = q_end;
            while q_start > 0 && is_ident_char(qualifier_bytes[q_start - 1]) {
                q_start -= 1;
            }
            if q_start < q_end {
                let qualifier = before_dot[q_start..q_end].to_string();
                return (Some(qualifier), name);
            }
        }
    }

    (None, name)
}

// ─────────────────────────────────────────────────────────────────────────────
// Hover Support
// ─────────────────────────────────────────────────────────────────────────────

use arth::compiler::ast::{
    EnumDecl, EnumVariant, ExternFuncDecl, FuncDecl, InterfaceDecl, ModuleDecl, ProviderDecl,
    StructDecl, TypeAliasDecl,
};

/// Information for hover display
#[derive(Debug, Clone)]
struct HoverInfo {
    /// The kind of symbol
    kind: &'static str,
    /// The signature or definition (code block)
    signature: String,
    /// Documentation comment if available
    doc: Option<String>,
    /// Additional details
    details: Option<String>,
}

impl HoverInfo {
    /// Format as markdown for hover display
    fn to_markdown(&self) -> String {
        let mut parts = Vec::new();

        // Add signature as code block
        parts.push(format!("```arth\n{}\n```", self.signature));

        // Add documentation
        if let Some(ref doc) = self.doc {
            parts.push(doc.clone());
        }

        // Add details
        if let Some(ref details) = self.details {
            parts.push(format!("*{}*", details));
        }

        parts.join("\n\n")
    }
}

/// Format a NamePath as a string
fn format_namepath(np: &arth::compiler::ast::NamePath) -> String {
    let base: String = np
        .path
        .iter()
        .map(|id| id.0.as_str())
        .collect::<Vec<_>>()
        .join(".");
    if np.type_args.is_empty() {
        base
    } else {
        let args: Vec<String> = np.type_args.iter().map(format_namepath).collect();
        format!("{}<{}>", base, args.join(", "))
    }
}

/// Format a function signature
fn format_func_signature(sig: &arth::compiler::ast::FuncSig) -> String {
    let mut parts = Vec::new();

    // Visibility
    match sig.vis {
        arth::compiler::ast::Visibility::Public => parts.push("public".to_string()),
        arth::compiler::ast::Visibility::Private => parts.push("private".to_string()),
        arth::compiler::ast::Visibility::Internal => parts.push("internal".to_string()),
        arth::compiler::ast::Visibility::Default => {}
    }

    // Modifiers
    if sig.is_static {
        parts.push("static".to_string());
    }
    if sig.is_async {
        parts.push("async".to_string());
    }
    if sig.is_final {
        parts.push("final".to_string());
    }
    if sig.is_unsafe {
        parts.push("unsafe".to_string());
    }

    // Return type
    if let Some(ref ret) = sig.ret {
        parts.push(format_namepath(ret));
    } else {
        parts.push("void".to_string());
    }

    // Function name with generics
    let name = if sig.generics.is_empty() {
        sig.name.0.clone()
    } else {
        let generics: Vec<String> = sig
            .generics
            .iter()
            .map(|g| {
                if let Some(ref bound) = g.bound {
                    format!("{} extends {}", g.name.0, format_namepath(bound))
                } else {
                    g.name.0.clone()
                }
            })
            .collect();
        format!("{}<{}>", sig.name.0, generics.join(", "))
    };
    parts.push(name);

    // Parameters
    let params: Vec<String> = sig
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name.0, format_namepath(&p.ty)))
        .collect();

    let mut result = parts.join(" ");
    result.push_str(&format!("({})", params.join(", ")));

    // Throws clause
    if !sig.throws.is_empty() {
        let throws: Vec<String> = sig.throws.iter().map(format_namepath).collect();
        result.push_str(&format!(" throws ({})", throws.join(", ")));
    }

    result
}

/// Format a struct declaration for hover
fn format_struct_hover(s: &StructDecl) -> HoverInfo {
    let mut sig = String::new();

    // Struct header
    sig.push_str("struct ");
    sig.push_str(&s.name.0);

    // Generics
    if !s.generics.is_empty() {
        let generics: Vec<String> = s
            .generics
            .iter()
            .map(|g| {
                if let Some(ref bound) = g.bound {
                    format!("{} extends {}", g.name.0, format_namepath(bound))
                } else {
                    g.name.0.clone()
                }
            })
            .collect();
        sig.push_str(&format!("<{}>", generics.join(", ")));
    }

    sig.push_str(" {\n");

    // Fields
    for field in &s.fields {
        sig.push_str("    ");
        match field.vis {
            arth::compiler::ast::Visibility::Public => sig.push_str("public "),
            arth::compiler::ast::Visibility::Private => sig.push_str("private "),
            arth::compiler::ast::Visibility::Internal => sig.push_str("internal "),
            arth::compiler::ast::Visibility::Default => {}
        }
        if field.is_final {
            sig.push_str("final ");
        }
        if field.is_shared {
            sig.push_str("shared ");
        }
        sig.push_str(&format_namepath(&field.ty));
        sig.push(' ');
        sig.push_str(&field.name.0);
        sig.push_str(";\n");
    }

    sig.push('}');

    HoverInfo {
        kind: "struct",
        signature: sig,
        doc: s.doc.clone(),
        details: Some(format!("{} field(s)", s.fields.len())),
    }
}

/// Format an interface declaration for hover
fn format_interface_hover(i: &InterfaceDecl) -> HoverInfo {
    let mut sig = String::new();

    sig.push_str("interface ");
    sig.push_str(&i.name.0);

    // Generics
    if !i.generics.is_empty() {
        let generics: Vec<String> = i
            .generics
            .iter()
            .map(|g| {
                if let Some(ref bound) = g.bound {
                    format!("{} extends {}", g.name.0, format_namepath(bound))
                } else {
                    g.name.0.clone()
                }
            })
            .collect();
        sig.push_str(&format!("<{}>", generics.join(", ")));
    }

    // Extends
    if !i.extends.is_empty() {
        let extends: Vec<String> = i.extends.iter().map(format_namepath).collect();
        sig.push_str(&format!(" extends {}", extends.join(", ")));
    }

    sig.push_str(" {\n");

    // Methods
    for method in &i.methods {
        sig.push_str("    ");
        sig.push_str(&format_func_signature(&method.sig));
        sig.push_str(";\n");
    }

    sig.push('}');

    HoverInfo {
        kind: "interface",
        signature: sig,
        doc: i.doc.clone(),
        details: Some(format!("{} method(s)", i.methods.len())),
    }
}

/// Format an enum declaration for hover
fn format_enum_hover(e: &EnumDecl) -> HoverInfo {
    let mut sig = String::new();

    if e.is_sealed {
        sig.push_str("sealed ");
    }
    sig.push_str("enum ");
    sig.push_str(&e.name.0);

    // Generics
    if !e.generics.is_empty() {
        let generics: Vec<String> = e
            .generics
            .iter()
            .map(|g| {
                if let Some(ref bound) = g.bound {
                    format!("{} extends {}", g.name.0, format_namepath(bound))
                } else {
                    g.name.0.clone()
                }
            })
            .collect();
        sig.push_str(&format!("<{}>", generics.join(", ")));
    }

    sig.push_str(" {\n");

    // Variants
    for variant in &e.variants {
        sig.push_str("    ");
        match variant {
            EnumVariant::Unit { name, .. } => {
                sig.push_str(&name.0);
            }
            EnumVariant::Tuple { name, types, .. } => {
                sig.push_str(&name.0);
                let type_strs: Vec<String> = types.iter().map(format_namepath).collect();
                sig.push_str(&format!("({})", type_strs.join(", ")));
            }
        }
        sig.push_str(",\n");
    }

    sig.push('}');

    HoverInfo {
        kind: "enum",
        signature: sig,
        doc: e.doc.clone(),
        details: Some(format!("{} variant(s)", e.variants.len())),
    }
}

/// Format a module declaration for hover
fn format_module_hover(m: &ModuleDecl) -> HoverInfo {
    let mut sig = String::new();

    if m.is_exported {
        sig.push_str("export ");
    }
    sig.push_str("module ");
    sig.push_str(&m.name.0);

    // Implements
    if !m.implements.is_empty() {
        let impls: Vec<String> = m.implements.iter().map(format_namepath).collect();
        sig.push_str(&format!(" implements {}", impls.join(", ")));
    }

    sig.push_str(" {\n");

    // Show function signatures (abbreviated)
    for func in &m.items {
        sig.push_str("    ");
        sig.push_str(&format_func_signature(&func.sig));
        sig.push_str(";\n");
    }

    sig.push('}');

    HoverInfo {
        kind: "module",
        signature: sig,
        doc: m.doc.clone(),
        details: Some(format!("{} function(s)", m.items.len())),
    }
}

/// Format a provider declaration for hover
fn format_provider_hover(p: &ProviderDecl) -> HoverInfo {
    let mut sig = String::new();

    sig.push_str("provider ");
    sig.push_str(&p.name.0);
    sig.push_str(" {\n");

    // Fields
    for field in &p.fields {
        sig.push_str("    ");
        match field.vis {
            arth::compiler::ast::Visibility::Public => sig.push_str("public "),
            arth::compiler::ast::Visibility::Private => sig.push_str("private "),
            arth::compiler::ast::Visibility::Internal => sig.push_str("internal "),
            arth::compiler::ast::Visibility::Default => {}
        }
        if field.is_final {
            sig.push_str("final ");
        }
        if field.is_shared {
            sig.push_str("shared ");
        }
        sig.push_str(&format_namepath(&field.ty));
        sig.push(' ');
        sig.push_str(&field.name.0);
        sig.push_str(";\n");
    }

    sig.push('}');

    HoverInfo {
        kind: "provider",
        signature: sig,
        doc: p.doc.clone(),
        details: Some(format!("{} field(s)", p.fields.len())),
    }
}

/// Format a type alias for hover
fn format_type_alias_hover(t: &TypeAliasDecl) -> HoverInfo {
    let sig = format!("type {} = {}", t.name.0, format_namepath(&t.aliased));

    HoverInfo {
        kind: "type alias",
        signature: sig,
        doc: t.doc.clone(),
        details: None,
    }
}

/// Format a function declaration for hover
fn format_function_hover(f: &FuncDecl) -> HoverInfo {
    HoverInfo {
        kind: "function",
        signature: format_func_signature(&f.sig),
        doc: f.sig.doc.clone(),
        details: None,
    }
}

/// Format an extern function for hover
fn format_extern_func_hover(e: &ExternFuncDecl) -> HoverInfo {
    let mut sig = String::new();

    match e.vis {
        arth::compiler::ast::Visibility::Public => sig.push_str("public "),
        arth::compiler::ast::Visibility::Private => sig.push_str("private "),
        arth::compiler::ast::Visibility::Internal => sig.push_str("internal "),
        arth::compiler::ast::Visibility::Default => {}
    }

    sig.push_str(&format!("extern \"{}\" ", e.abi));

    if let Some(ref ret) = e.ret {
        sig.push_str(&format_namepath(ret));
    } else {
        sig.push_str("void");
    }

    sig.push(' ');
    sig.push_str(&e.name.0);

    let params: Vec<String> = e
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name.0, format_namepath(&p.ty)))
        .collect();
    sig.push_str(&format!("({})", params.join(", ")));

    HoverInfo {
        kind: "extern function",
        signature: sig,
        doc: e.doc.clone(),
        details: Some(format!("ABI: {}", e.abi)),
    }
}

/// Find hover info for a symbol in an AST
fn find_hover_info(ast: &FileAst, name: &str, qualifier: Option<&str>) -> Option<HoverInfo> {
    // Build the qualified name to match
    let qualified_target = qualifier.map(|q| format!("{}.{}", q, name));

    for decl in &ast.decls {
        match decl {
            Decl::Module(m) => {
                // Match module name
                if m.name.0 == name && qualifier.is_none() {
                    return Some(format_module_hover(m));
                }

                // Match function in module
                if let Some(ref qt) = qualified_target {
                    for func in &m.items {
                        if format!("{}.{}", m.name.0, func.sig.name.0) == *qt {
                            return Some(format_function_hover(func));
                        }
                    }
                }

                // Match function by simple name if qualifier matches module
                if qualifier == Some(&m.name.0) {
                    for func in &m.items {
                        if func.sig.name.0 == name {
                            return Some(format_function_hover(func));
                        }
                    }
                }
            }
            Decl::Struct(s) => {
                if s.name.0 == name && qualifier.is_none() {
                    return Some(format_struct_hover(s));
                }

                // Match field in struct
                if qualifier == Some(&s.name.0) {
                    for field in &s.fields {
                        if field.name.0 == name {
                            let sig = format!("{}: {}", field.name.0, format_namepath(&field.ty));
                            return Some(HoverInfo {
                                kind: "field",
                                signature: sig,
                                doc: field.doc.clone(),
                                details: Some(format!("in struct {}", s.name.0)),
                            });
                        }
                    }
                }
            }
            Decl::Interface(i) => {
                if i.name.0 == name && qualifier.is_none() {
                    return Some(format_interface_hover(i));
                }

                // Match method in interface
                if qualifier == Some(&i.name.0) {
                    for method in &i.methods {
                        if method.sig.name.0 == name {
                            return Some(HoverInfo {
                                kind: "method",
                                signature: format_func_signature(&method.sig),
                                doc: method.sig.doc.clone(),
                                details: Some(format!("in interface {}", i.name.0)),
                            });
                        }
                    }
                }
            }
            Decl::Enum(e) => {
                if e.name.0 == name && qualifier.is_none() {
                    return Some(format_enum_hover(e));
                }

                // Match variant in enum
                if qualifier == Some(&e.name.0) {
                    for variant in &e.variants {
                        let variant_name = variant.name().0.as_str();
                        if variant_name == name {
                            let sig = match variant {
                                EnumVariant::Unit { name, .. } => name.0.clone(),
                                EnumVariant::Tuple { name, types, .. } => {
                                    let type_strs: Vec<String> =
                                        types.iter().map(format_namepath).collect();
                                    format!("{}({})", name.0, type_strs.join(", "))
                                }
                            };
                            return Some(HoverInfo {
                                kind: "enum variant",
                                signature: sig,
                                doc: None,
                                details: Some(format!("in enum {}", e.name.0)),
                            });
                        }
                    }
                }
            }
            Decl::Provider(p) => {
                if p.name.0 == name && qualifier.is_none() {
                    return Some(format_provider_hover(p));
                }
            }
            Decl::TypeAlias(t) => {
                if t.name.0 == name && qualifier.is_none() {
                    return Some(format_type_alias_hover(t));
                }
            }
            Decl::Function(f) => {
                if f.sig.name.0 == name && qualifier.is_none() {
                    return Some(format_function_hover(f));
                }
            }
            Decl::ExternFunc(e) => {
                if e.name.0 == name && qualifier.is_none() {
                    return Some(format_extern_func_hover(e));
                }
            }
        }
    }

    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Signature Help Support
// ─────────────────────────────────────────────────────────────────────────────

/// Information about the current function call context
#[derive(Debug, Clone)]
struct CallContext {
    /// The function name being called
    func_name: String,
    /// The qualifier if it's a qualified call (e.g., "Module" in "Module.func()")
    qualifier: Option<String>,
    /// The index of the current parameter (0-based)
    active_param: usize,
    /// Total number of parameters seen so far
    param_count: usize,
}

/// Parse the current line to find function call context at cursor position
fn parse_call_context(line: &str, col: usize) -> Option<CallContext> {
    if col == 0 || col > line.len() {
        return None;
    }

    let before_cursor = &line[..col];
    let bytes = before_cursor.as_bytes();

    // Find the matching opening parenthesis
    let mut paren_depth = 0;
    let mut open_paren_pos = None;

    for (i, &b) in bytes.iter().enumerate().rev() {
        match b {
            b')' => paren_depth += 1,
            b'(' => {
                if paren_depth == 0 {
                    open_paren_pos = Some(i);
                    break;
                }
                paren_depth -= 1;
            }
            _ => {}
        }
    }

    let open_paren_pos = open_paren_pos?;

    // Count commas to determine active parameter
    let inside_parens = &before_cursor[open_paren_pos + 1..];
    let mut comma_count = 0;
    let mut nested_parens: i32 = 0;
    let mut nested_brackets: i32 = 0;
    let mut nested_braces: i32 = 0;

    for &b in inside_parens.as_bytes() {
        match b {
            b'(' => nested_parens += 1,
            b')' => nested_parens = nested_parens.saturating_sub(1),
            b'[' => nested_brackets += 1,
            b']' => nested_brackets = nested_brackets.saturating_sub(1),
            b'{' => nested_braces += 1,
            b'}' => nested_braces = nested_braces.saturating_sub(1),
            b',' if nested_parens == 0 && nested_brackets == 0 && nested_braces == 0 => {
                comma_count += 1;
            }
            _ => {}
        }
    }

    // Extract function name before the parenthesis
    let before_paren = &before_cursor[..open_paren_pos];
    let before_paren = before_paren.trim_end();

    if before_paren.is_empty() {
        return None;
    }

    let before_bytes = before_paren.as_bytes();

    // Find the end of the function name
    let mut name_end = before_bytes.len();
    while name_end > 0 && !is_ident_char(before_bytes[name_end - 1]) {
        name_end -= 1;
    }

    if name_end == 0 {
        return None;
    }

    // Find the start of the function name
    let mut name_start = name_end;
    while name_start > 0 && is_ident_char(before_bytes[name_start - 1]) {
        name_start -= 1;
    }

    let func_name = before_paren[name_start..name_end].to_string();

    // Check for qualifier (Module.func pattern)
    let qualifier = if name_start >= 2 {
        let before_name = &before_paren[..name_start];
        if let Some(before_dot) = before_name.strip_suffix('.') {
            let dot_bytes = before_dot.as_bytes();

            let mut q_end = dot_bytes.len();
            while q_end > 0 && !is_ident_char(dot_bytes[q_end - 1]) {
                q_end -= 1;
            }

            let mut q_start = q_end;
            while q_start > 0 && is_ident_char(dot_bytes[q_start - 1]) {
                q_start -= 1;
            }

            if q_start < q_end {
                Some(before_dot[q_start..q_end].to_string())
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    Some(CallContext {
        func_name,
        qualifier,
        active_param: comma_count,
        param_count: comma_count + 1,
    })
}

/// Create a SignatureInformation from a function signature
fn create_signature_info(sig: &arth::compiler::ast::FuncSig) -> lsp::SignatureInformation {
    let mut label_parts = Vec::new();

    // Build label
    if let Some(ref ret) = sig.ret {
        label_parts.push(format_namepath(ret));
    } else {
        label_parts.push("void".to_string());
    }

    // Function name with generics
    let name = if sig.generics.is_empty() {
        sig.name.0.clone()
    } else {
        let generics: Vec<String> = sig
            .generics
            .iter()
            .map(|g| {
                if let Some(ref bound) = g.bound {
                    format!("{} extends {}", g.name.0, format_namepath(bound))
                } else {
                    g.name.0.clone()
                }
            })
            .collect();
        format!("{}<{}>", sig.name.0, generics.join(", "))
    };
    label_parts.push(name);

    // Build parameter labels with offsets
    let mut param_infos = Vec::new();
    let params_str: Vec<String> = sig
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name.0, format_namepath(&p.ty)))
        .collect();

    let mut label = label_parts.join(" ");
    let params_start = label.len() + 1; // +1 for the opening paren
    label.push('(');

    for (i, param_str) in params_str.iter().enumerate() {
        if i > 0 {
            label.push_str(", ");
        }
        let start = label.len() as u32;
        label.push_str(param_str);
        let end = label.len() as u32;

        param_infos.push(lsp::ParameterInformation {
            label: lsp::ParameterLabel::LabelOffsets([start, end]),
            documentation: None,
        });
    }

    label.push(')');

    // Add throws clause
    if !sig.throws.is_empty() {
        let throws: Vec<String> = sig.throws.iter().map(format_namepath).collect();
        label.push_str(&format!(" throws ({})", throws.join(", ")));
    }

    lsp::SignatureInformation {
        label,
        documentation: sig.doc.clone().map(|d| {
            lsp::Documentation::MarkupContent(lsp::MarkupContent {
                kind: lsp::MarkupKind::Markdown,
                value: d,
            })
        }),
        parameters: Some(param_infos),
        active_parameter: None,
    }
}

/// Find function signature in an AST
fn find_func_signature(
    ast: &FileAst,
    func_name: &str,
    qualifier: Option<&str>,
) -> Option<lsp::SignatureInformation> {
    for decl in &ast.decls {
        match decl {
            Decl::Module(m) => {
                // Match qualified call: Module.func()
                if qualifier == Some(&m.name.0) {
                    for func in &m.items {
                        if func.sig.name.0 == func_name {
                            return Some(create_signature_info(&func.sig));
                        }
                    }
                }

                // Match unqualified call to module function (if importing)
                if qualifier.is_none() {
                    for func in &m.items {
                        if func.sig.name.0 == func_name {
                            return Some(create_signature_info(&func.sig));
                        }
                    }
                }
            }
            Decl::Interface(i) => {
                if qualifier == Some(&i.name.0) {
                    for method in &i.methods {
                        if method.sig.name.0 == func_name {
                            return Some(create_signature_info(&method.sig));
                        }
                    }
                }
            }
            Decl::Function(f) => {
                if qualifier.is_none() && f.sig.name.0 == func_name {
                    return Some(create_signature_info(&f.sig));
                }
            }
            _ => {}
        }
    }

    None
}

/// Create signature info for stdlib function
fn create_stdlib_signature_info(
    func: &arth::compiler::stdlib::StdlibFunc,
) -> lsp::SignatureInformation {
    create_signature_info(&func.sig)
}

#[derive(Debug, Clone)]
struct FuncSymbol {
    name: String,
    name_range: lsp::Range,
}

// Very lightweight function parser for .arth files.
// Scans only inside 'module { ... }' blocks and extracts function names and ranges.
fn parse_functions(text: &str) -> Vec<FuncSymbol> {
    // Very lightweight function header scanners for both Arth and TS:
    // - Arth style: [mods] <ret> <name>(...)
    // - TS style:   [export] [async] function <name>(...)
    let arth_re = Regex::new(
        r"(?m)^[\t ]*(?:public|private|internal)?(?:[\t ]+async)?(?:[\t ]+(?:final|static))*[\t ]+[A-Za-z_][A-Za-z0-9_<>\[\], .]*[\t ]+([A-Za-z_][A-Za-z0-9_]*)[\t ]*\(",
    )
    .unwrap();
    let ts_re = Regex::new(
        r"(?m)^[\t ]*(?:export[\t ]+)?(?:async[\t ]+)?function[\t ]+([A-Za-z_][A-Za-z0-9_]*)[\t ]*\(",
    )
    .unwrap();
    // Pre-scan lines to map offsets to positions
    let mut line_offsets = Vec::new();
    let mut offset: usize = 0;
    for line in text.split_inclusive('\n') {
        line_offsets.push(offset);
        offset += line.len();
    }
    // Ensure there is at least one entry
    if line_offsets.is_empty() {
        line_offsets.push(0);
    }

    let to_position = |ofs: usize| -> lsp::Position {
        // binary search line
        let mut lo = 0usize;
        let mut hi = line_offsets.len();
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if line_offsets[mid] <= ofs {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let line_start = line_offsets[lo];
        let character = ofs.saturating_sub(line_start) as u32;
        lsp::Position {
            line: lo as u32,
            character,
        }
    };

    // If there is at least one module, consider entire text for now; otherwise, also allow top-level demos
    let mut out = Vec::new();

    // Arth-style functions
    for mat in arth_re.find_iter(text) {
        let name = arth_re
            .captures(mat.as_str())
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }

        // Compute name start by re-searching name within the match
        let name_start = mat.start() + mat.as_str().find(&name).unwrap_or(0);
        let name_end = name_start + name.len();
        let start_pos = to_position(name_start);
        let end_pos = to_position(name_end);

        out.push(FuncSymbol {
            name,
            name_range: lsp::Range {
                start: start_pos,
                end: end_pos,
            },
        });
    }

    // TS-style functions
    for mat in ts_re.find_iter(text) {
        if let Some(caps) = ts_re.captures(mat.as_str()) {
            if let Some(m) = caps.get(1) {
                let name = m.as_str().to_string();
                if name.is_empty() {
                    continue;
                }
                let name_start = mat.start() + m.start();
                let name_end = name_start + name.len();
                let start_pos = to_position(name_start);
                let end_pos = to_position(name_end);
                out.push(FuncSymbol {
                    name,
                    name_range: lsp::Range {
                        start: start_pos,
                        end: end_pos,
                    },
                });
            }
        }
    }
    out
}

fn word_at(line: &str, ch: usize) -> Option<&str> {
    if line.is_empty() {
        return None;
    }
    let bytes = line.as_bytes();
    let mut l = ch.min(bytes.len());
    if l > 0 && !is_ident_char(bytes[l]) {
        l = l.saturating_sub(1);
    }
    // expand left
    let mut start = l;
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }
    // expand right
    let mut end = l;
    while end < bytes.len() && is_ident_char(bytes[end]) {
        end += 1;
    }
    if start < end {
        Some(&line[start..end])
    } else {
        None
    }
}

#[inline]
fn is_ident_char(b: u8) -> bool {
    b == b'_' || (b as char).is_ascii_alphanumeric()
}

/// Compile a document and return the state.
fn compile_document(uri: &lsp::Url, text: &str) -> DocumentState {
    // Create a source file from the URI
    let path = uri
        .to_file_path()
        .unwrap_or_else(|_| PathBuf::from(uri.path()));
    let sf = SourceFile {
        path,
        text: text.to_string(),
    };

    // Parse the file
    let mut reporter = Reporter::new();
    let ast = parse_file(&sf, &mut reporter);

    // Collect diagnostics
    let diagnostics = reporter.diagnostics().to_vec();

    DocumentState {
        text: text.to_string(),
        ast: Some(ast),
        diagnostics,
    }
}

/// Convert compiler diagnostics to LSP diagnostics.
fn to_lsp_diagnostics(diagnostics: &[Diagnostic], text: &str) -> Vec<lsp::Diagnostic> {
    let line_offsets = compute_line_offsets(text);

    diagnostics
        .iter()
        .map(|d| {
            let range = if let Some(ref span) = d.span {
                // Convert byte offsets to line/column
                let start = offset_to_position(span.start, &line_offsets);
                let end = offset_to_position(span.end, &line_offsets);
                lsp::Range { start, end }
            } else {
                lsp::Range {
                    start: lsp::Position {
                        line: 0,
                        character: 0,
                    },
                    end: lsp::Position {
                        line: 0,
                        character: 0,
                    },
                }
            };

            let severity = match d.severity {
                Severity::Error => lsp::DiagnosticSeverity::ERROR,
                Severity::Warning => lsp::DiagnosticSeverity::WARNING,
                Severity::Note => lsp::DiagnosticSeverity::INFORMATION,
            };

            lsp::Diagnostic {
                range,
                severity: Some(severity),
                code: d
                    .code
                    .as_ref()
                    .map(|c| lsp::NumberOrString::String(c.clone())),
                source: Some("arth".to_string()),
                message: d.message.clone(),
                ..Default::default()
            }
        })
        .collect()
}

/// Compute line start offsets for position conversion.
fn compute_line_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    for (i, c) in text.char_indices() {
        if c == '\n' {
            offsets.push(i + 1);
        }
    }
    offsets
}

/// Convert a byte offset to an LSP position.
fn offset_to_position(offset: usize, line_offsets: &[usize]) -> lsp::Position {
    let line = line_offsets.iter().rposition(|&o| o <= offset).unwrap_or(0);
    let line_start = line_offsets[line];
    let character = offset.saturating_sub(line_start) as u32;
    lsp::Position {
        line: line as u32,
        character,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Find References Support
// ─────────────────────────────────────────────────────────────────────────────

/// Find all occurrences of a symbol in text using word boundary matching.
fn find_references_in_text(text: &str, symbol_name: &str, uri: &lsp::Url) -> Vec<lsp::Location> {
    let mut locations = Vec::new();

    // Build a regex that matches the symbol as a whole word
    let pattern = format!(r"\b{}\b", regex::escape(symbol_name));
    let re = match Regex::new(&pattern) {
        Ok(r) => r,
        Err(_) => return locations,
    };

    let line_offsets = compute_line_offsets(text);

    for mat in re.find_iter(text) {
        let start = offset_to_position(mat.start(), &line_offsets);
        let end = offset_to_position(mat.end(), &line_offsets);
        locations.push(lsp::Location {
            uri: uri.clone(),
            range: lsp::Range { start, end },
        });
    }

    locations
}

/// Find references in AST with better precision (can distinguish declarations from usages).
fn find_references_in_ast(
    ast: &FileAst,
    text: &str,
    symbol_name: &str,
    qualifier: Option<&str>,
    uri: &lsp::Url,
    include_declaration: bool,
) -> Vec<lsp::Location> {
    // First, do a text-based search to find all occurrences
    let mut locations = find_references_in_text(text, symbol_name, uri);

    // If we have a qualifier (e.g., Module.function), also search for qualified usages
    if let Some(q) = qualifier {
        let qualified_name = format!("{}.{}", q, symbol_name);
        let qualified_refs = find_references_in_text(text, &qualified_name, uri);
        for loc in qualified_refs {
            if !locations.iter().any(|l| l.range == loc.range) {
                locations.push(loc);
            }
        }
    }

    // Optionally filter out the declaration itself
    if !include_declaration {
        // Extract symbol locations to identify declarations
        let symbols = extract_symbols_with_locations(ast, text);
        for sym in &symbols {
            if sym.name == symbol_name {
                // Remove the declaration location
                locations.retain(|loc| loc.range != sym.name_range);
            }
        }
    }

    locations
}

/// Extract the qualifier from a position in text (e.g., for "Module.func", returns "Module").
fn extract_qualifier_at(line: &str, col: usize) -> Option<String> {
    // Look backwards from cursor for a dot
    let before = if col <= line.len() {
        &line[..col]
    } else {
        line
    };

    // Find the word at cursor
    let word_end = col;
    let mut word_start = col;
    let bytes = before.as_bytes();

    // Move to start of current word
    while word_start > 0 && is_ident_char(bytes[word_start - 1]) {
        word_start -= 1;
    }

    // Check if there's a dot before the word
    if word_start > 0 && bytes[word_start - 1] == b'.' {
        let dot_pos = word_start - 1;
        // Find the qualifier before the dot
        let qual_end = dot_pos;
        let mut qual_start = dot_pos;
        while qual_start > 0 && is_ident_char(bytes[qual_start - 1]) {
            qual_start -= 1;
        }
        if qual_start < qual_end {
            return Some(before[qual_start..qual_end].to_string());
        }
    }

    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Rename Support
// ─────────────────────────────────────────────────────────────────────────────

/// Check if a symbol at the given position can be renamed.
/// Returns the range and current name if valid, or None if not renameable.
fn can_rename_at(text: &str, line_num: u32, col: usize) -> Option<(lsp::Range, String)> {
    let line = text.lines().nth(line_num as usize)?;

    // Get the word at cursor
    let name = word_at(line, col)?;
    if name.is_empty() {
        return None;
    }

    // Check if this is a keyword (can't rename keywords)
    if ARTH_KEYWORDS.contains(&name) {
        return None;
    }

    // Check if this is a primitive type (can't rename)
    if ARTH_PRIMITIVE_TYPES.contains(&name) {
        return None;
    }

    // Check if this is a builtin type (can't rename)
    if ARTH_BUILTIN_TYPES.contains(&name) {
        return None;
    }

    // Find the exact range of the symbol
    let bytes = line.as_bytes();
    let mut start = col.min(bytes.len());
    if start > 0 && !is_ident_char(bytes[start]) {
        start = start.saturating_sub(1);
    }

    // Expand to find word boundaries
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = col.min(bytes.len());
    while end < bytes.len() && is_ident_char(bytes[end]) {
        end += 1;
    }

    let range = lsp::Range {
        start: lsp::Position {
            line: line_num,
            character: start as u32,
        },
        end: lsp::Position {
            line: line_num,
            character: end as u32,
        },
    };

    Some((range, name.to_string()))
}

/// Apply a rename to a single text, returning the new text and the edits made.
fn apply_rename_to_text(
    text: &str,
    old_name: &str,
    new_name: &str,
    uri: &lsp::Url,
) -> Vec<lsp::TextEdit> {
    let locations = find_references_in_text(text, old_name, uri);

    locations
        .into_iter()
        .map(|loc| lsp::TextEdit {
            range: loc.range,
            new_text: new_name.to_string(),
        })
        .collect()
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: lsp::InitializeParams) -> Result<lsp::InitializeResult> {
        // Capture workspace roots for cross-file navigation
        let mut roots = self.workspace_roots.write().await;
        if let Some(folders) = params.workspace_folders {
            for folder in folders {
                if let Ok(path) = folder.uri.to_file_path() {
                    roots.push(path);
                }
            }
        } else if let Some(root_uri) = params.root_uri {
            if let Ok(path) = root_uri.to_file_path() {
                roots.push(path);
            }
        }
        drop(roots);

        let capabilities = lsp::ServerCapabilities {
            // Request full text sync for simpler prototype parsing
            text_document_sync: Some(lsp::TextDocumentSyncCapability::Kind(
                lsp::TextDocumentSyncKind::FULL,
            )),
            hover_provider: Some(lsp::HoverProviderCapability::Simple(true)),
            completion_provider: Some(lsp::CompletionOptions {
                resolve_provider: Some(false),
                trigger_characters: Some(vec![
                    ".".to_string(),
                    "@".to_string(),
                    ":".to_string(),
                    "<".to_string(),
                    "(".to_string(),
                    ",".to_string(),
                    " ".to_string(),
                ]),
                ..Default::default()
            }),
            definition_provider: Some(lsp::OneOf::Left(true)),
            document_symbol_provider: Some(lsp::OneOf::Left(true)),
            signature_help_provider: Some(lsp::SignatureHelpOptions {
                trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                retrigger_characters: Some(vec![",".to_string(), ")".to_string()]),
                work_done_progress_options: Default::default(),
            }),
            references_provider: Some(lsp::OneOf::Left(true)),
            rename_provider: Some(lsp::OneOf::Right(lsp::RenameOptions {
                prepare_provider: Some(true),
                work_done_progress_options: Default::default(),
            })),
            ..Default::default()
        };

        Ok(lsp::InitializeResult {
            capabilities,
            server_info: Some(lsp::ServerInfo {
                name: "Arth Language Server".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: lsp::InitializedParams) {
        let _ = self
            .client
            .log_message(lsp::MessageType::INFO, "Arth LSP initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: lsp::DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;

        // Compile the document
        let state = compile_document(&uri, &text);

        // Convert and publish diagnostics
        let lsp_diags = to_lsp_diagnostics(&state.diagnostics, &text);
        self.client
            .publish_diagnostics(uri.clone(), lsp_diags, None)
            .await;

        // Store the state
        self.documents.write().await.insert(uri.clone(), state);

        let _ = self
            .client
            .log_message(lsp::MessageType::INFO, format!("opened {}", uri))
            .await;
    }

    async fn did_change(&self, params: lsp::DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;

        // With FULL sync, the last change contains full text
        if let Some(last) = params.content_changes.last() {
            let text = &last.text;

            // Compile the document
            let state = compile_document(&uri, text);

            // Convert and publish diagnostics
            let lsp_diags = to_lsp_diagnostics(&state.diagnostics, text);
            self.client
                .publish_diagnostics(uri.clone(), lsp_diags, None)
                .await;

            // Store the state
            self.documents.write().await.insert(uri.clone(), state);
        }

        let _ = self
            .client
            .log_message(lsp::MessageType::LOG, format!("changed {}", uri))
            .await;
    }

    async fn did_save(&self, params: lsp::DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        let _ = self
            .client
            .log_message(lsp::MessageType::INFO, format!("saved {}", uri))
            .await;
    }

    async fn did_close(&self, params: lsp::DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;

        // Clear diagnostics for closed file
        self.client
            .publish_diagnostics(uri.clone(), vec![], None)
            .await;

        self.documents.write().await.remove(&uri);
        let _ = self
            .client
            .log_message(lsp::MessageType::INFO, format!("closed {}", uri))
            .await;
    }

    async fn hover(&self, params: lsp::HoverParams) -> Result<Option<lsp::Hover>> {
        let lsp::HoverParams {
            text_document_position_params,
            ..
        } = params;
        let uri = text_document_position_params.text_document.uri;
        let pos = text_document_position_params.position;

        let docs = self.documents.read().await;
        let Some(state) = docs.get(&uri) else {
            return Ok(None);
        };

        // Get the line and extract qualified name
        let line = state.text.lines().nth(pos.line as usize).unwrap_or("");
        let col = pos.character as usize;
        let (qualifier, name) = extract_qualified_name(line, col);

        if name.is_empty() {
            return Ok(None);
        }

        // Try to find hover info in current document
        if let Some(ref ast) = state.ast {
            if let Some(info) = find_hover_info(ast, &name, qualifier.as_deref()) {
                let contents = lsp::HoverContents::Markup(lsp::MarkupContent {
                    kind: lsp::MarkupKind::Markdown,
                    value: info.to_markdown(),
                });
                return Ok(Some(lsp::Hover {
                    contents,
                    range: None,
                }));
            }
        }

        // Search other open documents
        for (other_uri, other_state) in docs.iter() {
            if *other_uri == uri {
                continue;
            }
            if let Some(ref ast) = other_state.ast {
                if let Some(info) = find_hover_info(ast, &name, qualifier.as_deref()) {
                    let contents = lsp::HoverContents::Markup(lsp::MarkupContent {
                        kind: lsp::MarkupKind::Markdown,
                        value: info.to_markdown(),
                    });
                    return Ok(Some(lsp::Hover {
                        contents,
                        range: None,
                    }));
                }
            }
        }

        // Release docs lock before file I/O
        drop(docs);

        // Search workspace files
        let roots = self.workspace_roots.read().await;
        for root in roots.iter() {
            let files = find_arth_files_in_dir(root);
            for file_path in files {
                // Skip already-open files
                if let Ok(file_uri) = lsp::Url::from_file_path(&file_path) {
                    let docs = self.documents.read().await;
                    if docs.contains_key(&file_uri) {
                        continue;
                    }
                    drop(docs);
                }

                // Parse file and look for hover info
                if let Ok(text) = std::fs::read_to_string(&file_path) {
                    let sf = SourceFile {
                        path: file_path.clone(),
                        text: text.clone(),
                    };
                    let mut reporter = Reporter::new();
                    let ast = parse_file(&sf, &mut reporter);

                    if let Some(info) = find_hover_info(&ast, &name, qualifier.as_deref()) {
                        let contents = lsp::HoverContents::Markup(lsp::MarkupContent {
                            kind: lsp::MarkupKind::Markdown,
                            value: info.to_markdown(),
                        });
                        return Ok(Some(lsp::Hover {
                            contents,
                            range: None,
                        }));
                    }
                }
            }
        }
        drop(roots);

        // Try stdlib for builtin types and functions
        if let Some(ref stdlib) = *self.stdlib {
            // Check for module.function pattern
            if let Some(ref q) = qualifier {
                for pkg in stdlib.packages() {
                    if let Some(module) = pkg.modules.get(q) {
                        for func in &module.functions {
                            if func.name == name {
                                let info = HoverInfo {
                                    kind: "function",
                                    signature: format_func_signature(&func.sig),
                                    doc: func.sig.doc.clone(),
                                    details: Some(format!("in module {}.{}", pkg.name, q)),
                                };
                                let contents = lsp::HoverContents::Markup(lsp::MarkupContent {
                                    kind: lsp::MarkupKind::Markdown,
                                    value: info.to_markdown(),
                                });
                                return Ok(Some(lsp::Hover {
                                    contents,
                                    range: None,
                                }));
                            }
                        }
                    }
                }
            }

            // Check for type names
            for pkg in stdlib.packages() {
                // Check structs
                if let Some(s) = pkg.structs.get(&name) {
                    let mut sig = format!("struct {}", s.name);
                    if !s.generics.is_empty() {
                        sig.push_str(&format!("<{}>", s.generics.join(", ")));
                    }
                    let info = HoverInfo {
                        kind: "struct",
                        signature: sig,
                        doc: s.doc.clone(),
                        details: Some(format!("stdlib: {}", pkg.name)),
                    };
                    let contents = lsp::HoverContents::Markup(lsp::MarkupContent {
                        kind: lsp::MarkupKind::Markdown,
                        value: info.to_markdown(),
                    });
                    return Ok(Some(lsp::Hover {
                        contents,
                        range: None,
                    }));
                }

                // Check interfaces
                if let Some(i) = pkg.interfaces.get(&name) {
                    let mut sig = format!("interface {}", i.name);
                    if !i.generics.is_empty() {
                        sig.push_str(&format!("<{}>", i.generics.join(", ")));
                    }
                    let info = HoverInfo {
                        kind: "interface",
                        signature: sig,
                        doc: i.doc.clone(),
                        details: Some(format!("stdlib: {}", pkg.name)),
                    };
                    let contents = lsp::HoverContents::Markup(lsp::MarkupContent {
                        kind: lsp::MarkupKind::Markdown,
                        value: info.to_markdown(),
                    });
                    return Ok(Some(lsp::Hover {
                        contents,
                        range: None,
                    }));
                }

                // Check enums
                if let Some(e) = pkg.enums.get(&name) {
                    let mut sig = format!("enum {}", e.name);
                    if !e.generics.is_empty() {
                        sig.push_str(&format!("<{}>", e.generics.join(", ")));
                    }
                    let info = HoverInfo {
                        kind: "enum",
                        signature: sig,
                        doc: e.doc.clone(),
                        details: Some(format!("stdlib: {}", pkg.name)),
                    };
                    let contents = lsp::HoverContents::Markup(lsp::MarkupContent {
                        kind: lsp::MarkupKind::Markdown,
                        value: info.to_markdown(),
                    });
                    return Ok(Some(lsp::Hover {
                        contents,
                        range: None,
                    }));
                }

                // Check modules
                if pkg.modules.contains_key(&name) {
                    let module = pkg.modules.get(&name).unwrap();
                    let func_names: Vec<&str> =
                        module.functions.iter().map(|f| f.name.as_str()).collect();
                    let info = HoverInfo {
                        kind: "module",
                        signature: format!("module {}", name),
                        doc: module.doc.clone(),
                        details: Some(format!(
                            "stdlib: {} | {} function(s): {}",
                            pkg.name,
                            module.functions.len(),
                            if func_names.len() <= 5 {
                                func_names.join(", ")
                            } else {
                                format!("{}, ...", func_names[..5].join(", "))
                            }
                        )),
                    };
                    let contents = lsp::HoverContents::Markup(lsp::MarkupContent {
                        kind: lsp::MarkupKind::Markdown,
                        value: info.to_markdown(),
                    });
                    return Ok(Some(lsp::Hover {
                        contents,
                        range: None,
                    }));
                }
            }
        }

        // Provide hover for primitive types
        let primitive_info: Option<(&str, &str)> = match name.as_str() {
            "int" => Some(("int", "32-bit signed integer")),
            "short" => Some(("short", "16-bit signed integer")),
            "long" => Some(("long", "64-bit signed integer")),
            "float" => Some(("float", "32-bit floating point")),
            "double" => Some(("double", "64-bit floating point")),
            "bool" => Some(("bool", "Boolean (true or false)")),
            "char" => Some(("char", "Unicode character")),
            "string" => Some(("string", "UTF-8 string")),
            "bytes" => Some(("bytes", "Byte array")),
            "void" => Some(("void", "No return value")),
            "i8" => Some(("i8", "8-bit signed integer")),
            "i16" => Some(("i16", "16-bit signed integer")),
            "i32" => Some(("i32", "32-bit signed integer")),
            "i64" => Some(("i64", "64-bit signed integer")),
            "i128" => Some(("i128", "128-bit signed integer")),
            "u8" => Some(("u8", "8-bit unsigned integer")),
            "u16" => Some(("u16", "16-bit unsigned integer")),
            "u32" => Some(("u32", "32-bit unsigned integer")),
            "u64" => Some(("u64", "64-bit unsigned integer")),
            "u128" => Some(("u128", "128-bit unsigned integer")),
            "f32" => Some(("f32", "32-bit floating point")),
            "f64" => Some(("f64", "64-bit floating point")),
            _ => None,
        };

        if let Some((type_name, desc)) = primitive_info {
            let info = HoverInfo {
                kind: "primitive type",
                signature: type_name.to_string(),
                doc: Some(desc.to_string()),
                details: None,
            };
            let contents = lsp::HoverContents::Markup(lsp::MarkupContent {
                kind: lsp::MarkupKind::Markdown,
                value: info.to_markdown(),
            });
            return Ok(Some(lsp::Hover {
                contents,
                range: None,
            }));
        }

        // Provide hover for keywords
        let keyword_info: Option<&str> = match name.as_str() {
            "package" => Some("Declares the package name for this file"),
            "import" => Some("Imports symbols from another package"),
            "module" => Some("Defines a module containing functions"),
            "struct" => Some("Defines a data structure with fields"),
            "interface" => Some("Defines an interface with method signatures"),
            "enum" => Some("Defines an enumeration type with variants"),
            "provider" => Some("Defines a provider for long-lived state"),
            "public" => Some("Makes a symbol visible outside the package"),
            "private" => Some("Restricts visibility to the current file"),
            "internal" => Some("Restricts visibility to the current package"),
            "async" => Some("Marks a function as asynchronous"),
            "await" => Some("Waits for an async operation to complete"),
            "throws" => Some("Declares exceptions a function may throw"),
            "try" => Some("Begins an exception handling block"),
            "catch" => Some("Handles a specific exception type"),
            "finally" => Some("Code that always runs after try/catch"),
            "throw" => Some("Throws an exception"),
            "return" => Some("Returns a value from a function"),
            "if" => Some("Conditional branch"),
            "else" => Some("Alternative branch"),
            "while" => Some("Loop while condition is true"),
            "for" => Some("Iterate over a range or collection"),
            "switch" => Some("Multi-way branch based on value"),
            "case" => Some("A branch in a switch statement"),
            "break" => Some("Exit from a loop or switch"),
            "continue" => Some("Skip to next loop iteration"),
            "final" => Some("Marks a field as immutable after initialization"),
            "static" => Some("Associates with the type rather than an instance"),
            "shared" => Some("Marks a field as thread-safe shared state"),
            "extends" => Some("Specifies interface inheritance or type bounds"),
            "implements" => Some("Declares interface implementation"),
            _ => None,
        };

        if let Some(desc) = keyword_info {
            let info = HoverInfo {
                kind: "keyword",
                signature: name.clone(),
                doc: Some(desc.to_string()),
                details: None,
            };
            let contents = lsp::HoverContents::Markup(lsp::MarkupContent {
                kind: lsp::MarkupKind::Markdown,
                value: info.to_markdown(),
            });
            return Ok(Some(lsp::Hover {
                contents,
                range: None,
            }));
        }

        Ok(None)
    }

    async fn signature_help(
        &self,
        params: lsp::SignatureHelpParams,
    ) -> Result<Option<lsp::SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        let docs = self.documents.read().await;
        let Some(state) = docs.get(&uri) else {
            return Ok(None);
        };

        // Get the current line and parse call context
        let line = state.text.lines().nth(pos.line as usize).unwrap_or("");
        let col = pos.character as usize;

        let Some(ctx) = parse_call_context(line, col) else {
            return Ok(None);
        };

        // Try to find the function signature in current document
        if let Some(ref ast) = state.ast {
            if let Some(mut sig_info) =
                find_func_signature(ast, &ctx.func_name, ctx.qualifier.as_deref())
            {
                sig_info.active_parameter = Some(ctx.active_param as u32);
                return Ok(Some(lsp::SignatureHelp {
                    signatures: vec![sig_info],
                    active_signature: Some(0),
                    active_parameter: Some(ctx.active_param as u32),
                }));
            }
        }

        // Search other open documents
        for (other_uri, other_state) in docs.iter() {
            if *other_uri == uri {
                continue;
            }
            if let Some(ref ast) = other_state.ast {
                if let Some(mut sig_info) =
                    find_func_signature(ast, &ctx.func_name, ctx.qualifier.as_deref())
                {
                    sig_info.active_parameter = Some(ctx.active_param as u32);
                    return Ok(Some(lsp::SignatureHelp {
                        signatures: vec![sig_info],
                        active_signature: Some(0),
                        active_parameter: Some(ctx.active_param as u32),
                    }));
                }
            }
        }

        // Release lock before file I/O
        drop(docs);

        // Search workspace files
        let roots = self.workspace_roots.read().await;
        for root in roots.iter() {
            let files = find_arth_files_in_dir(root);
            for file_path in files {
                if let Ok(file_uri) = lsp::Url::from_file_path(&file_path) {
                    let docs = self.documents.read().await;
                    if docs.contains_key(&file_uri) {
                        continue;
                    }
                    drop(docs);
                }

                if let Ok(text) = std::fs::read_to_string(&file_path) {
                    let sf = SourceFile {
                        path: file_path.clone(),
                        text: text.clone(),
                    };
                    let mut reporter = Reporter::new();
                    let ast = parse_file(&sf, &mut reporter);

                    if let Some(mut sig_info) =
                        find_func_signature(&ast, &ctx.func_name, ctx.qualifier.as_deref())
                    {
                        sig_info.active_parameter = Some(ctx.active_param as u32);
                        return Ok(Some(lsp::SignatureHelp {
                            signatures: vec![sig_info],
                            active_signature: Some(0),
                            active_parameter: Some(ctx.active_param as u32),
                        }));
                    }
                }
            }
        }
        drop(roots);

        // Search stdlib
        if let Some(ref stdlib) = *self.stdlib {
            // Check for qualified call: Module.func()
            if let Some(ref qualifier) = ctx.qualifier {
                for pkg in stdlib.packages() {
                    if let Some(module) = pkg.modules.get(qualifier) {
                        for func in &module.functions {
                            if func.name == ctx.func_name {
                                let mut sig_info = create_stdlib_signature_info(func);
                                sig_info.active_parameter = Some(ctx.active_param as u32);
                                return Ok(Some(lsp::SignatureHelp {
                                    signatures: vec![sig_info],
                                    active_signature: Some(0),
                                    active_parameter: Some(ctx.active_param as u32),
                                }));
                            }
                        }
                    }
                }
            }

            // Check for unqualified call to any stdlib module
            for pkg in stdlib.packages() {
                for module in pkg.modules.values() {
                    for func in &module.functions {
                        if func.name == ctx.func_name {
                            let mut sig_info = create_stdlib_signature_info(func);
                            sig_info.active_parameter = Some(ctx.active_param as u32);
                            return Ok(Some(lsp::SignatureHelp {
                                signatures: vec![sig_info],
                                active_signature: Some(0),
                                active_parameter: Some(ctx.active_param as u32),
                            }));
                        }
                    }
                }
            }
        }

        // Handle builtin functions
        let builtin_sigs: &[(&str, &str, &[(&str, &str)])] = &[
            ("println", "void", &[("message", "string")]),
            ("print", "void", &[("message", "string")]),
            (
                "assert",
                "void",
                &[("condition", "bool"), ("message", "string")],
            ),
            ("spawn", "Task<T>", &[("func", "() -> T")]),
            ("spawnBlocking", "Task<T>", &[("func", "() -> T")]),
        ];

        for (name, ret, params) in builtin_sigs {
            if *name == ctx.func_name {
                let mut label = format!("{} {}(", ret, name);
                let mut param_infos = Vec::new();

                for (i, (pname, ptype)) in params.iter().enumerate() {
                    if i > 0 {
                        label.push_str(", ");
                    }
                    let start = label.len() as u32;
                    label.push_str(&format!("{}: {}", pname, ptype));
                    let end = label.len() as u32;

                    param_infos.push(lsp::ParameterInformation {
                        label: lsp::ParameterLabel::LabelOffsets([start, end]),
                        documentation: None,
                    });
                }
                label.push(')');

                let sig_info = lsp::SignatureInformation {
                    label,
                    documentation: Some(lsp::Documentation::String(format!(
                        "Built-in function: {}",
                        name
                    ))),
                    parameters: Some(param_infos),
                    active_parameter: Some(ctx.active_param as u32),
                };

                return Ok(Some(lsp::SignatureHelp {
                    signatures: vec![sig_info],
                    active_signature: Some(0),
                    active_parameter: Some(ctx.active_param as u32),
                }));
            }
        }

        Ok(None)
    }

    async fn completion(
        &self,
        params: lsp::CompletionParams,
    ) -> Result<Option<lsp::CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;

        // Get document state
        let docs = self.documents.read().await;
        let state = docs.get(uri);

        // Get the current line
        let line = state
            .map(|s| {
                s.text
                    .lines()
                    .nth(pos.line as usize)
                    .unwrap_or("")
                    .to_string()
            })
            .unwrap_or_default();

        // Determine completion context
        let col = pos.character as usize;
        let context = determine_context(&line, col);

        // Extract the prefix being typed for filtering
        let prefix = extract_prefix(&line, col);

        let mut items = Vec::new();

        match context {
            CompletionContext::Attribute => {
                // Show attribute names
                for attr in ARTH_ATTRIBUTES {
                    if prefix.is_empty() || attr.starts_with(&prefix) {
                        let insert_text = if *attr == "derive" {
                            "derive($0)".to_string()
                        } else {
                            attr.to_string()
                        };
                        items.push(lsp::CompletionItem {
                            label: attr.to_string(),
                            kind: Some(lsp::CompletionItemKind::KEYWORD),
                            detail: Some("Attribute".into()),
                            insert_text: Some(insert_text),
                            insert_text_format: Some(lsp::InsertTextFormat::SNIPPET),
                            ..Default::default()
                        });
                    }
                }
            }

            CompletionContext::DeriveArgs => {
                // Show derive macro arguments
                for arg in ARTH_DERIVE_ARGS {
                    if prefix.is_empty() || arg.starts_with(&prefix) {
                        items.push(lsp::CompletionItem {
                            label: arg.to_string(),
                            kind: Some(lsp::CompletionItemKind::VALUE),
                            detail: Some("Derive trait".into()),
                            ..Default::default()
                        });
                    }
                }
            }

            CompletionContext::TypePosition | CompletionContext::ExtendsClause => {
                // Show types: primitives, builtins, user-defined, stdlib
                for ty in ARTH_PRIMITIVE_TYPES {
                    if prefix.is_empty() || ty.starts_with(&prefix) {
                        items.push(lsp::CompletionItem {
                            label: ty.to_string(),
                            kind: Some(lsp::CompletionItemKind::TYPE_PARAMETER),
                            detail: Some("Primitive type".into()),
                            sort_text: Some(format!("0_{}", ty)), // Primitives first
                            ..Default::default()
                        });
                    }
                }

                for ty in ARTH_BUILTIN_TYPES {
                    if prefix.is_empty() || ty.starts_with(&prefix) {
                        items.push(lsp::CompletionItem {
                            label: ty.to_string(),
                            kind: Some(lsp::CompletionItemKind::CLASS),
                            detail: Some("Built-in type".into()),
                            sort_text: Some(format!("1_{}", ty)),
                            ..Default::default()
                        });
                    }
                }

                // Add user-defined types from AST
                if let Some(s) = state {
                    if let Some(ref ast) = s.ast {
                        for (name, kind) in extract_user_symbols(ast) {
                            if matches!(
                                kind,
                                lsp::CompletionItemKind::STRUCT
                                    | lsp::CompletionItemKind::INTERFACE
                                    | lsp::CompletionItemKind::ENUM
                                    | lsp::CompletionItemKind::CLASS
                                    | lsp::CompletionItemKind::TYPE_PARAMETER
                            ) {
                                if prefix.is_empty() || name.starts_with(&prefix) {
                                    items.push(lsp::CompletionItem {
                                        label: name,
                                        kind: Some(kind),
                                        detail: Some("User-defined type".into()),
                                        sort_text: Some("2_".into()),
                                        ..Default::default()
                                    });
                                }
                            }
                        }
                    }
                }

                // Add stdlib types
                if let Some(ref stdlib) = *self.stdlib {
                    for pkg in stdlib.packages() {
                        for name in pkg.structs.keys() {
                            if prefix.is_empty() || name.starts_with(&prefix) {
                                items.push(lsp::CompletionItem {
                                    label: name.clone(),
                                    kind: Some(lsp::CompletionItemKind::STRUCT),
                                    detail: Some(format!("stdlib: {}", pkg.name)),
                                    sort_text: Some(format!("3_{}", name)),
                                    ..Default::default()
                                });
                            }
                        }
                        for name in pkg.interfaces.keys() {
                            if prefix.is_empty() || name.starts_with(&prefix) {
                                items.push(lsp::CompletionItem {
                                    label: name.clone(),
                                    kind: Some(lsp::CompletionItemKind::INTERFACE),
                                    detail: Some(format!("stdlib: {}", pkg.name)),
                                    sort_text: Some(format!("3_{}", name)),
                                    ..Default::default()
                                });
                            }
                        }
                        for name in pkg.enums.keys() {
                            if prefix.is_empty() || name.starts_with(&prefix) {
                                items.push(lsp::CompletionItem {
                                    label: name.clone(),
                                    kind: Some(lsp::CompletionItemKind::ENUM),
                                    detail: Some(format!("stdlib: {}", pkg.name)),
                                    sort_text: Some(format!("3_{}", name)),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
            }

            CompletionContext::ThrowsClause => {
                // Show exception types
                for exc in ARTH_EXCEPTIONS {
                    if prefix.is_empty() || exc.starts_with(&prefix) {
                        items.push(lsp::CompletionItem {
                            label: exc.to_string(),
                            kind: Some(lsp::CompletionItemKind::CLASS),
                            detail: Some("Exception type".into()),
                            ..Default::default()
                        });
                    }
                }

                // Add stdlib exception types (structs ending in Error)
                if let Some(ref stdlib) = *self.stdlib {
                    for pkg in stdlib.packages() {
                        for name in pkg.structs.keys() {
                            if name.ends_with("Error")
                                && (prefix.is_empty() || name.starts_with(&prefix))
                            {
                                items.push(lsp::CompletionItem {
                                    label: name.clone(),
                                    kind: Some(lsp::CompletionItemKind::CLASS),
                                    detail: Some(format!("Exception: {}", pkg.name)),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
            }

            CompletionContext::ImportPath => {
                // Show package names from stdlib
                if let Some(ref stdlib) = *self.stdlib {
                    for pkg_name in stdlib.package_names() {
                        if prefix.is_empty() || pkg_name.starts_with(&prefix) {
                            items.push(lsp::CompletionItem {
                                label: pkg_name.to_string(),
                                kind: Some(lsp::CompletionItemKind::MODULE),
                                detail: Some("Package".into()),
                                ..Default::default()
                            });
                        }
                    }
                }
            }

            CompletionContext::MemberAccess => {
                // After a dot - show modules and their functions from stdlib
                // Extract the identifier before the dot
                let before_dot = extract_receiver(&line, col);

                if let Some(ref stdlib) = *self.stdlib {
                    // Check if it's a module name
                    for pkg in stdlib.packages() {
                        for (mod_name, module) in &pkg.modules {
                            if mod_name == &before_dot {
                                // Show functions from this module
                                for func in &module.functions {
                                    items.push(lsp::CompletionItem {
                                        label: func.name.clone(),
                                        kind: Some(lsp::CompletionItemKind::FUNCTION),
                                        detail: Some(format!("{}.{}", mod_name, func.name)),
                                        insert_text: Some(format!("{}($0)", func.name)),
                                        insert_text_format: Some(lsp::InsertTextFormat::SNIPPET),
                                        ..Default::default()
                                    });
                                }
                            }
                        }
                    }
                }

                // Also show user-defined symbols
                if let Some(s) = state {
                    if let Some(ref ast) = s.ast {
                        for decl in &ast.decls {
                            if let Decl::Module(m) = decl {
                                if m.name.0 == before_dot {
                                    for func in &m.items {
                                        items.push(lsp::CompletionItem {
                                            label: func.sig.name.0.clone(),
                                            kind: Some(lsp::CompletionItemKind::FUNCTION),
                                            detail: Some(format!(
                                                "{}.{}",
                                                m.name.0, func.sig.name.0
                                            )),
                                            insert_text: Some(format!("{}($0)", func.sig.name.0)),
                                            insert_text_format: Some(
                                                lsp::InsertTextFormat::SNIPPET,
                                            ),
                                            ..Default::default()
                                        });
                                    }
                                }
                            }
                            if let Decl::Enum(e) = decl {
                                if e.name.0 == before_dot {
                                    for variant in &e.variants {
                                        items.push(lsp::CompletionItem {
                                            label: variant.name().0.clone(),
                                            kind: Some(lsp::CompletionItemKind::ENUM_MEMBER),
                                            detail: Some(format!(
                                                "{}.{}",
                                                e.name.0,
                                                variant.name().0
                                            )),
                                            ..Default::default()
                                        });
                                    }
                                }
                            }
                        }
                    }
                }

                // If no specific members found, show general stdlib modules
                if items.is_empty() {
                    if let Some(ref stdlib) = *self.stdlib {
                        for pkg in stdlib.packages() {
                            for mod_name in pkg.modules.keys() {
                                items.push(lsp::CompletionItem {
                                    label: mod_name.clone(),
                                    kind: Some(lsp::CompletionItemKind::MODULE),
                                    detail: Some(format!("Module: {}", pkg.name)),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
            }

            CompletionContext::TopLevel => {
                // Show declaration keywords
                let decl_keywords = [
                    "package",
                    "import",
                    "module",
                    "struct",
                    "interface",
                    "enum",
                    "provider",
                    "type",
                    "public",
                    "private",
                    "internal",
                    "export",
                    "sealed",
                    "async",
                    "static",
                    "final",
                    "extern",
                    "unsafe",
                ];
                for kw in decl_keywords {
                    if prefix.is_empty() || kw.starts_with(&prefix) {
                        items.push(lsp::CompletionItem {
                            label: kw.to_string(),
                            kind: Some(lsp::CompletionItemKind::KEYWORD),
                            sort_text: Some(format!("0_{}", kw)),
                            ..Default::default()
                        });
                    }
                }
            }

            CompletionContext::Identifier => {
                // Show all: keywords, types, builtins, user symbols

                // Keywords
                for kw in ARTH_KEYWORDS {
                    if prefix.is_empty() || kw.starts_with(&prefix) {
                        items.push(lsp::CompletionItem {
                            label: kw.to_string(),
                            kind: Some(lsp::CompletionItemKind::KEYWORD),
                            sort_text: Some(format!("2_{}", kw)),
                            ..Default::default()
                        });
                    }
                }

                // Built-in functions
                for (name, snippet) in ARTH_BUILTINS {
                    if prefix.is_empty() || name.starts_with(&prefix) {
                        items.push(lsp::CompletionItem {
                            label: name.to_string(),
                            kind: Some(lsp::CompletionItemKind::FUNCTION),
                            detail: Some("Built-in function".into()),
                            insert_text: Some(snippet.to_string()),
                            insert_text_format: Some(lsp::InsertTextFormat::SNIPPET),
                            sort_text: Some(format!("0_{}", name)),
                            ..Default::default()
                        });
                    }
                }

                // Primitive types
                for ty in ARTH_PRIMITIVE_TYPES {
                    if prefix.is_empty() || ty.starts_with(&prefix) {
                        items.push(lsp::CompletionItem {
                            label: ty.to_string(),
                            kind: Some(lsp::CompletionItemKind::TYPE_PARAMETER),
                            detail: Some("Primitive type".into()),
                            sort_text: Some(format!("1_{}", ty)),
                            ..Default::default()
                        });
                    }
                }

                // Built-in types
                for ty in ARTH_BUILTIN_TYPES {
                    if prefix.is_empty() || ty.starts_with(&prefix) {
                        items.push(lsp::CompletionItem {
                            label: ty.to_string(),
                            kind: Some(lsp::CompletionItemKind::CLASS),
                            detail: Some("Built-in type".into()),
                            sort_text: Some(format!("1_{}", ty)),
                            ..Default::default()
                        });
                    }
                }

                // User symbols from AST
                if let Some(s) = state {
                    if let Some(ref ast) = s.ast {
                        for (name, kind) in extract_user_symbols(ast) {
                            if prefix.is_empty() || name.starts_with(&prefix) {
                                items.push(lsp::CompletionItem {
                                    label: name,
                                    kind: Some(kind),
                                    detail: Some("User-defined".into()),
                                    sort_text: Some("0_".into()),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }

                // Stdlib modules
                if let Some(ref stdlib) = *self.stdlib {
                    for pkg in stdlib.packages() {
                        for mod_name in pkg.modules.keys() {
                            if prefix.is_empty() || mod_name.starts_with(&prefix) {
                                items.push(lsp::CompletionItem {
                                    label: mod_name.clone(),
                                    kind: Some(lsp::CompletionItemKind::MODULE),
                                    detail: Some(format!("Module: {}", pkg.name)),
                                    sort_text: Some(format!("3_{}", mod_name)),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
            }
        }

        // Deduplicate by label
        let mut seen = std::collections::HashSet::new();
        items.retain(|item| seen.insert(item.label.clone()));

        Ok(Some(lsp::CompletionResponse::Array(items)))
    }

    async fn goto_definition(
        &self,
        params: lsp::GotoDefinitionParams,
    ) -> Result<Option<lsp::GotoDefinitionResponse>> {
        let lsp::TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let uri = text_document.uri;
        let docs = self.documents.read().await;
        let Some(state) = docs.get(&uri) else {
            return Ok(None);
        };

        // Get the line and extract qualified name (e.g., "Module.function" or just "function")
        let line = state.text.lines().nth(position.line as usize).unwrap_or("");
        let col = position.character as usize;
        let (qualifier, name) = extract_qualified_name(line, col);

        if name.is_empty() {
            return Ok(None);
        }

        // Build search target
        let qualified_target = qualifier.as_ref().map(|q| format!("{}.{}", q, name));

        // Helper to find symbol in a list
        let find_symbol = |symbols: &[SymbolLocation]| -> Option<lsp::Range> {
            // First try qualified match
            if let Some(ref qt) = qualified_target {
                if let Some(sym) = symbols
                    .iter()
                    .find(|s| s.qualified_name.as_ref() == Some(qt))
                {
                    return Some(sym.name_range);
                }
            }

            // Then try simple name match
            // If we have a qualifier, look for the qualifier as a module/struct/enum
            if let Some(ref q) = qualifier {
                if let Some(sym) = symbols.iter().find(|s| s.name == *q) {
                    return Some(sym.name_range);
                }
            }

            // Otherwise match by simple name
            if let Some(sym) = symbols.iter().find(|s| s.name == name) {
                return Some(sym.name_range);
            }

            None
        };

        // 1. Search current document using AST
        if let Some(ref ast) = state.ast {
            let symbols = extract_symbols_with_locations(ast, &state.text);
            if let Some(range) = find_symbol(&symbols) {
                return Ok(Some(lsp::GotoDefinitionResponse::Scalar(lsp::Location {
                    uri: uri.clone(),
                    range,
                })));
            }
        }

        // 2. Search other open documents
        for (other_uri, other_state) in docs.iter() {
            if *other_uri == uri {
                continue;
            }
            if let Some(ref ast) = other_state.ast {
                let symbols = extract_symbols_with_locations(ast, &other_state.text);
                if let Some(range) = find_symbol(&symbols) {
                    return Ok(Some(lsp::GotoDefinitionResponse::Scalar(lsp::Location {
                        uri: other_uri.clone(),
                        range,
                    })));
                }
            }
        }

        // Release the docs lock before file I/O
        drop(docs);

        // 3. Search workspace files (not yet open)
        let roots = self.workspace_roots.read().await;
        for root in roots.iter() {
            let files = find_arth_files_in_dir(root);
            for file_path in files {
                // Skip files we already checked (open documents)
                if let Ok(file_uri) = lsp::Url::from_file_path(&file_path) {
                    let docs = self.documents.read().await;
                    if docs.contains_key(&file_uri) {
                        continue;
                    }
                    drop(docs);
                }

                if let Some((file_uri, symbols)) = parse_file_symbols(&file_path) {
                    if let Some(range) = find_symbol(&symbols) {
                        return Ok(Some(lsp::GotoDefinitionResponse::Scalar(lsp::Location {
                            uri: file_uri,
                            range,
                        })));
                    }
                }
            }
        }

        // 4. If we have a qualifier, search stdlib for module.function patterns
        if let Some(ref stdlib) = *self.stdlib {
            if let Some(ref q) = qualifier {
                // Check if qualifier is a stdlib module
                for pkg in stdlib.packages() {
                    if let Some(module) = pkg.modules.get(q) {
                        // Found the module, look for the function
                        if module.functions.iter().any(|f| f.name == name) {
                            // Return location to the stdlib source file if available
                            if let Some(src_file) = pkg.source_files.first() {
                                if let Ok(file_uri) = lsp::Url::from_file_path(src_file) {
                                    if let Some((_, symbols)) = parse_file_symbols(src_file) {
                                        if let Some(range) = find_symbol(&symbols) {
                                            return Ok(Some(lsp::GotoDefinitionResponse::Scalar(
                                                lsp::Location {
                                                    uri: file_uri,
                                                    range,
                                                },
                                            )));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    async fn references(&self, params: lsp::ReferenceParams) -> Result<Option<Vec<lsp::Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let include_declaration = params.context.include_declaration;

        let docs = self.documents.read().await;
        let Some(state) = docs.get(&uri) else {
            return Ok(None);
        };

        // Get the symbol name at the cursor position
        let line = state.text.lines().nth(pos.line as usize).unwrap_or("");
        let col = pos.character as usize;

        let Some(name) = word_at(line, col) else {
            return Ok(None);
        };
        let name = name.to_string();

        // Check for qualified name (Module.function)
        let qualifier = extract_qualifier_at(line, col);

        let mut all_refs = Vec::new();

        // 1. Search current document
        if let Some(ref ast) = state.ast {
            let refs = find_references_in_ast(
                ast,
                &state.text,
                &name,
                qualifier.as_deref(),
                &uri,
                include_declaration,
            );
            all_refs.extend(refs);
        } else {
            let refs = find_references_in_text(&state.text, &name, &uri);
            all_refs.extend(refs);
        }

        // 2. Search other open documents
        for (other_uri, other_state) in docs.iter() {
            if *other_uri == uri {
                continue;
            }
            if let Some(ref ast) = other_state.ast {
                let refs = find_references_in_ast(
                    ast,
                    &other_state.text,
                    &name,
                    qualifier.as_deref(),
                    other_uri,
                    true, // Include declarations in other files
                );
                all_refs.extend(refs);
            } else {
                let refs = find_references_in_text(&other_state.text, &name, other_uri);
                all_refs.extend(refs);
            }
        }

        // Release lock before file I/O
        drop(docs);

        // 3. Search workspace files (not yet open)
        let roots = self.workspace_roots.read().await;
        for root in roots.iter() {
            let files = find_arth_files_in_dir(root);
            for file_path in files {
                // Skip files we already checked (open documents)
                if let Ok(file_uri) = lsp::Url::from_file_path(&file_path) {
                    let docs = self.documents.read().await;
                    if docs.contains_key(&file_uri) {
                        continue;
                    }
                    drop(docs);

                    // Parse and search the file
                    if let Ok(text) = std::fs::read_to_string(&file_path) {
                        let sf = SourceFile {
                            path: file_path.clone(),
                            text: text.clone(),
                        };
                        let mut reporter = Reporter::new();
                        let ast = parse_file(&sf, &mut reporter);

                        let refs = find_references_in_ast(
                            &ast,
                            &text,
                            &name,
                            qualifier.as_deref(),
                            &file_uri,
                            true,
                        );
                        all_refs.extend(refs);
                    }
                }
            }
        }

        if all_refs.is_empty() {
            Ok(None)
        } else {
            // Sort references by file URI and position for consistent results
            all_refs.sort_by(|a, b| {
                a.uri
                    .as_str()
                    .cmp(b.uri.as_str())
                    .then(a.range.start.line.cmp(&b.range.start.line))
                    .then(a.range.start.character.cmp(&b.range.start.character))
            });
            Ok(Some(all_refs))
        }
    }

    async fn prepare_rename(
        &self,
        params: lsp::TextDocumentPositionParams,
    ) -> Result<Option<lsp::PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let pos = params.position;

        let docs = self.documents.read().await;
        let Some(state) = docs.get(&uri) else {
            return Ok(None);
        };

        // Check if the symbol at cursor can be renamed
        let col = pos.character as usize;
        match can_rename_at(&state.text, pos.line, col) {
            Some((range, _name)) => Ok(Some(lsp::PrepareRenameResponse::Range(range))),
            None => {
                // Return an error to indicate rename is not possible here
                Ok(None)
            }
        }
    }

    async fn rename(&self, params: lsp::RenameParams) -> Result<Option<lsp::WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let new_name = params.new_name;

        // Validate the new name is a valid identifier
        if new_name.is_empty() {
            return Ok(None);
        }
        let first_char = new_name.chars().next().unwrap();
        if !first_char.is_alphabetic() && first_char != '_' {
            return Ok(None);
        }
        if !new_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Ok(None);
        }

        // Check if new name is a keyword
        if ARTH_KEYWORDS.contains(&new_name.as_str()) {
            return Ok(None);
        }

        let docs = self.documents.read().await;
        let Some(state) = docs.get(&uri) else {
            return Ok(None);
        };

        // Get the old symbol name
        let line = state.text.lines().nth(pos.line as usize).unwrap_or("");
        let col = pos.character as usize;
        let Some(old_name) = word_at(line, col) else {
            return Ok(None);
        };
        let old_name = old_name.to_string();

        // Check if this symbol can be renamed
        if can_rename_at(&state.text, pos.line, col).is_none() {
            return Ok(None);
        }

        let mut changes: HashMap<lsp::Url, Vec<lsp::TextEdit>> = HashMap::new();

        // 1. Apply rename to current document
        let edits = apply_rename_to_text(&state.text, &old_name, &new_name, &uri);
        if !edits.is_empty() {
            changes.insert(uri.clone(), edits);
        }

        // 2. Apply rename to other open documents
        for (other_uri, other_state) in docs.iter() {
            if *other_uri == uri {
                continue;
            }
            let edits = apply_rename_to_text(&other_state.text, &old_name, &new_name, other_uri);
            if !edits.is_empty() {
                changes.insert(other_uri.clone(), edits);
            }
        }

        // Release lock before file I/O
        drop(docs);

        // 3. Apply rename to workspace files (not yet open)
        let roots = self.workspace_roots.read().await;
        for root in roots.iter() {
            let files = find_arth_files_in_dir(root);
            for file_path in files {
                if let Ok(file_uri) = lsp::Url::from_file_path(&file_path) {
                    // Skip files we already processed
                    if changes.contains_key(&file_uri) {
                        continue;
                    }

                    let docs = self.documents.read().await;
                    if docs.contains_key(&file_uri) {
                        continue;
                    }
                    drop(docs);

                    // Read and process the file
                    if let Ok(text) = std::fs::read_to_string(&file_path) {
                        let edits = apply_rename_to_text(&text, &old_name, &new_name, &file_uri);
                        if !edits.is_empty() {
                            changes.insert(file_uri, edits);
                        }
                    }
                }
            }
        }

        if changes.is_empty() {
            Ok(None)
        } else {
            Ok(Some(lsp::WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            }))
        }
    }

    async fn document_symbol(
        &self,
        params: lsp::DocumentSymbolParams,
    ) -> Result<Option<lsp::DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let docs = self.documents.read().await;
        let Some(state) = docs.get(&uri) else {
            return Ok(Some(lsp::DocumentSymbolResponse::Flat(vec![])));
        };

        // Use AST-based symbol extraction for comprehensive symbols
        let symbols = if let Some(ref ast) = state.ast {
            extract_symbols_with_locations(ast, &state.text)
        } else {
            // Fallback to regex-based function parsing
            let funcs = parse_functions(&state.text);
            funcs
                .into_iter()
                .map(|f| SymbolLocation {
                    name: f.name,
                    qualified_name: None,
                    kind: lsp::SymbolKind::FUNCTION,
                    name_range: f.name_range,
                    full_range: f.name_range,
                })
                .collect()
        };

        let mut syms = Vec::with_capacity(symbols.len());
        for sym in symbols {
            // Determine container name from qualified name
            let container_name = sym.qualified_name.as_ref().and_then(|qn| {
                qn.rsplit_once('.')
                    .map(|(container, _)| container.to_string())
            });

            #[allow(deprecated)]
            let si = lsp::SymbolInformation {
                name: sym.name,
                kind: sym.kind,
                tags: None,
                deprecated: None,
                location: lsp::Location {
                    uri: uri.clone(),
                    range: sym.name_range,
                },
                container_name,
            };
            syms.push(si);
        }
        Ok(Some(lsp::DocumentSymbolResponse::Flat(syms)))
    }
}

#[tokio::main]
async fn main() {
    // Stdio transport for typical editor integration
    let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());

    // Try to load stdlib for completions
    let stdlib = match find_stdlib_path() {
        Some(path) => match StdlibIndex::load(&path) {
            Ok(index) => {
                eprintln!(
                    "Loaded stdlib from {:?}: {} packages",
                    path,
                    index.package_count()
                );
                Some(index)
            }
            Err(e) => {
                eprintln!("Failed to load stdlib: {}", e);
                None
            }
        },
        None => {
            eprintln!("Stdlib not found, completions will be limited");
            None
        }
    };

    let stdlib = Arc::new(stdlib);
    let workspace_roots = Arc::new(RwLock::new(Vec::new()));

    let (service, socket) = LspService::new(|client| Backend {
        client,
        documents: Arc::new(RwLock::new(HashMap::new())),
        stdlib: stdlib.clone(),
        workspace_roots: workspace_roots.clone(),
    });

    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── determine_context ──────────────────────────────────────────

    #[test]
    fn context_attribute_after_at() {
        assert_eq!(determine_context("@test", 5), CompletionContext::Attribute);
    }

    #[test]
    fn context_derive_args() {
        assert_eq!(
            determine_context("@derive(Eq", 10),
            CompletionContext::DeriveArgs
        );
    }

    #[test]
    fn context_member_access_after_dot() {
        assert_eq!(
            determine_context("obj.", 4),
            CompletionContext::MemberAccess
        );
    }

    #[test]
    fn context_type_position_after_colon() {
        assert_eq!(
            determine_context("int x: ", 7),
            CompletionContext::TypePosition
        );
    }

    #[test]
    fn context_import_path() {
        // "import demo" without trailing dot → ImportPath
        assert_eq!(
            determine_context("import demo", 11),
            CompletionContext::ImportPath
        );
    }

    #[test]
    fn context_import_with_dot_is_member_access() {
        // "import demo." with trailing dot → MemberAccess (dot takes priority)
        assert_eq!(
            determine_context("import demo.", 12),
            CompletionContext::MemberAccess
        );
    }

    #[test]
    fn context_top_level_empty() {
        assert_eq!(determine_context("", 0), CompletionContext::TopLevel);
    }

    #[test]
    fn context_identifier_in_body() {
        assert_eq!(
            determine_context("    let x = foo", 15),
            CompletionContext::Identifier
        );
    }

    // ── extract_prefix ─────────────────────────────────────────────

    #[test]
    fn prefix_simple_ident() {
        assert_eq!(extract_prefix("    let ab", 10), "ab");
    }

    #[test]
    fn prefix_empty_at_start() {
        assert_eq!(extract_prefix("", 0), "");
    }

    #[test]
    fn prefix_after_dot() {
        // After "foo." the prefix should be empty (the dot resets it)
        let p = extract_prefix("foo.", 4);
        assert!(
            p.is_empty() || p == ".",
            "prefix after dot should be empty, got: {p}"
        );
    }

    // ── word_at ────────────────────────────────────────────────────

    #[test]
    fn word_at_middle() {
        assert_eq!(word_at("hello world", 2), Some("hello"));
    }

    #[test]
    fn word_at_second_word() {
        assert_eq!(word_at("hello world", 8), Some("world"));
    }

    #[test]
    fn word_at_empty() {
        assert_eq!(word_at("", 0), None);
    }

    #[test]
    fn word_at_space_boundary() {
        assert_eq!(word_at("a b", 1), Some("a"));
    }

    // ── compute_line_offsets ───────────────────────────────────────

    #[test]
    fn line_offsets_single_line() {
        assert_eq!(compute_line_offsets("hello"), vec![0]);
    }

    #[test]
    fn line_offsets_two_lines() {
        assert_eq!(compute_line_offsets("ab\ncd"), vec![0, 3]);
    }

    #[test]
    fn line_offsets_empty() {
        assert_eq!(compute_line_offsets(""), vec![0]);
    }

    #[test]
    fn line_offsets_trailing_newline() {
        assert_eq!(compute_line_offsets("a\n"), vec![0, 2]);
    }

    // ── offset_to_position ─────────────────────────────────────────

    #[test]
    fn offset_to_pos_first_line() {
        let offsets = compute_line_offsets("hello\nworld");
        let pos = offset_to_position(3, &offsets);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 3);
    }

    #[test]
    fn offset_to_pos_second_line() {
        let offsets = compute_line_offsets("hello\nworld");
        let pos = offset_to_position(8, &offsets);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 2);
    }

    #[test]
    fn offset_to_pos_line_start() {
        let offsets = compute_line_offsets("abc\ndef");
        let pos = offset_to_position(4, &offsets);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 0);
    }

    // ── is_ident_char ──────────────────────────────────────────────

    #[test]
    fn ident_char_alpha() {
        assert!(is_ident_char(b'a'));
        assert!(is_ident_char(b'Z'));
        assert!(is_ident_char(b'_'));
    }

    #[test]
    fn ident_char_non_ident() {
        assert!(!is_ident_char(b'.'));
        assert!(!is_ident_char(b' '));
        assert!(!is_ident_char(b'@'));
    }

    // ── extract_qualified_name ─────────────────────────────────────

    #[test]
    fn qualified_name_simple() {
        let (qualifier, name) = extract_qualified_name("let x = Foo", 11);
        assert!(qualifier.is_none());
        assert_eq!(name, "Foo");
    }

    #[test]
    fn qualified_name_dotted() {
        let (qualifier, name) = extract_qualified_name("Module.func()", 11);
        assert_eq!(qualifier.as_deref(), Some("Module"));
        assert_eq!(name, "func");
    }

    // ── compile_document ──────────────────────────────────────────

    #[test]
    fn compile_valid_document_has_ast() {
        let uri = lsp::Url::parse("file:///test.arth").unwrap();
        let text = "package test;\n\nmodule M { public void main() {} }\n";
        let state = compile_document(&uri, text);
        assert!(state.ast.is_some(), "valid doc should produce AST");
        assert!(
            state.diagnostics.is_empty(),
            "valid doc should have no errors"
        );
    }

    #[test]
    fn compile_invalid_document_has_diagnostics() {
        let uri = lsp::Url::parse("file:///test.arth").unwrap();
        let text = "package test\nmodule M {}\n"; // missing semicolons
        let state = compile_document(&uri, text);
        assert!(
            !state.diagnostics.is_empty(),
            "invalid doc should produce diagnostics"
        );
    }

    // ── to_lsp_diagnostics ────────────────────────────────────────

    #[test]
    fn lsp_diagnostics_conversion() {
        let text = "package test;\n\nmodule M {}\n";
        let diags = vec![Diagnostic {
            severity: Severity::Error,
            message: "test error".into(),
            file: None,
            code: Some("E001".into()),
            span: Some(arth::compiler::source::Span {
                start: 0,
                end: 7,
                start_line: 1,
                start_col: 1,
                end_line: 1,
                end_col: 8,
            }),
            labels: Vec::new(),
            suggestion: None,
        }];
        let lsp_diags = to_lsp_diagnostics(&diags, text);
        assert_eq!(lsp_diags.len(), 1);
        assert_eq!(lsp_diags[0].message, "test error");
        assert_eq!(lsp_diags[0].severity, Some(lsp::DiagnosticSeverity::ERROR));
        assert_eq!(
            lsp_diags[0].code,
            Some(lsp::NumberOrString::String("E001".into()))
        );
    }

    // ── is_partial_type ────────────────────────────────────────────

    #[test]
    fn partial_type_detection() {
        assert!(is_partial_type("Int"));
        assert!(is_partial_type("string"));
        assert!(!is_partial_type(""));
        assert!(!is_partial_type("123"));
    }
}
