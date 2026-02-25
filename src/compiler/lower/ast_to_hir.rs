//! AST to HIR lowering with optional conditional compilation filtering.
//!
//! This module provides functions to lower AST to HIR, with support for
//! filtering declarations based on `@cfg(...)` attributes.

use std::path::PathBuf;

use crate::compiler::ast::{
    Attr, Decl, EnumDecl, ExternFuncDecl, FileAst, FuncDecl, InterfaceDecl, ModuleDecl,
    ProviderDecl, StructDecl, TypeAliasDecl,
};
use crate::compiler::attrs::{CfgBackend, CfgPredicate, parse_attr_args, parse_cfg_predicate};
use crate::compiler::hir::{HirFile, make_hir_file};
use crate::compiler::source::SourceFile;

/// Lower an AST file to HIR without cfg filtering.
pub fn lower_file(sf: &SourceFile, ast: &FileAst) -> HirFile {
    let pkg = ast.package.as_ref().map(|p| p.to_string());
    make_hir_file(PathBuf::from(&sf.path), pkg, &ast.decls)
}

/// Lower an AST file to HIR with cfg filtering for the specified backend.
///
/// Declarations marked with `@cfg(backend = "...")` will be included only
/// if they match the specified backend.
pub fn lower_file_with_cfg(sf: &SourceFile, ast: &FileAst, backend: CfgBackend) -> HirFile {
    let pkg = ast.package.as_ref().map(|p| p.to_string());
    let filtered_decls = filter_decls_by_cfg(&ast.decls, backend);
    make_hir_file(PathBuf::from(&sf.path), pkg, &filtered_decls)
}

/// Filter a list of declarations based on cfg predicates.
fn filter_decls_by_cfg(decls: &[Decl], backend: CfgBackend) -> Vec<Decl> {
    decls
        .iter()
        .filter(|decl| decl_passes_cfg(decl, backend))
        .cloned()
        .collect()
}

/// Check if a declaration's cfg attributes pass for the given backend.
fn decl_passes_cfg(decl: &Decl, backend: CfgBackend) -> bool {
    let attrs = get_decl_attrs(decl);
    attrs_pass_cfg(attrs, backend)
}

/// Extract attributes from a declaration.
fn get_decl_attrs(decl: &Decl) -> &[Attr] {
    match decl {
        Decl::Module(ModuleDecl { attrs, .. }) => attrs,
        Decl::Struct(StructDecl { attrs, .. }) => attrs,
        Decl::Interface(InterfaceDecl { attrs, .. }) => attrs,
        Decl::Enum(EnumDecl { attrs, .. }) => attrs,
        Decl::Provider(ProviderDecl { attrs, .. }) => attrs,
        Decl::Function(FuncDecl { sig, .. }) => &sig.attrs,
        Decl::TypeAlias(TypeAliasDecl { attrs, .. }) => attrs,
        Decl::ExternFunc(ExternFuncDecl { attrs, .. }) => attrs,
    }
}

/// Check if all cfg attributes pass for the given backend.
fn attrs_pass_cfg(attrs: &[Attr], backend: CfgBackend) -> bool {
    for attr in attrs {
        // Check if this is a cfg attribute (last path component is "cfg")
        let attr_name = attr.name.path.last().map(|s| s.0.as_str());
        if attr_name == Some("cfg") {
            // Parse the attribute arguments
            let parsed_args = parse_attr_args(attr.args.as_deref());
            if let Ok(predicate) = parse_cfg_predicate(&parsed_args) {
                if !predicate.evaluate(backend) {
                    return false;
                }
            }
            // If parsing fails, we skip the predicate (error reported elsewhere)
        }
    }
    true
}

