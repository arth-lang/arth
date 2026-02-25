//! Attribute registry and validation for the Arth compiler.
//!
//! This module defines built-in attributes, their valid targets, argument schemas,
//! and provides validation infrastructure for the type checking phase.

use std::collections::HashSet;

use crate::compiler::source::Span;

/// Represents which language construct an attribute is attached to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AttrTarget {
    /// Function (including module methods)
    Function,
    /// Struct declaration
    Struct,
    /// Struct or provider field
    Field,
    /// Module declaration
    Module,
    /// Interface declaration
    Interface,
    /// Enum declaration
    Enum,
    /// Provider declaration
    Provider,
    /// Type alias declaration
    TypeAlias,
    /// External function declaration (FFI)
    ExternFunc,
}

impl AttrTarget {
    /// Human-readable description for error messages.
    pub fn description(self) -> &'static str {
        match self {
            AttrTarget::Function => "function",
            AttrTarget::Struct => "struct",
            AttrTarget::Field => "field",
            AttrTarget::Module => "module",
            AttrTarget::Interface => "interface",
            AttrTarget::Enum => "enum",
            AttrTarget::Provider => "provider",
            AttrTarget::TypeAlias => "type alias",
            AttrTarget::ExternFunc => "extern function",
        }
    }
}

/// Defines which argument forms an attribute accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttrArgSchema {
    /// No arguments allowed: `@test`
    None,
    /// Optional arguments (attribute works with or without): `@inline` or `@inline(always)`
    Optional(Box<AttrArgSchema>),
    /// Single string literal: `@intrinsic("math.sqrt")`
    SingleString,
    /// Single identifier: `@allow(unused_variable)`
    SingleIdent,
    /// List of identifiers: `@derive(Eq, Hash, Show)`
    IdentList,
    /// Key-value pairs: `@deprecated(since="2025.1", note="Use Foo")`
    KeyValuePairs,
    /// Raw expression string (for defaults): `@default(expr="DateTime.now()")`
    Expression,
}

/// Built-in attribute definitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltinAttr {
    /// `@test` - marks a function as a test
    Test,
    /// `@bench` - marks a function as a benchmark
    Bench,
    /// `@deprecated(since="...", note="...")` - marks declaration as deprecated
    Deprecated,
    /// `@inline` or `@inline(always)` / `@inline(never)` - inlining hint
    Inline,
    /// `@must_use` or `@must_use("reason")` - warns if return value unused
    MustUse,
    /// `@allow("lint-id")` - suppresses specific lint
    Allow,
    /// `@derive(Trait1, Trait2, ...)` - request codegen implementations
    Derive,
    /// `@intrinsic("domain.operation")` - maps function to intrinsic
    Intrinsic,
    /// `@rename(name="json_name")` or `@rename("name")` - field rename for codecs
    Rename,
    /// `@default(expr="...")` - default value for field
    Default,
    /// `@JsonIgnore` - skip field in JSON codec
    JsonIgnore,
    /// `@ffi_owned` - FFI ownership: caller owns returned value
    FfiOwned,
    /// `@ffi_borrowed` - FFI ownership: value is borrowed
    FfiBorrowed,
    /// `@ffi_transfers` - FFI ownership: ownership transfers to callee
    FfiTransfers,
    /// `@cfg(backend = "...")` - conditional compilation based on target backend
    Cfg,
}

