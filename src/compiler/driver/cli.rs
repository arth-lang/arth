use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use arth_vm as vm;

use super::config::parse_manifest;
use super::extlib::{
    ExternalPackage, ExternalSymbolKind, collect_native_lib_paths, discover_external_packages,
    load_external_package_symbols, resolve_dependencies,
};
use crate::compiler::ast::{Block, ControlKind, Decl, Stmt, Visibility};
use crate::compiler::codegen::cranelift as cg_cl;
use crate::compiler::codegen::linker::{compile_ir_to_native, detect_target_triple};
use crate::compiler::codegen::llvm_debug::{DebugInfoBuilder, SourceLineTable};
use crate::compiler::codegen::llvm_text::{
    emit_module_text, emit_module_text_for_target, emit_module_text_with_debug,
};
use crate::compiler::diagnostics::Reporter;
use crate::compiler::hir::dump;
use crate::compiler::ir::demo_add_module;
use crate::compiler::ir::{Module as IrModule, cfg::Cfg, dom::DomInfo, verify::verify_func};
use crate::compiler::lexer::{TokenKind, lex_all};
use crate::compiler::lower::ast_to_hir::lower_file;
use crate::compiler::lower::hir_to_ir::{
    EnumLowerContext, ExternSig, JsonCodecMeta, LoweringOptions, lower_hir_enum_to_ir,
    lower_hir_extern_func_to_ir, lower_hir_func_to_ir_demo, lower_hir_func_to_ir_native,
    lower_hir_func_to_ir_with_escape, lower_hir_struct_to_ir, make_extern_sig,
};
use crate::compiler::parser::parse_file;
use crate::compiler::resolve::{
    ExternalPackageSymbol, resolve_project, resolve_project_with_externals,
};
use crate::compiler::source::SourceFile;
use crate::compiler::typeck::const_eval::{ConstEvalContext, eval_integral_const};
use crate::compiler::typeck::typecheck_project;

use super::fs::{load_sources, read_entry_from_arth_toml};
use super::incremental::{
    CompileInputs, CompileSession, DepGraph, DepNode, Fingerprint, FingerprintKind,
    fingerprint_content, fingerprint_package_sources, is_cache_verbose, is_incremental_enabled,
};
use super::vm::control_messages;

/// Compute enum variant tags from HIR, using const_eval for explicit discriminants.
/// Returns a map from variant name to tag value.
fn compute_enum_tags(
    en: &crate::compiler::hir::HirEnum,
) -> std::collections::BTreeMap<String, i64> {
    use crate::compiler::hir::HirEnumVariant;

    let mut vmap: std::collections::BTreeMap<String, i64> = Default::default();
    let mut ctx = ConstEvalContext::new();
    let mut next_implicit_tag: i64 = 0;

    for v in &en.variants {
        let (name, discriminant) = match v {
            HirEnumVariant::Unit { name, discriminant } => (name, discriminant.as_deref()),
            HirEnumVariant::Tuple {
                name, discriminant, ..
            } => (name, discriminant.as_deref()),
        };

        // Update context with already-computed tags for forward references
        ctx.set_enum_variants(&vmap);

        let tag = if let Some(disc_expr) = discriminant {
            // Evaluate the discriminant expression
            match eval_integral_const(disc_expr, &mut ctx) {
                Ok(value) => value,
                Err(_e) => {
                    // TODO: Report error through a proper reporter
                    // For now, fall back to implicit tag
                    next_implicit_tag
                }
            }
        } else {
            next_implicit_tag
        };

        vmap.insert(name.clone(), tag);
        next_implicit_tag = tag + 1;
    }

    vmap
}

#[derive(Clone, Copy, Debug)]
pub enum Backend {
    Cranelift,
    Llvm,
    Vm,
}

impl Backend {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "cranelift" | "cl" => Some(Self::Cranelift),
            "llvm" => Some(Self::Llvm),
            "vm" => Some(Self::Vm),
            _ => None,
        }
    }

    /// Convert to CfgBackend for conditional compilation filtering.
    pub fn to_cfg_backend(self) -> crate::compiler::attrs::CfgBackend {
        use crate::compiler::attrs::CfgBackend;
        match self {
            Backend::Vm => CfgBackend::Vm,
            Backend::Llvm => CfgBackend::Llvm,
            Backend::Cranelift => CfgBackend::Cranelift,
        }
    }
}

/// Build options for incremental compilation.
#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Target backend
    pub backend: Backend,
    /// Optimization level (0–3). Applies to LLVM native backend; ignored for VM.
    pub opt_level: u8,
    /// Enable incremental compilation
    pub incremental: bool,
    /// Clear cache before building
    pub clean_cache: bool,
    /// Display cache hit/miss statistics
    pub show_cache_stats: bool,
    /// Emit DWARF debug information (native backends only)
    pub debug: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            backend: Backend::Vm,
            opt_level: 2,
            incremental: is_incremental_enabled(),
            clean_cache: false,
            show_cache_stats: false,
            debug: false,
        }
    }
}

/// Build with options, including incremental compilation support.
pub fn cmd_build_with_options(path: &Path, opts: BuildOptions) -> i32 {
    // For non-VM backends, delegate to existing function (incremental not yet supported)
    if !matches!(opts.backend, Backend::Vm) {
        return cmd_build_with_backend(path, opts.backend, opts.opt_level, opts.debug);
    }

    // Create compile inputs for this build
    let inputs = match opts.backend {
        Backend::Vm => CompileInputs::vm(),
        Backend::Llvm => CompileInputs::llvm(),
        Backend::Cranelift => CompileInputs::cranelift(),
    }
    .with_opt_level(opts.opt_level);

    // Try to open incremental session
    let session_result = if opts.incremental {
        CompileSession::new(path, inputs.clone())
    } else {
        CompileSession::new_non_incremental(path, inputs.clone())
    };

    let mut session = match session_result {
        Ok(s) => s,
        Err(e) => {
            if is_cache_verbose() {
                eprintln!(
                    "warning: could not open cache ({}), falling back to full build",
                    e
                );
            }
            return cmd_build_with_backend(path, opts.backend, opts.opt_level, opts.debug);
        }
    };

    // Clean cache if requested
    if opts.clean_cache {
        if let Err(e) = session.clear_cache() {
            eprintln!("warning: failed to clear cache: {}", e);
        } else if is_cache_verbose() {
            eprintln!("cache: cleared");
        }
    }

    // Load sources and compute fingerprints
    let Ok(sources) = load_sources(path) else {
        eprintln!("error: failed to read sources at {}", path.display());
        return 1;
    };

    if sources.is_empty() {
        eprintln!("no .arth files found at {}", path.display());
        return 1;
    }

    // Group sources by package and compute fingerprints
    let mut reporter = Reporter::new();
    let mut package_sources: std::collections::BTreeMap<String, Vec<(PathBuf, String)>> =
        std::collections::BTreeMap::new();

    for sf in &sources {
        let ast = parse_file(sf, &mut reporter);
        let pkg_name = ast
            .package
            .as_ref()
            .map(|p| {
                p.0.iter()
                    .map(|id| id.0.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
            })
            .unwrap_or_else(|| "default".to_string());

        package_sources
            .entry(pkg_name)
            .or_default()
            .push((sf.path.clone(), sf.text.clone()));
    }

    // Compute package fingerprints
    let mut source_fingerprints: std::collections::BTreeMap<String, Fingerprint> =
        std::collections::BTreeMap::new();

    for (pkg, files) in &package_sources {
        let fp = fingerprint_package_sources(files);
        source_fingerprints.insert(pkg.clone(), fp);
    }

    // Compute a combined project fingerprint from all package fingerprints
    let project_hash = fingerprint_content(&format!("{:?}", source_fingerprints));
    let project_fp = Fingerprint::new(project_hash.clone(), FingerprintKind::Package);

    // Check for project-level cache hit first (fast path)
    if opts.incremental {
        // Check if the __project__ entry exists and has matching fingerprint
        let cache_status = session.cache().status("__project__", &project_fp);

        if matches!(cache_status, super::incremental::PackageCacheStatus::Hit) {
            if let Ok(Some(bytecode)) = session.load_bytecode("__project__") {
                if is_cache_verbose() {
                    eprintln!("cache: project cache hit, using cached bytecode");
                }

                let out_dir = PathBuf::from("target/arth-out");
                let _ = std::fs::create_dir_all(&out_dir);
                let abc_path = out_dir.join("app.abc");

                if let Err(e) = std::fs::write(&abc_path, &bytecode) {
                    eprintln!("error: failed to write {}: {}", abc_path.display(), e);
                    return 1;
                }
                println!("vm: wrote {} (cached)", abc_path.display());

                // Run the program
                let prog = match vm::decode_program(&bytecode) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("error: failed to decode bytecode: {}", e);
                        return 1;
                    }
                };

                let ctx = vm::HostContext::std();
                let code = vm::run_program_with_host(&prog, &ctx);
                println!("vm: exit code {}", code);

                if opts.show_cache_stats {
                    println!("cache stats: 1 cached, 0 compiled, 0 invalidated by deps");
                }

                return 0;
            }
        }
    }

    // Build dependency graph (simple: each package as a node)
    let mut dep_graph = DepGraph::new();
    for pkg in source_fingerprints.keys() {
        dep_graph.add_node(DepNode::new(pkg));
    }
    dep_graph.compute_build_order();

    // Get incremental plan for individual packages
    let plan = session.plan(&source_fingerprints, &dep_graph);

    if is_cache_verbose() {
        eprintln!(
            "cache: {} package(s) to compile, {} cached",
            plan.recompile.len(),
            plan.cached.len()
        );
        for (pkg, reason) in &plan.reasons {
            eprintln!("cache:   {} - {}", pkg, reason);
        }
    }

    // Note: Currently we fall through to full build.
    // TODO: Implement per-package caching when package-level compilation is supported.

    // Legacy check for cached bytecode (should not trigger with new logic above)
    if !plan.needs_build() && opts.incremental {
        if let Ok(Some(bytecode)) = session.load_bytecode("__project__") {
            if is_cache_verbose() {
                eprintln!("cache: using cached bytecode");
            }

            let out_dir = PathBuf::from("target/arth-out");
            let _ = std::fs::create_dir_all(&out_dir);
            let abc_path = out_dir.join("app.abc");

            if let Err(e) = std::fs::write(&abc_path, &bytecode) {
                eprintln!("error: failed to write {}: {}", abc_path.display(), e);
                return 1;
            }
            println!("vm: wrote {} (cached)", abc_path.display());

            // Run the program
            let prog = match vm::decode_program(&bytecode) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: failed to decode bytecode: {}", e);
                    return 1;
                }
            };

            let ctx = vm::HostContext::std();
            let code = vm::run_program_with_host(&prog, &ctx);
            println!("vm: exit code {}", code);

            if opts.show_cache_stats {
                println!("cache stats: {}", session.stats());
            }

            return 0;
        }
    }

    // Fall through to full build
    let result = cmd_build_with_backend(path, opts.backend, opts.opt_level, opts.debug);

    if result == 0 && opts.incremental {
        // Cache the compiled bytecode for next time
        let abc_path = PathBuf::from("target/arth-out/app.abc");
        if abc_path.exists() {
            if let Ok(bytecode) = std::fs::read(&abc_path) {
                // Compute a combined fingerprint for the whole project
                let project_hash = fingerprint_content(&format!("{:?}", source_fingerprints));
                let project_fp = Fingerprint::new(project_hash, FingerprintKind::Package);
                let interface_fp =
                    Fingerprint::new("project".to_string(), FingerprintKind::Interface);

                if session
                    .mark_compiled("__project__", project_fp, interface_fp)
                    .is_ok()
                {
                    if session.store_bytecode("__project__", &bytecode).is_ok() {
                        if let Err(e) = session.save() {
                            if is_cache_verbose() {
                                eprintln!("cache: failed to save: {}", e);
                            }
                        } else if is_cache_verbose() {
                            eprintln!("cache: saved bytecode for next build");
                        }
                    }
                }
            }
        }
    }

    if opts.show_cache_stats {
        println!("cache stats: {}", session.stats());
    }

    result
}

