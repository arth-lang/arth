use super::*;
use crate::compiler::ast::{Expr, Stmt, Visibility};
use std::path::PathBuf;

#[test]
fn parses_package_clause() {
    let sf = SourceFile {
        path: PathBuf::from("test.arth"),
        text: String::from("package a.b.c;\n"),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    let pkg = ast.package.expect("expected package");
    let names: Vec<String> = pkg.0.into_iter().map(|Ident(s)| s).collect();
    assert_eq!(names, vec!["a", "b", "c"]);
}

#[test]
fn parses_imports_and_module() {
    let src = r#"
            package demo.async;
            import log.*;
            import concurrent.Channel;
            module Main { public void main() { println("hi"); } }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test2.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    assert_eq!(ast.imports.len(), 2);
    assert!(ast.imports[0].star);
    assert!(!ast.imports[1].star);
    assert!(!ast.decls.is_empty());
}

#[test]
fn parses_module_function_signature() {
    // Free functions at top level are no longer allowed; functions must be in modules
    let src = r#"
            module Greeter { public void greet(String name) { println(name); } }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test3.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    // Check that the module contains the function
    let has_fn = ast.decls.iter().any(|d| match d {
        Decl::Module(m) => m.items.iter().any(|f| f.sig.name.0 == "greet"),
        _ => false,
    });
    assert!(has_fn);
}

#[test]
fn parses_local_variable_declaration() {
    let src = r#"
            module Main { public void main() { int x = 1; x = x + 2; } }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_vars.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    // locate Main.main body
    let body = ast
        .decls
        .iter()
        .find_map(|d| {
            if let Decl::Module(m) = d {
                if m.name.0 == "Main" {
                    m.items.iter().find_map(|f| {
                        if f.sig.name.0 == "main" {
                            f.body.clone()
                        } else {
                            None
                        }
                    })
                } else {
                    None
                }
            } else {
                None
            }
        })
        .expect("expected main body");
    assert!(
        matches!(body.stmts.first(), Some(Stmt::VarDecl { name, init: Some(Expr::Int(1)), .. }) if name.0 == "x")
    );
    assert!(matches!(body.stmts.get(1), Some(Stmt::Assign { name, .. }) if name.0 == "x"));
}

#[test]
fn parses_numeric_literal_type_suffix_as_cast() {
    let src = r#"
            module Main { public void main() { int x = 1i32; float32 y = 1.0f32; } }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_numeric_suffix.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    // locate Main.main body
    let body = ast
        .decls
        .iter()
        .find_map(|d| {
            if let Decl::Module(m) = d {
                if m.name.0 == "Main" {
                    m.items.iter().find_map(|f| {
                        if f.sig.name.0 == "main" {
                            f.body.clone()
                        } else {
                            None
                        }
                    })
                } else {
                    None
                }
            } else {
                None
            }
        })
        .expect("expected main body");
    assert_eq!(body.stmts.len(), 2);
    // int x = 1i32;
    if let Some(Stmt::VarDecl {
        name,
        init: Some(expr),
        ..
    }) = body.stmts.first()
    {
        assert_eq!(name.0, "x");
        match expr {
            Expr::Cast(tp, inner) => {
                assert_eq!(tp.path.len(), 1);
                assert_eq!(tp.path[0].0.to_ascii_lowercase(), "i32");
                assert!(matches!(&**inner, Expr::Int(1)));
            }
            _ => panic!("expected cast for suffixed int literal"),
        }
    } else {
        panic!("expected first statement to be VarDecl with init");
    }
    // float32 y = 1.0f32;
    if let Some(Stmt::VarDecl {
        name,
        init: Some(expr),
        ..
    }) = body.stmts.get(1)
    {
        assert_eq!(name.0, "y");
        match expr {
            Expr::Cast(tp, inner) => {
                assert_eq!(tp.path.len(), 1);
                assert_eq!(tp.path[0].0.to_ascii_lowercase(), "f32");
                assert!(matches!(&**inner, Expr::Float(_)));
            }
            _ => panic!("expected cast for suffixed float literal"),
        }
    } else {
        panic!("expected second statement to be VarDecl with init");
    }
}

#[test]
fn parses_compound_assignment() {
    let src = r#"
            module Main { public void main() { int x = 1; x += 2; x *= 3; } }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_assign.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    let body = ast
        .decls
        .iter()
        .find_map(|d| {
            if let Decl::Module(m) = d {
                if m.name.0 == "Main" {
                    m.items.iter().find_map(|f| {
                        if f.sig.name.0 == "main" {
                            f.body.clone()
                        } else {
                            None
                        }
                    })
                } else {
                    None
                }
            } else {
                None
            }
        })
        .expect("expected main body");
    assert!(matches!(
        body.stmts.get(1),
        Some(Stmt::AssignOp {
            op: crate::compiler::ast::AssignOp::Add,
            ..
        })
    ));
    assert!(matches!(
        body.stmts.get(2),
        Some(Stmt::AssignOp {
            op: crate::compiler::ast::AssignOp::Mul,
            ..
        })
    ));
}

#[test]
fn parses_compound_assignment_mod_and_shifts() {
    let src = r#"
            module Main { public void main() { int x = 10; x %= 4; x <<= 1; x >>= 2; } }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_assign2.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    let body = ast
        .decls
        .iter()
        .find_map(|d| {
            if let Decl::Module(m) = d {
                if m.name.0 == "Main" {
                    m.items.iter().find_map(|f| {
                        if f.sig.name.0 == "main" {
                            f.body.clone()
                        } else {
                            None
                        }
                    })
                } else {
                    None
                }
            } else {
                None
            }
        })
        .expect("expected main body");
    use crate::compiler::ast::AssignOp::*;
    assert!(matches!(
        body.stmts.get(1),
        Some(Stmt::AssignOp { op: Mod, .. })
    ));
    assert!(matches!(
        body.stmts.get(2),
        Some(Stmt::AssignOp { op: Shl, .. })
    ));
    assert!(matches!(
        body.stmts.get(3),
        Some(Stmt::AssignOp { op: Shr, .. })
    ));
}

#[test]
fn parses_struct_with_generics_and_fields() {
    let src = r#"
            struct Pair<T extends Bound> { public final T first; private T second; }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test4.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    let st = ast
        .decls
        .iter()
        .find_map(|d| {
            if let Decl::Struct(s) = d {
                Some(s)
            } else {
                None
            }
        })
        .expect("struct not found");
    assert_eq!(st.name.0, "Pair");
    assert_eq!(st.generics.len(), 1);
    assert!(st.generics[0].bound.is_some());
    assert_eq!(st.fields.len(), 2);
    use Visibility::*;
    assert!(matches!(st.fields[0].vis, Public));
    assert!(st.fields[0].is_final);
    assert_eq!(st.fields[0].name.0, "first");
    assert!(matches!(st.fields[1].vis, Private));
    assert_eq!(st.fields[1].name.0, "second");
}

#[test]
fn parses_interface_with_generic_and_method() {
    let src = r#"
            interface Repo<T> { T get(); }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test5.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    let ifc = ast
        .decls
        .iter()
        .find_map(|d| {
            if let Decl::Interface(i) = d {
                Some(i)
            } else {
                None
            }
        })
        .expect("interface not found");
    assert_eq!(ifc.name.0, "Repo");
    assert_eq!(ifc.methods.len(), 1);
    let ret = ifc.methods[0]
        .sig
        .ret
        .as_ref()
        .expect("expected return type");
    assert_eq!(ret.path.len(), 1);
    assert_eq!(ret.path[0].0, "T");
}

#[test]
fn parses_enum_with_generics_and_tuple_variants() {
    let src = r#"
            enum Result<T, E> { Ok(T), Err(E) }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test6.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    let en = ast
        .decls
        .iter()
        .find_map(|d| if let Decl::Enum(e) = d { Some(e) } else { None })
        .expect("enum not found");
    assert_eq!(en.name.0, "Result");
    assert_eq!(en.generics.len(), 2);
    assert_eq!(en.variants.len(), 2);
    match &en.variants[0] {
        EnumVariant::Tuple { name, types, .. } => {
            assert_eq!(name.0, "Ok");
            assert_eq!(types.len(), 1);
        }
        _ => panic!("expected tuple"),
    }
    match &en.variants[1] {
        EnumVariant::Tuple { name, types, .. } => {
            assert_eq!(name.0, "Err");
            assert_eq!(types.len(), 1);
        }
        _ => panic!("expected tuple"),
    }
}

#[test]
fn parses_sealed_enum_header() {
    let src = r#"
            sealed enum Color { RED, GREEN, BLUE }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_sealed_enum.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    let en = ast
        .decls
        .iter()
        .find_map(|d| if let Decl::Enum(e) = d { Some(e) } else { None })
        .expect("enum not found");
    assert_eq!(en.name.0, "Color");
    assert_eq!(en.variants.len(), 3);
}

#[test]
fn parses_enum_explicit_discriminants() {
    let src = r#"
            enum Status { OK = 0, ERROR = 1, UNKNOWN = 2 }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_enum_discriminants.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    let en = ast
        .decls
        .iter()
        .find_map(|d| if let Decl::Enum(e) = d { Some(e) } else { None })
        .expect("enum not found");
    assert_eq!(en.name.0, "Status");
    assert_eq!(en.variants.len(), 3);

    // Check that discriminants are parsed
    match &en.variants[0] {
        EnumVariant::Unit { name, discriminant } => {
            assert_eq!(name.0, "OK");
            assert!(discriminant.is_some());
        }
        _ => panic!("expected unit variant"),
    }
    match &en.variants[1] {
        EnumVariant::Unit { name, discriminant } => {
            assert_eq!(name.0, "ERROR");
            assert!(discriminant.is_some());
        }
        _ => panic!("expected unit variant"),
    }
}

#[test]
fn parses_enum_mixed_discriminants() {
    let src = r#"
            enum Mixed { First, Second = 10, Third }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_enum_mixed.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    let en = ast
        .decls
        .iter()
        .find_map(|d| if let Decl::Enum(e) = d { Some(e) } else { None })
        .expect("enum not found");
    assert_eq!(en.name.0, "Mixed");
    assert_eq!(en.variants.len(), 3);

    // First has no discriminant
    match &en.variants[0] {
        EnumVariant::Unit { name, discriminant } => {
            assert_eq!(name.0, "First");
            assert!(discriminant.is_none());
        }
        _ => panic!("expected unit variant"),
    }
    // Second has discriminant
    match &en.variants[1] {
        EnumVariant::Unit { name, discriminant } => {
            assert_eq!(name.0, "Second");
            assert!(discriminant.is_some());
        }
        _ => panic!("expected unit variant"),
    }
    // Third has no discriminant
    match &en.variants[2] {
        EnumVariant::Unit { name, discriminant } => {
            assert_eq!(name.0, "Third");
            assert!(discriminant.is_none());
        }
        _ => panic!("expected unit variant"),
    }
}

#[test]
fn parses_enum_expression_discriminants() {
    let src = r#"
            enum Flags { A = 1, B = 2, C = 1 | 2 }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_enum_expr.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    let en = ast
        .decls
        .iter()
        .find_map(|d| if let Decl::Enum(e) = d { Some(e) } else { None })
        .expect("enum not found");
    assert_eq!(en.name.0, "Flags");
    assert_eq!(en.variants.len(), 3);

    // C has a complex expression discriminant
    match &en.variants[2] {
        EnumVariant::Unit { name, discriminant } => {
            assert_eq!(name.0, "C");
            assert!(discriminant.is_some());
        }
        _ => panic!("expected unit variant"),
    }
}

#[test]
fn parses_module_with_implements_clause() {
    let src = r#"
            module Foo implements Bar, Baz { public void main() {} }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_module_impl.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    let m = ast
        .decls
        .iter()
        .find_map(|d| {
            if let Decl::Module(m) = d {
                Some(m)
            } else {
                None
            }
        })
        .expect("module not found");
    assert_eq!(m.name.0, "Foo");
    assert_eq!(m.items.len(), 1);
    assert_eq!(m.items[0].sig.name.0, "main");
    // implements Bar, Baz (target type inferred from method signatures)
    let impls: Vec<String> = m
        .implements
        .iter()
        .map(|np| {
            np.path
                .iter()
                .map(|Ident(s)| s.clone())
                .collect::<Vec<_>>()
                .join(".")
        })
        .collect();
    assert_eq!(impls, vec!["Bar", "Baz"]);
}

#[test]
fn errors_on_method_inside_struct() {
    let src = r#"
            struct Bad {
                public int x;
                public int m() { return 0; }
            }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_struct_method.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let _ = parse_file(&sf, &mut r);
    assert!(r.has_errors(), "expected an error for method inside struct");
}

#[test]
fn parses_struct_field_with_generic_type_name() {
    let src = r#"
            struct Boxed { List<String> items; }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_struct_generic_field.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    let st = ast
        .decls
        .iter()
        .find_map(|d| {
            if let Decl::Struct(s) = d {
                Some(s)
            } else {
                None
            }
        })
        .expect("struct not found");
    assert_eq!(st.fields.len(), 1);
    assert_eq!(st.fields[0].name.0, "items");
}

#[test]
fn rejects_impl_block_with_error() {
    let src = r#"
            impl Foo { public int size() { return 0; } }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_impl_block_removed.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let _ = parse_file(&sf, &mut r);
    assert!(r.has_errors());
}

#[test]
fn parses_provider_with_fields() {
    let src = r#"
            provider CacheProvider { private final Map map; public shared int count; }
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_provider.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());
    let prov = ast
        .decls
        .iter()
        .find_map(|d| {
            if let Decl::Provider(p) = d {
                Some(p)
            } else {
                None
            }
        })
        .expect("provider not found");
    assert_eq!(prov.name.0, "CacheProvider");
    assert_eq!(prov.fields.len(), 2);
    assert_eq!(prov.fields[0].name.0, "map");
    assert_eq!(prov.fields[1].name.0, "count");
}

