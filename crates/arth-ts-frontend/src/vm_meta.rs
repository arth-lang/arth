//! VM integration metadata for TypeScript controllers.
//!
//! This module provides metadata structures that enable the Arth VM to:
//! - Mount controllers and manage their lifecycle
//! - Track provider instances per controller
//! - Dispatch events to the correct handlers
//! - Validate handler signatures at runtime

use std::collections::BTreeMap;

use arth::compiler::hir::{HirDecl, HirFile, HirFunc, HirProvider, HirType};
use serde::{Deserialize, Serialize};

/// Registry of all controllers in a compiled package.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ControllerRegistry {
    /// Map of controller name → controller info
    pub controllers: BTreeMap<String, ControllerInfo>,
    /// Schema version for compatibility checking
    pub schema_version: String,
}

/// Metadata for a single controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerInfo {
    /// Controller module name (e.g., "CounterController")
    pub name: String,
    /// Fully-qualified constructor name (e.g., "CounterController.constructor")
    pub constructor: Option<String>,
    /// Provider types used by this controller
    pub providers: Vec<ProviderInfo>,
    /// Event handlers exposed by this controller
    pub handlers: BTreeMap<String, HandlerInfo>,
    /// Whether this controller is the default export
    pub is_default: bool,
}

/// Information about a provider type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Provider type name (e.g., "State", "AppState")
    pub name: String,
    /// Whether this provider is initialized in the constructor
    pub initialized_in_constructor: bool,
    /// Provider fields for state tracking
    pub fields: Vec<ProviderFieldInfo>,
}

/// Information about a provider field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderFieldInfo {
    /// Field name
    pub name: String,
    /// Field type as string
    pub field_type: String,
    /// Whether the field is final (readonly)
    pub is_final: bool,
    /// Whether the field is shared (mutable across threads)
    pub is_shared: bool,
}

/// Metadata for an event handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandlerInfo {
    /// Handler method name (e.g., "increment")
    pub name: String,
    /// Fully-qualified name (e.g., "CounterController.increment")
    pub qualified_name: String,
    /// Number of parameters (arity)
    pub arity: u8,
    /// Parameter information
    pub params: Vec<ParamInfo>,
    /// Return type (None for void)
    pub return_type: Option<String>,
    /// Whether the first parameter is a provider (auto-injected by VM)
    pub provider_param: Option<String>,
    /// Whether this is an async handler
    pub is_async: bool,
}

/// Parameter information for a handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamInfo {
    /// Parameter name
    pub name: String,
    /// Parameter type as string
    pub param_type: String,
    /// Whether the parameter is optional
    pub is_optional: bool,
}

/// Event metadata types that can be passed to handlers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // Future use: event dispatch validation
pub enum EventMetadataType {
    /// Click event with position
    Click,
    /// Key down event with key info
    KeyDown,
    /// Input event with value
    Input,
    /// Form submit event
    Submit,
    /// Focus event
    Focus,
    /// Blur event
    Blur,
    /// Custom event with arbitrary data
    Custom,
}

/// Schema version for VM metadata
pub const VM_META_SCHEMA_VERSION: &str = "vm-meta-v1";

impl ControllerRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            controllers: BTreeMap::new(),
            schema_version: VM_META_SCHEMA_VERSION.to_string(),
        }
    }

    /// Add a controller to the registry.
    pub fn add_controller(&mut self, info: ControllerInfo) {
        self.controllers.insert(info.name.clone(), info);
    }

    /// Get a controller by name.
    pub fn get_controller(&self, name: &str) -> Option<&ControllerInfo> {
        self.controllers.get(name)
    }

    /// Get handler info for a controller.handler pair.
    pub fn get_handler(&self, controller: &str, handler: &str) -> Option<&HandlerInfo> {
        self.controllers
            .get(controller)
            .and_then(|c| c.handlers.get(handler))
    }

    /// List all controller names.
    pub fn controller_names(&self) -> Vec<&str> {
        self.controllers.keys().map(|s| s.as_str()).collect()
    }
}

impl ControllerInfo {
    /// Create a new controller info.
    pub fn new(name: String) -> Self {
        Self {
            name,
            constructor: None,
            providers: Vec::new(),
            handlers: BTreeMap::new(),
            is_default: false,
        }
    }

    /// Add a handler to the controller.
    pub fn add_handler(&mut self, handler: HandlerInfo) {
        self.handlers.insert(handler.name.clone(), handler);
    }

    /// Check if this controller uses any providers.
    pub fn has_providers(&self) -> bool {
        !self.providers.is_empty()
    }

    /// Get the primary provider (first provider that's initialized in constructor).
    pub fn primary_provider(&self) -> Option<&ProviderInfo> {
        self.providers.iter().find(|p| p.initialized_in_constructor)
    }
}

impl HandlerInfo {
    /// Get the effective arity (excluding auto-injected provider param).
    pub fn effective_arity(&self) -> u8 {
        if self.provider_param.is_some() {
            self.arity.saturating_sub(1)
        } else {
            self.arity
        }
    }
}

