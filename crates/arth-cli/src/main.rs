//! Unified Arth CLI - supports both .arth and .ts files.

use std::env;
use std::path::PathBuf;

use arth::compiler::driver;
use arth::compiler::hir::dump_hir;

fn print_usage() {
    eprintln!(
        "arth - Arth compiler\n\n\
Usage:\n  arth <command> [options] <path>\n\n\
Commands:\n  \
lex        Tokenize sources and print tokens\n  \
parse      Parse sources and print package declarations\n  \
check      Run frontend checks (supports .arth and .ts files)\n  \
build      Build project (supports .arth and .ts files)\n             \
Options: --backend <llvm|cranelift|vm> (default: vm)\n                      \
--lib  Build as distributable library\n                      \
--out-dir <dir>  Output directory (for .ts guest packages)\n                      \
--no-incremental  Disable incremental compilation\n                      \
--clean-cache     Clear cache before building\n                      \
--show-cache-stats  Display cache hit/miss statistics\n  \
run        Run a project or compiled bytecode\n             \
Options: --backend <llvm|cranelift|vm> (default: vm)\n             \
Supports: .arth files, .ts files, .abc bytecode, .tsguest.json packages\n  \
fmt        Format source files\n             \
Options: --check  Check formatting without modifying files\n  \
lint       Run linter checks\n             \
Options: --json  Output results in JSON format\n                      \
-W, --warnings-as-errors  Treat warnings as errors\n  \
test       Run tests\n             \
Options: --filter <name>  Only run tests matching name\n                      \
--json  Output results in JSON format\n                      \
--compact  Output results in compact format\n                      \
--bench  Also run benchmarks\n  \
cache      Cache management commands\n             \
Subcommands: status, clean, gc, path\n\n\
Notes:\n  \
- <path> may be a .arth, .ts file, or a directory.\n  \
- .ts files are compiled as guest modules with capability restrictions.\n  \
- For 'run', <path> may also be a .abc file or .tsguest.json package.\n  \
- Native backend (`--backend llvm`) requires host LLVM/clang tooling and emits an unsandboxed binary.\n  \
- Dependencies from ~/.arth/libs are automatically loaded based on arth.toml.\n\n\
Environment Variables:\n  \
ARTH_INCREMENTAL=0|1    Enable/disable incremental compilation\n  \
ARTH_CACHE_VERBOSE=1    Print cache decisions during compilation\n  \
ARTH_CACHE_DIR          Override cache directory location"
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

/// Detected source type based on file extension or directory contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceType {
    Arth,
    TypeScript,
    Bytecode,
    TsGuestPackage,
}

fn detect_source_type(path: &std::path::Path) -> SourceType {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        match ext {
            "ts" | "tsx" => return SourceType::TypeScript,
            "arth" => return SourceType::Arth,
            "abc" => return SourceType::Bytecode,
            "json" if path.to_string_lossy().ends_with(".tsguest.json") => {
                return SourceType::TsGuestPackage;
            }
            _ => {}
        }
    }

    // For directories, check what files they contain
    if path.is_dir() {
        // Prefer .ts if any .ts files exist, otherwise .arth
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                    if ext == "ts" || ext == "tsx" {
                        return SourceType::TypeScript;
                    }
                }
            }
        }
    }

    SourceType::Arth
}

// --- TypeScript command handlers ---