pub fn cmd_lex(path: &Path) -> i32 {
    let Ok(sources) = load_sources(path) else {
        eprintln!("error: failed to read sources at {}", path.display());
        return 1;
    };
    if sources.is_empty() {
        eprintln!("no .arth files found at {}", path.display());
        return 1;
    }
    for sf in &sources {
        println!("==> {}", sf.path.display());
        for t in lex_all(&sf.text) {
            match t.kind {
                TokenKind::Package => println!("{:>4}..{:>4}  PACKAGE", t.span.start, t.span.end),
                TokenKind::Module => println!("{:>4}..{:>4}  MODULE", t.span.start, t.span.end),
                TokenKind::Public => println!("{:>4}..{:>4}  PUBLIC", t.span.start, t.span.end),
                TokenKind::Import => println!("{:>4}..{:>4}  IMPORT", t.span.start, t.span.end),
                TokenKind::Internal => println!("{:>4}..{:>4}  INTERNAL", t.span.start, t.span.end),
                TokenKind::Private => println!("{:>4}..{:>4}  PRIVATE", t.span.start, t.span.end),
                TokenKind::Export => println!("{:>4}..{:>4}  EXPORT", t.span.start, t.span.end),
                TokenKind::Struct => println!("{:>4}..{:>4}  STRUCT", t.span.start, t.span.end),
                TokenKind::Interface => {
                    println!("{:>4}..{:>4}  INTERFACE", t.span.start, t.span.end)
                }
                TokenKind::Enum => println!("{:>4}..{:>4}  ENUM", t.span.start, t.span.end),
                TokenKind::Sealed => println!("{:>4}..{:>4}  SEALED", t.span.start, t.span.end),
                TokenKind::Provider => println!("{:>4}..{:>4}  PROVIDER", t.span.start, t.span.end),
                TokenKind::Shared => println!("{:>4}..{:>4}  SHARED", t.span.start, t.span.end),
                TokenKind::Extends => println!("{:>4}..{:>4}  EXTENDS", t.span.start, t.span.end),
                TokenKind::Implements => {
                    println!("{:>4}..{:>4}  IMPLEMENTS", t.span.start, t.span.end)
                }
                TokenKind::Static => println!("{:>4}..{:>4}  STATIC", t.span.start, t.span.end),
                TokenKind::Final => println!("{:>4}..{:>4}  FINAL", t.span.start, t.span.end),
                TokenKind::Void => println!("{:>4}..{:>4}  VOID", t.span.start, t.span.end),
                TokenKind::Async => println!("{:>4}..{:>4}  ASYNC", t.span.start, t.span.end),
                TokenKind::Await => println!("{:>4}..{:>4}  AWAIT", t.span.start, t.span.end),
                TokenKind::Throws => println!("{:>4}..{:>4}  THROWS", t.span.start, t.span.end),
                TokenKind::Var => println!("{:>4}..{:>4}  VAR", t.span.start, t.span.end),
                TokenKind::Fn => println!("{:>4}..{:>4}  FN", t.span.start, t.span.end),
                TokenKind::Type => println!("{:>4}..{:>4}  TYPE", t.span.start, t.span.end),
                TokenKind::Unsafe => println!("{:>4}..{:>4}  UNSAFE", t.span.start, t.span.end),
                TokenKind::Extern => println!("{:>4}..{:>4}  EXTERN", t.span.start, t.span.end),
                TokenKind::Panic => println!("{:>4}..{:>4}  PANIC", t.span.start, t.span.end),
                TokenKind::True => println!("{:>4}..{:>4}  TRUE", t.span.start, t.span.end),
                TokenKind::False => println!("{:>4}..{:>4}  FALSE", t.span.start, t.span.end),
                TokenKind::Ident(ref s) => {
                    println!("{:>4}..{:>4}  IDENT({})", t.span.start, t.span.end, s)
                }
                TokenKind::Number(n) => {
                    println!("{:>4}..{:>4}  NUMBER({})", t.span.start, t.span.end, n)
                }
                TokenKind::If => println!("{:>4}..{:>4}  IF", t.span.start, t.span.end),
                TokenKind::Else => println!("{:>4}..{:>4}  ELSE", t.span.start, t.span.end),
                TokenKind::While => println!("{:>4}..{:>4}  WHILE", t.span.start, t.span.end),
                TokenKind::For => println!("{:>4}..{:>4}  FOR", t.span.start, t.span.end),
                TokenKind::Switch => println!("{:>4}..{:>4}  SWITCH", t.span.start, t.span.end),
                TokenKind::Case => println!("{:>4}..{:>4}  CASE", t.span.start, t.span.end),
                TokenKind::Default => println!("{:>4}..{:>4}  DEFAULT", t.span.start, t.span.end),
                TokenKind::Try => println!("{:>4}..{:>4}  TRY", t.span.start, t.span.end),
                TokenKind::Catch => println!("{:>4}..{:>4}  CATCH", t.span.start, t.span.end),
                TokenKind::Finally => println!("{:>4}..{:>4}  FINALLY", t.span.start, t.span.end),
                TokenKind::Break => println!("{:>4}..{:>4}  BREAK", t.span.start, t.span.end),
                TokenKind::Continue => println!("{:>4}..{:>4}  CONTINUE", t.span.start, t.span.end),
                TokenKind::Return => println!("{:>4}..{:>4}  RETURN", t.span.start, t.span.end),
                TokenKind::Throw => println!("{:>4}..{:>4}  THROW", t.span.start, t.span.end),
                TokenKind::Dot => println!("{:>4}..{:>4}  '.'", t.span.start, t.span.end),
                TokenKind::Colon => println!("{:>4}..{:>4}  ':'", t.span.start, t.span.end),
                TokenKind::Question => println!("{:>4}..{:>4}  '?'", t.span.start, t.span.end),
                TokenKind::Semicolon => println!("{:>4}..{:>4}  ';'", t.span.start, t.span.end),
                TokenKind::LParen => println!("{:>4}..{:>4}  '('", t.span.start, t.span.end),
                TokenKind::RParen => println!("{:>4}..{:>4}  ')'", t.span.start, t.span.end),
                TokenKind::LBrace => println!("{:>4}..{:>4}  '{{'", t.span.start, t.span.end),
                TokenKind::RBrace => println!("{:>4}..{:>4}  '}}'", t.span.start, t.span.end),
                TokenKind::Comma => println!("{:>4}..{:>4}  ','", t.span.start, t.span.end),
                TokenKind::StringLit(ref s) => println!(
                    "{:>4}..{:>4}  STRING(\"{}\")",
                    t.span.start,
                    t.span.end,
                    s.replace('\n', "\\n")
                ),
                TokenKind::Float(f) => {
                    println!("{:>4}..{:>4}  FLOAT({})", t.span.start, t.span.end, f)
                }
                TokenKind::CharLit(c) => {
                    println!("{:>4}..{:>4}  CHAR('{}')", t.span.start, t.span.end, c)
                }
                TokenKind::DocLine(ref s) => {
                    println!("{:>4}..{:>4}  DOCLINE(\"{}\")", t.span.start, t.span.end, s)
                }
                TokenKind::DocBlock(ref s) => println!(
                    "{:>4}..{:>4}  DOCBLOCK(\"{}\")",
                    t.span.start,
                    t.span.end,
                    s.replace('\n', "\\n")
                ),
                TokenKind::AndAnd => println!("{:>4}..{:>4}  '&&'", t.span.start, t.span.end),
                TokenKind::OrOr => println!("{:>4}..{:>4}  '||'", t.span.start, t.span.end),
                TokenKind::Le => println!("{:>4}..{:>4}  '<='", t.span.start, t.span.end),
                TokenKind::Ge => println!("{:>4}..{:>4}  '>='", t.span.start, t.span.end),
                TokenKind::Shl => println!("{:>4}..{:>4}  '<<'", t.span.start, t.span.end),
                TokenKind::Shr => println!("{:>4}..{:>4}  '>>'", t.span.start, t.span.end),
                TokenKind::Ampersand => println!("{:>4}..{:>4}  '&'", t.span.start, t.span.end),
                TokenKind::Pipe => println!("{:>4}..{:>4}  '|'", t.span.start, t.span.end),
                TokenKind::Caret => println!("{:>4}..{:>4}  '^'", t.span.start, t.span.end),
                TokenKind::Arrow => println!("{:>4}..{:>4}  '->'", t.span.start, t.span.end),
                TokenKind::Plus => println!("{:>4}..{:>4}  '+'", t.span.start, t.span.end),
                TokenKind::Minus => println!("{:>4}..{:>4}  '-'", t.span.start, t.span.end),
                TokenKind::Star => println!("{:>4}..{:>4}  '*'", t.span.start, t.span.end),
                TokenKind::Slash => println!("{:>4}..{:>4}  '/'", t.span.start, t.span.end),
                TokenKind::Percent => println!("{:>4}..{:>4}  '%'", t.span.start, t.span.end),
                TokenKind::Eq => println!("{:>4}..{:>4}  '='", t.span.start, t.span.end),
                TokenKind::EqEq => println!("{:>4}..{:>4}  '=='", t.span.start, t.span.end),
                TokenKind::Bang => println!("{:>4}..{:>4}  '!'", t.span.start, t.span.end),
                TokenKind::BangEq => println!("{:>4}..{:>4}  '!='", t.span.start, t.span.end),
                TokenKind::At => println!("{:>4}..{:>4}  '@'", t.span.start, t.span.end),
                TokenKind::LBracket => println!("{:>4}..{:>4}  '['", t.span.start, t.span.end),
                TokenKind::RBracket => println!("{:>4}..{:>4}  ']'", t.span.start, t.span.end),
                TokenKind::DotDot => println!("{:>4}..{:>4}  '..'", t.span.start, t.span.end),
                TokenKind::PercentEq => println!("{:>4}..{:>4}  '%='", t.span.start, t.span.end),
                TokenKind::PlusEq => println!("{:>4}..{:>4}  '+='", t.span.start, t.span.end),
                TokenKind::MinusEq => println!("{:>4}..{:>4}  '-='", t.span.start, t.span.end),
                TokenKind::StarEq => println!("{:>4}..{:>4}  '*='", t.span.start, t.span.end),
                TokenKind::SlashEq => println!("{:>4}..{:>4}  '/='", t.span.start, t.span.end),
                TokenKind::ShlEq => println!("{:>4}..{:>4}  '<<='", t.span.start, t.span.end),
                TokenKind::ShrEq => println!("{:>4}..{:>4}  '>>='", t.span.start, t.span.end),
                TokenKind::AndEq => println!("{:>4}..{:>4}  '&='", t.span.start, t.span.end),
                TokenKind::OrEq => println!("{:>4}..{:>4}  '|='", t.span.start, t.span.end),
                TokenKind::XorEq => println!("{:>4}..{:>4}  '^='", t.span.start, t.span.end),
                TokenKind::Lt => println!("{:>4}..{:>4}  '<'", t.span.start, t.span.end),
                TokenKind::Gt => println!("{:>4}..{:>4}  '>'", t.span.start, t.span.end),
                TokenKind::Eof => println!("{:>4}..{:>4}  <eof>", t.span.start, t.span.end),
                TokenKind::Unknown(c) => {
                    println!("{:>4}..{:>4}  <unknown '{}'>", t.span.start, t.span.end, c)
                }
                _ => {}
            }
        }
        println!();
    }
    0
}

pub fn cmd_parse(path: &Path) -> i32 {
    let Ok(sources) = load_sources(path) else {
        eprintln!("error: failed to read sources at {}", path.display());
        return 1;
    };
    if sources.is_empty() {
        eprintln!("no .arth files found at {}", path.display());
        return 1;
    }
    let mut reporter = Reporter::new();
    for sf in &sources {
        let ast = parse_file(sf, &mut reporter);
        if let Some(pkg) = ast.package {
            println!("{}: package {}", sf.path.display(), pkg);
        } else {
            println!("{}: <no package>", sf.path.display());
        }
    }
    let had_errors = reporter.has_errors();
    reporter.drain_to_stderr();
    if had_errors { 1 } else { 0 }
}