/// Extract controller registry from HIR files.
pub fn extract_controller_registry(hirs: &[HirFile]) -> ControllerRegistry {
    let mut registry = ControllerRegistry::new();

    // First pass: collect all provider definitions
    let mut provider_defs: BTreeMap<String, &HirProvider> = BTreeMap::new();
    for hir in hirs {
        for decl in &hir.decls {
            if let HirDecl::Provider(p) = decl {
                provider_defs.insert(p.name.clone(), p);
            }
        }
    }

    // Second pass: extract controller info from modules
    for hir in hirs {
        for decl in &hir.decls {
            if let HirDecl::Module(m) = decl {
                let mut controller = ControllerInfo::new(m.name.clone());
                controller.is_default = m.is_exported;

                // Extract providers from constructor
                for func in &m.funcs {
                    if func.sig.name == "constructor" {
                        controller.constructor = Some(format!("{}.constructor", m.name));
                        extract_provider_inits(func, &provider_defs, &mut controller.providers);
                    }
                }

                // Extract handlers (all public functions except constructor)
                for func in &m.funcs {
                    if func.sig.name != "constructor" {
                        let handler = extract_handler_info(&m.name, func, &provider_defs);
                        controller.add_handler(handler);
                    }
                }

                if !controller.handlers.is_empty() || controller.constructor.is_some() {
                    registry.add_controller(controller);
                }
            }
        }
    }

    registry
}

/// Extract provider initializations from a constructor.
fn extract_provider_inits(
    func: &HirFunc,
    provider_defs: &BTreeMap<String, &HirProvider>,
    providers: &mut Vec<ProviderInfo>,
) {
    // Look for provider type annotations in the function body
    if let Some(body) = &func.body {
        for stmt in &body.stmts {
            if let arth::compiler::hir::HirStmt::VarDecl { ty, .. } = stmt
                && let Some(provider_name) = extract_type_name(ty)
                && let Some(provider_def) = provider_defs.get(&provider_name)
            {
                let fields = provider_def
                    .fields
                    .iter()
                    .map(|f| ProviderFieldInfo {
                        name: f.name.clone(),
                        field_type: type_to_string(&f.ty),
                        is_final: f.is_final,
                        is_shared: f.is_shared,
                    })
                    .collect();

                providers.push(ProviderInfo {
                    name: provider_name,
                    initialized_in_constructor: true,
                    fields,
                });
            }
        }
    }
}

/// Extract handler info from a function.
fn extract_handler_info(
    module_name: &str,
    func: &HirFunc,
    provider_defs: &BTreeMap<String, &HirProvider>,
) -> HandlerInfo {
    let name = func.sig.name.clone();
    let qualified_name = format!("{}.{}", module_name, name);
    let arity = func.sig.params.len() as u8;
    let is_async = func.sig.is_async;

    // Check if first param is a provider type
    let provider_param = func.sig.params.first().and_then(|p| {
        let type_name = extract_type_name(&p.ty)?;
        if provider_defs.contains_key(&type_name) {
            Some(type_name)
        } else {
            None
        }
    });

    let params: Vec<ParamInfo> = func
        .sig
        .params
        .iter()
        .map(|p| ParamInfo {
            name: p.name.clone(),
            param_type: type_to_string(&p.ty),
            is_optional: false, // Optional tracking is done at HIR lowering time
        })
        .collect();

    let return_type = func.sig.ret.as_ref().map(type_to_string);

    HandlerInfo {
        name,
        qualified_name,
        arity,
        params,
        return_type,
        provider_param,
        is_async,
    }
}

/// Extract the primary type name from a HirType.
fn extract_type_name(ty: &HirType) -> Option<String> {
    match ty {
        HirType::Name { path } => path.last().cloned(),
        HirType::Generic { path, .. } => path.last().cloned(),
        HirType::TypeParam { name } => Some(name.clone()),
    }
}

/// Convert a HirType to a string representation.
fn type_to_string(ty: &HirType) -> String {
    match ty {
        HirType::Name { path } => path.join("."),
        HirType::Generic { path, args } => {
            let args_str: Vec<String> = args.iter().map(type_to_string).collect();
            format!("{}<{}>", path.join("."), args_str.join(", "))
        }
        HirType::TypeParam { name } => name.clone(),
    }
}

/// Generate JSON representation of the controller registry.
pub fn serialize_controller_registry(registry: &ControllerRegistry) -> String {
    serde_json::to_string_pretty(registry).unwrap_or_else(|_| "{}".to_string())
}

