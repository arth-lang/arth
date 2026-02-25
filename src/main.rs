#![allow(dead_code)]
// Suppress common clippy warnings in compiler code
#![allow(clippy::collapsible_if)]
#![allow(clippy::type_complexity)]
#![allow(clippy::redundant_closure)]
#![allow(clippy::manual_strip)]
#![allow(clippy::derivable_impls)]
#![allow(clippy::len_zero)]
#![allow(clippy::needless_return)]
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::iter_kv_map)]
#![allow(clippy::match_ref_pats)]
#![allow(clippy::get_first)]
#![allow(clippy::clone_on_copy)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::explicit_counter_loop)]
#![allow(clippy::unwrap_or_default)]
#![allow(unused_imports)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::len_without_is_empty)]
#![allow(clippy::redundant_pattern_matching)]
#![allow(clippy::redundant_field_names)]
#![allow(clippy::only_used_in_recursion)]
#![allow(clippy::match_like_matches_macro)]
#![allow(clippy::useless_conversion)]
#![allow(clippy::option_as_ref_deref)]
#![allow(clippy::enum_variant_names)]
#![allow(clippy::unnecessary_get_then_check)]
#![allow(clippy::for_kv_map)]
#![allow(clippy::needless_range_loop)]
#![allow(unused_variables)]

mod compiler;

use std::env;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;

use compiler::driver;

/// Wraps a closure in `catch_unwind` so that any panic inside the compiler
/// produces a clean ICE (Internal Compiler Error) message instead of an
/// ugly stack trace.  Returns the closure's exit code on success, or 101
/// (the `rustc` ICE convention) on panic.
fn run_command<F: FnOnce() -> i32>(f: F) -> i32 {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(code) => code,
        Err(payload) => {
            let message = if let Some(s) = payload.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            eprintln!("error: internal compiler error: {}", message);
            eprintln!("note: this is a bug in the Arth compiler; please report it");
            101
        }
    }
}