pub fn cmd_check(path: &Path) -> i32 {
    let Ok(sources) = load_sources(path) else {
        eprintln!("error: failed to read sources at {}", path.display());
        return 1;
    };
    let mut reporter = Reporter::new();
    let mut asts: Vec<(SourceFile, crate::compiler::ast::FileAst)> =
        Vec::with_capacity(sources.len());
    for sf in &sources {
        let ast = parse_file(sf, &mut reporter);
        asts.push((sf.clone(), ast));
    }
    if sources.is_empty() {
        eprintln!("no .arth files found at {}", path.display());
        return 1;
    }

    // Load external package symbols for resolution
    let (ext_symbols, _, _) = load_external_symbols_for_resolver(path);

    // Phase 4: resolve names, imports, and packages
    let resolved = if ext_symbols.is_empty() {
        resolve_project(path, &asts, &mut reporter)
    } else {
        resolve_project_with_externals(path, &asts, &ext_symbols, &mut reporter)
    };

    // Phase 5: local typing and basic rules and generic bound checks
    typecheck_project(path, &asts, &resolved, &mut reporter);
    let had_errors = reporter.has_errors();
    reporter.drain_to_stderr();
    if had_errors {
        1
    } else {
        println!("check: {} file(s) OK", sources.len());
        0
    }
}

