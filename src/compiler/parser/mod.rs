use std::cell::Cell;

use crate::compiler::ast::{
    AstId, Attr, Decl, EnumDecl, EnumVariant, ExternFuncDecl, FileAst, FuncDecl, FuncSig, Ident,
    ImportSpec, InterfaceDecl, InterfaceMethod, ModuleDecl, NamePath, PackageName, Param,
    ProviderDecl, StructDecl, StructField, Visibility,
};
use crate::compiler::diagnostics::{Diagnostic, Reporter};
use crate::compiler::lexer::{TokenKind, lex_all_with_reporter};
use crate::compiler::source::SourceFile;
use crate::compiler::source::Span;

mod attrs;
pub mod control;
pub mod expr;
mod func;
mod stmt;
mod types;
mod util;

/// Maximum allowed nesting depth for expressions and blocks.
/// Prevents stack overflow on pathological input.
pub(crate) const MAX_NESTING_DEPTH: usize = 256;

thread_local! {
    static PARSE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Increment the parse depth counter and check against the limit.
/// Returns `false` (depth exceeded) or `true` (ok to proceed).
/// Caller must call `dec_depth()` when leaving the scope.
pub(super) fn inc_depth() -> bool {
    PARSE_DEPTH.with(|d| {
        let cur = d.get();
        if cur >= MAX_NESTING_DEPTH {
            return false;
        }
        d.set(cur + 1);
        true
    })
}

/// Decrement the parse depth counter.
pub(super) fn dec_depth() {
    PARSE_DEPTH.with(|d| {
        let cur = d.get();
        if cur > 0 {
            d.set(cur - 1);
        }
    });
}

/// Reset parse depth to zero (called at the start of each file parse).
fn reset_depth() {
    PARSE_DEPTH.with(|d| d.set(0));
}

#[allow(unused_assignments)]
pub fn parse_file(sf: &SourceFile, reporter: &mut Reporter) -> FileAst {
    reset_depth();
    let tokens = lex_all_with_reporter(&sf.text, &sf.path, reporter);
    let mut i = 0usize;
    let bump = |i: &mut usize| {
        let old = *i;
        *i += 1;
        old
    };

    let mut package: Option<PackageName> = None;
    let mut imports: Vec<ImportSpec> = Vec::new();
    let mut decls: Vec<Decl> = Vec::new();
    let mut next_ast_id: u32 = 1;

    // Optional package clause at the very top
    if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::Package)) {
        bump(&mut i);
        // Expect Ident ('.' Ident)* ';'
        let mut parts: Vec<Ident> = Vec::new();
        match tokens.get(i) {
            Some(t) => match &t.kind {
                TokenKind::Ident(s) => parts.push(Ident(s.clone())),
                TokenKind::Async => parts.push(Ident("async".to_string())), // allow demo.async
                _ => {
                    reporter.emit(
                        Diagnostic::error("expected package name identifier")
                            .with_file(sf.path.clone())
                            .with_span(t.span),
                    );
                }
            },
            None => {
                reporter.emit(
                    Diagnostic::error("incomplete package declaration").with_file(sf.path.clone()),
                );
                return FileAst {
                    package: None,
                    imports: Vec::new(),
                    decls: Vec::new(),
                };
            }
        }
        bump(&mut i);
        loop {
            match tokens.get(i) {
                Some(t) if matches!(t.kind, TokenKind::Dot) => {
                    bump(&mut i);
                    match tokens.get(i) {
                        Some(t2) => match &t2.kind {
                            TokenKind::Ident(s) => {
                                parts.push(Ident(s.clone()));
                                bump(&mut i);
                            }
                            TokenKind::Async => {
                                parts.push(Ident("async".to_string()));
                                bump(&mut i);
                            }
                            _ => {
                                reporter.emit(
                                    Diagnostic::error(
                                        "expected identifier after '.' in package name",
                                    )
                                    .with_file(sf.path.clone())
                                    .with_span(t2.span),
                                );
                                break;
                            }
                        },
                        None => {
                            reporter.emit(
                                Diagnostic::error("incomplete package name after '.'")
                                    .with_file(sf.path.clone()),
                            );
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
        match tokens.get(i) {
            Some(t) if matches!(t.kind, TokenKind::Semicolon) => {
                bump(&mut i);
            }
            Some(t) => {
                reporter.emit(
                    Diagnostic::error("expected ';' after package declaration")
                        .with_file(sf.path.clone())
                        .with_span(t.span),
                );
            }
            None => {
                reporter.emit(
                    Diagnostic::error("incomplete package declaration; missing ';'")
                        .with_file(sf.path.clone()),
                );
            }
        }
        if !parts.is_empty() {
            package = Some(PackageName(parts));
        }
    }

    // Zero or more import declarations
    loop {
        if !matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::Import)) {
            break;
        }
        bump(&mut i);
        // Parse import path: ident ('.' ident)* ('.' '*')? ';'
        let mut path: Vec<Ident> = Vec::new();
        let mut star = false;
        // first segment
        match tokens.get(i) {
            Some(t) => match &t.kind {
                TokenKind::Ident(s) => {
                    path.push(Ident(s.clone()));
                    bump(&mut i);
                }
                TokenKind::Async => {
                    path.push(Ident("async".to_string()));
                    bump(&mut i);
                }
                _ => {
                    reporter.emit(
                        Diagnostic::error("expected identifier after 'import'")
                            .with_file(sf.path.clone())
                            .with_span(t.span),
                    );
                }
            },
            None => {
                reporter.emit(Diagnostic::error("incomplete import").with_file(sf.path.clone()));
                break;
            }
        }
        loop {
            match tokens.get(i) {
                Some(t) if matches!(t.kind, TokenKind::Dot) => {
                    if matches!(tokens.get(i + 1).map(|t| &t.kind), Some(TokenKind::Star)) {
                        bump(&mut i); // '.'
                        bump(&mut i); // '*'
                        star = true;
                        break;
                    } else {
                        bump(&mut i);
                        match tokens.get(i) {
                            Some(t2) => match &t2.kind {
                                TokenKind::Ident(s) => {
                                    path.push(Ident(s.clone()));
                                    bump(&mut i);
                                }
                                TokenKind::Async => {
                                    path.push(Ident("async".to_string()));
                                    bump(&mut i);
                                }
                                _ => {
                                    reporter.emit(
                                        Diagnostic::error("expected identifier after '.'")
                                            .with_file(sf.path.clone())
                                            .with_span(t2.span),
                                    );
                                    break;
                                }
                            },
                            None => {
                                reporter.emit(
                                    Diagnostic::error("incomplete import path")
                                        .with_file(sf.path.clone()),
                                );
                                break;
                            }
                        }
                    }
                }
                _ => break,
            }
        }
        // Parse optional alias: `as Alias`
        let mut alias: Option<Ident> = None;
        if matches!(tokens.get(i).map(|t| &t.kind), Some(TokenKind::As)) {
            bump(&mut i); // consume 'as'
            match tokens.get(i) {
                Some(t) => match &t.kind {
                    TokenKind::Ident(s) => {
                        alias = Some(Ident(s.clone()));
                        bump(&mut i);
                    }
                    _ => {
                        reporter.emit(
                            Diagnostic::error("expected identifier after 'as'")
                                .with_file(sf.path.clone())
                                .with_span(t.span),
                        );
                    }
                },
                None => {
                    reporter.emit(
                        Diagnostic::error("expected identifier after 'as'")
                            .with_file(sf.path.clone()),
                    );
                }
            }
        }
        match tokens.get(i) {
            Some(t) if matches!(t.kind, TokenKind::Semicolon) => {
                bump(&mut i);
            }
            Some(t) => reporter.emit(
                Diagnostic::error("expected ';' after import")
                    .with_file(sf.path.clone())
                    .with_span(t.span),
            ),
            None => reporter.emit(
                Diagnostic::error("incomplete import; missing ';'").with_file(sf.path.clone()),
            ),
        }
        if !path.is_empty() {
            imports.push(ImportSpec { path, star, alias });
        }
    }

    // Parse top-level declarations (minimal) and also try to capture Main.main body.
    let mut j = i;
    // Note: we still compute `main_body` via legacy scan below; here we only collect controls/prints.
    while j < tokens.len() {
        let start_j = j;
        // Handle leading docs/attrs at top-level: attach to decls or functions as appropriate
        let mut leading_doc: Option<String> = None;
        let mut leading_attrs: Vec<Attr> = Vec::new();
        if matches!(
            tokens.get(j).map(|t| &t.kind),
            Some(TokenKind::DocLine(_) | TokenKind::DocBlock(_) | TokenKind::At)
        ) {
            let mut k = j;
            let (doc0, attrs0) = attrs::take_leading_doc_attrs(&sf.text, &tokens, &mut k);
            match tokens.get(k).map(|t| &t.kind) {
                Some(
                    TokenKind::Module
                    | TokenKind::Struct
                    | TokenKind::Interface
                    | TokenKind::Enum
                    | TokenKind::Provider
                    | TokenKind::Extern,
                ) => {
                    // Non-function declaration: carry docs/attrs forward to the decl
                    leading_doc = doc0;
                    leading_attrs = attrs0;
                    j = k;
                }
                Some(
                    TokenKind::Public
                    | TokenKind::Internal
                    | TokenKind::Private
                    | TokenKind::Static
                    | TokenKind::Final
                    | TokenKind::Async
                    | TokenKind::Ident(_)
                    | TokenKind::Void,
                ) => {
                    // Free function: parse directly from after the docs/attrs
                    // But if a visibility is followed by a non-function decl keyword, don't parse as function.
                    if matches!(
                        tokens.get(k).map(|t| &t.kind),
                        Some(TokenKind::Public | TokenKind::Internal | TokenKind::Private)
                    ) && matches!(
                        tokens.get(k + 1).map(|t| &t.kind),
                        Some(
                            TokenKind::Struct
                                | TokenKind::Enum
                                | TokenKind::Interface
                                | TokenKind::Provider
                                | TokenKind::Module
                        )
                    ) {
                        leading_doc = doc0;
                        leading_attrs = attrs0;
                        j = k;
                        continue;
                    }
                    if let Some((fd, nj)) = func::parse_function_decl(
                        &tokens,
                        k,
                        reporter,
                        &sf.path,
                        doc0,
                        attrs0,
                        &mut next_ast_id,
                    ) {
                        decls.push(Decl::Function(fd));
                        j = nj;
                        continue;
                    } else {
                        j = util::skip_to_sync(&tokens, k);
                        continue;
                    }
                }
                _ => {
                    // Unknown; just move past docs/attrs
                    j = k;
                }
            }
        }
        // Allow optional 'sealed' before 'enum'
        let mut saw_sealed_enum = false;
        if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Sealed))
            && matches!(tokens.get(j + 1).map(|t| &t.kind), Some(TokenKind::Enum))
        {
            saw_sealed_enum = true;
            j += 1; // consume 'sealed' and let the 'enum' arm handle the rest
        }
        // Check for 'export' keyword before 'module'
        let mut is_export_module = false;
        if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Export))
            && matches!(tokens.get(j + 1).map(|t| &t.kind), Some(TokenKind::Module))
        {
            is_export_module = true;
            j += 1; // consume 'export'
        }
        // Permit optional top-level visibility before declarations like struct/enum/interface/provider/module.
        if matches!(
            tokens.get(j).map(|t| &t.kind),
            Some(TokenKind::Public | TokenKind::Internal | TokenKind::Private)
        ) && matches!(
            tokens.get(j + 1).map(|t| &t.kind),
            Some(
                TokenKind::Struct
                    | TokenKind::Enum
                    | TokenKind::Interface
                    | TokenKind::Provider
                    | TokenKind::Module
            )
        ) {
            j += 1; // skip visibility; current AST does not carry it for these decls yet
        }
        match tokens.get(j).map(|t| &t.kind) {
            Some(TokenKind::Module) => {
                j += 1;
                let mod_start = tokens
                    .get(j - 1)
                    .map(|t| t.span)
                    .unwrap_or_else(|| Span::new(0, 0));
                let name = types::parse_ident(&tokens, &mut j).unwrap_or(Ident(String::new()));
                // Optional: module implements clause
                // Syntax: `module Foo implements Interface1, Interface2 { ... }`
                // Target type is inferred from method signatures.
                let mut implements: Vec<NamePath> = Vec::new();
                if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Implements)) {
                    j += 1;
                    // Parse comma-separated interface list
                    if let Some(ty0) = types::parse_type_name(&tokens, &mut j) {
                        implements.push(ty0);
                    }
                    while matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Comma)) {
                        j += 1;
                        if let Some(ty) = types::parse_type_name(&tokens, &mut j) {
                            implements.push(ty);
                        }
                    }
                }
                if !matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::LBrace)) {
                    reporter.emit(
                        Diagnostic::error("expected '{' to start module body")
                            .with_file(sf.path.clone()),
                    );
                    j = util::skip_to_sync(&tokens, j);
                    continue;
                }
                j += 1;
                let mut items: Vec<FuncDecl> = Vec::new();
                while j < tokens.len() {
                    if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::RBrace)) {
                        let _end_span = tokens
                            .get(j)
                            .map(|t| t.span)
                            .unwrap_or_else(|| Span::new(0, 0));
                        j += 1;
                        break;
                    }
                    // Leading docs/attrs before module member
                    let (ldoc, lattrs) = attrs::take_leading_doc_attrs(&sf.text, &tokens, &mut j);
                    if let Some((fd, nj)) = func::parse_function_decl(
                        &tokens,
                        j,
                        reporter,
                        &sf.path,
                        ldoc,
                        lattrs,
                        &mut next_ast_id,
                    ) {
                        j = nj;
                        items.push(fd);
                    } else {
                        j = util::skip_to_sync(&tokens, j);
                    }
                }
                let span = util::join_span(
                    mod_start,
                    tokens.get(j - 1).map(|t| t.span).unwrap_or(mod_start),
                );
                let id = {
                    let idv = next_ast_id;
                    next_ast_id += 1;
                    AstId(idv)
                };
                decls.push(Decl::Module(ModuleDecl {
                    name,
                    is_exported: is_export_module,
                    implements,
                    items,
                    doc: leading_doc.take(),
                    attrs: std::mem::take(&mut leading_attrs),
                    span,
                    id,
                }));
            }
            Some(TokenKind::Type) => {
                // Parse: type Name = Qualified.Type;
                let t_kw = tokens.get(j).map(|t| t.span).unwrap_or(Span::new(0, 0));
                j += 1;
                let name = types::parse_ident(&tokens, &mut j).unwrap_or(Ident(String::new()));
                // '='
                if !matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Eq)) {
                    reporter.emit(
                        Diagnostic::error("expected '=' in type alias")
                            .with_file(sf.path.clone())
                            .with_span(t_kw),
                    );
                    j = util::skip_to_sync(&tokens, j);
                    continue;
                }
                j += 1;
                let rhs = if let Some(np) = types::parse_type_name(&tokens, &mut j) {
                    np
                } else {
                    reporter.emit(
                        Diagnostic::error("expected type name in alias")
                            .with_file(sf.path.clone())
                            .with_span(t_kw),
                    );
                    j = util::skip_to_sync(&tokens, j);
                    continue;
                };
                let end_span = tokens.get(j).map(|t| t.span).unwrap_or(t_kw);
                if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                    j += 1;
                } else {
                    reporter.emit(
                        Diagnostic::error("expected ';' after type alias")
                            .with_file(sf.path.clone())
                            .with_span(end_span),
                    );
                }
                let id = AstId(next_ast_id);
                next_ast_id += 1;
                decls.push(Decl::TypeAlias(crate::compiler::ast::TypeAliasDecl {
                    name,
                    aliased: rhs,
                    doc: leading_doc.take(),
                    attrs: std::mem::take(&mut leading_attrs),
                    span: Span::new(t_kw.start, end_span.end),
                    id,
                }));
                continue;
            }
            Some(TokenKind::Struct) => {
                j += 1;
                let st_start = tokens
                    .get(j - 1)
                    .map(|t| t.span)
                    .unwrap_or_else(|| Span::new(0, 0));
                let name = types::parse_ident(&tokens, &mut j).unwrap_or(Ident(String::new()));
                let generics = func::parse_generic_params(&tokens, &mut j);
                if !matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::LBrace)) {
                    reporter.emit(
                        Diagnostic::error("expected '{' to start struct body")
                            .with_file(sf.path.clone()),
                    );
                    j = util::skip_to_sync(&tokens, j);
                    continue;
                }
                j += 1;
                let mut fields: Vec<StructField> = Vec::new();
                while j < tokens.len() {
                    if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::RBrace)) {
                        let _end_span = tokens.get(j).map(|t| t.span);
                        j += 1;
                        break;
                    }
                    // Leading docs/attrs before fields
                    let (fdoc, fattrs) = attrs::take_leading_doc_attrs(&sf.text, &tokens, &mut j);
                    let field_start = tokens
                        .get(j)
                        .map(|t| t.span)
                        .unwrap_or_else(|| Span::new(0, 0));
                    let (vis, is_final, is_shared, nj) = func::parse_field_mods(&tokens, j);
                    j = nj;
                    // Detect and disallow methods inside struct bodies.
                    // Heuristic: after modifiers, a type (or 'void') + ident followed by '(' indicates a method.
                    if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Void)) {
                        // 'void name(' => definitely a method in this grammar
                        if matches!(
                            tokens.get(j + 1).map(|t| &t.kind),
                            Some(TokenKind::Ident(_))
                        ) && matches!(
                            tokens.get(j + 2).map(|t| &t.kind),
                            Some(TokenKind::LParen)
                        ) {
                            let span = tokens.get(j).map(|t| t.span);
                            reporter.emit(
                                Diagnostic::error(
                                    "methods are not allowed inside 'struct'; declare them in an 'impl' block",
                                )
                                .with_file(sf.path.clone())
                                .with_span(span.unwrap_or_else(|| tokens[j].span)),
                            );
                            j = util::skip_to_sync(&tokens, j);
                            continue;
                        }
                    }
                    // Parse a field: Type Name (';')?
                    if let Some(ty) = types::parse_type_name(&tokens, &mut j) {
                        if let Some(fname) = types::parse_ident(&tokens, &mut j) {
                            // If we see '(', this was actually a method signature; emit a targeted error.
                            if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::LParen)) {
                                let span = tokens.get(j - 1).map(|t| t.span);
                                reporter.emit(
                                    Diagnostic::error(
                                        "methods are not allowed inside 'struct'; declare them in an 'impl' block",
                                    )
                                    .with_file(sf.path.clone())
                                    .with_span(span.unwrap_or_else(|| tokens[j - 1].span)),
                                );
                                j = util::skip_to_sync(&tokens, j);
                                continue;
                            }
                            if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Semicolon))
                            {
                                j += 1;
                            }
                            let end_span = tokens.get(j - 1).map(|t| t.span).unwrap_or(field_start);
                            fields.push(StructField {
                                vis,
                                is_final,
                                is_shared,
                                name: fname,
                                ty,
                                doc: fdoc,
                                attrs: fattrs,
                                span: util::join_span(field_start, end_span),
                            });
                        } else {
                            reporter.emit(
                                Diagnostic::error("expected field name").with_file(sf.path.clone()),
                            );
                            j = util::skip_to_sync(&tokens, j);
                        }
                    } else {
                        reporter.emit(
                            Diagnostic::error("expected field type").with_file(sf.path.clone()),
                        );
                        j = util::skip_to_sync(&tokens, j);
                    }
                }
                let span = util::join_span(
                    st_start,
                    tokens.get(j - 1).map(|t| t.span).unwrap_or(st_start),
                );
                let id = {
                    let idv = next_ast_id;
                    next_ast_id += 1;
                    AstId(idv)
                };
                decls.push(Decl::Struct(StructDecl {
                    name,
                    generics,
                    fields,
                    doc: leading_doc.take(),
                    attrs: std::mem::take(&mut leading_attrs),
                    span,
                    id,
                }));
            }
            Some(TokenKind::Interface) => {
                j += 1;
                let if_start = tokens
                    .get(j - 1)
                    .map(|t| t.span)
                    .unwrap_or_else(|| Span::new(0, 0));
                let name = types::parse_ident(&tokens, &mut j).unwrap_or(Ident(String::new()));
                let if_generics = func::parse_generic_params(&tokens, &mut j);
                // Optional 'extends' clause (supports generics: interface B<T> extends A<T>)
                let mut extends: Vec<NamePath> = Vec::new();
                if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Extends)) {
                    j += 1;
                    if let Some(np) = types::parse_type_name(&tokens, &mut j) {
                        extends.push(np);
                    }
                    while matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Comma)) {
                        j += 1;
                        if let Some(np) = types::parse_type_name(&tokens, &mut j) {
                            extends.push(np);
                        }
                    }
                }
                if !matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::LBrace)) {
                    reporter.emit(
                        Diagnostic::error("expected '{' to start interface body")
                            .with_file(sf.path.clone()),
                    );
                    j = util::skip_to_sync(&tokens, j);
                    continue;
                }
                j += 1;
                let mut methods: Vec<InterfaceMethod> = Vec::new();
                while j < tokens.len() {
                    if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::RBrace)) {
                        let _end_span = tokens.get(j).map(|t| t.span);
                        j += 1;
                        break;
                    }
                    // Leading docs/attrs before interface method signatures
                    let (mdoc, mattrs) = attrs::take_leading_doc_attrs(&sf.text, &tokens, &mut j);
                    if let Some((method, nj)) =
                        func::parse_interface_method(&tokens, j, reporter, &sf.path, mdoc, mattrs)
                    {
                        j = nj;
                        methods.push(method);
                    } else {
                        j = util::skip_to_sync(&tokens, j);
                    }
                }
                let span = util::join_span(
                    if_start,
                    tokens.get(j - 1).map(|t| t.span).unwrap_or(if_start),
                );
                let id = {
                    let idv = next_ast_id;
                    next_ast_id += 1;
                    AstId(idv)
                };
                decls.push(Decl::Interface(InterfaceDecl {
                    name,
                    generics: if_generics,
                    extends,
                    methods,
                    doc: leading_doc.take(),
                    attrs: std::mem::take(&mut leading_attrs),
                    span,
                    id,
                }));
            }
            Some(TokenKind::Enum) => {
                j += 1;
                let en_start = tokens
                    .get(j - 1)
                    .map(|t| t.span)
                    .unwrap_or_else(|| Span::new(0, 0));
                let name = types::parse_ident(&tokens, &mut j).unwrap_or(Ident(String::new()));
                let generics = func::parse_generic_params(&tokens, &mut j);
                if !matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::LBrace)) {
                    reporter.emit(
                        Diagnostic::error("expected '{' to start enum body")
                            .with_file(sf.path.clone()),
                    );
                    j = util::skip_to_sync(&tokens, j);
                    continue;
                }
                j += 1;
                let mut variants: Vec<EnumVariant> = Vec::new();
                while j < tokens.len() {
                    if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::RBrace)) {
                        let _end_span = tokens.get(j).map(|t| t.span);
                        j += 1;
                        break;
                    }
                    if let Some(vname) = types::parse_ident(&tokens, &mut j) {
                        // Parse tuple types if present (e.g., Variant(Type1, Type2))
                        let types =
                            if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::LParen)) {
                                j += 1;
                                let mut tys: Vec<NamePath> = Vec::new();
                                loop {
                                    if let Some(ty) = types::parse_name_path(&tokens, &mut j) {
                                        tys.push(ty);
                                        if matches!(
                                            tokens.get(j).map(|t| &t.kind),
                                            Some(TokenKind::Comma)
                                        ) {
                                            j += 1;
                                            continue;
                                        }
                                    }
                                    break;
                                }
                                if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::RParen))
                                {
                                    j += 1;
                                }
                                Some(tys)
                            } else {
                                None
                            };

                        // Parse optional discriminant: = expr
                        let discriminant =
                            if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Eq)) {
                                j += 1; // consume '='
                                expr::parse_expr(&tokens, &mut j).map(Box::new)
                            } else {
                                None
                            };

                        // Build the variant
                        let variant = if let Some(tys) = types {
                            EnumVariant::Tuple {
                                name: vname,
                                types: tys,
                                discriminant,
                            }
                        } else {
                            EnumVariant::Unit {
                                name: vname,
                                discriminant,
                            }
                        };
                        variants.push(variant);
                        if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Comma)) {
                            j += 1;
                        }
                    } else {
                        j = util::skip_to_sync(&tokens, j);
                    }
                }
                let span = util::join_span(
                    en_start,
                    tokens.get(j - 1).map(|t| t.span).unwrap_or(en_start),
                );
                let id = {
                    let idv = next_ast_id;
                    next_ast_id += 1;
                    AstId(idv)
                };
                decls.push(Decl::Enum(EnumDecl {
                    name,
                    generics,
                    variants,
                    is_sealed: saw_sealed_enum,
                    doc: leading_doc.take(),
                    attrs: std::mem::take(&mut leading_attrs),
                    span,
                    id,
                }));
            }
            Some(TokenKind::Provider) => {
                j += 1;
                let pv_start = tokens
                    .get(j - 1)
                    .map(|t| t.span)
                    .unwrap_or_else(|| Span::new(0, 0));
                let name = types::parse_ident(&tokens, &mut j).unwrap_or(Ident(String::new()));
                if !matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::LBrace)) {
                    reporter.emit(
                        Diagnostic::error("expected '{' to start provider body")
                            .with_file(sf.path.clone()),
                    );
                    j = util::skip_to_sync(&tokens, j);
                    continue;
                }
                j += 1;
                let mut fields: Vec<StructField> = Vec::new();
                while j < tokens.len() {
                    if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::RBrace)) {
                        let _end_span = tokens.get(j).map(|t| t.span);
                        j += 1;
                        break;
                    }
                    // Leading docs/attrs before fields in provider
                    let (fdoc, fattrs) = attrs::take_leading_doc_attrs(&sf.text, &tokens, &mut j);
                    let field_start = tokens
                        .get(j)
                        .map(|t| t.span)
                        .unwrap_or_else(|| Span::new(0, 0));
                    let (vis, is_final, is_shared, nj) = func::parse_field_mods(&tokens, j);
                    j = nj;
                    if let Some(ty) = types::parse_type_name(&tokens, &mut j) {
                        if let Some(fname) = types::parse_ident(&tokens, &mut j) {
                            if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Semicolon))
                            {
                                j += 1;
                            }
                            let end_span = tokens.get(j - 1).map(|t| t.span).unwrap_or(field_start);
                            fields.push(StructField {
                                vis,
                                is_final,
                                is_shared,
                                name: fname,
                                ty,
                                doc: fdoc,
                                attrs: fattrs,
                                span: util::join_span(field_start, end_span),
                            });
                        } else {
                            reporter.emit(
                                Diagnostic::error("expected field name in provider")
                                    .with_file(sf.path.clone()),
                            );
                            j = util::skip_to_sync(&tokens, j);
                        }
                    } else {
                        reporter.emit(
                            Diagnostic::error("expected field type in provider")
                                .with_file(sf.path.clone()),
                        );
                        j = util::skip_to_sync(&tokens, j);
                    }
                }
                let span = util::join_span(
                    pv_start,
                    tokens.get(j - 1).map(|t| t.span).unwrap_or(pv_start),
                );
                let id = {
                    let idv = next_ast_id;
                    next_ast_id += 1;
                    AstId(idv)
                };
                decls.push(Decl::Provider(ProviderDecl {
                    name,
                    fields,
                    doc: leading_doc.take(),
                    attrs: std::mem::take(&mut leading_attrs),
                    span,
                    id,
                }));
            }
            // Extern function declaration: extern "C" fn name(params) -> ret;
            // Can be preceded by optional visibility (public/internal/private)
            Some(TokenKind::Extern) => {
                let extern_start = tokens
                    .get(j)
                    .map(|t| t.span)
                    .unwrap_or_else(|| Span::new(0, 0));
                j += 1; // consume 'extern'

                // Parse ABI string (e.g., "C")
                let abi = match tokens.get(j).map(|t| &t.kind) {
                    Some(TokenKind::StringLit(s)) => {
                        j += 1;
                        s.clone()
                    }
                    _ => {
                        reporter.emit(
                            Diagnostic::error("expected ABI string (e.g., \"C\") after 'extern'")
                                .with_file(sf.path.clone()),
                        );
                        "C".to_string() // default ABI
                    }
                };

                // Expect 'fn' keyword
                if !matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Fn)) {
                    reporter.emit(
                        Diagnostic::error("expected 'fn' after extern ABI specification")
                            .with_file(sf.path.clone()),
                    );
                    j = util::skip_to_sync(&tokens, j);
                    continue;
                }
                j += 1; // consume 'fn'

                // Parse function name
                let name = match types::parse_ident(&tokens, &mut j) {
                    Some(n) => n,
                    None => {
                        reporter.emit(
                            Diagnostic::error("expected function name after 'extern fn'")
                                .with_file(sf.path.clone()),
                        );
                        j = util::skip_to_sync(&tokens, j);
                        continue;
                    }
                };

                // Parse parameters
                if !matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::LParen)) {
                    reporter.emit(
                        Diagnostic::error("expected '(' after extern function name")
                            .with_file(sf.path.clone()),
                    );
                    j = util::skip_to_sync(&tokens, j);
                    continue;
                }
                j += 1;
                let mut params: Vec<Param> = Vec::new();
                loop {
                    if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::RParen)) {
                        j += 1;
                        break;
                    }
                    if let Some(pty) = types::parse_type_name(&tokens, &mut j) {
                        if let Some(pn) = types::parse_ident(&tokens, &mut j) {
                            params.push(Param { name: pn, ty: pty });
                        }
                        if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Comma)) {
                            j += 1;
                            continue;
                        }
                        if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::RParen)) {
                            j += 1;
                            break;
                        }
                    } else {
                        j = util::skip_to_sync(&tokens, j);
                        break;
                    }
                }

                // Parse optional return type: -> Type
                let ret = if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Arrow)) {
                    j += 1;
                    types::parse_type_name(&tokens, &mut j)
                } else {
                    None
                };

                // Expect semicolon
                if matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Semicolon)) {
                    j += 1;
                }

                let extern_end = tokens
                    .get(j.saturating_sub(1))
                    .map(|t| t.span)
                    .unwrap_or(extern_start);

                let id = {
                    let idv = next_ast_id;
                    next_ast_id += 1;
                    AstId(idv)
                };

                decls.push(Decl::ExternFunc(ExternFuncDecl {
                    vis: Visibility::Default, // extern functions default to package-private
                    abi,
                    name,
                    ret,
                    params,
                    doc: leading_doc.take(),
                    attrs: std::mem::take(&mut leading_attrs),
                    span: util::join_span(extern_start, extern_end),
                    id,
                }));
            }
            Some(
                TokenKind::Public
                | TokenKind::Internal
                | TokenKind::Private
                | TokenKind::Static
                | TokenKind::Final
                | TokenKind::Async
                | TokenKind::Unsafe
                | TokenKind::At
                | TokenKind::Ident(_)
                | TokenKind::Void,
            ) => {
                // Leading docs/attrs before free function declarations
                let (ldoc, lattrs) = attrs::take_leading_doc_attrs(&sf.text, &tokens, &mut j);
                // If after stripping attrs there is a visibility modifier followed by a
                // known top-level decl token (struct/enum/interface/provider/module),
                // or directly such a token, treat this as that declaration rather than a function.
                if matches!(
                    tokens.get(j).map(|t| &t.kind),
                    Some(TokenKind::Public | TokenKind::Internal | TokenKind::Private)
                ) && matches!(
                    tokens.get(j + 1).map(|t| &t.kind),
                    Some(
                        TokenKind::Struct
                            | TokenKind::Enum
                            | TokenKind::Interface
                            | TokenKind::Provider
                            | TokenKind::Module
                    )
                ) {
                    continue;
                }
                if matches!(
                    tokens.get(j).map(|t| &t.kind),
                    Some(
                        TokenKind::Struct
                            | TokenKind::Enum
                            | TokenKind::Interface
                            | TokenKind::Provider
                            | TokenKind::Module
                    )
                ) {
                    continue;
                }
                if let Some((fd, nj)) = func::parse_function_decl(
                    &tokens,
                    j,
                    reporter,
                    &sf.path,
                    ldoc,
                    lattrs,
                    &mut next_ast_id,
                ) {
                    j = nj;
                    // Reject free functions at parse time: functions must be declared inside a module
                    reporter.emit(
                        Diagnostic::error(
                            "free functions are not supported; declare inside a module",
                        )
                        .with_file(sf.path.clone())
                        .with_span(fd.span.clone()),
                    );
                    decls.push(Decl::Function(fd));
                } else {
                    j = util::skip_to_sync(&tokens, j);
                }
            }
            Some(TokenKind::Eof) | None => break,
            _ => {
                j = util::skip_to_sync(&tokens, j);
            }
        }
        if j == start_j {
            j += 1;
        }
    }

    FileAst {
        package,
        imports,
        decls,
    }
}

#[cfg(test)]
mod tests;
