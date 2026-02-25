//! Complete TypeScript → Arth bytecode compilation.
//!
//! This module provides the full compilation pipeline from TypeScript source
//! to Arth VM bytecode, combining TS→HIR lowering with HIR→IR→bytecode compilation.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use arth::compiler::driver::compile_ir_to_program_with_offsets;
use arth::compiler::hir::{HirDecl, HirEnumVariant, HirFile};
use arth::compiler::ir;
use arth::compiler::lower::hir_to_ir::{EnumLowerContext, lower_hir_func_to_ir_demo};
use arth_vm::{ExportEntry, Library, Program, encode_library};

use crate::lower::{
    TsLoweringError, TsLoweringOptions, lower_ts_project_to_hir, lower_ts_str_to_hir,
};
use crate::package::{
    TS_GUEST_MANIFEST_VERSION, TS_GUEST_SCHEMA_VERSION, TsGuestExport, TsGuestManifest,
};
use crate::vm_meta::{ControllerRegistry, extract_controller_registry};

/// Result of compiling TypeScript to Arth bytecode.
#[derive(Debug)]
pub struct TsCompileResult {
    /// Compiled Arth VM program
    pub program: Program,
    /// Guest manifest with exports/imports/capabilities
    pub manifest: TsGuestManifest,
    /// Serialized bytecode bytes
    pub bytecode: Vec<u8>,
    /// Controller registry for VM integration
    pub controller_registry: ControllerRegistry,
}

/// Error during TypeScript compilation.
#[derive(Debug)]
pub enum TsCompileError {
    /// Error during TS→HIR lowering
    Lowering(TsLoweringError),
    /// No functions found to compile
    NoFunctions,
    /// IO error
    Io(std::io::Error),
}

