pub mod cache;
mod cli;
pub mod compile;
pub mod config;
pub mod extlib;
mod fs;
pub mod incremental;
pub mod lockfile;
mod vm;

pub use cli::{
    Backend, BuildOptions, FormatOptions, LintOptions, TestOptions, cmd_build_lib,
    cmd_build_with_backend, cmd_build_with_options, cmd_check_with_dump, cmd_emit_llvm_demo,
    cmd_fmt, cmd_lex, cmd_lint, cmd_parse, cmd_run_auto, cmd_run_with_backend, cmd_test,
};
pub use compile::{CompileResult, compile_project_to_program};
pub use config::{
    DependencySpec, DetailedDependency, LockDependency, LockMetadata, LockPackage, Lockfile,
    LockfileError, Manifest, ManifestError, Package, parse_lockfile, parse_manifest,
    serialize_lockfile, write_lockfile,
};
pub use extlib::{
    ExternalPackage, ExternalPackageIndex, ExtlibError, collect_native_lib_paths,
    discover_external_packages, resolve_dependencies,
};
pub use lockfile::{
    LOCKFILE_VERSION, LockfileDiff, LockfileGenError, LockfileOptions, ValidationIssue,
    compute_checksum, diff_lockfiles, generate_and_write_lockfile, generate_lockfile,
    validate_lockfile, verify_checksum,
};
#[allow(unused_imports)]
pub use vm::{compile_ir_to_program, compile_ir_to_program_with_offsets};

// Incremental compilation infrastructure
pub use incremental::{
    CacheError, CacheStats, CompileInputs, CompileSession, DepGraph, DepGraphBuilder, DepNode,
    Fingerprint, FingerprintKind, GcStats, IncrementalPlan, PackageCacheStatus, PackageInterface,
    ProjectCache, RecompileReason, SessionStats, is_cache_verbose, is_incremental_enabled,
};
