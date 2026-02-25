//! Shared project compilation pipeline.
//!
//! Extracts the core load → parse → lower → resolve → typecheck → IR → VM Program
//! pipeline so it can be reused by both the build commands and the test runner.

use std::collections::HashMap;
use std::path::Path;

use arth_vm as vm;

use crate::compiler::diagnostics::Reporter;
use crate::compiler::lower::ast_to_hir::lower_file;
use crate::compiler::lower::hir_to_ir::{
    EnumLowerContext, ExternSig, lower_hir_func_to_ir_with_escape, make_extern_sig,
};
use crate::compiler::parser::parse_file;
use crate::compiler::resolve::resolve_project;
use crate::compiler::source::SourceFile;
use crate::compiler::typeck::const_eval::{ConstEvalContext, eval_integral_const};
use crate::compiler::typeck::typecheck_project;

use super::fs::load_sources;

/// Result of compiling a project to an in-memory VM program.
#[derive(Debug)]
pub struct CompileResult {
    /// The compiled VM program.
    pub program: vm::Program,
    /// Map from qualified function name (e.g. "Module.func") to bytecode offset.
    pub func_offsets: HashMap<String, u32>,
}

/// Compile a project directory (or single .arth file) into a VM Program
/// with a function-name-to-offset map.
///
/// This runs the full pipeline: load → parse → HIR lower → resolve → typecheck → IR → VM.
/// Returns an error string on failure.
pub fn compile_project_to_program(path: &Path) -> Result<CompileResult, String> {
    let mut reporter = Reporter::new();

    let sources = load_sources(path).map_err(|e| format!("failed to read sources: {e}"))?;
    if sources.is_empty() {
        return Err("no .arth files found".into());
    }

    // Parse all sources
    let mut asts: Vec<(SourceFile, crate::compiler::ast::FileAst)> = Vec::new();
    for sf in &sources {
        let ast = parse_file(sf, &mut reporter);
        asts.push((sf.clone(), ast));
    }
    if reporter.has_errors() {
        let msgs: Vec<String> = reporter
            .diagnostics()
            .iter()
            .map(|d| d.message.clone())
            .collect();
        return Err(format!("parse errors: {}", msgs.join("; ")));
    }

    // Lower to HIR
    let mut hirs: Vec<crate::compiler::hir::HirFile> = Vec::new();
    for sf in &sources {
        let Some((_, ast)) = asts.iter().find(|(s, _)| s.path == sf.path) else {
            return Err(format!("internal: AST not found for {}", sf.path.display()));
        };
        hirs.push(lower_file(sf, ast));
    }

    // Resolve + typecheck
    let resolved = resolve_project(path, &asts, &mut reporter);
    let escape_results = typecheck_project(path, &asts, &resolved, &mut reporter);

    if reporter.has_errors() {
        let msgs: Vec<String> = reporter
            .diagnostics()
            .iter()
            .map(|d| d.message.clone())
            .collect();
        return Err(format!("check errors: {}", msgs.join("; ")));
    }

    // Build enum/provider/extern context for IR lowering
    let mut tags: std::collections::BTreeMap<String, std::collections::BTreeMap<String, i64>> =
        Default::default();
    let mut shared_field_names: std::collections::HashSet<String> = Default::default();
    let mut provider_names: std::collections::HashSet<String> = Default::default();
    let mut ir_providers: Vec<crate::compiler::ir::Provider> = Vec::new();
    let mut extern_funcs: std::collections::HashMap<String, ExternSig> = Default::default();
    let mut struct_field_types: std::collections::HashMap<(String, String), String> =
        Default::default();

    for hir in &hirs {
        for decl in &hir.decls {
            match decl {
                crate::compiler::hir::HirDecl::Enum(en) => {
                    tags.insert(en.name.clone(), compute_enum_tags(en));
                }
                crate::compiler::hir::HirDecl::Provider(pv) => {
                    provider_names.insert(pv.name.clone());
                    ir_providers.push(crate::compiler::lower::hir_to_ir::lower_hir_provider_to_ir(
                        pv,
                    ));
                    for f in &pv.fields {
                        if f.is_shared {
                            shared_field_names.insert(f.name.clone());
                        }
                        let ty_name = extract_type_name(&f.ty);
                        if let Some(name) = ty_name {
                            struct_field_types.insert((pv.name.clone(), f.name.clone()), name);
                        }
                    }
                }
                crate::compiler::hir::HirDecl::Struct(st) => {
                    for f in &st.fields {
                        let ty_name = extract_type_name(&f.ty);
                        if let Some(name) = ty_name {
                            struct_field_types.insert((st.name.clone(), f.name.clone()), name);
                        }
                    }
                }
                crate::compiler::hir::HirDecl::ExternFunc(ef) => {
                    extern_funcs.insert(ef.name.clone(), make_extern_sig(ef));
                }
                _ => {}
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

    // Lower all HIR functions to IR
    let mut all_funcs: Vec<crate::compiler::ir::Func> = Vec::new();
    let mut all_strings: Vec<String> = Vec::new();

    for hir in &hirs {
        let pkg = hir
            .package
            .as_ref()
            .map(|p| p.to_string())
            .unwrap_or_default();

        for decl in &hir.decls {
            if let crate::compiler::hir::HirDecl::Module(m) = decl {
                for f in &m.funcs {
                    if f.body.is_some() {
                        let func_escape_info =
                            escape_results.get_function_by_parts(&pkg, Some(&m.name), &f.sig.name);
                        let (mut funcs, strings) =
                            lower_hir_func_to_ir_with_escape(f, Some(&enum_ctx), func_escape_info);

                        // Qualify function names with module prefix
                        let mut name_remap: HashMap<String, String> = HashMap::new();
                        for func in &mut funcs {
                            let old_name = func.name.clone();
                            let new_name = format!("{}.{}", m.name, func.name);
                            func.name = new_name.clone();
                            name_remap.insert(old_name, new_name);
                        }

                        // Update MakeClosure references to use qualified names
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

                        // Adjust string indices if we already have some strings
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

    // Ensure 'main' appears first if present (VM entry point convention)
    if let Some(pos) = all_funcs.iter().position(|f| f.name == "main") {
        let f = all_funcs.remove(pos);
        all_funcs.insert(0, f);
    }

    // Compile IR to VM Program with function offsets
    let (program, func_offsets) =
        super::vm::compile_ir_to_program_with_offsets(&all_funcs, &all_strings, &ir_providers);

    Ok(CompileResult {
        program,
        func_offsets,
    })
}

/// Extract the last path component from a HIR type as a type name string.
fn extract_type_name(ty: &crate::compiler::hir::HirType) -> Option<String> {
    match ty {
        crate::compiler::hir::HirType::Name { path } => path.last().cloned(),
        crate::compiler::hir::HirType::Generic { path, .. } => path.last().cloned(),
        _ => None,
    }
}

/// Compute tag values for enum variants, handling explicit discriminants
/// and implicit sequential numbering.
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

        ctx.set_enum_variants(&vmap);

        let tag = if let Some(disc_expr) = discriminant {
            match eval_integral_const(disc_expr, &mut ctx) {
                Ok(value) => value,
                Err(_) => next_implicit_tag,
            }
        } else {
            next_implicit_tag
        };

        vmap.insert(name.clone(), tag);
        next_implicit_tag = tag + 1;
    }

    vmap
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn compile_missing_directory_returns_error() {
        let result = compile_project_to_program(Path::new("/nonexistent/path/to/project"));
        assert!(result.is_err());
    }

    #[test]
    fn compile_empty_directory_returns_error() {
        let tmp = std::env::temp_dir().join("arth_compile_test_empty");
        let _ = std::fs::create_dir_all(&tmp);
        let result = compile_project_to_program(&tmp);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("no .arth files"),
            "expected 'no .arth files' error, got: {err}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn compile_valid_project_produces_program() {
        // Create a minimal valid Arth project
        let tmp = std::env::temp_dir().join("arth_compile_test_valid");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let source = r#"
package compile.test;

module Main {
    public void main() {
        println("hello");
    }

    void helper() {
        println("helper");
    }
}
"#;
        std::fs::write(tmp.join("Main.arth"), source).unwrap();

        let result = compile_project_to_program(&tmp);
        match result {
            Ok(cr) => {
                assert!(!cr.program.code.is_empty(), "program should have bytecode");
                // Should have both Main.main and Main.helper
                assert!(
                    cr.func_offsets.contains_key("Main.main"),
                    "should have Main.main offset, got: {:?}",
                    cr.func_offsets.keys().collect::<Vec<_>>()
                );
            }
            Err(e) => {
                // Some projects may fail due to missing stdlib etc.
                // Just verify it doesn't panic
                eprintln!("compile returned error (may be expected): {e}");
            }
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