fn print_usage() {
    eprintln!(
        "arth - Arth compiler\n\n\
Usage:\n  arth <command> [options] <path>\n\n\
Commands:\n  \
lex        Tokenize sources and print tokens\n  \
parse      Parse sources and print package declarations\n  \
check      Run frontend checks\n  \
build      Build project\n             \
Options: --backend <llvm|cranelift|vm> (default: vm)\n                      \
--opt <0|1|2|3>   Optimization level for native backend (default: 2)\n                      \
--debug   Emit DWARF debug info (native backends only)\n                      \
--lib  Build as distributable library\n                      \
--no-incremental  Disable incremental compilation\n                      \
--clean-cache     Clear cache before building\n                      \
--show-cache-stats  Display cache hit/miss statistics\n  \
run        Run a project (.arth/dir) or a compiled .abc\n             \
Options: --backend <llvm|cranelift|vm> (default: vm)\n                      \
--opt <0|1|2|3>   Optimization level for native backend (default: 2)\n                      \
--debug   Emit DWARF debug info (native backends only)\n\
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
- <path> may be a .arth file or a directory containing .arth files.\n  \
- For 'run', <path> may be a project directory, a single .arth file, or a .abc file.\n  \
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

fn main() {
    let mut args = env::args().skip(1);
    let cmd = args.next();
    match cmd.as_deref() {
        Some("lex") => {
            let path = expect_path(args.next());
            let code = run_command(|| driver::cmd_lex(&path));
            std::process::exit(code);
        }
        Some("parse") => {
            let path = expect_path(args.next());
            let code = run_command(|| driver::cmd_parse(&path));
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
            let code = run_command(|| driver::cmd_check_with_dump(&path, dump_hir, dump_ir));
            std::process::exit(code);
        }
        Some("build") => {
            // Accept optional flags before path.
            let mut backend = driver::Backend::Vm;
            let mut lib_mode = false;
            let mut opt_level: u8 = 2;
            let mut incremental = driver::incremental::is_incremental_enabled();
            let mut clean_cache = false;
            let mut show_cache_stats = false;
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
                } else if arg == "--lib" {
                    lib_mode = true;
                } else if arg == "--no-incremental" {
                    incremental = false;
                } else if arg == "--incremental" {
                    incremental = true;
                } else if arg == "--clean-cache" {
                    clean_cache = true;
                } else if arg == "--show-cache-stats" {
                    show_cache_stats = true;
                } else if arg == "--debug" {
                    debug = true;
                } else {
                    path_arg = Some(arg);
                    // Remaining args (if any) are ignored for now.
                    break;
                }
            }
            let path = expect_path(path_arg);
            let code = run_command(|| {
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
            });
            std::process::exit(code);
        }
        Some("run") => {
            // Accept optional --backend, --opt, and --debug before path.
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
            let code =
                run_command(|| driver::cmd_run_with_backend(&path, backend, opt_level, debug));
            std::process::exit(code);
        }
        Some("emit-llvm") => {
            // Demo: emit a small LLVM IR text module either to stdout or a file path.
            let out = args.next().map(PathBuf::from);
            let code = run_command(|| driver::cmd_emit_llvm_demo(out.as_deref()));
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
            let code = run_command(|| driver::cmd_fmt(&path, opts));
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
            let code = run_command(|| driver::cmd_lint(&path, opts));
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
            let code = run_command(|| driver::cmd_test(&path, opts));
            std::process::exit(code);
        }
        Some("cache") => {
            // Cache management subcommands
            let subcmd = args.next();
            let code = match subcmd.as_deref() {
                Some("status") => {
                    let path = args
                        .next()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("."));
                    run_command(|| cmd_cache_status(&path))
                }
                Some("clean") => {
                    let path = args
                        .next()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("."));
                    run_command(|| cmd_cache_clean(&path))
                }
                Some("gc") => {
                    let path = args
                        .next()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("."));
                    run_command(|| cmd_cache_gc(&path))
                }
                Some("path") => {
                    let path = args
                        .next()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("."));
                    run_command(|| cmd_cache_path(&path))
                }
                _ => {
                    eprintln!("Usage: arth cache <status|clean|gc|path> [path]");
                    2
                }
            };
            std::process::exit(code);
        }
        _ => {
            print_usage();
            std::process::exit(2);
        }
    }
}

fn cmd_cache_status(path: &std::path::Path) -> i32 {
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
            0
        }
        Err(e) => {
            println!("Cache not found or not accessible: {}", e);
            0
        }
    }
}

fn cmd_cache_clean(path: &std::path::Path) -> i32 {
    use driver::incremental::{CompileInputs, ProjectCache};

    let inputs = CompileInputs::vm();
    match ProjectCache::open(path, &inputs) {
        Ok(mut cache) => {
            let old_stats = cache.stats();
            if let Err(e) = cache.clear() {
                eprintln!("Error clearing cache: {}", e);
                return 1;
            }
            println!(
                "Cleared {} entries ({} total)",
                old_stats.entries, old_stats
            );
            0
        }
        Err(e) => {
            eprintln!("Error opening cache: {}", e);
            1
        }
    }
}

fn cmd_cache_gc(path: &std::path::Path) -> i32 {
    use driver::incremental::{CompileInputs, DEFAULT_MAX_AGE_DAYS, ProjectCache};

    let inputs = CompileInputs::vm();
    match ProjectCache::open(path, &inputs) {
        Ok(mut cache) => match cache.gc(DEFAULT_MAX_AGE_DAYS) {
            Ok(stats) => {
                println!(
                    "Garbage collection complete: {} entries removed, {} bytes freed, {} entries kept",
                    stats.entries_removed, stats.bytes_freed, stats.entries_kept
                );
                0
            }
            Err(e) => {
                eprintln!("Error during garbage collection: {}", e);
                1
            }
        },
        Err(e) => {
            eprintln!("Error opening cache: {}", e);
            1
        }
    }
}

fn cmd_cache_path(path: &std::path::Path) -> i32 {
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
    0
}