impl std::fmt::Display for TsCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TsCompileError::Lowering(e) => write!(f, "{}", e),
            TsCompileError::NoFunctions => write!(f, "no functions found in TypeScript source"),
            TsCompileError::Io(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for TsCompileError {}

impl From<TsLoweringError> for TsCompileError {
    fn from(e: TsLoweringError) -> Self {
        TsCompileError::Lowering(e)
    }
}

impl From<std::io::Error> for TsCompileError {
    fn from(e: std::io::Error) -> Self {
        TsCompileError::Io(e)
    }
}

/// Compile a TypeScript file to Arth bytecode.
///
/// This is the main entry point for compiling TypeScript to Arth VM bytecode.
/// It performs the full pipeline: TS → HIR → IR → bytecode.
///
/// # Arguments
/// * `path` - Path to TypeScript file or directory containing TypeScript files
///
/// # Returns
/// * `TsCompileResult` containing the compiled program, manifest, and serialized bytecode
pub fn compile_ts_file(path: &Path) -> Result<TsCompileResult, TsCompileError> {
    compile_ts_project(path, TsLoweringOptions::default())
}

/// Compile a TypeScript project with custom options.
pub fn compile_ts_project(
    path: &Path,
    opts: TsLoweringOptions,
) -> Result<TsCompileResult, TsCompileError> {
    // Step 1: Lower TypeScript to HIR
    let hirs = lower_ts_project_to_hir(path, opts)?;

    if hirs.is_empty() {
        return Err(TsCompileError::Lowering(TsLoweringError::ParseError(
            format!("no TypeScript files found in {}", path.display()),
        )));
    }

    // Step 2: Compile HIR to bytecode
    compile_hirs_to_bytecode(&hirs)
}

/// Compile TypeScript source string to Arth bytecode.
///
/// # Arguments
/// * `source` - TypeScript source code
/// * `name` - Name for the module (used in manifest)
pub fn compile_ts_string(source: &str, name: &str) -> Result<TsCompileResult, TsCompileError> {
    // Step 1: Lower TypeScript to HIR
    let hir = lower_ts_str_to_hir(source, name, TsLoweringOptions::default())?;

    // Step 2: Compile HIR to bytecode
    compile_hirs_to_bytecode(&[hir])
}

/// Compile HIR files to bytecode.
fn compile_hirs_to_bytecode(hirs: &[HirFile]) -> Result<TsCompileResult, TsCompileError> {
    // Build enum context for lowering
    let enum_ctx = build_enum_context(hirs);

    // Lower HIR to IR
    let mut all_funcs: Vec<ir::Func> = Vec::new();
    let mut all_strings: Vec<String> = Vec::new();

    for hir in hirs {
        for decl in &hir.decls {
            if let HirDecl::Module(m) = decl {
                for func in &m.funcs {
                    if func.body.is_some() {
                        // Debug: Print function being lowered
                        eprintln!(
                            "[TS_COMPILE] Lowering function: {}.{} (body has {} stmts)",
                            m.name,
                            func.sig.name,
                            func.body.as_ref().map(|b| b.stmts.len()).unwrap_or(0)
                        );
                        let (mut funcs, strings) = lower_hir_func_to_ir_demo(func, Some(&enum_ctx));
                        // Debug: Print IR generated
                        for irf in &funcs {
                            let total_insts: usize = irf.blocks.iter().map(|b| b.insts.len()).sum();
                            eprintln!(
                                "[TS_COMPILE]   -> IR func '{}': {} blocks, {} total instructions",
                                irf.name,
                                irf.blocks.len(),
                                total_insts
                            );
                        }

                        // Adjust string indices to account for already-collected strings
                        let base = all_strings.len() as u32;
                        if base > 0 {
                            for irf in &mut funcs {
                                for block in &mut irf.blocks {
                                    for inst in &mut block.insts {
                                        if let ir::InstKind::ConstStr(ix) = &mut inst.kind {
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

    // Ensure main is first (entry point convention)
    if let Some(pos) = all_funcs.iter().position(|f| f.name == "main") {
        let f = all_funcs.remove(pos);
        all_funcs.insert(0, f);
    }

    if all_funcs.is_empty() {
        return Err(TsCompileError::NoFunctions);
    }

    // Debug: Print summary before compilation
    eprintln!(
        "[TS_COMPILE] Compiling {} IR functions to bytecode...",
        all_funcs.len()
    );
    for f in &all_funcs {
        let total_insts: usize = f.blocks.iter().map(|b| b.insts.len()).sum();
        eprintln!(
            "[TS_COMPILE]   - '{}': {} params, {} blocks, {} instructions",
            f.name,
            f.params.len(),
            f.blocks.len(),
            total_insts
        );
    }

    // Compile IR to VM program and get function offsets for exports
    let (program, func_offsets) = compile_ir_to_program_with_offsets(&all_funcs, &all_strings, &[]);

    // Debug: Print bytecode statistics
    eprintln!(
        "[TS_COMPILE] Bytecode: {} opcodes, {} strings",
        program.code.len(),
        program.strings.len()
    );
    for (name, offset) in &func_offsets {
        eprintln!("[TS_COMPILE]   - '{}' at offset {}", name, offset);
    }

    // Build manifest first to get the export list
    let manifest = build_manifest(hirs);

    // Build export entries using the function offsets
    let mut exports_map: HashMap<String, ExportEntry> = HashMap::new();
    for export in &manifest.exports {
        // Look up the function by name in all_funcs to get its arity
        let arity = all_funcs
            .iter()
            .find(|f| f.name == export.name)
            .map(|f| f.params.len() as u8)
            .unwrap_or(0);

        // Look up the bytecode offset
        if let Some(&offset) = func_offsets.get(&export.name) {
            // Use qualified name for export if available, to match CallSymbol resolution.
            // CallSymbol uses qualified names like "HostHelpers.loadPartial", so exports
            // must use qualified names for cross-module function calls to resolve correctly.
            let export_name = export
                .qualified_name
                .clone()
                .unwrap_or_else(|| export.name.clone());
            exports_map.insert(
                export_name.clone(),
                ExportEntry {
                    name: export_name,
                    offset,
                    arity,
                },
            );
        }
    }

    // Create library with exports
    let library = Library {
        program: program.clone(),
        exports: exports_map,
        id: manifest.module_name.clone(),
    };

    // Serialize bytecode as library format (includes exports)
    let bytecode = encode_library(&library);

    // Extract controller registry for VM integration
    let controller_registry = extract_controller_registry(hirs);

    Ok(TsCompileResult {
        program,
        manifest,
        bytecode,
        controller_registry,
    })
}

/// Build enum lowering context from HIR files.
fn build_enum_context(hirs: &[HirFile]) -> EnumLowerContext {
    let mut tags: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
    let mut shared_field_names: HashSet<String> = HashSet::new();

    for hir in hirs {
        for decl in &hir.decls {
            match decl {
                HirDecl::Enum(en) => {
                    let mut vmap: BTreeMap<String, i64> = BTreeMap::new();
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

/// Check if a function has the @export attribute.
fn is_exported_func(func: &arth::compiler::hir::HirFunc) -> bool {
    func.sig.attrs.iter().any(|a| a.name == "export")
}

/// Build manifest from HIR files.
fn build_manifest(hirs: &[HirFile]) -> TsGuestManifest {
    let mut exports: Vec<TsGuestExport> = Vec::new();
    let mut entry: Option<String> = None;
    let mut module_name = "Main".to_string();

    for hir in hirs {
        for decl in &hir.decls {
            match decl {
                HirDecl::Module(m) => {
                    if exports.is_empty() {
                        module_name = m.name.clone();
                    }
                    for func in &m.funcs {
                        // Only export functions that have the @export attribute
                        if !is_exported_func(func) {
                            continue;
                        }
                        let name = func.sig.name.clone();
                        let arity = func.sig.params.len() as u8;
                        if entry.is_none() && name == "main" {
                            entry = Some(name.clone());
                        }
                        exports.push(TsGuestExport {
                            qualified_name: Some(format!("{}.{}", m.name, name)),
                            name,
                            kind: "func".to_string(),
                            arity: Some(arity),
                        });
                    }
                }
                HirDecl::Struct(s) => {
                    exports.push(TsGuestExport {
                        qualified_name: Some(format!("{}.{}", module_name, s.name)),
                        name: s.name.clone(),
                        kind: "struct".to_string(),
                        arity: None,
                    });
                }
                HirDecl::Enum(e) => {
                    exports.push(TsGuestExport {
                        qualified_name: Some(format!("{}.{}", module_name, e.name)),
                        name: e.name.clone(),
                        kind: "enum".to_string(),
                        arity: None,
                    });
                }
                HirDecl::Interface(i) => {
                    exports.push(TsGuestExport {
                        qualified_name: Some(format!("{}.{}", module_name, i.name)),
                        name: i.name.clone(),
                        kind: "interface".to_string(),
                        arity: None,
                    });
                }
                HirDecl::Provider(p) => {
                    exports.push(TsGuestExport {
                        qualified_name: Some(format!("{}.{}", module_name, p.name)),
                        name: p.name.clone(),
                        kind: "provider".to_string(),
                        arity: None,
                    });
                }
                _ => {}
            }
        }
    }

    let package = hirs
        .first()
        .and_then(|h| h.package.as_ref().map(|p| p.to_string()));

    TsGuestManifest {
        format_version: TS_GUEST_MANIFEST_VERSION,
        schema_version: TS_GUEST_SCHEMA_VERSION.to_string(),
        language: "ts".to_string(),
        module_name,
        package,
        entry,
        exports,
        imports: Vec::new(),
        capabilities: Vec::new(),
        host_capabilities: Vec::new(),
        bytecode: "main.abc".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_simple_ts() {
        // NOTE: Only exported functions are lowered to HIR (see lower.rs:351-378)
        let source = r#"
            export function add(a: number, b: number): number {
                return a + b;
            }

            export function main(): number {
                return add(1, 2);
            }
        "#;

        let result = compile_ts_string(source, "test").unwrap();

        assert!(!result.bytecode.is_empty());
        assert_eq!(result.manifest.language, "ts");
        assert!(result.manifest.exports.iter().any(|e| e.name == "main"));
    }

    #[test]
    fn test_compile_default_export_controller_class() {
        // Regular classes are pure behavior containers (no fields allowed).
        // State is passed as parameters.
        let source = r#"
            export default class Controller {
                getUser(name: string): string {
                    return name;
                }

                validateEmail(email: string): boolean {
                    return true;
                }
            }
        "#;

        let result = compile_ts_string(source, "test").unwrap();

        assert!(!result.bytecode.is_empty());
        assert!(result.manifest.exports.iter().any(|e| e.name == "getUser"));
        assert!(
            result
                .manifest
                .exports
                .iter()
                .any(|e| e.name == "validateEmail")
        );
    }

    #[test]
    fn test_compile_provider_class_with_state() {
        // @provider decorated classes can have fields (state).
        // Include a function so the compile step has something to export.
        let source = r#"
            @provider
            class AppState {
                count: number = 0;
                message: string = "hello";
            }

            export function init(): void {
                // Initialize provider
            }
        "#;

        let result = compile_ts_string(source, "test").unwrap();

        assert!(!result.bytecode.is_empty());
        assert!(result.manifest.exports.iter().any(|e| e.name == "init"));
    }

    #[test]
    fn test_compile_class_with_state_errors() {
        // Regular classes with fields should error.
        let source = r#"
            export default class Controller {
                state = { count: 0 };

                increment(): void {
                    this.state.count++;
                }
            }
        "#;

        let result = compile_ts_string(source, "test");

        assert!(result.is_err(), "regular class with fields should error");
        let err = result.unwrap_err();
        assert!(
            matches!(err, TsCompileError::Lowering(_)),
            "should be a lowering error"
        );
    }

    #[test]
    fn test_compile_no_functions_error() {
        let source = "const x = 42;";
        let result = compile_ts_string(source, "test");

        assert!(matches!(result, Err(TsCompileError::NoFunctions)));
    }

    // Phase 5: Export & Symbol Table Tests

    #[test]
    fn test_export_qualified_names() {
        let source = r#"
            export function add(a: number, b: number): number {
                return a + b;
            }

            export function multiply(x: number, y: number): number {
                return x * y;
            }
        "#;

        let result = compile_ts_string(source, "MathUtils").unwrap();

        // Check that qualified names are generated
        let add_export = result.manifest.exports.iter().find(|e| e.name == "add");
        assert!(add_export.is_some());
        let add = add_export.unwrap();
        assert!(add.qualified_name.is_some());
        assert!(add.qualified_name.as_ref().unwrap().ends_with(".add"));

        let mul_export = result
            .manifest
            .exports
            .iter()
            .find(|e| e.name == "multiply");
        assert!(mul_export.is_some());
        let mul = mul_export.unwrap();
        assert!(mul.qualified_name.is_some());
        assert!(mul.qualified_name.as_ref().unwrap().ends_with(".multiply"));
    }

    #[test]
    fn test_export_arity_tracking() {
        let source = r#"
            export function noArgs(): number {
                return 42;
            }

            export function oneArg(x: number): number {
                return x * 2;
            }

            export function twoArgs(a: number, b: number): number {
                return a + b;
            }

            export function threeArgs(a: number, b: number, c: number): number {
                return a + b + c;
            }
        "#;

        let result = compile_ts_string(source, "test").unwrap();

        let no_args = result
            .manifest
            .exports
            .iter()
            .find(|e| e.name == "noArgs")
            .unwrap();
        assert_eq!(no_args.arity, Some(0));

        let one_arg = result
            .manifest
            .exports
            .iter()
            .find(|e| e.name == "oneArg")
            .unwrap();
        assert_eq!(one_arg.arity, Some(1));

        let two_args = result
            .manifest
            .exports
            .iter()
            .find(|e| e.name == "twoArgs")
            .unwrap();
        assert_eq!(two_args.arity, Some(2));

        let three_args = result
            .manifest
            .exports
            .iter()
            .find(|e| e.name == "threeArgs")
            .unwrap();
        assert_eq!(three_args.arity, Some(3));
    }

    #[test]
    fn test_export_kinds() {
        let source = r#"
            export type User = {
                name: string;
                age: number;
            };

            export function getUser(): string {
                return "user";
            }
        "#;

        let result = compile_ts_string(source, "test").unwrap();

        // Check function export kind
        let func_export = result
            .manifest
            .exports
            .iter()
            .find(|e| e.name == "getUser")
            .unwrap();
        assert_eq!(func_export.kind, "func");
        assert!(func_export.arity.is_some());

        // Check struct export kind
        let struct_export = result
            .manifest
            .exports
            .iter()
            .find(|e| e.name == "User")
            .unwrap();
        assert_eq!(struct_export.kind, "struct");
        assert!(struct_export.arity.is_none());
    }

    #[test]
    fn test_provider_export_kind() {
        let source = r#"
            @provider
            class AppState {
                count: number = 0;
            }

            export function init(): void {
                // Initialize
            }
        "#;

        let result = compile_ts_string(source, "test").unwrap();

        // Check provider export kind
        let provider_export = result
            .manifest
            .exports
            .iter()
            .find(|e| e.name == "AppState");
        assert!(provider_export.is_some());
        let provider = provider_export.unwrap();
        assert_eq!(provider.kind, "provider");
        assert!(provider.qualified_name.is_some());
    }

    #[test]
    fn test_interface_export_kind() {
        let source = r#"
            export interface Comparable {
                compare(other: Comparable): number;
            }

            export function compare(a: Comparable): number {
                return 0;
            }
        "#;

        let result = compile_ts_string(source, "test").unwrap();

        // Check interface export kind
        let iface_export = result
            .manifest
            .exports
            .iter()
            .find(|e| e.name == "Comparable");
        assert!(iface_export.is_some());
        let iface = iface_export.unwrap();
        assert_eq!(iface.kind, "interface");
        assert!(iface.qualified_name.is_some());
    }

    #[test]
    fn test_class_methods_export_qualified_names() {
        let source = r#"
            export default class Calculator {
                add(a: number, b: number): number {
                    return a + b;
                }

                subtract(a: number, b: number): number {
                    return a - b;
                }
            }
        "#;

        let result = compile_ts_string(source, "Calculator").unwrap();

        // Both methods should be exported with qualified names
        let add = result
            .manifest
            .exports
            .iter()
            .find(|e| e.name == "add")
            .unwrap();
        assert!(add.qualified_name.is_some());
        assert!(add.qualified_name.as_ref().unwrap().contains("Calculator"));
        assert_eq!(add.arity, Some(2));

        let sub = result
            .manifest
            .exports
            .iter()
            .find(|e| e.name == "subtract")
            .unwrap();
        assert!(sub.qualified_name.is_some());
        assert_eq!(sub.arity, Some(2));
    }

    // Phase 7: VM Integration Tests

    #[test]
    fn test_controller_registry_extracted() {
        let source = r#"
            export default class CounterController {
                increment(): void {}
                decrement(): void {}
            }
        "#;

        let result = compile_ts_string(source, "counter").unwrap();

        // Verify controller registry is populated
        assert!(!result.controller_registry.controllers.is_empty());
        let controller = result
            .controller_registry
            .get_controller("CounterController")
            .expect("controller should exist");
        assert_eq!(controller.name, "CounterController");
        assert!(controller.is_default);
        assert_eq!(controller.handlers.len(), 2);
    }

    #[test]
    fn test_controller_registry_handlers() {
        let source = r#"
            export default class Controller {
                process(name: string, count: number): number {
                    return count;
                }
            }
        "#;

        let result = compile_ts_string(source, "test").unwrap();

        let handler = result
            .controller_registry
            .get_handler("Controller", "process")
            .expect("handler should exist");

        assert_eq!(handler.name, "process");
        assert_eq!(handler.qualified_name, "Controller.process");
        assert_eq!(handler.arity, 2);
        assert_eq!(handler.params.len(), 2);
    }

    #[test]
    fn test_controller_registry_with_provider() {
        let source = r#"
            @provider
            class State {
                count: number = 0;
            }

            export default class Controller {
                constructor() {
                    const state: State = { count: 0 };
                }

                increment(state: State): void {}
            }
        "#;

        let result = compile_ts_string(source, "test").unwrap();

        let controller = result
            .controller_registry
            .get_controller("Controller")
            .expect("controller should exist");

        // Verify constructor is tracked
        assert!(controller.constructor.is_some());
        assert_eq!(
            controller.constructor.as_ref().unwrap(),
            "Controller.constructor"
        );

        // Verify provider is tracked
        assert_eq!(controller.providers.len(), 1);
        assert_eq!(controller.providers[0].name, "State");
        assert!(controller.providers[0].initialized_in_constructor);

        // Verify handler has provider param
        let handler = controller.handlers.get("increment").unwrap();
        assert_eq!(handler.provider_param, Some("State".to_string()));
        assert_eq!(handler.effective_arity(), 0); // Excludes provider param
    }

    #[test]
    fn test_controller_registry_multiple_controllers() {
        let source = r#"
            export class UserController {
                getUser(id: string): string {
                    return id;
                }
            }

            export class OrderController {
                getOrder(id: string): string {
                    return id;
                }
            }
        "#;

        let result = compile_ts_string(source, "test").unwrap();

        // Both controllers should be in the registry
        let names = result.controller_registry.controller_names();
        assert!(names.contains(&"UserController"));
        assert!(names.contains(&"OrderController"));
    }

    #[test]
    fn test_controller_registry_serialization() {
        let source = r#"
            export default class Controller {
                action(): void {}
            }
        "#;

        let result = compile_ts_string(source, "test").unwrap();

        // Verify we can serialize and deserialize the registry
        let json = crate::vm_meta::serialize_controller_registry(&result.controller_registry);
        assert!(json.contains("Controller"));
        assert!(json.contains("action"));

        let parsed = crate::vm_meta::deserialize_controller_registry(&json).expect("should parse");
        assert!(parsed.get_controller("Controller").is_some());
    }
}