fn cmd_check_ts(path: &std::path::Path, dump_hir_flag: bool) -> i32 {
    use arth_ts_frontend::{TsLoweringOptions, lower_ts_project_to_hir};

    match lower_ts_project_to_hir(path, TsLoweringOptions::default()) {
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

fn cmd_build_ts(path: &std::path::Path, out_dir: Option<&std::path::Path>) -> i32 {
    use arth_ts_frontend::compile_ts_file;

    match compile_ts_file(path) {
        Ok(mut result) => {
            let out = out_dir.unwrap_or_else(|| std::path::Path::new("target/ts-guest"));
            std::fs::create_dir_all(out).ok();

            let base_name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("module");

            // Use consistent naming for bytecode and manifest
            let bytecode_name = format!("{base_name}.abc");
            let bytecode_path = out.join(&bytecode_name);
            let manifest_path = out.join(format!("{base_name}.tsguest.json"));

            // Update manifest to reference the correct bytecode file
            result.manifest.bytecode = bytecode_name;

            if let Err(e) = std::fs::write(&bytecode_path, &result.bytecode) {
                eprintln!("error: failed to write bytecode: {e}");
                return 1;
            }

            let manifest_json = match serde_json::to_string_pretty(&result.manifest) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("error: failed to serialize manifest: {e}");
                    return 1;
                }
            };
            if let Err(e) = std::fs::write(&manifest_path, manifest_json) {
                eprintln!("error: failed to write manifest: {e}");
                return 1;
            }

            println!(
                "ts-build: wrote {} and {}",
                manifest_path.display(),
                bytecode_path.display()
            );
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn cmd_run_ts(path: &std::path::Path) -> i32 {
    use arth_ts_frontend::compile_ts_file;

    match compile_ts_file(path) {
        Ok(result) => {
            let code = arth_vm::run_program(&result.program);
            println!("vm: exit code {code}");
            if code == 0 { 0 } else { 1 }
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn cmd_run_ts_package(manifest_path: &std::path::Path) -> i32 {
    use arth_ts_frontend::load_ts_guest_package;

    match load_ts_guest_package(manifest_path) {
        Ok((_manifest, program)) => {
            let code = arth_vm::run_program(&program);
            println!("vm: exit code {code}");
            if code == 0 { 0 } else { 1 }
        }
        Err(e) => {
            eprintln!("error: failed to load TS guest package: {e}");
            1
        }
    }
}

// --- Cache management ---

fn cmd_cache_status(path: &std::path::Path) {
    use driver::incremental::{CompileInputs, ProjectCache};

    let inputs = CompileInputs::vm();
    match ProjectCache::open(path, &inputs) {
        Ok(cache) => {
            let stats = cache.stats();
            println!("Arth Cache Status");
            println!("  Location: {}", cache.root().display());
            println!(
                "  Config: {}...",
                &stats.config_fingerprint[..16.min(stats.config_fingerprint.len())]
            );
            println!("  Entries: {}", stats.entries);
            println!("  Size: {}", stats);
        }
        Err(e) => {
            println!("Cache not found or not accessible: {}", e);
        }
    }
}

fn cmd_cache_clean(path: &std::path::Path) {
    use driver::incremental::{CompileInputs, ProjectCache};

    let inputs = CompileInputs::vm();
    match ProjectCache::open(path, &inputs) {
        Ok(mut cache) => {
            let old_stats = cache.stats();
            if let Err(e) = cache.clear() {
                eprintln!("Error clearing cache: {}", e);
                std::process::exit(1);
            }
            println!(
                "Cleared {} entries ({} total)",
                old_stats.entries, old_stats
            );
        }
        Err(e) => {
            eprintln!("Error opening cache: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_cache_gc(path: &std::path::Path) {
    use driver::incremental::{CompileInputs, DEFAULT_MAX_AGE_DAYS, ProjectCache};

    let inputs = CompileInputs::vm();
    match ProjectCache::open(path, &inputs) {
        Ok(mut cache) => match cache.gc(DEFAULT_MAX_AGE_DAYS) {
            Ok(stats) => {
                println!(
                    "Garbage collection complete: {} entries removed, {} bytes freed, {} entries kept",
                    stats.entries_removed, stats.bytes_freed, stats.entries_kept
                );
            }
            Err(e) => {
                eprintln!("Error during garbage collection: {}", e);
                std::process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("Error opening cache: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_cache_path(path: &std::path::Path) {
    use driver::incremental::{CompileInputs, ProjectCache};

    let inputs = CompileInputs::vm();
    match ProjectCache::open(path, &inputs) {
        Ok(cache) => {
            println!("{}", cache.root().display());
        }
        Err(_) => {
            // Print expected path even if cache doesn't exist
            let cache_path = path.join("target").join(".arth-cache").join("vm");
            println!("{}", cache_path.display());
        }
    }
}

fn main() {
    let mut args = env::args().skip(1);
    let cmd = args.next();
    match cmd.as_deref() {
        Some("lex") => {
            let path = expect_path(args.next());
            let code = driver::cmd_lex(&path);
            std::process::exit(code);
        }
        Some("parse") => {
            let path = expect_path(args.next());
            let code = driver::cmd_parse(&path);
            std::process::exit(code);
        }
        Some("check") => {
            // Optional flags: --dump-hir, --dump-ir to print lowered HIR/IR
            let mut dump_hir = false;
            let mut dump_ir = false;
            let mut path_arg: Option<String> = None;
            for arg in args.by_ref() {
                if arg == "--dump-hir" {
                    dump_hir = true;
                } else if arg == "--dump-ir" {
                    dump_ir = true;
                } else {
                    path_arg = Some(arg);
                    break;
                }
            }
            let path = expect_path(path_arg);
            let code = match detect_source_type(&path) {
                SourceType::TypeScript => cmd_check_ts(&path, dump_hir),
                _ => driver::cmd_check_with_dump(&path, dump_hir, dump_ir),
            };
            std::process::exit(code);
        }
        Some("build") => {
            // Accept optional flags before path.
            let mut backend = driver::Backend::Vm;
            let mut opt_level: u8 = 2;
            let mut debug = false;
            let mut lib_mode = false;
            let mut incremental = driver::incremental::is_incremental_enabled();
            let mut clean_cache = false;
            let mut show_cache_stats = false;
            let mut out_dir: Option<PathBuf> = None;
            let mut path_arg: Option<String> = None;
            let iter = args.by_ref();
            while let Some(arg) = iter.next() {
                if arg == "--backend" {
                    let Some(b) = iter.next() else {
                        eprintln!("--backend requires a value (llvm|cranelift|vm)");
                        std::process::exit(2);
                    };
                    match driver::Backend::parse(&b) {
                        Some(bk) => backend = bk,
                        None => {
                            eprintln!("unknown backend: {} (use llvm|cranelift|vm)", b);
                            std::process::exit(2);
                        }
                    }
                } else if arg == "--opt" {
                    let Some(val) = iter.next() else {
                        eprintln!("--opt requires a value (0, 1, 2, or 3)");
                        std::process::exit(2);
                    };
                    match val.parse::<u8>() {
                        Ok(n) if n <= 3 => opt_level = n,
                        _ => {
                            eprintln!("invalid optimization level: {} (use 0, 1, 2, or 3)", val);
                            std::process::exit(2);
                        }
                    }
                } else if arg == "--debug" {
                    debug = true;
                } else if arg == "--lib" {
                    lib_mode = true;
                } else if arg == "--out-dir" {
                    out_dir = iter.next().map(PathBuf::from);
                } else if arg == "--no-incremental" {
                    incremental = false;
                } else if arg == "--incremental" {
                    incremental = true;
                } else if arg == "--clean-cache" {
                    clean_cache = true;
                } else if arg == "--show-cache-stats" {
                    show_cache_stats = true;
                } else {
                    path_arg = Some(arg);
                    // Remaining args (if any) are ignored for now.
                    break;
                }
            }
            let path = expect_path(path_arg);
            let code = match detect_source_type(&path) {
                SourceType::TypeScript => cmd_build_ts(&path, out_dir.as_deref()),
                _ => {
                    if lib_mode {
                        driver::cmd_build_lib(&path)
                    } else {
                        let opts = driver::BuildOptions {
                            backend,
                            opt_level,
                            incremental,
                            clean_cache,
                            show_cache_stats,
                            debug,
                        };
                        driver::cmd_build_with_options(&path, opts)
                    }
                }
            };
            std::process::exit(code);
        }
        Some("run") => {
            // Dispatch based on source type:
            // - .arth files/dirs: build+run via Arth compiler
            // - .ts files/dirs: compile+run via TS frontend
            // - .abc files: run directly via VM
            // - .tsguest.json: load package and run
            let mut backend = driver::Backend::Vm;
            let mut opt_level: u8 = 2;
            let mut debug = false;
            let mut path_arg: Option<String> = None;
            let iter = args.by_ref();
            while let Some(arg) = iter.next() {
                if arg == "--backend" {
                    let Some(b) = iter.next() else {
                        eprintln!("--backend requires a value (llvm|cranelift|vm)");
                        std::process::exit(2);
                    };
                    match driver::Backend::parse(&b) {
                        Some(bk) => backend = bk,
                        None => {
                            eprintln!("unknown backend: {} (use llvm|cranelift|vm)", b);
                            std::process::exit(2);
                        }
                    }
                } else if arg == "--opt" {
                    let Some(val) = iter.next() else {
                        eprintln!("--opt requires a value (0, 1, 2, or 3)");
                        std::process::exit(2);
                    };
                    match val.parse::<u8>() {
                        Ok(n) if n <= 3 => opt_level = n,
                        _ => {
                            eprintln!("invalid optimization level: {} (use 0, 1, 2, or 3)", val);
                            std::process::exit(2);
                        }
                    }
                } else if arg == "--debug" {
                    debug = true;
                } else {
                    path_arg = Some(arg);
                    break;
                }
            }

            let path = expect_path(path_arg);
            let code = match detect_source_type(&path) {
                SourceType::TypeScript => {
                    if !matches!(backend, driver::Backend::Vm) {
                        eprintln!("error: --backend is only supported for .arth sources");
                        2
                    } else {
                        cmd_run_ts(&path)
                    }
                }
                SourceType::TsGuestPackage => {
                    if !matches!(backend, driver::Backend::Vm) {
                        eprintln!("error: --backend is only supported for .arth sources");
                        2
                    } else {
                        cmd_run_ts_package(&path)
                    }
                }
                _ => driver::cmd_run_with_backend(&path, backend, opt_level, debug),
            };
            std::process::exit(code);
        }
        Some("emit-llvm") => {
            // Demo: emit a small LLVM IR text module either to stdout or a file path.
            let out = args.next().map(PathBuf::from);
            let code = driver::cmd_emit_llvm_demo(out.as_deref());
            std::process::exit(code);
        }
        Some("fmt") => {
            // Format source files
            let mut check = false;
            let mut path_arg: Option<String> = None;
            for arg in args.by_ref() {
                if arg == "--check" {
                    check = true;
                } else {
                    path_arg = Some(arg);
                    break;
                }
            }
            let path = expect_path(path_arg);
            let opts = driver::FormatOptions {
                check,
                stdin: false,
            };
            let code = driver::cmd_fmt(&path, opts);
            std::process::exit(code);
        }
        Some("lint") => {
            // Run linter checks
            let mut json_format = false;
            let mut warnings_as_errors = false;
            let mut path_arg: Option<String> = None;
            for arg in args.by_ref() {
                if arg == "--json" {
                    json_format = true;
                } else if arg == "-W" || arg == "--warnings-as-errors" {
                    warnings_as_errors = true;
                } else {
                    path_arg = Some(arg);
                    break;
                }
            }
            let path = expect_path(path_arg);
            let format = if json_format {
                "json".to_string()
            } else {
                "human".to_string()
            };
            let opts = driver::LintOptions {
                format,
                warnings_as_errors,
            };
            let code = driver::cmd_lint(&path, opts);
            std::process::exit(code);
        }
        Some("test") => {
            // Run tests
            let mut filter: Option<String> = None;
            let mut format = "human".to_string();
            let mut benchmarks = false;
            let mut path_arg: Option<String> = None;
            let iter = args.by_ref();
            while let Some(arg) = iter.next() {
                if arg == "--filter" {
                    filter = iter.next();
                } else if arg == "--json" {
                    format = "json".to_string();
                } else if arg == "--compact" {
                    format = "compact".to_string();
                } else if arg == "--bench" {
                    benchmarks = true;
                } else {
                    path_arg = Some(arg);
                    break;
                }
            }
            let path = expect_path(path_arg);
            let opts = driver::TestOptions {
                filter,
                format,
                benchmarks,
            };
            let code = driver::cmd_test(&path, opts);
            std::process::exit(code);
        }
        Some("cache") => {
            // Cache management subcommands
            let subcmd = args.next();
            match subcmd.as_deref() {
                Some("status") => {
                    let path = args
                        .next()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("."));
                    cmd_cache_status(&path);
                }
                Some("clean") => {
                    let path = args
                        .next()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("."));
                    cmd_cache_clean(&path);
                }
                Some("gc") => {
                    let path = args
                        .next()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("."));
                    cmd_cache_gc(&path);
                }
                Some("path") => {
                    let path = args
                        .next()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("."));
                    cmd_cache_path(&path);
                }
                _ => {
                    eprintln!("Usage: arth cache <status|clean|gc|path> [path]");
                    std::process::exit(2);
                }
            }
        }
        _ => {
            print_usage();
            std::process::exit(2);
        }
    }
}