/// Parse a controller registry from JSON.
pub fn deserialize_controller_registry(json: &str) -> Result<ControllerRegistry, String> {
    serde_json::from_str(json).map_err(|e| format!("Failed to parse controller registry: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::{TsLoweringOptions, lower_ts_str_to_hir};

    #[test]
    fn test_extract_simple_controller() {
        let source = r#"
export default class CounterController {
    increment(): void {
        // increment
    }

    decrement(): void {
        // decrement
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let registry = extract_controller_registry(&[hir]);

        assert_eq!(registry.controllers.len(), 1);
        let controller = registry.get_controller("CounterController").unwrap();
        assert_eq!(controller.name, "CounterController");
        assert!(controller.is_default);
        assert_eq!(controller.handlers.len(), 2);
        assert!(controller.handlers.contains_key("increment"));
        assert!(controller.handlers.contains_key("decrement"));
    }

    #[test]
    fn test_extract_controller_with_constructor() {
        let source = r#"
@provider
class State {
    count: number = 0;
}

export default class CounterController {
    constructor() {
        const state: State = { count: 0 };
    }

    increment(state: State): void {
        // increment
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let registry = extract_controller_registry(&[hir]);

        let controller = registry.get_controller("CounterController").unwrap();
        assert!(controller.constructor.is_some());
        assert_eq!(
            controller.constructor.as_ref().unwrap(),
            "CounterController.constructor"
        );
    }

    #[test]
    fn test_extract_handler_info() {
        let source = r#"
@provider
class State {
    count: number = 0;
}

export default class Controller {
    increment(state: State, step: number): number {
        return 0;
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let registry = extract_controller_registry(&[hir]);

        let handler = registry.get_handler("Controller", "increment").unwrap();
        assert_eq!(handler.name, "increment");
        assert_eq!(handler.qualified_name, "Controller.increment");
        assert_eq!(handler.arity, 2);
        assert_eq!(handler.provider_param, Some("State".to_string()));
        assert_eq!(handler.effective_arity(), 1); // Excludes provider param
        assert_eq!(handler.params.len(), 2);
        assert_eq!(handler.params[0].name, "self"); // renamed from state
        assert_eq!(handler.params[1].name, "step");
    }

    #[test]
    fn test_extract_provider_info() {
        let source = r#"
@provider
class AppState {
    readonly capacity: number = 100;
    count: number = 0;
    message: string = "";
}

export default class Controller {
    constructor() {
        const state: AppState = { capacity: 100, count: 0, message: "" };
    }

    update(state: AppState): void {
        // update
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let registry = extract_controller_registry(&[hir]);

        let controller = registry.get_controller("Controller").unwrap();
        assert_eq!(controller.providers.len(), 1);

        let provider = &controller.providers[0];
        assert_eq!(provider.name, "AppState");
        assert!(provider.initialized_in_constructor);
        assert_eq!(provider.fields.len(), 3);

        // Check field properties
        let capacity = provider
            .fields
            .iter()
            .find(|f| f.name == "capacity")
            .unwrap();
        assert!(capacity.is_final);
        assert!(!capacity.is_shared);

        let count = provider.fields.iter().find(|f| f.name == "count").unwrap();
        assert!(!count.is_final);
        assert!(count.is_shared);
    }

    #[test]
    fn test_handler_with_async() {
        let source = r#"
export default class Controller {
    async fetchData(): Promise<string> {
        return "";
    }
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let registry = extract_controller_registry(&[hir]);

        let handler = registry.get_handler("Controller", "fetchData").unwrap();
        assert!(handler.is_async);
        assert!(handler.return_type.is_some());
    }

    #[test]
    fn test_multiple_controllers() {
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
        // Note: Need to use separate class declarations for multiple controllers
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let registry = extract_controller_registry(&[hir]);

        // Both controllers should be extracted
        let names = registry.controller_names();
        assert!(names.contains(&"UserController"));
        assert!(names.contains(&"OrderController"));
    }

    #[test]
    fn test_serialize_deserialize_registry() {
        let source = r#"
export default class Controller {
    doSomething(): void {}
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let registry = extract_controller_registry(&[hir]);
        let json = serialize_controller_registry(&registry);

        let parsed = deserialize_controller_registry(&json).expect("should parse");
        assert_eq!(parsed.controllers.len(), registry.controllers.len());
        assert!(parsed.get_controller("Controller").is_some());
    }

    #[test]
    fn test_handler_param_types() {
        let source = r#"
export default class Controller {
    process(name: string, count: number, active: boolean): void {}
}
"#;
        let hir = lower_ts_str_to_hir(source, "test.ts", TsLoweringOptions::default())
            .expect("should parse and lower");

        let registry = extract_controller_registry(&[hir]);

        let handler = registry.get_handler("Controller", "process").unwrap();
        assert_eq!(handler.params.len(), 3);
        assert_eq!(handler.params[0].param_type, "String");
        assert_eq!(handler.params[1].param_type, "Int");
        assert_eq!(handler.params[2].param_type, "Bool");
    }

    #[test]
    fn test_registry_schema_version() {
        let registry = ControllerRegistry::new();
        assert_eq!(registry.schema_version, VM_META_SCHEMA_VERSION);
    }
}
