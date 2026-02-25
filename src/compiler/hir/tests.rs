use super::*;
use crate::compiler::diagnostics::Reporter;
use crate::compiler::parser::parse_file;
use crate::compiler::source::SourceFile;

#[test]
fn golden_dump_module_and_func() {
    let code = r#"
package demo.app;

module Main {
  public void main() { }
}
"#;
    let sf = SourceFile {
        path: PathBuf::from("/mem/demo/Main.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let pkg = ast.package.as_ref().map(|p| p.to_string());
    let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);
    let dump = dump_hir(&hir);
    let expected = "hir-file: /mem/demo/Main.arth\npackage: demo.app\nsource_language: arth\nguest: no\ndecls:\n- module Main\n  - func main()\n".to_string();
    assert_eq!(dump, expected);
}

#[test]
fn golden_dump_all_decl_kinds() {
    // Tests all declaration kinds - functions now live inside modules
    let code = r#"
package demo.all;

struct Point { int x; }

interface Service { int get(int id); }

enum Status { OK, Value(int) }

provider Prov { int cap; }

module M {
  public void main() {}
  int id() {}
}

module MathFns {
  int add(int a, int b) {}
}
"#;
    let sf = SourceFile {
        path: PathBuf::from("/mem/demo/all.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors(), "unexpected parse errors");
    let pkg = ast.package.as_ref().map(|p| p.to_string());
    let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);
    let dump = dump_hir(&hir);
    let expected = [
        "hir-file: /mem/demo/all.arth",
        "package: demo.all",
        "source_language: arth",
        "guest: no",
        "decls:",
        "- struct Point",
        "- interface Service",
        "  - sig get(id)",
        "- enum Status",
        "  - OK",
        "  - Value(..)",
        "- provider Prov",
        "- module M",
        "  - func main()",
        "  - func id()",
        "- module MathFns",
        "  - func add(a, b)",
        "",
    ]
    .join("\n");
    assert_eq!(dump, expected);
}

#[test]
fn golden_dump_with_docs_and_attrs_stable() {
    let code = r#"
package demo.attrs;

/// Point docs
@Serializable
struct Point {
  /// field docs
  @Json(name="x")
  int x;
}

module M {
  /// main docs
  @Entry
  public void main() {}
}
"#;
    let sf = SourceFile {
        path: PathBuf::from("/mem/demo/attrs.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let pkg = ast.package.as_ref().map(|p| p.to_string());
    let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);
    // Dump intentionally omits docs/attrs; ensure names remain correct/stable
    let dump = dump_hir(&hir);
    let expected = [
        "hir-file: /mem/demo/attrs.arth",
        "package: demo.attrs",
        "source_language: arth",
        "guest: no",
        "decls:",
        "- struct Point",
        "- module M",
        "  - func main()",
        "",
    ]
    .join("\n");
    assert_eq!(dump, expected);
}

#[test]
fn lowering_for_desugars_to_block_while() {
    let code = r#"
package demo.ctrl;

module M {
  void main() {
    for (i = 0; i < 10; i += 1) {
      println("x");
    }
  }
}
"#;
    let sf = SourceFile {
        path: PathBuf::from("/mem/demo/ctrl_for.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let pkg = ast.package.as_ref().map(|p| p.to_string());
    let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);
    // Navigate to module M.main body
    let m = match &hir.decls[0] {
        HirDecl::Module(m) => m,
        _ => panic!("expected module"),
    };
    let f = m
        .funcs
        .iter()
        .find(|f| f.sig.name == "main")
        .expect("main not found");
    let body = f.body.as_ref().expect("body missing");
    // Expect body stmts: one Block (the lowered 'for')
    assert_eq!(body.stmts.len(), 1);
    match &body.stmts[0] {
        HirStmt::Block(b) => {
            // Inside desugared block: optional init assign, then While
            assert!(!b.stmts.is_empty());
            match &b.stmts[0] {
                HirStmt::Assign { name, .. } => assert_eq!(name, "i"),
                _ => panic!("expected init assignment in for lowering"),
            }
            match &b.stmts[1] {
                HirStmt::While { cond, body, .. } => {
                    // Cond should be a binary or bool
                    match cond {
                        HirExpr::Binary { .. } | HirExpr::Bool { .. } => {}
                        _ => panic!("unexpected cond expr"),
                    }
                    // Body should end with the step (AssignOp)
                    assert!(body.stmts.len() >= 2);
                    match body.stmts.last().unwrap() {
                        HirStmt::AssignOp { name, .. } => assert_eq!(name, "i"),
                        _ => panic!("expected step as last stmt in while body"),
                    }
                }
                _ => panic!("expected while in for lowering"),
            }
        }
        _ => panic!("expected for to lower to a Block"),
    }
}

#[test]
fn lowering_switch_collects_cases_and_default() {
    let code = r#"
package demo.ctrl;

module M {
  void main() {
    switch (x) {
      case 1: { println("a"); }
      default: { println("d"); }
    }
  }
}
"#;
    let sf = SourceFile {
        path: PathBuf::from("/mem/demo/ctrl_switch.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let pkg = ast.package.as_ref().map(|p| p.to_string());
    let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);
    let m = match &hir.decls[0] {
        HirDecl::Module(m) => m,
        _ => panic!("expected module"),
    };
    let f = m
        .funcs
        .iter()
        .find(|f| f.sig.name == "main")
        .expect("main not found");
    let body = f.body.as_ref().expect("body missing");
    // Find the first Switch statement in body
    let sw = body
        .stmts
        .iter()
        .find(|s| matches!(s, HirStmt::Switch { .. }));
    match sw.expect("switch not found in body") {
        HirStmt::Switch { cases, default, .. } => {
            assert_eq!(cases.len(), 1);
            assert!(default.is_some());
        }
        _ => panic!("expected a switch statement"),
    }
}

#[test]
fn lowering_else_if_nests_in_else_block() {
    let code = r#"
package demo.ctrl;

module M {
  void main() {
    if (a) { println("a"); } else if (b) { println("b"); } else { println("c"); }
  }
}
"#;
    let sf = SourceFile {
        path: PathBuf::from("/mem/demo/ctrl_if.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let pkg = ast.package.as_ref().map(|p| p.to_string());
    let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);
    let m = match &hir.decls[0] {
        HirDecl::Module(m) => m,
        _ => panic!("expected module"),
    };
    let f = m
        .funcs
        .iter()
        .find(|f| f.sig.name == "main")
        .expect("main not found");
    let body = f.body.as_ref().expect("body missing");
    assert_eq!(body.stmts.len(), 1);
    match &body.stmts[0] {
        HirStmt::If { else_blk, .. } => {
            let Some(else_blk) = else_blk else {
                panic!("expected else block")
            };
            // Else block should contain a nested If
            assert_eq!(else_blk.stmts.len(), 1);
            match &else_blk.stmts[0] {
                HirStmt::If { .. } => {}
                _ => panic!("expected nested if"),
            }
        }
        _ => panic!("expected an if statement"),
    }
}

#[test]
fn span_roundtrip_sig_and_body_blocks() {
    let code = r#"
package demo.spans;

module M {
  public void main() {
    println("hello");
  }
}
"#;
    let sf = SourceFile {
        path: PathBuf::from("/mem/demo/spans.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let pkg = ast.package.as_ref().map(|p| p.to_string());
    let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);
    let m = match &hir.decls[0] {
        HirDecl::Module(m) => m,
        _ => panic!("expected module"),
    };
    let f = m
        .funcs
        .iter()
        .find(|f| f.sig.name == "main")
        .expect("main not found");
    // Function sig span should map to a slice containing the function name
    let sspan = f.sig.span.as_ref().expect("sig span missing");
    assert_eq!(sspan.file.as_ref(), &sf.path);
    let slice = &sf.text[sspan.start as usize..sspan.end as usize];
    assert!(
        slice.contains("main"),
        "sig slice missing function name: {:?}",
        slice
    );
    // Body block span covers braces
    let bspan = &f.body.as_ref().expect("body").span;
    let bslice = &sf.text[bspan.start as usize..bspan.end as usize];
    let trimmed = bslice.trim();
    assert!(
        trimmed.starts_with('{') && trimmed.ends_with('}'),
        "body slice should cover braces: {:?}",
        trimmed
    );
}

#[test]
fn lambda_basic_no_captures() {
    let code = r#"
package demo.lambda;

module M {
  void main() {
    fn(int x) { return x * 2; }
  }
}
"#;
    let sf = SourceFile {
        path: PathBuf::from("/mem/demo/lambda.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors(), "parse errors detected");
    let pkg = ast.package.as_ref().map(|p| p.to_string());
    let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);

    // Navigate to the lambda expression
    let m = match &hir.decls[0] {
        HirDecl::Module(m) => m,
        _ => panic!("expected module"),
    };
    let f = m
        .funcs
        .iter()
        .find(|f| f.sig.name == "main")
        .expect("main not found");
    let body = f.body.as_ref().expect("body missing");

    // Find the lambda in the expression statement
    match &body.stmts[0] {
        HirStmt::Expr { expr, .. } => {
            match expr {
                HirExpr::Lambda {
                    params, captures, ..
                } => {
                    assert_eq!(params.len(), 1);
                    assert_eq!(params[0].name, "x");
                    // Lambda uses only its parameter, so no captures
                    assert_eq!(
                        captures.len(),
                        0,
                        "expected no captures, got: {:?}",
                        captures
                    );
                }
                _ => panic!("expected lambda expression, got: {:?}", expr),
            }
        }
        _ => panic!("expected expression statement"),
    }
}

#[test]
fn lambda_captures_enclosing_variable() {
    let code = r#"
package demo.lambda;

module M {
  void main() {
    int multiplier = 5;
    fn(int x) { return x * multiplier; }
  }
}
"#;
    let sf = SourceFile {
        path: PathBuf::from("/mem/demo/lambda_closure.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors(), "parse errors detected");
    let pkg = ast.package.as_ref().map(|p| p.to_string());
    let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);

    let m = match &hir.decls[0] {
        HirDecl::Module(m) => m,
        _ => panic!("expected module"),
    };
    let f = m
        .funcs
        .iter()
        .find(|f| f.sig.name == "main")
        .expect("main not found");
    let body = f.body.as_ref().expect("body missing");

    // Second statement should be the lambda
    match &body.stmts[1] {
        HirStmt::Expr { expr, .. } => {
            match expr {
                HirExpr::Lambda {
                    params, captures, ..
                } => {
                    assert_eq!(params.len(), 1);
                    assert_eq!(params[0].name, "x");
                    // Lambda should capture 'multiplier'
                    assert_eq!(captures.len(), 1, "expected 1 capture");
                    assert_eq!(
                        captures[0].0, "multiplier",
                        "expected to capture 'multiplier'"
                    );
                }
                _ => panic!("expected lambda expression"),
            }
        }
        _ => panic!("expected expression statement with lambda"),
    }
}

#[test]
fn lambda_captures_multiple_variables() {
    let code = r#"
package demo.lambda;

module M {
  void main() {
    int a = 1;
    int b = 2;
    int c = 3;
    fn(int x) { return x + a + b; }
  }
}
"#;
    let sf = SourceFile {
        path: PathBuf::from("/mem/demo/lambda_multi.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors(), "parse errors detected");
    let pkg = ast.package.as_ref().map(|p| p.to_string());
    let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);

    let m = match &hir.decls[0] {
        HirDecl::Module(m) => m,
        _ => panic!("expected module"),
    };
    let f = m
        .funcs
        .iter()
        .find(|f| f.sig.name == "main")
        .expect("main not found");
    let body = f.body.as_ref().expect("body missing");

    // Fourth statement should be the lambda
    match &body.stmts[3] {
        HirStmt::Expr { expr, .. } => {
            match expr {
                HirExpr::Lambda { captures, .. } => {
                    // Lambda should capture 'a' and 'b' but not 'c' (not used)
                    assert_eq!(
                        captures.len(),
                        2,
                        "expected 2 captures, got: {:?}",
                        captures
                    );
                    let captured_names: Vec<&str> =
                        captures.iter().map(|(n, _)| n.as_str()).collect();
                    assert!(captured_names.contains(&"a"), "expected to capture 'a'");
                    assert!(captured_names.contains(&"b"), "expected to capture 'b'");
                    assert!(
                        !captured_names.contains(&"c"),
                        "'c' should not be captured (unused)"
                    );
                }
                _ => panic!("expected lambda expression"),
            }
        }
        _ => panic!("expected expression statement with lambda"),
    }
}

#[test]
fn lambda_nested_does_not_capture_from_inner_scope() {
    let code = r#"
package demo.lambda;

module M {
  void main() {
    fn(int x) {
      int local = 10;
      return x + local;
    }
  }
}
"#;
    let sf = SourceFile {
        path: PathBuf::from("/mem/demo/lambda_local.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors(), "parse errors detected");
    let pkg = ast.package.as_ref().map(|p| p.to_string());
    let hir = make_hir_file(sf.path.clone(), pkg, &ast.decls);

    let m = match &hir.decls[0] {
        HirDecl::Module(m) => m,
        _ => panic!("expected module"),
    };
    let f = m
        .funcs
        .iter()
        .find(|f| f.sig.name == "main")
        .expect("main not found");
    let body = f.body.as_ref().expect("body missing");

    match &body.stmts[0] {
        HirStmt::Expr { expr, .. } => {
            match expr {
                HirExpr::Lambda {
                    params, captures, ..
                } => {
                    assert_eq!(params.len(), 1);
                    // Lambda declares 'local' inside its body, so it shouldn't be captured
                    assert_eq!(captures.len(), 0, "local variables should not be captured");
                }
                _ => panic!("expected lambda expression"),
            }
        }
        _ => panic!("expected expression statement"),
    }
}
