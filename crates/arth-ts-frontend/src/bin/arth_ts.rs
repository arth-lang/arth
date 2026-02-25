//! DEPRECATED: Use `arth` CLI instead.
//!
//! The `arth-ts` binary is deprecated. The unified `arth` CLI now handles both
//! .arth and .ts files automatically based on file extension:
//!
//! - `arth check foo.ts` - Check TypeScript files
//! - `arth build foo.ts` - Build TypeScript to bytecode
//! - `arth run foo.ts` - Compile and run TypeScript
//! - `arth run foo.tsguest.json` - Run a packaged TS guest module
//!
//! This binary will be removed in a future release.

use std::env;
use std::path::{Path, PathBuf};

use arth::compiler::driver::compile_ir_to_program;
use arth::compiler::hir::{HirDecl, HirEnumVariant, HirFile, dump_hir};
use arth::compiler::lower::hir_to_ir::{EnumLowerContext, lower_hir_func_to_ir_demo};
use arth_ts_frontend::{
    TsLoweringError, TsLoweringOptions, load_ts_guest_package, lower_ts_project_to_hir,
    write_ts_guest_package,
};
use arth_vm::run_program;

fn print_usage() {
    eprintln!(
        "arth-ts - Arth TS subset CLI (prototype)\n\n\
Usage:\n  arth-ts check <file.ts> [--dump-hir]\n  arth-ts run <file.ts>\n  arth-ts package <file.ts> [--out-dir <dir>]\n  arth-ts run-pkg <module.tsguest.json>\n\n\
Notes:\n  - Supports multi-file TS projects via relative imports (./foo, ../bar).\n  - The entry file must export a function `main(...)` as the entry point.\n  - Host imports use arth:* namespace (arth:log, arth:time, etc.)."
    );
}

fn expect_path(arg: Option<String>) -> PathBuf {
    match arg {
        Some(p) => PathBuf::from(p),
        None => {
            print_usage();
            std::process::exit(2);
        }
    }
}

fn lower_ts_project(path: &Path) -> Result<Vec<HirFile>, TsLoweringError> {
    lower_ts_project_to_hir(path, TsLoweringOptions { package: None })
}