impl BuiltinAttr {
    /// Canonical name of the attribute (without @).
    pub fn name(self) -> &'static str {
        match self {
            BuiltinAttr::Test => "test",
            BuiltinAttr::Bench => "bench",
            BuiltinAttr::Deprecated => "deprecated",
            BuiltinAttr::Inline => "inline",
            BuiltinAttr::MustUse => "must_use",
            BuiltinAttr::Allow => "allow",
            BuiltinAttr::Derive => "derive",
            BuiltinAttr::Intrinsic => "intrinsic",
            BuiltinAttr::Rename => "rename",
            BuiltinAttr::Default => "default",
            BuiltinAttr::JsonIgnore => "JsonIgnore",
            BuiltinAttr::FfiOwned => "ffi_owned",
            BuiltinAttr::FfiBorrowed => "ffi_borrowed",
            BuiltinAttr::FfiTransfers => "ffi_transfers",
            BuiltinAttr::Cfg => "cfg",
        }
    }

    /// Try to parse an attribute name into a builtin.
    pub fn from_name(name: &str) -> Option<BuiltinAttr> {
        match name {
            "test" => Some(BuiltinAttr::Test),
            "bench" => Some(BuiltinAttr::Bench),
            "deprecated" => Some(BuiltinAttr::Deprecated),
            "inline" => Some(BuiltinAttr::Inline),
            "must_use" => Some(BuiltinAttr::MustUse),
            "allow" => Some(BuiltinAttr::Allow),
            "derive" => Some(BuiltinAttr::Derive),
            "intrinsic" => Some(BuiltinAttr::Intrinsic),
            "rename" => Some(BuiltinAttr::Rename),
            "default" => Some(BuiltinAttr::Default),
            "JsonIgnore" => Some(BuiltinAttr::JsonIgnore),
            "ffi_owned" => Some(BuiltinAttr::FfiOwned),
            "ffi_borrowed" => Some(BuiltinAttr::FfiBorrowed),
            "ffi_transfers" => Some(BuiltinAttr::FfiTransfers),
            "cfg" => Some(BuiltinAttr::Cfg),
            _ => None,
        }
    }

    /// Returns allowed targets for this attribute.
    pub fn allowed_targets(self) -> &'static [AttrTarget] {
        use AttrTarget::*;
        match self {
            BuiltinAttr::Test | BuiltinAttr::Bench => &[Function],
            BuiltinAttr::Deprecated => &[
                Function, Struct, Field, Module, Interface, Enum, Provider, TypeAlias, ExternFunc,
            ],
            BuiltinAttr::Inline => &[Function, ExternFunc],
            BuiltinAttr::MustUse => &[Function, Struct, Enum],
            BuiltinAttr::Allow => &[
                Function, Struct, Field, Module, Interface, Enum, Provider, TypeAlias, ExternFunc,
            ],
            BuiltinAttr::Derive => &[Struct, Enum],
            BuiltinAttr::Intrinsic => &[Function, ExternFunc],
            BuiltinAttr::Rename => &[Field],
            BuiltinAttr::Default => &[Field],
            BuiltinAttr::JsonIgnore => &[Field],
            BuiltinAttr::FfiOwned | BuiltinAttr::FfiBorrowed | BuiltinAttr::FfiTransfers => {
                &[ExternFunc]
            }
            // @cfg can be applied to any declaration for conditional compilation
            BuiltinAttr::Cfg => &[
                Function, Struct, Field, Module, Interface, Enum, Provider, TypeAlias, ExternFunc,
            ],
        }
    }

    /// Returns expected argument schema for this attribute.
    pub fn arg_schema(self) -> AttrArgSchema {
        match self {
            BuiltinAttr::Test | BuiltinAttr::Bench | BuiltinAttr::JsonIgnore => AttrArgSchema::None,
            BuiltinAttr::Deprecated => AttrArgSchema::KeyValuePairs,
            BuiltinAttr::Inline => AttrArgSchema::Optional(Box::new(AttrArgSchema::SingleIdent)),
            BuiltinAttr::MustUse => AttrArgSchema::Optional(Box::new(AttrArgSchema::SingleString)),
            BuiltinAttr::Allow => AttrArgSchema::SingleIdent,
            BuiltinAttr::Derive => AttrArgSchema::IdentList,
            BuiltinAttr::Intrinsic => AttrArgSchema::SingleString,
            BuiltinAttr::Rename => AttrArgSchema::KeyValuePairs, // name="..." or just "..."
            BuiltinAttr::Default => AttrArgSchema::KeyValuePairs, // expr="..."
            BuiltinAttr::FfiOwned | BuiltinAttr::FfiBorrowed | BuiltinAttr::FfiTransfers => {
                AttrArgSchema::None
            }
            // @cfg uses key=value for conditions: @cfg(backend = "vm")
            BuiltinAttr::Cfg => AttrArgSchema::KeyValuePairs,
        }
    }

    /// Whether this attribute can appear multiple times on the same target.
    pub fn allow_multiple(self) -> bool {
        match self {
            BuiltinAttr::Allow => true, // Can suppress multiple lints
            BuiltinAttr::Cfg => true,   // Multiple cfg conditions can be stacked
            _ => false,
        }
    }
}

/// Parsed attribute argument values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttrArg {
    /// No arguments
    None,
    /// Single string value: "value"
    String(String),
    /// Single identifier: value
    Ident(String),
    /// List of identifiers: A, B, C
    IdentList(Vec<String>),
    /// Key-value pairs: key="value", flag=true
    KeyValues(Vec<(String, String)>),
}

/// A validated attribute with parsed arguments.
#[derive(Clone, Debug)]
pub struct ValidatedAttr {
    pub builtin: Option<BuiltinAttr>,
    pub name: String,
    pub args: AttrArg,
    pub span: Option<Span>,
}

/// Attribute validation error.
#[derive(Clone, Debug)]
pub struct AttrError {
    pub message: String,
    pub span: Option<Span>,
}

impl AttrError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span: None,
        }
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }
}