#[test]
fn parses_attributes_and_doc_comments() {
    let src = r#"
            /// Top struct docs
            @deprecated("old")
            struct Boxed {
                /// Field docs
                @inline
                List<String> items;
            }

            /// Function docs
            @test
            public void greet(String name) {}
        "#;
    let sf = SourceFile {
        path: PathBuf::from("test_attrs_docs.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors());

    // Struct docs/attrs
    let st = ast
        .decls
        .iter()
        .find_map(|d| {
            if let Decl::Struct(s) = d {
                Some(s)
            } else {
                None
            }
        })
        .expect("struct not found");
    assert_eq!(st.name.0, "Boxed");
    assert!(st.doc.as_deref().unwrap_or("").contains("Top struct docs"));
    assert_eq!(st.attrs.len(), 1);
    assert_eq!(st.attrs[0].name.path[0].0, "deprecated");
    assert_eq!(st.fields.len(), 1);
    assert!(
        st.fields[0]
            .doc
            .as_deref()
            .unwrap_or("")
            .contains("Field docs")
    );
    assert_eq!(st.fields[0].attrs.len(), 1);
    assert_eq!(st.fields[0].attrs[0].name.path[0].0, "inline");

    // Free function docs/attrs
    let f = ast
        .decls
        .iter()
        .find_map(|d| {
            if let Decl::Function(f) = d {
                Some(f)
            } else {
                None
            }
        })
        .expect("function not found");
    assert_eq!(f.sig.name.0, "greet");
    assert!(f.sig.doc.as_deref().unwrap_or("").contains("Function docs"));
    assert_eq!(f.sig.attrs.len(), 1);
    assert_eq!(f.sig.attrs[0].name.path[0].0, "test");
}

#[test]
fn parses_panic_statement() {
    let src = r#"
        module Test {
            public void crash() {
                panic("something went wrong");
            }
        }
    "#;
    let sf = SourceFile {
        path: PathBuf::from("test_panic.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let ast = parse_file(&sf, &mut r);
    assert!(!r.has_errors(), "parse errors");
    let module = ast
        .decls
        .iter()
        .find_map(|d| {
            if let Decl::Module(m) = d {
                Some(m)
            } else {
                None
            }
        })
        .expect("module not found");
    let func = module.items.first().expect("function not found");
    let body = func.body.as_ref().expect("function body not found");
    assert_eq!(body.stmts.len(), 1);
    match &body.stmts[0] {
        Stmt::Panic(e) => {
            // Verify it's a string expression
            assert!(matches!(e, Expr::Str(_)));
        }
        other => panic!("expected panic statement, got {:?}", other),
    }
}

#[test]
fn parser_error_recovery_stress_keeps_following_modules() {
    let src = r#"
        package demo.recovery;

        struct Bad {
            int x
            int y;
        }

        impl Gone {
            public void nope() {}
        }

        module Good {
            public void main() {
                int a = ;
                if (a < ) { }
                panic();
                int b = 2;
            }
        }

        module Tail {
            public void ping() {}
        }
    "#;
    let sf = SourceFile {
        path: PathBuf::from("test_recovery_stress.arth"),
        text: src.to_string(),
    };
    let mut r = Reporter::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse_file(&sf, &mut r)));
    assert!(result.is_ok(), "parser panicked on recovery stress input");
    let ast = result.expect("parser should return an AST");

    assert!(
        r.has_errors(),
        "malformed input should emit parser diagnostics"
    );

    let modules: Vec<String> = ast
        .decls
        .iter()
        .filter_map(|d| {
            if let Decl::Module(m) = d {
                Some(m.name.0.clone())
            } else {
                None
            }
        })
        .collect();
    assert!(modules.contains(&"Good".to_string()));
    assert!(modules.contains(&"Tail".to_string()));

    let good_main_body = ast
        .decls
        .iter()
        .find_map(|d| {
            if let Decl::Module(m) = d {
                if m.name.0 == "Good" {
                    m.items.iter().find_map(|f| {
                        if f.sig.name.0 == "main" {
                            f.body.clone()
                        } else {
                            None
                        }
                    })
                } else {
                    None
                }
            } else {
                None
            }
        })
        .expect("expected Good.main body");

    assert!(
        good_main_body
            .stmts
            .iter()
            .any(|s| matches!(s, Stmt::VarDecl { name, .. } if name.0 == "b")),
        "parser should recover enough to parse trailing valid statements in the same function"
    );
}
