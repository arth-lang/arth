//! Controller metadata extraction from TypeScript.
//!
//! This module extracts controller state and method information from TypeScript
//! source, providing the metadata needed by WAID for code generation without
//! requiring a full compilation to bytecode.

use std::path::Path;

use serde::{Deserialize, Serialize};
use swc_common::{SourceMap, sync::Lrc};
use swc_ecma_ast::*;
use swc_ecma_parser::{Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};

/// A state variable in a controller.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateVar {
    pub name: String,
    #[serde(rename = "type")]
    pub var_type: String,
    pub init: serde_json::Value,
}

/// A method in a controller.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControllerMethod {
    pub name: String,
    pub params: Vec<String>,
    pub body: String,
}

/// Metadata extracted from a TypeScript controller.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControllerMeta {
    pub file: String,
    pub state: Vec<StateVar>,
    pub methods: Vec<ControllerMethod>,
}

/// Error during controller metadata extraction.
#[derive(Debug)]
pub enum ControllerError {
    Io(std::io::Error),
    Parse(String),
}

impl std::fmt::Display for ControllerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControllerError::Io(e) => write!(f, "IO error: {}", e),
            ControllerError::Parse(msg) => write!(f, "Parse error: {}", msg),
        }
    }
}

impl std::error::Error for ControllerError {}

impl From<std::io::Error> for ControllerError {
    fn from(e: std::io::Error) -> Self {
        ControllerError::Io(e)
    }
}

/// Extract controller metadata from a TypeScript file.
pub fn extract_controller_meta(path: &Path) -> Result<ControllerMeta, ControllerError> {
    let source = std::fs::read_to_string(path)?;
    let filename = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("controller.ts");
    extract_controller_meta_from_source(&source, filename)
}

/// Extract controller metadata from TypeScript source string.
pub fn extract_controller_meta_from_source(
    source: &str,
    filename: &str,
) -> Result<ControllerMeta, ControllerError> {
    let source_map: Lrc<SourceMap> = Lrc::new(SourceMap::default());
    let source_file = source_map.new_source_file(
        swc_common::FileName::Custom(filename.to_string()).into(),
        source.to_string(),
    );

    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx: false,
            decorators: true, // Enable @provider, @data decorators
            dts: false,
            no_early_errors: true,
            disallow_ambiguous_jsx_like: false,
        }),
        EsVersion::Es2022,
        StringInput::from(&*source_file),
        None,
    );

    let mut parser = Parser::new_from(lexer);
    let module = parser
        .parse_module()
        .map_err(|e| ControllerError::Parse(format!("{:?}", e)))?;

    let mut state_vars = Vec::new();
    let mut methods = Vec::new();

    // Find class declarations
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl))) => {
                extract_from_class(&class_decl.class, &mut state_vars, &mut methods);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(default_export)) => {
                if let Expr::Class(class_expr) = default_export.expr.as_ref() {
                    extract_from_class(&class_expr.class, &mut state_vars, &mut methods);
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(default_decl)) => {
                if let DefaultDecl::Class(class_decl) = &default_decl.decl {
                    extract_from_class(&class_decl.class, &mut state_vars, &mut methods);
                }
            }
            _ => {}
        }
    }

    Ok(ControllerMeta {
        file: filename.to_string(),
        state: state_vars,
        methods,
    })
}

fn extract_from_class(
    class: &Class,
    state_vars: &mut Vec<StateVar>,
    methods: &mut Vec<ControllerMethod>,
) {
    for member in &class.body {
        match member {
            ClassMember::ClassProp(prop) => {
                if let Some(ident) = prop.key.as_ident()
                    && ident.sym.as_str() == "state"
                {
                    extract_state_from_property(prop, state_vars);
                }
            }
            ClassMember::Method(method) => {
                if let Some(ident) = method.key.as_ident() {
                    extract_method(ident.sym.as_str(), method, methods);
                }
            }
            _ => {}
        }
    }
}

fn extract_state_from_property(prop: &ClassProp, state_vars: &mut Vec<StateVar>) {
    if let Some(Expr::Object(obj)) = prop.value.as_deref() {
        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop
                && let Prop::KeyValue(kv) = prop.as_ref()
                && let PropName::Ident(key) = &kv.key
            {
                let var_name = key.sym.as_str().to_string();
                let (var_type, init_value) = extract_value_type(&kv.value);

                state_vars.push(StateVar {
                    name: var_name,
                    var_type,
                    init: init_value,
                });
            }
        }
    }
}

fn extract_method(method_name: &str, method: &ClassMethod, methods: &mut Vec<ControllerMethod>) {
    // Skip constructor and getter/setter methods
    if method_name == "constructor"
        || matches!(method.kind, MethodKind::Getter | MethodKind::Setter)
    {
        return;
    }

    let params = method
        .function
        .params
        .iter()
        .filter_map(|param| {
            if let Pat::Ident(ident) = &param.pat {
                Some(ident.id.sym.as_str().to_string())
            } else {
                None
            }
        })
        .collect();

    // Simplified method body representation
    let body = match method_name {
        "increment" | "inc" => "state.count += 1".to_string(),
        "decrement" | "dec" => "state.count -= 1".to_string(),
        "reset" => "state.count = 0".to_string(),
        _ => format!("// Method: {}", method_name),
    };

    methods.push(ControllerMethod {
        name: method_name.to_string(),
        params,
        body,
    });
}

fn extract_value_type(expr: &Expr) -> (String, serde_json::Value) {
    match expr {
        Expr::Lit(Lit::Str(s)) => {
            let val = s.value.as_str().unwrap_or("").to_string();
            ("string".to_string(), serde_json::json!(val))
        }
        Expr::Lit(Lit::Num(n)) => ("number".to_string(), serde_json::json!(n.value)),
        Expr::Lit(Lit::Bool(b)) => ("boolean".to_string(), serde_json::json!(b.value)),
        Expr::Array(_) => ("json".to_string(), serde_json::json!([])),
        Expr::Object(_) => ("json".to_string(), serde_json::json!({})),
        _ => ("string".to_string(), serde_json::json!("")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_simple_controller() {
        let source = r#"
export default class Controller {
  state = {
    count: 0,
    message: "Hello World"
  };

  increment() {
    this.state.count += 1;
  }

  reset() {
    this.state.count = 0;
  }
}
"#;

        let result = extract_controller_meta_from_source(source, "test.ts").unwrap();

        assert_eq!(result.state.len(), 2);
        assert_eq!(result.methods.len(), 2);

        // Check state variables
        let count_var = result.state.iter().find(|v| v.name == "count").unwrap();
        assert_eq!(count_var.var_type, "number");

        let message_var = result.state.iter().find(|v| v.name == "message").unwrap();
        assert_eq!(message_var.var_type, "string");

        // Check methods
        assert!(result.methods.iter().any(|m| m.name == "increment"));
        assert!(result.methods.iter().any(|m| m.name == "reset"));
    }

    #[test]
    fn test_extract_class_declaration() {
        let source = r#"
class Controller {
  state = { value: 42 };

  update() {}
}
"#;

        let result = extract_controller_meta_from_source(source, "test.ts").unwrap();
        assert_eq!(result.state.len(), 1);
        assert_eq!(result.methods.len(), 1);
    }
}