/// Parse attribute arguments from a raw string.
pub fn parse_attr_args(raw: Option<&str>) -> AttrArg {
    let Some(raw) = raw else {
        return AttrArg::None;
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return AttrArg::None;
    }

    // Try to parse as key=value pairs first
    if let Some(kv) = try_parse_key_values(trimmed) {
        return AttrArg::KeyValues(kv);
    }

    // Try to parse as identifier list (comma-separated identifiers)
    if let Some(idents) = try_parse_ident_list(trimmed) {
        return AttrArg::IdentList(idents);
    }

    // Try to parse as single string
    if let Some(s) = try_parse_string(trimmed) {
        return AttrArg::String(s);
    }

    // Try to parse as single identifier
    if is_identifier(trimmed) {
        return AttrArg::Ident(trimmed.to_string());
    }

    // Fallback: treat entire content as a string
    AttrArg::String(trimmed.to_string())
}

fn try_parse_string(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        // Remove quotes and unescape basic sequences
        let inner = &trimmed[1..trimmed.len() - 1];
        Some(unescape_string(inner))
    } else {
        None
    }
}

fn unescape_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some('"') => result.push('"'),
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn is_identifier(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return false;
    }
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

fn try_parse_ident_list(s: &str) -> Option<Vec<String>> {
    // Only parse as list if there's at least one comma
    if !s.contains(',') {
        return None;
    }
    let parts: Vec<&str> = s.split(',').map(|p| p.trim()).collect();
    if parts.is_empty() {
        return None;
    }
    // All parts must be identifiers
    if parts.iter().all(|p| is_identifier(p)) {
        Some(parts.iter().map(|p| p.to_string()).collect())
    } else {
        None
    }
}

fn try_parse_key_values(s: &str) -> Option<Vec<(String, String)>> {
    // Check if it looks like key=value format
    if !s.contains('=') {
        return None;
    }

    let mut result = Vec::new();
    let mut remaining = s;

    while !remaining.trim().is_empty() {
        remaining = remaining.trim();

        // Parse key
        let key_end = remaining.find('=').or_else(|| remaining.find(','))?;
        let eq_pos = remaining.find('=')?;

        if eq_pos > key_end {
            return None; // Comma before equals, invalid
        }

        let key = remaining[..eq_pos].trim();
        if !is_identifier(key) {
            return None;
        }

        remaining = &remaining[eq_pos + 1..];
        remaining = remaining.trim();

        // Parse value
        let value = if remaining.starts_with('"') {
            // String value
            let end = find_string_end(remaining)?;
            let val = &remaining[1..end];
            remaining = &remaining[end + 1..];
            unescape_string(val)
        } else {
            // Identifier or other value
            let end = remaining.find(',').unwrap_or(remaining.len());
            let val = remaining[..end].trim();
            remaining = if end < remaining.len() {
                &remaining[end..]
            } else {
                ""
            };
            val.to_string()
        };

        result.push((key.to_string(), value));

        // Skip comma if present
        remaining = remaining.trim();
        if remaining.starts_with(',') {
            remaining = &remaining[1..];
        }
    }

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn find_string_end(s: &str) -> Option<usize> {
    // s starts with '"', find matching closing quote
    let inner = &s[1..];
    let mut i = 0;
    let chars: Vec<char> = inner.chars().collect();
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2; // Skip escaped char
        } else if chars[i] == '"' {
            return Some(i + 1); // +1 because we skipped opening quote
        } else {
            i += 1;
        }
    }
    None
}

/// Validate an attribute against its target and schema.
pub fn validate_attr(
    name: &str,
    args: Option<&str>,
    target: AttrTarget,
) -> Result<ValidatedAttr, AttrError> {
    let builtin = BuiltinAttr::from_name(name);

    // If it's a known builtin, validate target and arguments
    if let Some(b) = builtin {
        // Check target
        let allowed = b.allowed_targets();
        if !allowed.contains(&target) {
            let allowed_str: Vec<_> = allowed.iter().map(|t| t.description()).collect();
            return Err(AttrError::new(format!(
                "attribute @{} is not allowed on {}; allowed on: {}",
                name,
                target.description(),
                allowed_str.join(", ")
            )));
        }

        // Parse and validate arguments
        let parsed_args = parse_attr_args(args);
        validate_args_against_schema(name, &parsed_args, b.arg_schema())?;

        Ok(ValidatedAttr {
            builtin: Some(b),
            name: name.to_string(),
            args: parsed_args,
            span: None,
        })
    } else {
        // Unknown attribute - emit warning but allow it
        // Custom attributes may be used by external tools
        Ok(ValidatedAttr {
            builtin: None,
            name: name.to_string(),
            args: parse_attr_args(args),
            span: None,
        })
    }
}