/// Evaluate a single cfg predicate against the current backend.
#[allow(dead_code)]
pub fn evaluate_cfg(predicate: &CfgPredicate, backend: CfgBackend) -> bool {
    predicate.evaluate(backend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::ast::{FuncSig, Ident, NamePath, Visibility};
    use crate::compiler::source::Span;

    fn make_cfg_attr(args: &str) -> Attr {
        Attr {
            name: NamePath {
                path: vec![Ident("cfg".to_string())],
                type_args: vec![],
            },
            args: Some(args.to_string()),
        }
    }

    fn make_test_func_sig(attrs: Vec<Attr>) -> FuncSig {
        FuncSig {
            vis: Visibility::Public,
            is_static: false,
            is_final: false,
            is_async: false,
            is_unsafe: false,
            name: Ident("test".to_string()),
            ret: None,
            params: vec![],
            generics: vec![],
            throws: vec![],
            doc: None,
            attrs,
            span: Span::new(0, 10),
        }
    }

    fn make_test_decl(attrs: Vec<Attr>) -> Decl {
        Decl::Function(FuncDecl {
            sig: make_test_func_sig(attrs),
            body: None,
            span: Span::new(0, 10),
            id: crate::compiler::ast::AstId(1),
            body_id: None,
        })
    }

    #[test]
    fn test_no_cfg_attrs_pass() {
        let decl = make_test_decl(vec![]);
        assert!(decl_passes_cfg(&decl, CfgBackend::Vm));
        assert!(decl_passes_cfg(&decl, CfgBackend::Llvm));
        assert!(decl_passes_cfg(&decl, CfgBackend::Cranelift));
    }

    #[test]
    fn test_cfg_backend_vm_pass() {
        let decl = make_test_decl(vec![make_cfg_attr("backend=\"vm\"")]);
        assert!(decl_passes_cfg(&decl, CfgBackend::Vm));
        assert!(!decl_passes_cfg(&decl, CfgBackend::Llvm));
        assert!(!decl_passes_cfg(&decl, CfgBackend::Cranelift));
    }

    #[test]
    fn test_cfg_backend_llvm_pass() {
        let decl = make_test_decl(vec![make_cfg_attr("backend=\"llvm\"")]);
        assert!(!decl_passes_cfg(&decl, CfgBackend::Vm));
        assert!(decl_passes_cfg(&decl, CfgBackend::Llvm));
        assert!(!decl_passes_cfg(&decl, CfgBackend::Cranelift));
    }

    #[test]
    fn test_cfg_backend_native_alias() {
        // "native" is an alias for "llvm"
        let decl = make_test_decl(vec![make_cfg_attr("backend=\"native\"")]);
        assert!(!decl_passes_cfg(&decl, CfgBackend::Vm));
        assert!(decl_passes_cfg(&decl, CfgBackend::Llvm));
    }

    #[test]
    fn test_multiple_cfg_attrs_and() {
        // Multiple cfg attrs act as AND - both must pass
        // This tests an impossible condition: backend must be both vm and llvm
        let decl = make_test_decl(vec![
            make_cfg_attr("backend=\"vm\""),
            make_cfg_attr("backend=\"llvm\""),
        ]);
        assert!(!decl_passes_cfg(&decl, CfgBackend::Vm));
        assert!(!decl_passes_cfg(&decl, CfgBackend::Llvm));
    }

    #[test]
    fn test_filter_decls_by_cfg() {
        let decls = vec![
            make_test_decl(vec![make_cfg_attr("backend=\"vm\"")]),
            make_test_decl(vec![make_cfg_attr("backend=\"llvm\"")]),
            make_test_decl(vec![]), // No cfg, always included
        ];

        let vm_filtered = filter_decls_by_cfg(&decls, CfgBackend::Vm);
        assert_eq!(vm_filtered.len(), 2); // vm-specific + no-cfg

        let llvm_filtered = filter_decls_by_cfg(&decls, CfgBackend::Llvm);
        assert_eq!(llvm_filtered.len(), 2); // llvm-specific + no-cfg
    }

    #[test]
    fn test_cfg_shorthand() {
        // Test @cfg(vm) shorthand syntax
        let decl = make_test_decl(vec![Attr {
            name: NamePath {
                path: vec![Ident("cfg".to_string())],
                type_args: vec![],
            },
            args: Some("vm".to_string()),
        }]);
        assert!(decl_passes_cfg(&decl, CfgBackend::Vm));
        assert!(!decl_passes_cfg(&decl, CfgBackend::Llvm));
    }

    #[test]
    fn test_module_cfg() {
        // Test cfg on module declaration
        let module = Decl::Module(ModuleDecl {
            name: Ident("TestModule".to_string()),
            is_exported: true,
            implements: vec![],
            items: vec![],
            doc: None,
            attrs: vec![make_cfg_attr("backend=\"vm\"")],
            span: Span::new(0, 10),
            id: crate::compiler::ast::AstId(2),
        });
        assert!(decl_passes_cfg(&module, CfgBackend::Vm));
        assert!(!decl_passes_cfg(&module, CfgBackend::Llvm));
    }
}