pub fn cmd_check_with_dump(path: &Path, dump_hir_flag: bool, dump_ir_flag: bool) -> i32 {
    let Ok(sources) = load_sources(path) else {
        eprintln!("error: failed to read sources at {}", path.display());
        return 1;
    };
    if sources.is_empty() {
        eprintln!("no .arth files found at {}", path.display());
        return 1;
    }
    let mut reporter = Reporter::new();
    let mut asts: Vec<(SourceFile, crate::compiler::ast::FileAst)> =
        Vec::with_capacity(sources.len());
    for sf in &sources {
        let ast = parse_file(sf, &mut reporter);
        if dump_hir_flag || dump_ir_flag {
            let hir = lower_file(sf, &ast);
            if dump_hir_flag {
                let dump_txt = dump(&hir);
                println!("{}", dump_txt);
            }
            if dump_ir_flag {
                // Find first function body and lower it to IR for debugging
                'outer: for d in &hir.decls {
                    if let crate::compiler::hir::HirDecl::Module(m) = d {
                        // Build enum tag + shared field context once per HIR file
                        let mut tags: std::collections::BTreeMap<
                            String,
                            std::collections::BTreeMap<String, i64>,
                        > = Default::default();
                        let mut shared_field_names: std::collections::HashSet<String> =
                            Default::default();
                        let mut extern_funcs: std::collections::HashMap<String, ExternSig> =
                            Default::default();
                        let mut struct_field_types: std::collections::HashMap<
                            (String, String),
                            String,
                        > = Default::default();
                        for d2 in &hir.decls {
                            if let crate::compiler::hir::HirDecl::Enum(en) = d2 {
                                let vmap = compute_enum_tags(en);
                                tags.insert(en.name.clone(), vmap);
                            } else if let crate::compiler::hir::HirDecl::Provider(pv) = d2 {
                                for f in &pv.fields {
                                    if f.is_shared {
                                        shared_field_names.insert(f.name.clone());
                                    }
                                    // Extract type name from HirType
                                    let ty_name = match &f.ty {
                                        crate::compiler::hir::HirType::Name { path } => {
                                            path.last().cloned()
                                        }
                                        crate::compiler::hir::HirType::Generic { path, .. } => {
                                            path.last().cloned()
                                        }
                                        _ => None,
                                    };
                                    if let Some(name) = ty_name {
                                        struct_field_types
                                            .insert((pv.name.clone(), f.name.clone()), name);
                                    }
                                }
                            } else if let crate::compiler::hir::HirDecl::Struct(st) = d2 {
                                for f in &st.fields {
                                    let ty_name = match &f.ty {
                                        crate::compiler::hir::HirType::Name { path } => {
                                            path.last().cloned()
                                        }
                                        crate::compiler::hir::HirType::Generic { path, .. } => {
                                            path.last().cloned()
                                        }
                                        _ => None,
                                    };
                                    if let Some(name) = ty_name {
                                        struct_field_types
                                            .insert((st.name.clone(), f.name.clone()), name);
                                    }
                                }
                            } else if let crate::compiler::hir::HirDecl::ExternFunc(ef) = d2 {
                                extern_funcs.insert(ef.name.clone(), make_extern_sig(ef));
                            }
                        }
                        let enum_ctx = EnumLowerContext {
                            tags,
                            shared_field_names,
                            type_aliases: Default::default(),
                            types_needing_drop: Default::default(),
                            json_codec_structs: Default::default(),
                            provider_names: Default::default(),
                            extern_funcs,
                            struct_field_types,
                        };
                        for f in &m.funcs {
                            if f.body.is_some() {
                                let (ir_funcs, ir_strings) =
                                    lower_hir_func_to_ir_demo(f, Some(&enum_ctx));
                                // Promote/opt each and collect into a module for text dump
                                let mut module = IrModule::new("debug");
                                module.strings = ir_strings;
                                // Add extern function declarations to the module
                                for d2 in &hir.decls {
                                    if let crate::compiler::hir::HirDecl::ExternFunc(ef) = d2 {
                                        module.extern_funcs.push(lower_hir_extern_func_to_ir(ef));
                                    }
                                }
                                let mut first: Option<crate::compiler::ir::Func> = None;
                                for mut irf in ir_funcs {
                                    crate::compiler::ir::ssa::mem2reg_promote(&mut irf);
                                    crate::compiler::ir::opt::run_simple_opts(&mut irf);
                                    if first.is_none() {
                                        first = Some(irf.clone());
                                    }
                                    module.funcs.push(irf);
                                }
                                let ll = emit_module_text(&module);
                                println!("-- IR (as LLVM text) --\n{}", ll);
                                if let Some(ir_func) = first {
                                    let cfg = Cfg::build(&ir_func);
                                    println!("-- CFG --\n{}", cfg.dump(&ir_func));
                                    let dom = DomInfo::compute(&ir_func, &cfg);
                                    println!("-- Dominators/Frontiers --\n{}", dom.dump());
                                    match verify_func(&ir_func) {
                                        Ok(()) => println!("-- Verify --\nOK"),
                                        Err(errs) => {
                                            println!("-- Verify --");
                                            for e in errs {
                                                println!("error: {}", e);
                                            }
                                        }
                                    }
                                    println!(
                                        "-- SSA Dump --\n{}",
                                        crate::compiler::ir::ssa::dump_ssa(&ir_func)
                                    );
                                }
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
        asts.push((sf.clone(), ast));
    }

    // Load external package symbols for resolution
    let (ext_symbols, _, _) = load_external_symbols_for_resolver(path);

    // Perform resolution after parsing and optional dump
    let resolved = if ext_symbols.is_empty() {
        resolve_project(path, &asts, &mut reporter)
    } else {
        resolve_project_with_externals(path, &asts, &ext_symbols, &mut reporter)
    };

    // Run typechecker as part of check-with-dump too
    let _escape_results = typecheck_project(path, &asts, &resolved, &mut reporter);
    let had_errors = reporter.has_errors();
    reporter.drain_to_stderr();
    if had_errors { 1 } else { 0 }
}

pub fn cmd_emit_llvm_demo(out: Option<&Path>) -> i32 {
    let m = demo_add_module();
    let text = emit_module_text(&m);
    if let Some(p) = out {
        if let Err(e) = std::fs::write(p, text) {
            eprintln!("error: failed to write {}: {}", p.display(), e);
            return 1;
        }
        println!("wrote {}", p.display());
    } else {
        print!("{}", text);
    }
    0
}

pub fn cmd_run_vm(path: &Path) -> i32 {
    match vm::run_abc_file(path) {
        Ok(code) => {
            println!("vm: exit code {}", code);
            0
        }
        Err(e) => {
            eprintln!("error: vm failed: {}", e);
            1
        }
    }
}

/// Build a distributable library (.abc with export table) from a project.
/// Usage: arth build --lib <path>
/// Output: target/arth-out/lib.abc
pub fn cmd_build_lib(path: &Path) -> i32 {
    // First run check
    let check = cmd_check(path);
    if check != 0 {
        return check;
    }

    let mut reporter = Reporter::new();
    let Ok(sources) = load_sources(path) else {
        eprintln!("error: failed to read sources at {}", path.display());
        return 1;
    };

    if sources.is_empty() {
        eprintln!("no .arth files found at {}", path.display());
        return 1;
    }

    // Parse all sources
    let mut asts: Vec<(SourceFile, crate::compiler::ast::FileAst)> = Vec::new();
    for sf in &sources {
        let ast = parse_file(sf, &mut reporter);
        asts.push((sf.clone(), ast));
    }

    // Lower to HIR and collect public functions for exports
    let mut hirs: Vec<crate::compiler::hir::HirFile> = Vec::new();
    let mut exports: Vec<(String, u8)> = Vec::new(); // (name, arity)

    for sf in &sources {
        let Some((_, ast)) = asts.iter().find(|(s, _)| s.path == sf.path) else {
            eprintln!("error: internal: AST not found for {}", sf.path.display());
            return 1;
        };
        let hir = lower_file(sf, ast);

        // Get package name for export prefix
        let pkg_prefix = hir
            .package
            .as_ref()
            .map(|p| p.0.join("."))
            .unwrap_or_default();

        // Collect function exports from modules
        // Note: For now we export all functions with bodies from exported modules
        // A more sophisticated implementation would check AST-level visibility
        for d in &hir.decls {
            if let crate::compiler::hir::HirDecl::Module(m) = d {
                if m.is_exported {
                    for f in &m.funcs {
                        if f.body.is_some() {
                            // Include package prefix in export name (e.g., "demo.Math.add")
                            let export_name = if pkg_prefix.is_empty() {
                                format!("{}.{}", m.name, f.sig.name)
                            } else {
                                format!("{}.{}.{}", pkg_prefix, m.name, f.sig.name)
                            };
                            let arity = f.sig.params.len() as u8;
                            exports.push((export_name, arity));
                        }
                    }
                }
            }
        }

        hirs.push(hir);
    }

    // Run typechecker
    let resolved = resolve_project(path, &asts, &mut reporter);
    let escape_results = typecheck_project(path, &asts, &resolved, &mut reporter);

    if reporter.has_errors() {
        reporter.drain_to_stderr();
        return 1;
    }

    // Build enum context for lowering
    let mut tags: std::collections::BTreeMap<String, std::collections::BTreeMap<String, i64>> =
        Default::default();
    let mut shared_field_names: std::collections::HashSet<String> = Default::default();
    let mut provider_names: std::collections::HashSet<String> = Default::default();
    let mut ir_providers: Vec<crate::compiler::ir::Provider> = Vec::new();
    let mut extern_funcs: std::collections::HashMap<String, ExternSig> = Default::default();
    let mut struct_field_types: std::collections::HashMap<(String, String), String> =
        Default::default();

    for hir in &hirs {
        for d2 in &hir.decls {
            if let crate::compiler::hir::HirDecl::Enum(en) = d2 {
                let vmap = compute_enum_tags(en);
                tags.insert(en.name.clone(), vmap);
            } else if let crate::compiler::hir::HirDecl::Provider(pv) = d2 {
                provider_names.insert(pv.name.clone());
                ir_providers.push(crate::compiler::lower::hir_to_ir::lower_hir_provider_to_ir(
                    pv,
                ));
                for f in &pv.fields {
                    if f.is_shared {
                        shared_field_names.insert(f.name.clone());
                    }
                    let ty_name = match &f.ty {
                        crate::compiler::hir::HirType::Name { path } => path.last().cloned(),
                        crate::compiler::hir::HirType::Generic { path, .. } => path.last().cloned(),
                        _ => None,
                    };
                    if let Some(name) = ty_name {
                        struct_field_types.insert((pv.name.clone(), f.name.clone()), name);
                    }
                }
            } else if let crate::compiler::hir::HirDecl::Struct(st) = d2 {
                for f in &st.fields {
                    let ty_name = match &f.ty {
                        crate::compiler::hir::HirType::Name { path } => path.last().cloned(),
                        crate::compiler::hir::HirType::Generic { path, .. } => path.last().cloned(),
                        _ => None,
                    };
                    if let Some(name) = ty_name {
                        struct_field_types.insert((st.name.clone(), f.name.clone()), name);
                    }
                }
            } else if let crate::compiler::hir::HirDecl::ExternFunc(ef) = d2 {
                extern_funcs.insert(ef.name.clone(), make_extern_sig(ef));
            }
        }
    }

    let enum_ctx = EnumLowerContext {
        tags,
        shared_field_names,
        type_aliases: Default::default(),
        types_needing_drop: Default::default(),
        json_codec_structs: Default::default(),
        provider_names,
        extern_funcs,
        struct_field_types,
    };

    // Lower all functions to IR
    let mut all_funcs: Vec<crate::compiler::ir::Func> = Vec::new();
    let mut all_strings: Vec<String> = Vec::new();

    for hir in &hirs {
        let pkg = hir
            .package
            .as_ref()
            .map(|p| p.to_string())
            .unwrap_or_default();
        for d in &hir.decls {
            if let crate::compiler::hir::HirDecl::Module(m) = d {
                for f in &m.funcs {
                    if f.body.is_some() {
                        let func_escape_info =
                            escape_results.get_function_by_parts(&pkg, Some(&m.name), &f.sig.name);
                        let (mut funcs, strings) =
                            lower_hir_func_to_ir_with_escape(f, Some(&enum_ctx), func_escape_info);

                        // Qualify function names with module prefix
                        let mut name_remap: std::collections::HashMap<String, String> =
                            std::collections::HashMap::new();
                        for func in &mut funcs {
                            let old_name = func.name.clone();
                            let new_name = format!("{}.{}", m.name, func.name);
                            func.name = new_name.clone();
                            name_remap.insert(old_name, new_name);
                        }

                        // Update MakeClosure references
                        for func in &mut funcs {
                            for b in &mut func.blocks {
                                for inst in &mut b.insts {
                                    if let crate::compiler::ir::InstKind::MakeClosure {
                                        ref mut func,
                                        ..
                                    } = inst.kind
                                    {
                                        if let Some(new_name) = name_remap.get(func) {
                                            *func = new_name.clone();
                                        }
                                    }
                                }
                            }
                        }

                        let base = all_strings.len() as u32;
                        if base > 0 {
                            for func in &mut funcs {
                                for b in &mut func.blocks {
                                    for inst in &mut b.insts {
                                        if let crate::compiler::ir::InstKind::ConstStr(ref mut ix) =
                                            inst.kind
                                        {
                                            *ix += base;
                                        }
                                    }
                                }
                            }
                        }

                        all_strings.extend(strings);
                        all_funcs.extend(funcs);
                    }
                }
            }
        }
    }

    reporter.drain_to_stderr();

    // Compile to bytecode and get function offsets
    let (prog, func_offsets) =
        super::vm::compile_ir_to_program_with_offsets(&all_funcs, &all_strings, &ir_providers);

    // Build export entries using actual bytecode offsets
    let export_entries: Vec<vm::ExportEntry> = exports
        .iter()
        .filter_map(|(name, arity)| {
            // Export name is like "wf.echo_message.Main.execute",
            // function name in func_offsets is "Main.execute" (module.function).
            // We need to try progressively stripping package prefix segments.
            if let Some(&offset) = func_offsets.get(name.as_str()) {
                return Some(vm::ExportEntry {
                    name: name.clone(),
                    offset,
                    arity: *arity,
                });
            }

            // Strip package prefix segments one at a time until we find a match
            let mut remaining = name.as_str();
            while let Some((_prefix, rest)) = remaining.split_once('.') {
                if let Some(&offset) = func_offsets.get(rest) {
                    return Some(vm::ExportEntry {
                        name: name.clone(),
                        offset,
                        arity: *arity,
                    });
                }
                remaining = rest;
            }

            None
        })
        .collect();

    // Create library with exports
    let lib = vm::Library::new(
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("lib")
            .to_string(),
        prog,
        export_entries,
    );

    // Write library file
    let out_dir = PathBuf::from("target/arth-out");
    let _ = std::fs::create_dir_all(&out_dir);
    let lib_path = out_dir.join("lib.abc");

    let lib_bytes = vm::encode_library(&lib);
    if let Err(e) = std::fs::write(&lib_path, lib_bytes) {
        eprintln!("error: failed to write {}: {}", lib_path.display(), e);
        return 1;
    }

    println!(
        "lib: wrote {} ({} exports)",
        lib_path.display(),
        exports.len()
    );
    for (name, arity) in &exports {
        println!("  - {}({})", name, arity);
    }

    0
}

/// Build a native executable from a project using the LLVM backend.
/// Usage: arth build --backend llvm <path>
/// Output: target/arth-out/app (native binary)
pub fn cmd_build_native(path: &Path, opt_level: u8, debug: bool) -> i32 {
    let mut reporter = Reporter::new();
    let Ok(sources) = load_sources(path) else {
        eprintln!("error: failed to read sources at {}", path.display());
        return 1;
    };

    if sources.is_empty() {
        eprintln!("no .arth files found at {}", path.display());
        return 1;
    }

    // Parse all sources
    let mut asts: Vec<(SourceFile, crate::compiler::ast::FileAst)> = Vec::new();
    for sf in &sources {
        let ast = parse_file(sf, &mut reporter);
        asts.push((sf.clone(), ast));
    }

    // Lower to HIR
    let mut hirs: Vec<crate::compiler::hir::HirFile> = Vec::new();
    for sf in &sources {
        let Some((_, ast)) = asts.iter().find(|(s, _)| s.path == sf.path) else {
            eprintln!("error: internal: AST not found for {}", sf.path.display());
            return 1;
        };
        let hir = lower_file(sf, ast);
        hirs.push(hir);
    }

    // Load external package symbols for resolution
    let (ext_symbols, _, _) = load_external_symbols_for_resolver(path);

    // Run resolution and typechecker
    let resolved = if ext_symbols.is_empty() {
        resolve_project(path, &asts, &mut reporter)
    } else {
        resolve_project_with_externals(path, &asts, &ext_symbols, &mut reporter)
    };
    let escape_results = typecheck_project(path, &asts, &resolved, &mut reporter);

    if reporter.has_errors() {
        reporter.drain_to_stderr();
        return 1;
    }

    // Build enum context for lowering
    let mut tags: std::collections::BTreeMap<String, std::collections::BTreeMap<String, i64>> =
        Default::default();
    let mut shared_field_names: std::collections::HashSet<String> = Default::default();
    let mut provider_names: std::collections::HashSet<String> = Default::default();
    let mut extern_funcs: std::collections::HashMap<String, ExternSig> = Default::default();
    let mut struct_field_types: std::collections::HashMap<(String, String), String> =
        Default::default();
    // Build struct definitions for native struct lowering
    let mut struct_defs: std::collections::HashMap<String, Vec<(String, String)>> =
        Default::default();

    for hir in &hirs {
        for d2 in &hir.decls {
            if let crate::compiler::hir::HirDecl::Enum(en) = d2 {
                let vmap = compute_enum_tags(en);
                tags.insert(en.name.clone(), vmap);
            } else if let crate::compiler::hir::HirDecl::Provider(pv) = d2 {
                provider_names.insert(pv.name.clone());
                for f in &pv.fields {
                    if f.is_shared {
                        shared_field_names.insert(f.name.clone());
                    }
                    let ty_name = match &f.ty {
                        crate::compiler::hir::HirType::Name { path } => path.last().cloned(),
                        crate::compiler::hir::HirType::Generic { path, .. } => path.last().cloned(),
                        _ => None,
                    };
                    if let Some(name) = ty_name {
                        struct_field_types.insert((pv.name.clone(), f.name.clone()), name);
                    }
                }
            } else if let crate::compiler::hir::HirDecl::Struct(st) = d2 {
                // Build struct_defs for native struct lowering
                let mut field_defs: Vec<(String, String)> = Vec::new();
                for f in &st.fields {
                    let ty_name = match &f.ty {
                        crate::compiler::hir::HirType::Name { path } => {
                            path.last().cloned().unwrap_or_else(|| "i64".to_string())
                        }
                        crate::compiler::hir::HirType::Generic { path, .. } => {
                            path.last().cloned().unwrap_or_else(|| "i64".to_string())
                        }
                        _ => "i64".to_string(),
                    };
                    struct_field_types.insert((st.name.clone(), f.name.clone()), ty_name.clone());
                    field_defs.push((f.name.clone(), ty_name));
                }
                struct_defs.insert(st.name.clone(), field_defs);
            } else if let crate::compiler::hir::HirDecl::ExternFunc(ef) = d2 {
                extern_funcs.insert(ef.name.clone(), make_extern_sig(ef));
            }
        }
    }

    let enum_ctx = EnumLowerContext {
        tags,
        shared_field_names,
        type_aliases: Default::default(),
        types_needing_drop: Default::default(),
        json_codec_structs: Default::default(),
        provider_names,
        extern_funcs,
        struct_field_types,
    };

    // Create lowering options for native struct support (LLVM backend)
    let lowering_options = LoweringOptions {
        use_native_structs: true,
        struct_defs,
    };

    // Lower all functions to IR
    let mut all_funcs: Vec<crate::compiler::ir::Func> = Vec::new();
    let mut all_strings: Vec<String> = Vec::new();

    for hir in &hirs {
        let pkg = hir
            .package
            .as_ref()
            .map(|p| p.to_string())
            .unwrap_or_default();
        for d in &hir.decls {
            if let crate::compiler::hir::HirDecl::Module(m) = d {
                for f in &m.funcs {
                    if f.body.is_some() {
                        let func_escape_info =
                            escape_results.get_function_by_parts(&pkg, Some(&m.name), &f.sig.name);
                        // Use native struct lowering for LLVM backend
                        let (mut funcs, strings) = lower_hir_func_to_ir_native(
                            f,
                            Some(&enum_ctx),
                            func_escape_info,
                            &lowering_options,
                        );

                        // Qualify function names with module prefix (except "main")
                        let mut name_remap: std::collections::HashMap<String, String> =
                            std::collections::HashMap::new();
                        for func in &mut funcs {
                            if func.name != "main" {
                                let old_name = func.name.clone();
                                let new_name = format!("{}.{}", m.name, func.name);
                                func.name = new_name.clone();
                                name_remap.insert(old_name, new_name);
                            }
                        }

                        // Update MakeClosure references
                        for func in &mut funcs {
                            for b in &mut func.blocks {
                                for inst in &mut b.insts {
                                    if let crate::compiler::ir::InstKind::MakeClosure {
                                        ref mut func,
                                        ..
                                    } = inst.kind
                                    {
                                        if let Some(new_name) = name_remap.get(func) {
                                            *func = new_name.clone();
                                        }
                                    }
                                }
                            }
                        }

                        // Adjust string indices
                        let base = all_strings.len() as u32;
                        if base > 0 {
                            for func in &mut funcs {
                                for b in &mut func.blocks {
                                    for inst in &mut b.insts {
                                        if let crate::compiler::ir::InstKind::ConstStr(ref mut ix) =
                                            inst.kind
                                        {
                                            *ix += base;
                                        }
                                    }
                                }
                            }
                        }

                        // Run optimization passes
                        for func in &mut funcs {
                            crate::compiler::ir::ssa::mem2reg_promote(func);
                            crate::compiler::ir::opt::run_simple_opts(func);
                        }

                        all_strings.extend(strings);
                        all_funcs.extend(funcs);
                    }
                }
            }
        }
    }

    // Ensure 'main' appears first if present
    if let Some(pos) = all_funcs.iter().position(|f| f.name == "main") {
        let f = all_funcs.remove(pos);
        all_funcs.insert(0, f);
    }

    if all_funcs.is_empty() {
        eprintln!("error: no functions found to compile");
        return 1;
    }

    // Check for main function
    if !all_funcs.iter().any(|f| f.name == "main") {
        eprintln!("error: no main() function found");
        return 1;
    }

    reporter.drain_to_stderr();

    // Build IR module
    let mut module = IrModule::new(path.file_name().and_then(|n| n.to_str()).unwrap_or("app"));
    module.strings = all_strings;
    module.funcs = all_funcs;

    // Add extern function declarations, struct, and enum definitions to the module
    // Collect struct and enum names first (for resolving nested type references)
    let mut struct_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut enum_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for hir in &hirs {
        for d in &hir.decls {
            match d {
                crate::compiler::hir::HirDecl::Struct(st) => {
                    struct_names.insert(st.name.clone());
                }
                crate::compiler::hir::HirDecl::Enum(en) => {
                    enum_names.insert(en.name.clone());
                }
                _ => {}
            }
        }
    }

    // Lower struct, enum, and extern func declarations
    for hir in &hirs {
        for d in &hir.decls {
            match d {
                crate::compiler::hir::HirDecl::ExternFunc(ef) => {
                    module.extern_funcs.push(lower_hir_extern_func_to_ir(ef));
                }
                crate::compiler::hir::HirDecl::Struct(st) => {
                    module
                        .structs
                        .push(lower_hir_struct_to_ir(st, &struct_names, &enum_names));
                }
                crate::compiler::hir::HirDecl::Enum(en) => {
                    module
                        .enums
                        .push(lower_hir_enum_to_ir(en, &struct_names, &enum_names));
                }
                _ => {}
            }
        }
    }

    // Security warning for native builds
    eprintln!("\x1b[33mwarning\x1b[0m: Native compilation produces an unsandboxed binary.");
    eprintln!("         Only compile code from trusted sources.");
    eprintln!("         See docs/security.md for details.\n");

    // Emit LLVM IR with target metadata so LLVM selects the correct ABI.
    let target = detect_target_triple();
    let llvm_ir = if debug {
        // Build source line table from loaded sources
        let source_pairs: Vec<(PathBuf, &str)> = sources
            .iter()
            .map(|sf| (sf.path.clone(), sf.text.as_str()))
            .collect();
        let line_table = SourceLineTable::from_sources(&source_pairs);

        let project_dir = path.parent().unwrap_or(path).to_string_lossy().into_owned();
        let mut debug_builder = DebugInfoBuilder::new("arth 0.1.0", &project_dir, line_table);
        emit_module_text_with_debug(&module, Some(&target), &mut debug_builder)
    } else {
        emit_module_text_for_target(&module, Some(&target))
    };

    // Set up output paths
    let out_dir = PathBuf::from("target/arth-out");
    let _ = std::fs::create_dir_all(&out_dir);
    let bin_path = out_dir.join("app");

    // Compile to native using linker
    // Note: The linker will write the IR to a temp file internally
    match compile_ir_to_native(&llvm_ir, &bin_path, None, opt_level, debug) {
        Ok(()) => {
            println!("build: success\n  binary: {}", bin_path.display());
            0
        }
        Err(e) => {
            eprintln!("error: {}", e);
            1
        }
    }
}

/// Load external packages from ~/.arth/libs based on project manifest dependencies.
/// Returns the resolved packages and sets up native library paths for FFI loading.
fn load_external_packages(path: &Path) -> (Vec<ExternalPackage>, Vec<vm::Library>) {
    let manifest_path = if path.is_dir() {
        path.join("arth.toml")
    } else {
        path.parent().unwrap_or(path).join("arth.toml")
    };

    let manifest = match parse_manifest(&manifest_path) {
        Ok(m) => m,
        Err(_) => return (Vec::new(), Vec::new()), // No manifest = no dependencies
    };

    if manifest.dependencies.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let ext_index = match discover_external_packages() {
        Ok(idx) => idx,
        Err(e) => {
            eprintln!("warning: failed to discover external packages: {}", e);
            return (Vec::new(), Vec::new());
        }
    };

    let ext_packages = match resolve_dependencies(&manifest, &ext_index) {
        Ok(pkgs) => pkgs,
        Err(e) => {
            eprintln!("error: {}", e);
            return (Vec::new(), Vec::new());
        }
    };

    // Set native library paths for FFI auto-loading
    let native_paths = collect_native_lib_paths(&ext_packages);
    if !native_paths.is_empty() {
        vm::set_external_lib_paths(native_paths);
    }

    // Load library bytecode
    let mut libraries = Vec::new();
    for pkg in &ext_packages {
        match vm::load_library(&pkg.bytecode_path, &pkg.id) {
            Ok(lib) => libraries.push(lib),
            Err(e) => {
                eprintln!("warning: failed to load library '{}': {}", pkg.id, e);
            }
        }
    }

    (ext_packages, libraries)
}

/// Load external packages and extract their symbols for the resolver.
/// Returns (symbols for resolver, packages for linking, libraries for linking)
fn load_external_symbols_for_resolver(
    path: &Path,
) -> (
    Vec<ExternalPackageSymbol>,
    Vec<ExternalPackage>,
    Vec<vm::Library>,
) {
    let manifest_path = if path.is_dir() {
        path.join("arth.toml")
    } else {
        path.parent().unwrap_or(path).join("arth.toml")
    };

    let manifest = match parse_manifest(&manifest_path) {
        Ok(m) => m,
        Err(_) => return (Vec::new(), Vec::new(), Vec::new()),
    };

    if manifest.dependencies.is_empty() {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    let ext_index = match discover_external_packages() {
        Ok(idx) => idx,
        Err(e) => {
            eprintln!("warning: failed to discover external packages: {}", e);
            return (Vec::new(), Vec::new(), Vec::new());
        }
    };

    let ext_packages = match resolve_dependencies(&manifest, &ext_index) {
        Ok(pkgs) => pkgs,
        Err(e) => {
            eprintln!("error: {}", e);
            return (Vec::new(), Vec::new(), Vec::new());
        }
    };

    // Collect native library paths for FFI
    let native_paths = collect_native_lib_paths(&ext_packages);
    if !native_paths.is_empty() {
        vm::set_external_lib_paths(native_paths);
    }

    // Extract symbols for the resolver
    let mut resolver_symbols = Vec::new();
    for pkg in &ext_packages {
        match load_external_package_symbols(pkg) {
            Ok(syms) => {
                for sym in syms {
                    resolver_symbols.push(ExternalPackageSymbol {
                        package: sym.package.clone(),
                        name: sym.symbol_name.clone(),
                        is_module: sym.kind == ExternalSymbolKind::Module,
                    });
                }
            }
            Err(e) => {
                eprintln!("warning: failed to load symbols from '{}': {}", pkg.id, e);
            }
        }
    }

    // Load libraries for bytecode linking
    let mut libraries = Vec::new();
    for pkg in &ext_packages {
        match vm::Library::from_bytes(
            pkg.id.clone(),
            &std::fs::read(&pkg.bytecode_path).unwrap_or_default(),
        ) {
            Ok(lib) => libraries.push(lib),
            Err(e) => {
                eprintln!("warning: failed to load library '{}': {}", pkg.id, e);
            }
        }
    }

    (resolver_symbols, ext_packages, libraries)
}

/// Run a project (directory or single .arth) by building to VM bytecode, or
/// run a precompiled .abc directly. This wires `arth run <path>` to the
/// expected UX.
pub fn cmd_run_auto(path: &Path) -> i32 {
    cmd_run_with_backend(path, Backend::Vm, 2, false)
}

/// Run a project or bytecode for a specific backend.
///
/// Behavior:
/// - VM: existing build+run flow for source paths, or direct `.abc` execution.
/// - LLVM: build native binary, then execute `target/arth-out/app`.
/// - Cranelift: delegates to existing backend build flow.
pub fn cmd_run_with_backend(path: &Path, backend: Backend, opt_level: u8, debug: bool) -> i32 {
    if path.extension() == Some(OsStr::new("abc")) {
        if !matches!(backend, Backend::Vm) {
            eprintln!("error: .abc bytecode can only run with --backend vm");
            return 2;
        }
        return cmd_run_vm(path);
    }

    match backend {
        Backend::Vm => {
            // Delegate to the VM backend build path, which currently compiles sources,
            // writes target/arth-out/app.abc, and runs it.
            cmd_build_with_backend(path, Backend::Vm, opt_level, false)
        }
        Backend::Llvm => {
            let code = cmd_build_with_backend(path, Backend::Llvm, opt_level, debug);
            if code != 0 {
                return code;
            }

            let bin_path = PathBuf::from("target").join("arth-out").join("app");
            let status = Command::new(&bin_path).status();
            match status {
                Ok(status) => {
                    let exit = status.code().unwrap_or(-1);
                    println!("native: exit code {}", exit);
                    0
                }
                Err(e) => {
                    eprintln!("error: failed to execute {}: {}", bin_path.display(), e);
                    1
                }
            }
        }
        Backend::Cranelift => cmd_build_with_backend(path, Backend::Cranelift, opt_level, false),
    }
}

// (debug helpers removed)

pub fn cmd_build_with_backend(path: &Path, backend: Backend, opt_level: u8, debug: bool) -> i32 {
    let check = cmd_check(path);
    if check != 0 {
        return check;
    }

    match backend {
        Backend::Llvm => cmd_build_native(path, opt_level, debug),
        Backend::Cranelift => {
            let mut reporter = Reporter::new();
            let msg: Option<String> = if let Some(entry) = read_entry_from_arth_toml(path) {
                match SourceFile::load_from_path(&entry) {
                    Ok(sf) => {
                        let ast = parse_file(&sf, &mut reporter);
                        let expected_mod = module_name_from_path(&entry);
                        let (_ctrls, prints, first) = {
                            let (c, p, f, _) = scan_demo_info(&ast, expected_mod.as_deref(), true);
                            (c, p, f)
                        };
                        prints.first().cloned().or(first)
                    }
                    Err(e) => {
                        eprintln!("error: failed to read entry {}: {}", entry.display(), e);
                        None
                    }
                }
            } else {
                let Ok(sources) = load_sources(path) else {
                    eprintln!("error: failed to read sources at {}", path.display());
                    return 1;
                };
                let mut found: Option<String> = None;
                for sf in &sources {
                    let ast = parse_file(sf, &mut reporter);
                    let (_ctrls, prints, first) = {
                        let (c, p, f, _) = scan_demo_info(&ast, None, false);
                        (c, p, f)
                    };
                    if let Some(s) = prints.first().cloned().or(first) {
                        found = Some(s);
                        break;
                    }
                }
                found
            };
            reporter.drain_to_stderr();

            // Security warning for JIT builds
            eprintln!(
                "\x1b[33mwarning\x1b[0m: Cranelift JIT compiles to native code without sandboxing."
            );
            eprintln!("         Only run code from trusted sources.");
            eprintln!("         See docs/security.md for details.\n");

            let m = demo_add_module();
            match cg_cl::compile_module_with_message(&m, msg.as_deref()) {
                Ok(()) => {
                    println!("build: success (cranelift jit)");
                    0
                }
                Err(e) => {
                    eprintln!("error: {}", e);
                    1
                }
            }
        }
        Backend::Vm => {
            let mut reporter = Reporter::new();
            let mut messages: Vec<String> = Vec::new();

            // Load external package symbols early for resolution
            let (ext_symbols, ext_packages, ext_libraries) =
                load_external_symbols_for_resolver(path);
            if let Some(entry) = read_entry_from_arth_toml(path) {
                match SourceFile::load_from_path(&entry) {
                    Ok(sf) => {
                        let ast = parse_file(&sf, &mut reporter);
                        let expected_mod = module_name_from_path(&entry);
                        let (ctrls, prints, first, body) =
                            scan_demo_info(&ast, expected_mod.as_deref(), true);
                        messages.extend(control_messages(&ctrls));
                        if body.is_none() {
                            if let Some(mn) = expected_mod.as_deref() {
                                eprintln!(
                                    "error: entry '{}' must declare module '{}' with public main()",
                                    entry.display(),
                                    mn
                                );
                            } else {
                                eprintln!(
                                    "error: entry '{}' does not contain a public main()",
                                    entry.display()
                                );
                            }
                            return 1;
                        }
                        if !prints.is_empty() {
                            messages.extend(prints);
                        } else if let Some(s) = first {
                            messages.push(s);
                        }
                    }
                    Err(e) => {
                        eprintln!("error: failed to read entry {}: {}", entry.display(), e);
                    }
                }
            } else {
                let Ok(sources) = load_sources(path) else {
                    eprintln!("error: failed to read sources at {}", path.display());
                    return 1;
                };
                for sf in &sources {
                    let ast = parse_file(sf, &mut reporter);
                    let (ctrls, prints, first, _) = scan_demo_info(&ast, None, false);
                    messages.extend(control_messages(&ctrls));
                    if !prints.is_empty() {
                        messages.extend(prints);
                        break;
                    }
                    if let Some(s) = first {
                        messages.push(s);
                        break;
                    }
                }
            }
            reporter.drain_to_stderr();
            if messages.is_empty() {
                messages.push("Hello, world!".to_string());
            }

            // Prefer IR→VM logging program if entry has a public main
            let prog = if let Some(entry) = read_entry_from_arth_toml(path) {
                if let Ok(sf) = SourceFile::load_from_path(&entry) {
                    let ast = parse_file(&sf, &mut reporter);
                    let expected_mod = module_name_from_path(&entry);
                    let (_, _, _, body) = scan_demo_info(&ast, expected_mod.as_deref(), true);
                    if body.is_some() {
                        // Multi-file/multi-module lowering: parse and lower all project sources
                        let program_or_hirs_and_escape = if let Ok(sources) = load_sources(path) {
                            let mut hirs: Vec<crate::compiler::hir::HirFile> = Vec::new();
                            let mut asts: Vec<(SourceFile, crate::compiler::ast::FileAst)> =
                                Vec::new();
                            for sf in &sources {
                                let ast = parse_file(sf, &mut reporter);
                                asts.push((sf.clone(), ast.clone()));
                                let hir = lower_file(sf, &ast);
                                hirs.push(hir);
                            }
                            // Run typechecker to get escape analysis results
                            let resolved = if ext_symbols.is_empty() {
                                resolve_project(path, &asts, &mut reporter)
                            } else {
                                resolve_project_with_externals(
                                    path,
                                    &asts,
                                    &ext_symbols,
                                    &mut reporter,
                                )
                            };
                            let escape_results =
                                typecheck_project(path, &asts, &resolved, &mut reporter);
                            Ok((hirs, asts, escape_results))
                        } else {
                            Err(vm::compile_messages_to_program(&messages))
                        };
                        match program_or_hirs_and_escape {
                            Err(prog) => prog,
                            Ok((hirs, _asts, escape_results)) => {
                                // Aggregate enum tags and shared provider field names across files
                                let mut tags: std::collections::BTreeMap<
                                    String,
                                    std::collections::BTreeMap<String, i64>,
                                > = Default::default();
                                let mut shared_field_names: std::collections::HashSet<String> =
                                    Default::default();
                                let mut provider_names: std::collections::HashSet<String> =
                                    Default::default();
                                let mut ir_providers: Vec<crate::compiler::ir::Provider> =
                                    Vec::new();
                                let mut extern_funcs: std::collections::HashMap<String, ExternSig> =
                                    Default::default();
                                let mut struct_field_types: std::collections::HashMap<
                                    (String, String),
                                    String,
                                > = Default::default();
                                for hir in &hirs {
                                    for d2 in &hir.decls {
                                        if let crate::compiler::hir::HirDecl::Enum(en) = d2 {
                                            let vmap = compute_enum_tags(en);
                                            tags.insert(en.name.clone(), vmap);
                                        } else if let crate::compiler::hir::HirDecl::Provider(pv) =
                                            d2
                                        {
                                            provider_names.insert(pv.name.clone());
                                            ir_providers.push(crate::compiler::lower::hir_to_ir::lower_hir_provider_to_ir(pv));
                                            for f in &pv.fields {
                                                if f.is_shared {
                                                    shared_field_names.insert(f.name.clone());
                                                }
                                                // Record field type for nested provider access detection
                                                let ty_name = match &f.ty {
                                                    crate::compiler::hir::HirType::Name {
                                                        path,
                                                    } => path.last().cloned(),
                                                    crate::compiler::hir::HirType::Generic {
                                                        path,
                                                        ..
                                                    } => path.last().cloned(),
                                                    _ => None,
                                                };
                                                if let Some(name) = ty_name {
                                                    struct_field_types.insert(
                                                        (pv.name.clone(), f.name.clone()),
                                                        name,
                                                    );
                                                }
                                            }
                                        } else if let crate::compiler::hir::HirDecl::Struct(st) = d2
                                        {
                                            // Record struct field types for nested access detection
                                            for f in &st.fields {
                                                let ty_name = match &f.ty {
                                                    crate::compiler::hir::HirType::Name {
                                                        path,
                                                    } => path.last().cloned(),
                                                    crate::compiler::hir::HirType::Generic {
                                                        path,
                                                        ..
                                                    } => path.last().cloned(),
                                                    _ => None,
                                                };
                                                if let Some(name) = ty_name {
                                                    struct_field_types.insert(
                                                        (st.name.clone(), f.name.clone()),
                                                        name,
                                                    );
                                                }
                                            }
                                        } else if let crate::compiler::hir::HirDecl::ExternFunc(
                                            ef,
                                        ) = d2
                                        {
                                            extern_funcs
                                                .insert(ef.name.clone(), make_extern_sig(ef));
                                        }
                                    }
                                }
                                // Type aliases are currently reserved for FFI bindings; disable aliasing in lowering.
                                // For the VM backend demo path we do not require JsonCodec metadata.
                                let json_codec_structs: std::collections::BTreeMap<
                                    String,
                                    JsonCodecMeta,
                                > = Default::default();
                                let enum_ctx = EnumLowerContext {
                                    tags,
                                    shared_field_names,
                                    type_aliases: Default::default(),
                                    types_needing_drop: Default::default(),
                                    json_codec_structs,
                                    provider_names,
                                    extern_funcs,
                                    struct_field_types,
                                };

                                // Lower all functions with bodies across all modules and free functions in all files
                                let mut all_funcs: Vec<crate::compiler::ir::Func> = Vec::new();
                                let mut all_strings: Vec<String> = Vec::new();
                                for hir in &hirs {
                                    let pkg = hir
                                        .package
                                        .as_ref()
                                        .map(|p| p.to_string())
                                        .unwrap_or_default();
                                    for d in &hir.decls {
                                        if let crate::compiler::hir::HirDecl::Module(m) = d {
                                            for f in &m.funcs {
                                                if f.body.is_some() {
                                                    // Look up escape info for this function
                                                    let func_escape_info = escape_results
                                                        .get_function_by_parts(
                                                            &pkg,
                                                            Some(&m.name),
                                                            &f.sig.name,
                                                        );
                                                    let (mut funcs, strings) =
                                                        lower_hir_func_to_ir_with_escape(
                                                            f,
                                                            Some(&enum_ctx),
                                                            func_escape_info,
                                                        );
                                                    // Qualify function names with module prefix (except "main")
                                                    // Also build a mapping for lambda function renames
                                                    let mut name_remap: std::collections::HashMap<
                                                        String,
                                                        String,
                                                    > = std::collections::HashMap::new();
                                                    for func in &mut funcs {
                                                        if func.name != "main" {
                                                            let old_name = func.name.clone();
                                                            let new_name =
                                                                format!("{}.{}", m.name, func.name);
                                                            func.name = new_name.clone();
                                                            name_remap.insert(old_name, new_name);
                                                        }
                                                    }
                                                    // Update MakeClosure references to use qualified names
                                                    for func in &mut funcs {
                                                        for b in &mut func.blocks {
                                                            for inst in &mut b.insts {
                                                                if let crate::compiler::ir::InstKind::MakeClosure { ref mut func, .. } = inst.kind {
                                                                    if let Some(new_name) = name_remap.get(func) {
                                                                        *func = new_name.clone();
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                    let base = all_strings.len() as u32;
                                                    if base > 0 {
                                                        // Adjust string indices in funcs
                                                        for func in &mut funcs {
                                                            for b in &mut func.blocks {
                                                                for inst in &mut b.insts {
                                                                    if let crate::compiler::ir::InstKind::ConstStr(ref mut ix) = inst.kind { *ix += base; }
                                                                }
                                                            }
                                                        }
                                                    }
                                                    all_strings.extend(strings);
                                                    all_funcs.extend(funcs);
                                                }
                                            }
                                        }
                                    }
                                }
                                // Ensure 'main' appears first if present
                                if let Some(pos) = all_funcs.iter().position(|f| f.name == "main") {
                                    let f = all_funcs.remove(pos);
                                    all_funcs.insert(0, f);
                                }
                                super::vm::compile_ir_to_program(
                                    &all_funcs,
                                    &all_strings,
                                    &ir_providers,
                                )
                            }
                        }
                    } else {
                        vm::compile_messages_to_program(&messages)
                    }
                } else {
                    vm::compile_messages_to_program(&messages)
                }
            } else if path.extension().and_then(|s| s.to_str()) == Some("arth") {
                // Single-file mode: try IR path for standalone .arth files
                if let Ok(sf) = SourceFile::load_from_path(path) {
                    let ast = parse_file(&sf, &mut reporter);
                    // For single file mode, look for any module with main(), not one matching filename
                    let (_, _, _, body) = scan_demo_info(&ast, None, false);
                    if body.is_some() {
                        // Single-file IR lowering
                        let hir = lower_file(&sf, &ast);
                        let hirs = vec![hir];
                        let asts = vec![(sf.clone(), ast.clone())];
                        let resolved =
                            resolve_project(path.parent().unwrap_or(path), &asts, &mut reporter);
                        let escape_results = typecheck_project(
                            path.parent().unwrap_or(path),
                            &asts,
                            &resolved,
                            &mut reporter,
                        );

                        // Aggregate enum tags, provider info, and extern funcs
                        let mut tags: std::collections::BTreeMap<
                            String,
                            std::collections::BTreeMap<String, i64>,
                        > = Default::default();
                        let mut shared_field_names: std::collections::HashSet<String> =
                            Default::default();
                        let mut provider_names: std::collections::HashSet<String> =
                            Default::default();
                        let mut ir_providers: Vec<crate::compiler::ir::Provider> = Vec::new();
                        let mut extern_funcs: std::collections::HashMap<String, ExternSig> =
                            Default::default();
                        let mut struct_field_types: std::collections::HashMap<
                            (String, String),
                            String,
                        > = Default::default();
                        for hir in &hirs {
                            for d2 in &hir.decls {
                                if let crate::compiler::hir::HirDecl::Enum(en) = d2 {
                                    let vmap = compute_enum_tags(en);
                                    tags.insert(en.name.clone(), vmap);
                                } else if let crate::compiler::hir::HirDecl::Provider(pv) = d2 {
                                    provider_names.insert(pv.name.clone());
                                    ir_providers.push(
                                        crate::compiler::lower::hir_to_ir::lower_hir_provider_to_ir(
                                            pv,
                                        ),
                                    );
                                    for f in &pv.fields {
                                        if f.is_shared {
                                            shared_field_names.insert(f.name.clone());
                                        }
                                        // Record field type for nested provider access detection
                                        let ty_name = match &f.ty {
                                            crate::compiler::hir::HirType::Name { path } => {
                                                path.last().cloned()
                                            }
                                            crate::compiler::hir::HirType::Generic {
                                                path, ..
                                            } => path.last().cloned(),
                                            _ => None,
                                        };
                                        if let Some(name) = ty_name {
                                            struct_field_types
                                                .insert((pv.name.clone(), f.name.clone()), name);
                                        }
                                    }
                                } else if let crate::compiler::hir::HirDecl::Struct(st) = d2 {
                                    // Record struct field types for nested access detection
                                    for f in &st.fields {
                                        let ty_name = match &f.ty {
                                            crate::compiler::hir::HirType::Name { path } => {
                                                path.last().cloned()
                                            }
                                            crate::compiler::hir::HirType::Generic {
                                                path, ..
                                            } => path.last().cloned(),
                                            _ => None,
                                        };
                                        if let Some(name) = ty_name {
                                            struct_field_types
                                                .insert((st.name.clone(), f.name.clone()), name);
                                        }
                                    }
                                } else if let crate::compiler::hir::HirDecl::ExternFunc(ef) = d2 {
                                    extern_funcs.insert(ef.name.clone(), make_extern_sig(ef));
                                }
                            }
                        }
                        let json_codec_structs: std::collections::BTreeMap<String, JsonCodecMeta> =
                            Default::default();
                        let enum_ctx = EnumLowerContext {
                            tags,
                            shared_field_names,
                            type_aliases: Default::default(),
                            types_needing_drop: Default::default(),
                            json_codec_structs,
                            provider_names,
                            extern_funcs,
                            struct_field_types,
                        };

                        // Lower all functions
                        let mut all_funcs: Vec<crate::compiler::ir::Func> = Vec::new();
                        let mut all_strings: Vec<String> = Vec::new();
                        for hir in &hirs {
                            let pkg = hir
                                .package
                                .as_ref()
                                .map(|p| p.to_string())
                                .unwrap_or_default();
                            for d in &hir.decls {
                                if let crate::compiler::hir::HirDecl::Module(m) = d {
                                    for f in &m.funcs {
                                        if f.body.is_some() {
                                            let func_escape_info = escape_results
                                                .get_function_by_parts(
                                                    &pkg,
                                                    Some(&m.name),
                                                    &f.sig.name,
                                                );
                                            let (mut funcs, strings) =
                                                lower_hir_func_to_ir_with_escape(
                                                    f,
                                                    Some(&enum_ctx),
                                                    func_escape_info,
                                                );
                                            // Qualify function names with module prefix (except "main")
                                            // Also build a mapping for lambda function renames
                                            let mut name_remap: std::collections::HashMap<
                                                String,
                                                String,
                                            > = std::collections::HashMap::new();
                                            for func in &mut funcs {
                                                if func.name != "main" {
                                                    let old_name = func.name.clone();
                                                    let new_name =
                                                        format!("{}.{}", m.name, func.name);
                                                    func.name = new_name.clone();
                                                    name_remap.insert(old_name, new_name);
                                                }
                                            }
                                            // Update MakeClosure references to use qualified names
                                            for func in &mut funcs {
                                                for b in &mut func.blocks {
                                                    for inst in &mut b.insts {
                                                        if let crate::compiler::ir::InstKind::MakeClosure { ref mut func, .. } = inst.kind {
                                                            if let Some(new_name) = name_remap.get(func) {
                                                                *func = new_name.clone();
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            let base = all_strings.len() as u32;
                                            if base > 0 {
                                                for func in &mut funcs {
                                                    for b in &mut func.blocks {
                                                        for inst in &mut b.insts {
                                                            if let crate::compiler::ir::InstKind::ConstStr(ref mut ix) = inst.kind { *ix += base; }
                                                        }
                                                    }
                                                }
                                            }
                                            all_strings.extend(strings);
                                            all_funcs.extend(funcs);
                                        }
                                    }
                                }
                            }
                        }

                        // Ensure 'main' appears first
                        if let Some(pos) = all_funcs.iter().position(|f| f.name == "main") {
                            let f = all_funcs.remove(pos);
                            all_funcs.insert(0, f);
                        }
                        super::vm::compile_ir_to_program(&all_funcs, &all_strings, &ir_providers)
                    } else {
                        vm::compile_messages_to_program(&messages)
                    }
                } else {
                    vm::compile_messages_to_program(&messages)
                }
            } else {
                vm::compile_messages_to_program(&messages)
            };
            let out_dir = PathBuf::from("target/arth-out");
            let _ = std::fs::create_dir_all(&out_dir);
            let abc_path = out_dir.join("app.abc");
            if let Err(e) = vm::write_abc_file(&abc_path, &prog) {
                eprintln!("error: failed to write {}: {}", abc_path.display(), e);
                return 1;
            }
            println!("vm: wrote {}", abc_path.display());

            // Use already-loaded external packages from resolution phase
            if !ext_packages.is_empty() {
                println!("vm: loaded {} external package(s)", ext_packages.len());
            }

            // Link libraries with main program if any
            let final_prog = if ext_libraries.is_empty() {
                prog
            } else {
                let linked = vm::link_programs(prog, ext_libraries);
                // Set up the symbol table for cross-library function calls
                vm::set_linked_symbol_table(linked.symbol_table);
                linked.program
            };

            let ctx = vm::HostContext::std();
            let code = vm::run_program_with_host(&final_prog, &ctx);
            println!("vm: exit code {}", code);
            0
        }
    }
}
fn module_name_from_path(p: &Path) -> Option<String> {
    p.file_stem().map(|s| s.to_string_lossy().to_string())
}

fn find_main_body(
    ast: &crate::compiler::ast::FileAst,
    expected_module: Option<&str>,
    require_public: bool,
) -> Option<Block> {
    for d in &ast.decls {
        if let Decl::Module(m) = d {
            // If we have an expected module name (entry file), enforce it; else fall back to "Main" for demos.
            let want_name = expected_module.unwrap_or("Main");
            if m.name.0 != want_name {
                continue;
            }
            for f in &m.items {
                if f.sig.name.0 == "main"
                    && f.sig.ret.is_none()
                    && (!require_public || f.sig.vis == Visibility::Public)
                    && let Some(b) = &f.body
                {
                    return Some(b.clone());
                }
            }
        }
    }
    None
}

fn collect_from_block(
    block: &Block,
    prints: &mut Vec<String>,
    controls: &mut Vec<ControlKind>,
    first_string: &mut Option<String>,
) {
    for s in &block.stmts {
        match s {
            Stmt::PrintStr(t) => {
                prints.push(t.clone());
                if first_string.is_none() {
                    *first_string = Some(t.clone());
                }
            }
            Stmt::PrintRawStr(t) => {
                prints.push(t.clone());
                if first_string.is_none() {
                    *first_string = Some(t.clone());
                }
            }
            Stmt::PrintExpr(_) => {}
            Stmt::PrintRawExpr(_) => {}
            Stmt::VarDecl { .. } => {}
            Stmt::AssignOp { .. } => {}
            Stmt::If {
                then_blk, else_blk, ..
            } => {
                controls.push(ControlKind::If);
                collect_from_block(then_blk, prints, controls, first_string);
                if let Some(b) = else_blk {
                    controls.push(ControlKind::Else);
                    collect_from_block(b, prints, controls, first_string);
                }
            }
            Stmt::While { body, .. } => {
                controls.push(ControlKind::While);
                collect_from_block(body, prints, controls, first_string);
            }
            Stmt::For { body, .. } => {
                controls.push(ControlKind::For);
                collect_from_block(body, prints, controls, first_string);
            }
            Stmt::Labeled { stmt, .. } => {
                // Collect controls within the labeled statement as usual
                match &**stmt {
                    Stmt::While { body, .. } => {
                        controls.push(ControlKind::While);
                        collect_from_block(body, prints, controls, first_string);
                    }
                    Stmt::For { body, .. } => {
                        controls.push(ControlKind::For);
                        collect_from_block(body, prints, controls, first_string);
                    }
                    Stmt::Switch { cases, default, .. } => {
                        controls.push(ControlKind::Switch);
                        for (_, blk) in cases {
                            controls.push(ControlKind::Case);
                            collect_from_block(blk, prints, controls, first_string);
                        }
                        if let Some(db) = default {
                            controls.push(ControlKind::Default);
                            collect_from_block(db, prints, controls, first_string);
                        }
                    }
                    Stmt::If {
                        then_blk, else_blk, ..
                    } => {
                        controls.push(ControlKind::If);
                        collect_from_block(then_blk, prints, controls, first_string);
                        if let Some(b) = else_blk {
                            controls.push(ControlKind::Else);
                            collect_from_block(b, prints, controls, first_string);
                        }
                    }
                    Stmt::Block(b) => collect_from_block(b, prints, controls, first_string),
                    _ => {}
                }
            }
            Stmt::Switch { cases, default, .. } => {
                controls.push(ControlKind::Switch);
                for (_, blk) in cases {
                    controls.push(ControlKind::Case);
                    collect_from_block(blk, prints, controls, first_string);
                }
                if let Some(db) = default {
                    controls.push(ControlKind::Default);
                    collect_from_block(db, prints, controls, first_string);
                }
            }
            Stmt::Try {
                try_blk,
                catches,
                finally_blk,
            } => {
                controls.push(ControlKind::Try);
                collect_from_block(try_blk, prints, controls, first_string);
                for c in catches {
                    controls.push(ControlKind::Catch);
                    collect_from_block(&c.blk, prints, controls, first_string);
                }
                if let Some(fb) = finally_blk {
                    controls.push(ControlKind::Finally);
                    collect_from_block(fb, prints, controls, first_string);
                }
            }
            Stmt::Assign { .. } => {}
            Stmt::FieldAssign { .. } => {}
            Stmt::Break(_) => controls.push(ControlKind::Break),
            Stmt::Continue(_) => controls.push(ControlKind::Continue),
            Stmt::Return(_) => controls.push(ControlKind::Return),
            Stmt::Throw(_) => controls.push(ControlKind::Throw),
            Stmt::Panic(_) => {} // Panic doesn't contribute to control kind summary
            Stmt::Block(b) => collect_from_block(b, prints, controls, first_string),
            Stmt::Unsafe(b) => collect_from_block(b, prints, controls, first_string),
            Stmt::Expr(e) => {
                // Recognize logger level calls in demo summary: logger.info("ev", "msg", Fields.of(...))
                use crate::compiler::ast::Expr as AE;
                fn format_log_call(e: &crate::compiler::ast::Expr) -> Option<String> {
                    let AE::Call(callee, args) = e else {
                        return None;
                    };
                    let AE::Member(_obj, level_ident) = &**callee else {
                        return None;
                    };
                    let level = level_ident.0.as_str();
                    if !(matches!(level, "trace" | "debug" | "info" | "warn" | "error")) {
                        return None;
                    }
                    let mut event_s: Option<String> = None;
                    let mut message_s: Option<String> = None;
                    if let Some(s) = args.first().and_then(|a| match a {
                        AE::Str(s) => Some(s.clone()),
                        _ => None,
                    }) {
                        event_s = Some(s);
                    }
                    if let Some(AE::Str(s)) = args.get(1) {
                        message_s = Some(s.clone());
                    }
                    let mut fields_txt: Vec<String> = Vec::new();
                    if let Some(a2) = args.get(2)
                        && let AE::Call(fcallee, fargs) = a2
                        && let AE::Member(obj2, name2) = &**fcallee
                        && name2.0 == "of"
                    {
                        let is_fields = match &**obj2 {
                            AE::Ident(crate::compiler::ast::Ident(s)) if s == "Fields" => true,
                            AE::Member(pkg, ident) => {
                                matches!((**pkg).clone(), AE::Ident(crate::compiler::ast::Ident(p)) if p=="log")
                                    && ident.0 == "Fields"
                            }
                            _ => false,
                        };
                        if is_fields {
                            let mut it = fargs.iter();
                            while let Some(k) = it.next() {
                                let v = it.next();
                                let key = match k {
                                    AE::Str(s) => s.clone(),
                                    _ => "?".to_string(),
                                };
                                let val = match v {
                                    Some(AE::Str(s)) => s.clone(),
                                    Some(AE::Int(n)) => n.to_string(),
                                    Some(AE::Bool(b)) => {
                                        if *b {
                                            "true".to_string()
                                        } else {
                                            "false".to_string()
                                        }
                                    }
                                    _ => "?".to_string(),
                                };
                                fields_txt.push(format!("{}={}", key, val));
                            }
                        }
                    }
                    let lvl = match level {
                        "trace" => "TRACE",
                        "debug" => "DEBUG",
                        "info" => "INFO",
                        "warn" => "WARN",
                        _ => "ERROR",
                    };
                    let mut line = String::new();
                    line.push_str(lvl);
                    if let Some(ev) = event_s {
                        line.push(' ');
                        line.push_str(&ev);
                    }
                    if let Some(msg) = message_s {
                        line.push_str(": ");
                        line.push_str(&msg);
                    }
                    if !fields_txt.is_empty() {
                        line.push(' ');
                        line.push_str(&fields_txt.join(" "));
                    }
                    Some(line)
                }
                if let Some(line) = format_log_call(e) {
                    if first_string.is_none() {
                        *first_string = Some(line.clone());
                    }
                    prints.push(line);
                }
            }
        }
    }
}

fn scan_demo_info(
    ast: &crate::compiler::ast::FileAst,
    expected_module: Option<&str>,
    require_public: bool,
) -> (Vec<ControlKind>, Vec<String>, Option<String>, Option<Block>) {
    let mut controls = Vec::new();
    let mut prints = Vec::new();
    let mut first_string: Option<String> = None;
    let body = find_main_body(ast, expected_module, require_public);
    if let Some(ref b) = body {
        collect_from_block(b, &mut prints, &mut controls, &mut first_string);
    }
    (controls, prints, first_string, body)
}

/// Build an index of structs with @derive(JsonCodec) attribute.
/// Returns a map from struct name to JsonCodecMeta containing field metadata.
#[allow(dead_code)]
fn build_json_codec_index(
    asts: &[(SourceFile, crate::compiler::ast::FileAst)],
) -> std::collections::BTreeMap<String, JsonCodecMeta> {
    use crate::compiler::ast::Decl as AD;
    let mut result = std::collections::BTreeMap::new();

    for (_sf, ast) in asts {
        for d in &ast.decls {
            if let AD::Struct(s) = d {
                // Check for @derive(JsonCodec) attribute
                let mut has_json_codec = false;
                let mut ignore_unknown = false;

                for attr in &s.attrs {
                    let attr_name = attr
                        .name
                        .path
                        .last()
                        .map(|i| i.0.as_str())
                        .unwrap_or_default();
                    if attr_name == "derive" {
                        if let Some(args) = &attr.args {
                            if args.contains("JsonCodec") {
                                has_json_codec = true;
                                // Check for ignoreUnknown option
                                if args.contains("ignoreUnknown") {
                                    ignore_unknown = true;
                                }
                            }
                        }
                    }
                }

                if !has_json_codec {
                    continue;
                }

                // Build field metadata: "name:idx,name:idx,...;flags"
                let mut field_parts = Vec::new();
                for (idx, field) in s.fields.iter().enumerate() {
                    // Check for @JsonIgnore attribute on field
                    let has_json_ignore = field.attrs.iter().any(|a| {
                        a.name
                            .path
                            .last()
                            .map(|i| i.0.as_str() == "JsonIgnore")
                            .unwrap_or(false)
                    });

                    if has_json_ignore {
                        continue; // Skip ignored fields
                    }

                    // Check for @rename attribute to get JSON field name
                    let json_name = field
                        .attrs
                        .iter()
                        .find_map(|a| {
                            let name = a.name.path.last().map(|i| i.0.as_str()).unwrap_or_default();
                            if name == "rename" {
                                // Parse @rename(name="json_name") or @rename("json_name")
                                if let Some(args) = &a.args {
                                    // Try to extract name="..." pattern
                                    if let Some(start) = args.find("name=\"") {
                                        let rest = &args[start + 6..];
                                        if let Some(end) = rest.find('"') {
                                            return Some(rest[..end].to_string());
                                        }
                                    }
                                    // Try simple @rename("name") pattern
                                    if args.starts_with('"') && args.ends_with('"') {
                                        return Some(args[1..args.len() - 1].to_string());
                                    }
                                }
                            }
                            None
                        })
                        .unwrap_or_else(|| field.name.0.clone());

                    field_parts.push(format!("{}:{}", json_name, idx));
                }

                // Build flags
                let mut flags = String::new();
                if ignore_unknown {
                    flags.push('I');
                }

                // Construct field_meta string
                let field_meta = if flags.is_empty() {
                    field_parts.join(",")
                } else {
                    format!("{};{}", field_parts.join(","), flags)
                };

                result.insert(s.name.0.clone(), JsonCodecMeta { field_meta });
            }
        }
    }

    result
}

// ============================================================================
// Formatter Command
// ============================================================================

/// Options for the format command.
#[derive(Clone, Debug, Default)]
pub struct FormatOptions {
    /// Only check if files are formatted (don't modify).
    pub check: bool,
    /// Format input from stdin and write to stdout.
    pub stdin: bool,
}

/// Format source files.
pub fn cmd_fmt(path: &Path, opts: FormatOptions) -> i32 {
    use crate::compiler::fmt::{FormatConfig, format};
    use std::fs;

    let config = FormatConfig::default();

    // Collect all .arth files
    let files = collect_arth_files(path);
    if files.is_empty() {
        eprintln!("No .arth files found");
        return 1;
    }

    let mut needs_formatting = false;
    let mut errors = false;

    for file_path in &files {
        // Read and parse the file
        let text = match fs::read_to_string(file_path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{}: {}", file_path.display(), e);
                errors = true;
                continue;
            }
        };

        let sf = SourceFile {
            path: file_path.clone(),
            text,
        };

        let mut reporter = Reporter::new();
        let ast = parse_file(&sf, &mut reporter);

        if reporter.has_errors() {
            eprintln!("{}: parse errors", file_path.display());
            reporter.drain_to_stderr();
            errors = true;
            continue;
        }

        let result = format(&ast, &config);

        if opts.check {
            // Just check if file is formatted
            if result.output != sf.text {
                println!("Would reformat: {}", file_path.display());
                needs_formatting = true;
            }
        } else {
            // Write formatted output
            if result.output != sf.text {
                if let Err(e) = fs::write(file_path, &result.output) {
                    eprintln!("{}: {}", file_path.display(), e);
                    errors = true;
                } else {
                    println!("Formatted: {}", file_path.display());
                }
            }
        }
    }

    if errors || (opts.check && needs_formatting) {
        1
    } else {
        0
    }
}

/// Collect all .arth files from a path (file or directory).
fn collect_arth_files(path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if path.is_file() {
        if path.extension().is_some_and(|e| e == "arth") {
            files.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        collect_arth_files_recursive(path, &mut files);
    }

    files
}

fn collect_arth_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Skip hidden directories and target
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with('.') && name != "target" {
                    collect_arth_files_recursive(&path, files);
                }
            } else if path.extension().is_some_and(|e| e == "arth") {
                files.push(path);
            }
        }
    }
}

// ============================================================================
// Lint Command
// ============================================================================

/// Options for the lint command.
#[derive(Clone, Debug, Default)]
pub struct LintOptions {
    /// Output format: "human" or "json".
    pub format: String,
    /// Treat warnings as errors.
    pub warnings_as_errors: bool,
}

/// Run lints on source files.
pub fn cmd_lint(path: &Path, opts: LintOptions) -> i32 {
    use crate::compiler::lint::{LintConfig, run_lints};
    use crate::compiler::typeck::attrs::validate_attributes;

    // Load and parse sources
    let sources = match load_sources(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error loading sources: {}", e);
            return 1;
        }
    };

    if sources.is_empty() {
        eprintln!("No .arth files found");
        return 1;
    }

    // Parse all files
    let mut reporter = Reporter::new();
    let mut asts = Vec::new();
    for sf in &sources {
        let ast = parse_file(sf, &mut reporter);
        asts.push(ast);
    }

    if reporter.has_errors() {
        reporter.drain_to_stderr();
        return 1;
    }

    // Lower to HIR
    let hirs: Vec<_> = sources
        .iter()
        .zip(asts.iter())
        .map(|(sf, ast)| lower_file(sf, ast))
        .collect();

    // Analyze attributes to get AllowIndex
    let mut attr_reporter = Reporter::new();
    let files: Vec<_> = sources
        .iter()
        .zip(asts.iter())
        .map(|(sf, ast)| (sf.clone(), ast.clone()))
        .collect();
    let analysis = validate_attributes(&files, &mut attr_reporter);

    // Configure lints
    let mut lint_config = LintConfig::new();
    if opts.warnings_as_errors {
        lint_config.warnings_as_errors = true;
    }

    // Run lints
    let mut lint_reporter = run_lints(&hirs, &analysis.allows, &lint_config);

    // Output results
    let diags = lint_reporter.diagnostics();
    if opts.format == "json" {
        // JSON output
        print!("[");
        for (i, diag) in diags.iter().enumerate() {
            if i > 0 {
                print!(",");
            }
            print!(
                "{{\"severity\":\"{:?}\",\"message\":{:?}",
                diag.severity, diag.message
            );
            if let Some(ref code) = diag.code {
                print!(",\"code\":{:?}", code);
            }
            if let Some(ref file) = diag.file {
                print!(",\"file\":{:?}", file.display().to_string());
            }
            print!("}}");
        }
        println!("]");
    } else {
        // Human output
        lint_reporter.drain_to_stderr();
    }

    if lint_reporter.has_errors() || (opts.warnings_as_errors && lint_reporter.has_warnings()) {
        1
    } else {
        0
    }
}

// ============================================================================
// Test Command
// ============================================================================

/// Options for the test command.
#[derive(Clone, Debug, Default)]
pub struct TestOptions {
    /// Filter pattern for test names.
    pub filter: Option<String>,
    /// Output format: "human", "json", or "compact".
    pub format: String,
    /// Run benchmarks as well.
    pub benchmarks: bool,
}

/// Run tests in a project.
pub fn cmd_test(path: &Path, opts: TestOptions) -> i32 {
    use crate::compiler::test_runner::{
        ExecutionContext, ReportConfig, ReportFormat, TestConfig, discover_tests, format_results,
        run_tests,
    };
    use crate::compiler::typeck::attrs::validate_attributes;

    // Load and parse sources
    let sources = match load_sources(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error loading sources: {}", e);
            return 1;
        }
    };

    if sources.is_empty() {
        eprintln!("No .arth files found");
        return 1;
    }

    // Parse all files
    let mut reporter = Reporter::new();
    let mut asts = Vec::new();
    for sf in &sources {
        let ast = parse_file(sf, &mut reporter);
        asts.push(ast);
    }

    if reporter.has_errors() {
        reporter.drain_to_stderr();
        return 1;
    }

    // Analyze attributes to get TestCollection
    let mut attr_reporter = Reporter::new();
    let files: Vec<_> = sources
        .iter()
        .zip(asts.iter())
        .map(|(sf, ast)| (sf.clone(), ast.clone()))
        .collect();
    let analysis = validate_attributes(&files, &mut attr_reporter);

    // Configure test discovery
    let mut test_config = TestConfig::new().with_benchmarks(opts.benchmarks);

    if let Some(ref filter) = opts.filter {
        test_config = test_config.with_filter(filter);
    }

    // Discover tests
    let tests = discover_tests(&analysis.tests, &test_config);

    if tests.is_empty() {
        println!("No tests found");
        return 0;
    }

    println!("Found {} tests", tests.len());

    // Compile the project to get an executable program
    let exec_ctx = match super::compile::compile_project_to_program(path) {
        Ok(cr) => Some(ExecutionContext {
            program: cr.program,
            func_offsets: cr.func_offsets,
        }),
        Err(e) => {
            eprintln!("warning: compilation failed, tests will be skipped: {}", e);
            None
        }
    };

    // Run tests with the compiled program
    let (results, summary) = run_tests(&tests, &test_config, exec_ctx.as_ref());

    // Configure report
    let report_format = match opts.format.as_str() {
        "json" => ReportFormat::Json,
        "compact" => ReportFormat::Compact,
        _ => ReportFormat::Human,
    };

    let report_config = ReportConfig {
        format: report_format,
        color: true,
        show_passed: true,
        show_output: true,
    };

    // Generate and print report
    let report = format_results(&results, &summary, &report_config);
    print!("{}", report.output);

    if summary.all_passed() { 0 } else { 1 }
}