fn validate_args_against_schema(
    name: &str,
    args: &AttrArg,
    schema: AttrArgSchema,
) -> Result<(), AttrError> {
    match schema {
        AttrArgSchema::None => {
            if !matches!(args, AttrArg::None) {
                return Err(AttrError::new(format!(
                    "attribute @{} does not accept arguments",
                    name
                )));
            }
        }
        AttrArgSchema::Optional(inner_schema) => {
            if !matches!(args, AttrArg::None) {
                validate_args_against_schema(name, args, *inner_schema)?;
            }
        }
        AttrArgSchema::SingleString => {
            if !matches!(args, AttrArg::String(_)) {
                return Err(AttrError::new(format!(
                    "attribute @{} expects a string argument: @{}(\"value\")",
                    name, name
                )));
            }
        }
        AttrArgSchema::SingleIdent => {
            let is_valid = match args {
                AttrArg::Ident(_) => true,
                AttrArg::IdentList(v) if v.len() == 1 => true,
                _ => false,
            };
            if !is_valid {
                return Err(AttrError::new(format!(
                    "attribute @{} expects an identifier argument: @{}(name)",
                    name, name
                )));
            }
        }
        AttrArgSchema::IdentList => {
            if !matches!(args, AttrArg::IdentList(_) | AttrArg::Ident(_)) {
                return Err(AttrError::new(format!(
                    "attribute @{} expects a list of identifiers: @{}(A, B, C)",
                    name, name
                )));
            }
        }
        AttrArgSchema::KeyValuePairs => {
            // Accept key-values, single string, or single ident (for shorthand)
            match args {
                AttrArg::KeyValues(_) | AttrArg::String(_) | AttrArg::Ident(_) => {}
                _ => {
                    return Err(AttrError::new(format!(
                        "attribute @{} expects key=value arguments or a string",
                        name
                    )));
                }
            }
        }
        AttrArgSchema::Expression => {
            // Expression is stored as string or key-value with expr key
            match args {
                AttrArg::String(_) => {}
                AttrArg::KeyValues(kv) => {
                    if !kv.iter().any(|(k, _)| k == "expr") {
                        return Err(AttrError::new(format!(
                            "attribute @{} expects expr=\"...\" argument",
                            name
                        )));
                    }
                }
                _ => {
                    return Err(AttrError::new(format!(
                        "attribute @{} expects an expression: @{}(expr=\"...\")",
                        name, name
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Check for duplicate attributes on a target.
pub fn check_duplicates(attrs: &[ValidatedAttr]) -> Vec<AttrError> {
    let mut errors = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();

    for attr in attrs {
        if let Some(builtin) = attr.builtin {
            if !builtin.allow_multiple() && seen.contains(attr.name.as_str()) {
                errors.push(AttrError::new(format!(
                    "duplicate attribute @{} is not allowed",
                    attr.name
                )));
            }
            seen.insert(&attr.name);
        }
    }

    errors
}

/// Known derive targets for validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeriveTarget {
    Eq,
    Hash,
    Show,
    JsonCodec,
    BinaryCodec,
    Default,
    Copy,
    Clone,
}

impl DeriveTarget {
    pub fn from_name(name: &str) -> Option<DeriveTarget> {
        match name {
            "Eq" => Some(DeriveTarget::Eq),
            "Hash" => Some(DeriveTarget::Hash),
            "Show" => Some(DeriveTarget::Show),
            "JsonCodec" => Some(DeriveTarget::JsonCodec),
            "BinaryCodec" => Some(DeriveTarget::BinaryCodec),
            "Default" => Some(DeriveTarget::Default),
            "Copy" => Some(DeriveTarget::Copy),
            "Clone" => Some(DeriveTarget::Clone),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DeriveTarget::Eq => "Eq",
            DeriveTarget::Hash => "Hash",
            DeriveTarget::Show => "Show",
            DeriveTarget::JsonCodec => "JsonCodec",
            DeriveTarget::BinaryCodec => "BinaryCodec",
            DeriveTarget::Default => "Default",
            DeriveTarget::Copy => "Copy",
            DeriveTarget::Clone => "Clone",
        }
    }
}

/// Parse derive arguments and return the list of derive targets.
pub fn parse_derive_targets(args: &AttrArg) -> Result<Vec<DeriveTarget>, AttrError> {
    let idents = match args {
        AttrArg::IdentList(list) => list.clone(),
        AttrArg::Ident(id) => vec![id.clone()],
        _ => {
            return Err(AttrError::new(
                "@derive expects a list of derive targets: @derive(Eq, Hash, JsonCodec)",
            ));
        }
    };

    let mut targets = Vec::new();
    for ident in &idents {
        match DeriveTarget::from_name(ident) {
            Some(t) => targets.push(t),
            None => {
                return Err(AttrError::new(format!(
                    "unknown derive target '{}'; known targets: Eq, Hash, Show, JsonCodec, BinaryCodec, Default, Copy, Clone",
                    ident
                )));
            }
        }
    }

    Ok(targets)
}

/// Parse @deprecated arguments.
#[derive(Clone, Debug, Default)]
pub struct DeprecatedInfo {
    pub since: Option<String>,
    pub note: Option<String>,
}

pub fn parse_deprecated_args(args: &AttrArg) -> DeprecatedInfo {
    match args {
        AttrArg::KeyValues(kv) => {
            let mut info = DeprecatedInfo::default();
            for (key, value) in kv {
                match key.as_str() {
                    "since" => info.since = Some(value.clone()),
                    "note" => info.note = Some(value.clone()),
                    _ => {} // Ignore unknown keys
                }
            }
            info
        }
        AttrArg::String(s) => DeprecatedInfo {
            since: None,
            note: Some(s.clone()),
        },
        _ => DeprecatedInfo::default(),
    }
}

/// Parse @inline arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineHint {
    /// Default: compiler decides
    Default,
    /// `@inline(always)` - always inline
    Always,
    /// `@inline(never)` - never inline
    Never,
}

pub fn parse_inline_args(args: &AttrArg) -> InlineHint {
    match args {
        AttrArg::Ident(s) | AttrArg::String(s) => match s.as_str() {
            "always" => InlineHint::Always,
            "never" => InlineHint::Never,
            _ => InlineHint::Default,
        },
        AttrArg::IdentList(list) if list.len() == 1 => match list[0].as_str() {
            "always" => InlineHint::Always,
            "never" => InlineHint::Never,
            _ => InlineHint::Default,
        },
        _ => InlineHint::Default,
    }
}

/// Parse @must_use reason if provided.
pub fn parse_must_use_reason(args: &AttrArg) -> Option<String> {
    match args {
        AttrArg::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Parse @rename arguments.
pub fn parse_rename_args(args: &AttrArg) -> Option<String> {
    match args {
        AttrArg::String(s) => Some(s.clone()),
        AttrArg::KeyValues(kv) => {
            for (key, value) in kv {
                if key == "name" {
                    return Some(value.clone());
                }
            }
            None
        }
        _ => None,
    }
}

/// Parse @default expression.
pub fn parse_default_expr(args: &AttrArg) -> Option<String> {
    match args {
        AttrArg::String(s) => Some(s.clone()),
        AttrArg::KeyValues(kv) => {
            for (key, value) in kv {
                if key == "expr" {
                    return Some(value.clone());
                }
            }
            None
        }
        _ => None,
    }
}

// =============================================================================
// Conditional Compilation (@cfg)
// =============================================================================

/// Target backend for conditional compilation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CfgBackend {
    /// VM bytecode interpreter/JIT
    Vm,
    /// LLVM native compilation
    Llvm,
    /// Cranelift JIT compilation
    Cranelift,
}

impl CfgBackend {
    /// Parse backend name from string.
    pub fn from_name(name: &str) -> Option<CfgBackend> {
        match name.to_lowercase().as_str() {
            "vm" => Some(CfgBackend::Vm),
            "llvm" | "native" => Some(CfgBackend::Llvm),
            "cranelift" | "jit" => Some(CfgBackend::Cranelift),
            _ => None,
        }
    }

    /// Get canonical name.
    pub fn name(self) -> &'static str {
        match self {
            CfgBackend::Vm => "vm",
            CfgBackend::Llvm => "llvm",
            CfgBackend::Cranelift => "cranelift",
        }
    }
}

/// A conditional compilation predicate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CfgPredicate {
    /// Backend match: @cfg(backend = "vm")
    Backend(CfgBackend),
    /// Feature flag: @cfg(feature = "async")
    Feature(String),
    /// OS target: @cfg(os = "linux")
    Os(String),
    /// Architecture: @cfg(arch = "x86_64")
    Arch(String),
    /// Negation: @cfg(not(backend = "vm"))
    Not(Box<CfgPredicate>),
    /// All conditions: @cfg(all(backend = "vm", feature = "async"))
    All(Vec<CfgPredicate>),
    /// Any condition: @cfg(any(backend = "vm", backend = "cranelift"))
    Any(Vec<CfgPredicate>),
}

impl CfgPredicate {
    /// Evaluate this predicate against a given backend.
    pub fn evaluate(&self, current_backend: CfgBackend) -> bool {
        match self {
            CfgPredicate::Backend(b) => *b == current_backend,
            CfgPredicate::Feature(_feature) => {
                // TODO: Check against enabled features from manifest
                false
            }
            CfgPredicate::Os(os) => {
                let current_os = if cfg!(target_os = "macos") {
                    "macos"
                } else if cfg!(target_os = "linux") {
                    "linux"
                } else if cfg!(target_os = "windows") {
                    "windows"
                } else {
                    "unknown"
                };
                os == current_os
            }
            CfgPredicate::Arch(arch) => {
                let current_arch = if cfg!(target_arch = "x86_64") {
                    "x86_64"
                } else if cfg!(target_arch = "aarch64") {
                    "aarch64"
                } else if cfg!(target_arch = "arm") {
                    "arm"
                } else {
                    "unknown"
                };
                arch == current_arch
            }
            CfgPredicate::Not(inner) => !inner.evaluate(current_backend),
            CfgPredicate::All(preds) => preds.iter().all(|p| p.evaluate(current_backend)),
            CfgPredicate::Any(preds) => preds.iter().any(|p| p.evaluate(current_backend)),
        }
    }
}

/// Error when parsing cfg predicate.
#[derive(Clone, Debug)]
pub struct CfgParseError {
    pub message: String,
}

impl CfgParseError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
        }
    }
}

/// Parse @cfg arguments into a predicate.
pub fn parse_cfg_predicate(args: &AttrArg) -> Result<CfgPredicate, CfgParseError> {
    match args {
        AttrArg::KeyValues(kv) => {
            if kv.len() != 1 {
                return Err(CfgParseError::new(
                    "@cfg expects exactly one condition: @cfg(backend = \"vm\")",
                ));
            }
            let (key, value) = &kv[0];
            parse_single_cfg_kv(key, value)
        }
        AttrArg::Ident(ident) => {
            // Support shorthand: @cfg(vm) as alias for @cfg(backend = "vm")
            if let Some(backend) = CfgBackend::from_name(ident) {
                Ok(CfgPredicate::Backend(backend))
            } else {
                Err(CfgParseError::new(format!(
                    "unknown cfg shorthand '{}'; use @cfg(backend = \"vm\") syntax",
                    ident
                )))
            }
        }
        _ => Err(CfgParseError::new(
            "@cfg expects key=value syntax: @cfg(backend = \"vm\")",
        )),
    }
}

fn parse_single_cfg_kv(key: &str, value: &str) -> Result<CfgPredicate, CfgParseError> {
    match key {
        "backend" => {
            let backend = CfgBackend::from_name(value).ok_or_else(|| {
                CfgParseError::new(format!(
                    "unknown backend '{}'; valid backends: vm, llvm, cranelift",
                    value
                ))
            })?;
            Ok(CfgPredicate::Backend(backend))
        }
        "feature" => Ok(CfgPredicate::Feature(value.to_string())),
        "os" => Ok(CfgPredicate::Os(value.to_string())),
        "arch" => Ok(CfgPredicate::Arch(value.to_string())),
        _ => Err(CfgParseError::new(format!(
            "unknown cfg key '{}'; valid keys: backend, feature, os, arch",
            key
        ))),
    }
}

/// Check if all cfg predicates on an item pass for the given backend.
pub fn evaluate_cfg_attrs(attrs: &[ValidatedAttr], backend: CfgBackend) -> bool {
    for attr in attrs {
        if attr.builtin == Some(BuiltinAttr::Cfg) {
            match parse_cfg_predicate(&attr.args) {
                Ok(pred) => {
                    if !pred.evaluate(backend) {
                        return false;
                    }
                }
                Err(_) => {
                    // Invalid cfg predicate - skip (error reported elsewhere)
                    continue;
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_from_name() {
        assert_eq!(BuiltinAttr::from_name("test"), Some(BuiltinAttr::Test));
        assert_eq!(
            BuiltinAttr::from_name("deprecated"),
            Some(BuiltinAttr::Deprecated)
        );
        assert_eq!(BuiltinAttr::from_name("unknown"), None);
    }

    #[test]
    fn test_parse_attr_args_none() {
        assert_eq!(parse_attr_args(None), AttrArg::None);
        assert_eq!(parse_attr_args(Some("")), AttrArg::None);
        assert_eq!(parse_attr_args(Some("  ")), AttrArg::None);
    }

    #[test]
    fn test_parse_attr_args_string() {
        assert_eq!(
            parse_attr_args(Some("\"math.sqrt\"")),
            AttrArg::String("math.sqrt".to_string())
        );
    }

    #[test]
    fn test_parse_attr_args_ident() {
        assert_eq!(
            parse_attr_args(Some("always")),
            AttrArg::Ident("always".to_string())
        );
    }

    #[test]
    fn test_parse_attr_args_ident_list() {
        assert_eq!(
            parse_attr_args(Some("Eq, Hash, Show")),
            AttrArg::IdentList(vec![
                "Eq".to_string(),
                "Hash".to_string(),
                "Show".to_string()
            ])
        );
    }

    #[test]
    fn test_parse_attr_args_key_values() {
        let args = parse_attr_args(Some("since=\"2025.1\", note=\"Use Foo\""));
        match args {
            AttrArg::KeyValues(kv) => {
                assert_eq!(kv.len(), 2);
                assert!(kv.iter().any(|(k, v)| k == "since" && v == "2025.1"));
                assert!(kv.iter().any(|(k, v)| k == "note" && v == "Use Foo"));
            }
            _ => panic!("expected KeyValues"),
        }
    }

    #[test]
    fn test_validate_test_attr() {
        // @test on function - valid
        let result = validate_attr("test", None, AttrTarget::Function);
        assert!(result.is_ok());

        // @test on struct - invalid
        let result = validate_attr("test", None, AttrTarget::Struct);
        assert!(result.is_err());

        // @test with args - invalid
        let result = validate_attr("test", Some("foo"), AttrTarget::Function);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_derive_attr() {
        // @derive on struct - valid
        let result = validate_attr("derive", Some("Eq, Hash"), AttrTarget::Struct);
        assert!(result.is_ok());

        // @derive on function - invalid
        let result = validate_attr("derive", Some("Eq"), AttrTarget::Function);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_derive_targets() {
        let args = AttrArg::IdentList(vec!["Eq".to_string(), "Hash".to_string()]);
        let targets = parse_derive_targets(&args).unwrap();
        assert_eq!(targets, vec![DeriveTarget::Eq, DeriveTarget::Hash]);

        // Unknown target
        let args = AttrArg::IdentList(vec!["Unknown".to_string()]);
        assert!(parse_derive_targets(&args).is_err());
    }

    #[test]
    fn test_parse_deprecated_args() {
        let args = AttrArg::KeyValues(vec![
            ("since".to_string(), "2025.1".to_string()),
            ("note".to_string(), "Use NewThing".to_string()),
        ]);
        let info = parse_deprecated_args(&args);
        assert_eq!(info.since, Some("2025.1".to_string()));
        assert_eq!(info.note, Some("Use NewThing".to_string()));
    }

    #[test]
    fn test_check_duplicates() {
        let attrs = vec![
            ValidatedAttr {
                builtin: Some(BuiltinAttr::Test),
                name: "test".to_string(),
                args: AttrArg::None,
                span: None,
            },
            ValidatedAttr {
                builtin: Some(BuiltinAttr::Test),
                name: "test".to_string(),
                args: AttrArg::None,
                span: None,
            },
        ];
        let errors = check_duplicates(&attrs);
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_inline_hint_parsing() {
        assert_eq!(parse_inline_args(&AttrArg::None), InlineHint::Default);
        assert_eq!(
            parse_inline_args(&AttrArg::Ident("always".to_string())),
            InlineHint::Always
        );
        assert_eq!(
            parse_inline_args(&AttrArg::Ident("never".to_string())),
            InlineHint::Never
        );
    }

    // =========================================================================
    // Cfg attribute tests
    // =========================================================================

    #[test]
    fn test_cfg_backend_from_name() {
        assert_eq!(CfgBackend::from_name("vm"), Some(CfgBackend::Vm));
        assert_eq!(CfgBackend::from_name("VM"), Some(CfgBackend::Vm));
        assert_eq!(CfgBackend::from_name("llvm"), Some(CfgBackend::Llvm));
        assert_eq!(CfgBackend::from_name("native"), Some(CfgBackend::Llvm));
        assert_eq!(
            CfgBackend::from_name("cranelift"),
            Some(CfgBackend::Cranelift)
        );
        assert_eq!(CfgBackend::from_name("jit"), Some(CfgBackend::Cranelift));
        assert_eq!(CfgBackend::from_name("unknown"), None);
    }

    #[test]
    fn test_validate_cfg_attr() {
        // @cfg on function - valid
        let result = validate_attr("cfg", Some("backend=\"vm\""), AttrTarget::Function);
        assert!(result.is_ok());

        // @cfg on struct - valid
        let result = validate_attr("cfg", Some("backend=\"llvm\""), AttrTarget::Struct);
        assert!(result.is_ok());

        // @cfg on module - valid
        let result = validate_attr("cfg", Some("backend=\"cranelift\""), AttrTarget::Module);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_cfg_predicate_backend() {
        let args = AttrArg::KeyValues(vec![("backend".to_string(), "vm".to_string())]);
        let pred = parse_cfg_predicate(&args).unwrap();
        assert_eq!(pred, CfgPredicate::Backend(CfgBackend::Vm));

        let args = AttrArg::KeyValues(vec![("backend".to_string(), "llvm".to_string())]);
        let pred = parse_cfg_predicate(&args).unwrap();
        assert_eq!(pred, CfgPredicate::Backend(CfgBackend::Llvm));
    }

    #[test]
    fn test_parse_cfg_predicate_shorthand() {
        // @cfg(vm) should work as shorthand for @cfg(backend = "vm")
        let args = AttrArg::Ident("vm".to_string());
        let pred = parse_cfg_predicate(&args).unwrap();
        assert_eq!(pred, CfgPredicate::Backend(CfgBackend::Vm));
    }

    #[test]
    fn test_parse_cfg_predicate_feature() {
        let args = AttrArg::KeyValues(vec![("feature".to_string(), "async".to_string())]);
        let pred = parse_cfg_predicate(&args).unwrap();
        assert_eq!(pred, CfgPredicate::Feature("async".to_string()));
    }

    #[test]
    fn test_parse_cfg_predicate_invalid() {
        // Unknown key
        let args = AttrArg::KeyValues(vec![("invalid".to_string(), "value".to_string())]);
        assert!(parse_cfg_predicate(&args).is_err());

        // Invalid backend
        let args = AttrArg::KeyValues(vec![("backend".to_string(), "invalid".to_string())]);
        assert!(parse_cfg_predicate(&args).is_err());
    }

    #[test]
    fn test_cfg_predicate_evaluate_backend() {
        let pred = CfgPredicate::Backend(CfgBackend::Vm);
        assert!(pred.evaluate(CfgBackend::Vm));
        assert!(!pred.evaluate(CfgBackend::Llvm));
        assert!(!pred.evaluate(CfgBackend::Cranelift));

        let pred = CfgPredicate::Backend(CfgBackend::Llvm);
        assert!(!pred.evaluate(CfgBackend::Vm));
        assert!(pred.evaluate(CfgBackend::Llvm));
    }

    #[test]
    fn test_cfg_predicate_evaluate_not() {
        let pred = CfgPredicate::Not(Box::new(CfgPredicate::Backend(CfgBackend::Vm)));
        assert!(!pred.evaluate(CfgBackend::Vm));
        assert!(pred.evaluate(CfgBackend::Llvm));
        assert!(pred.evaluate(CfgBackend::Cranelift));
    }

    #[test]
    fn test_cfg_predicate_evaluate_all() {
        let pred = CfgPredicate::All(vec![
            CfgPredicate::Backend(CfgBackend::Vm),
            CfgPredicate::Os("macos".to_string()),
        ]);
        // All must be true
        #[cfg(target_os = "macos")]
        assert!(pred.evaluate(CfgBackend::Vm));
        #[cfg(not(target_os = "macos"))]
        assert!(!pred.evaluate(CfgBackend::Vm));
    }

    #[test]
    fn test_cfg_predicate_evaluate_any() {
        let pred = CfgPredicate::Any(vec![
            CfgPredicate::Backend(CfgBackend::Vm),
            CfgPredicate::Backend(CfgBackend::Llvm),
        ]);
        assert!(pred.evaluate(CfgBackend::Vm));
        assert!(pred.evaluate(CfgBackend::Llvm));
        assert!(!pred.evaluate(CfgBackend::Cranelift));
    }

    #[test]
    fn test_evaluate_cfg_attrs() {
        let attrs = vec![ValidatedAttr {
            builtin: Some(BuiltinAttr::Cfg),
            name: "cfg".to_string(),
            args: AttrArg::KeyValues(vec![("backend".to_string(), "vm".to_string())]),
            span: None,
        }];

        assert!(evaluate_cfg_attrs(&attrs, CfgBackend::Vm));
        assert!(!evaluate_cfg_attrs(&attrs, CfgBackend::Llvm));
    }

    #[test]
    fn test_evaluate_cfg_attrs_multiple() {
        // Multiple cfg attrs should all need to pass
        let attrs = vec![
            ValidatedAttr {
                builtin: Some(BuiltinAttr::Cfg),
                name: "cfg".to_string(),
                args: AttrArg::KeyValues(vec![("backend".to_string(), "vm".to_string())]),
                span: None,
            },
            ValidatedAttr {
                builtin: Some(BuiltinAttr::Cfg),
                name: "cfg".to_string(),
                args: AttrArg::KeyValues(vec![("backend".to_string(), "llvm".to_string())]),
                span: None,
            },
        ];

        // Can't be both vm AND llvm, so this should fail
        assert!(!evaluate_cfg_attrs(&attrs, CfgBackend::Vm));
        assert!(!evaluate_cfg_attrs(&attrs, CfgBackend::Llvm));
    }

    #[test]
    fn test_evaluate_cfg_attrs_empty() {
        // No cfg attrs means everything passes
        let attrs: Vec<ValidatedAttr> = vec![];
        assert!(evaluate_cfg_attrs(&attrs, CfgBackend::Vm));
        assert!(evaluate_cfg_attrs(&attrs, CfgBackend::Llvm));
    }
}