fn build_enum_ctx(hirs: &[HirFile]) -> EnumLowerContext {
    let mut tags = std::collections::BTreeMap::new();
    let mut shared_field_names = std::collections::HashSet::new();

    for hir in hirs {
        for decl in &hir.decls {
            match decl {
                HirDecl::Enum(en) => {
                    let mut vmap = std::collections::BTreeMap::new();
                    for (i, v) in en.variants.iter().enumerate() {
                        match v {
                            HirEnumVariant::Unit { name, .. } => {
                                vmap.insert(name.clone(), i as i64);
                            }
                            HirEnumVariant::Tuple { name, .. } => {
                                vmap.insert(name.clone(), i as i64);
                            }
                        }
                    }
                    tags.insert(en.name.clone(), vmap);
                }
                HirDecl::Provider(pv) => {
                    for field in &pv.fields {
                        if field.is_shared {
                            shared_field_names.insert(field.name.clone());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    EnumLowerContext {
        tags,
        shared_field_names,
        type_aliases: Default::default(),
        types_needing_drop: Default::default(),
        json_codec_structs: Default::default(),
        provider_names: Default::default(),
        extern_funcs: Default::default(),
        struct_field_types: Default::default(),
    }
}

fn lower_hir_to_ir(hirs: &[HirFile]) -> (Vec<arth::compiler::ir::Func>, Vec<String>) {
    let enum_ctx = build_enum_ctx(hirs);
    let mut all_funcs = Vec::new();
    let mut all_strings = Vec::new();

    for hir in hirs {
        for decl in &hir.decls {
            if let HirDecl::Module(m) = decl {
                for func in &m.funcs {
                    if func.body.is_some() {
                        let (mut funcs, strings) = lower_hir_func_to_ir_demo(func, Some(&enum_ctx));
                        let base = all_strings.len() as u32;
                        if base > 0 {
                            // Adjust string indices in funcs
                            for irf in &mut funcs {
                                for block in &mut irf.blocks {
                                    for inst in &mut block.insts {
                                        if let arth::compiler::ir::InstKind::ConstStr(ix) =
                                            &mut inst.kind
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

    if let Some(pos) = all_funcs.iter().position(|f| f.name == "main") {
        let f = all_funcs.remove(pos);
        all_funcs.insert(0, f);
    }

    (all_funcs, all_strings)
}

fn cmd_check_ts(path: &Path, dump_hir_flag: bool) -> i32 {
    match lower_ts_project(path) {
        Ok(hirs) => {
            if dump_hir_flag {
                for hir in &hirs {
                    let txt = dump_hir(hir);
                    println!("{txt}");
                }
            }
            println!("ts-check: OK ({}, {} file(s))", path.display(), hirs.len());
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn cmd_run_ts(path: &Path) -> i32 {
    match lower_ts_project(path) {
        Ok(hirs) => {
            let (funcs, strings) = lower_hir_to_ir(&hirs);
            if funcs.is_empty() {
                eprintln!(
                    "error: no functions with bodies were lowered from {}; expected an exported `main`",
                    path.display()
                );
                return 1;
            }
            let prog = compile_ir_to_program(&funcs, &strings, &[]);
            let code = run_program(&prog);
            println!("vm: exit code {}", code);
            if code == 0 { 0 } else { 1 }
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn cmd_package_ts(path: &Path, out_dir: &Path) -> i32 {
    match lower_ts_project(path) {
        Ok(hirs) => {
            if hirs.is_empty() {
                eprintln!("error: no files were loaded from {}", path.display());
                return 1;
            }
            let (funcs, strings) = lower_hir_to_ir(&hirs);
            if funcs.is_empty() {
                eprintln!(
                    "error: no functions with bodies were lowered from {}; expected an exported `main`",
                    path.display()
                );
                return 1;
            }
            let prog = compile_ir_to_program(&funcs, &strings, &[]);
            let base_name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("module");
            // Use the entry file (first loaded) for the manifest
            let entry_hir = &hirs[0];
            match write_ts_guest_package(entry_hir, &prog, out_dir, base_name) {
                Ok((manifest_path, bytecode_path)) => {
                    println!(
                        "ts-package: wrote manifest {} and bytecode {} ({} file(s))",
                        manifest_path.display(),
                        bytecode_path.display(),
                        hirs.len()
                    );
                    0
                }
                Err(e) => {
                    eprintln!("error: failed to write TS guest package: {}", e);
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn cmd_run_package(manifest_path: &Path) -> i32 {
    match load_ts_guest_package(manifest_path) {
        Ok((_manifest, program)) => {
            let code = run_program(&program);
            println!("vm: exit code {}", code);
            if code == 0 { 0 } else { 1 }
        }
        Err(e) => {
            eprintln!("error: failed to load TS guest package: {}", e);
            1
        }
    }
}

fn main() {
    // Print deprecation warning
    eprintln!("WARNING: arth-ts is deprecated. Use 'arth' instead:");
    eprintln!("  arth check foo.ts    (instead of arth-ts check foo.ts)");
    eprintln!("  arth build foo.ts    (instead of arth-ts package foo.ts)");
    eprintln!("  arth run foo.ts      (instead of arth-ts run foo.ts)");
    eprintln!();

    let mut args = env::args().skip(1);
    let cmd = args.next();
    match cmd.as_deref() {
        Some("check") => {
            let mut dump_hir_flag = false;
            let mut path_arg: Option<String> = None;
            for arg in args {
                if arg == "--dump-hir" {
                    dump_hir_flag = true;
                } else {
                    path_arg = Some(arg);
                    break;
                }
            }
            let path = expect_path(path_arg);
            let code = cmd_check_ts(&path, dump_hir_flag);
            std::process::exit(code);
        }
        Some("run") => {
            let path = expect_path(args.next());
            let code = cmd_run_ts(&path);
            std::process::exit(code);
        }
        Some("package") => {
            let mut out_dir: Option<String> = None;
            let mut path_arg: Option<String> = None;
            while let Some(arg) = args.next() {
                if arg == "--out-dir" {
                    out_dir = args.next();
                } else {
                    path_arg = Some(arg);
                    break;
                }
            }
            let path = expect_path(path_arg);
            let out = out_dir
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("target/ts-guest"));
            let code = cmd_package_ts(&path, &out);
            std::process::exit(code);
        }
        Some("run-pkg") => {
            let path = expect_path(args.next());
            let code = cmd_run_package(&path);
            std::process::exit(code);
        }
        _ => {
            print_usage();
            std::process::exit(2);
        }
    }
}
