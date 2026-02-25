use super::*;
use crate::compiler::parser::parse_file;
use crate::compiler::resolve::{ResolvedProgram, resolve_project};
use crate::compiler::source::SourceFile;

fn collect_typecheck_diags(
    code: &str,
) -> Vec<(String, Option<String>, String, Option<(usize, usize)>)> {
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/phase2/typeck_determinism.arth"),
        text: code.to_string(),
    };

    let mut parse_reporter = Reporter::new();
    let ast = parse_file(&sf, &mut parse_reporter);
    assert!(
        !parse_reporter.has_errors(),
        "test fixture should be syntactically valid: {:?}",
        parse_reporter
            .diagnostics()
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
    );

    let mut type_reporter = Reporter::new();
    let rp = ResolvedProgram::empty();
    typecheck_project(
        std::path::Path::new("/mem"),
        &[(sf, ast)],
        &rp,
        &mut type_reporter,
    );

    type_reporter
        .diagnostics()
        .iter()
        .map(|d| {
            (
                format!("{:?}", d.severity),
                d.code.clone(),
                d.message.clone(),
                d.span.as_ref().map(|s| (s.start, s.end)),
            )
        })
        .collect()
}

#[test]
fn typecheck_diagnostics_are_deterministic_for_same_input() {
    let code = r#"
package phase2.det;

module M {
    void main() {
        int x = "oops";
        unknownVar = 1;
        int y;
        println(y);
    }
}
"#;

    let first = collect_typecheck_diags(code);
    let second = collect_typecheck_diags(code);

    assert_eq!(
        first, second,
        "typecheck diagnostics should be stable across repeated runs"
    );
}

#[test]
fn typecheck_semantic_errors_do_not_panic() {
    let code = r#"
package phase2.no_panic;

enum Status {
    Ok,
    Err
}

module M {
    void main() {
        Status s = Status.Ok;
        switch (s) {
            case Status.Ok:
                println("ok");
        }
        int a;
        if (true) {
            a = 1;
        }
        println(a);
        panic(42);
    }
}
"#;

    let result = std::panic::catch_unwind(|| collect_typecheck_diags(code));
    assert!(
        result.is_ok(),
        "typecheck should emit diagnostics for invalid programs instead of panicking"
    );
}

#[test]
fn parser_diagnostics_have_primary_span_actionable_message_and_stable_code() {
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/phase2/diag_parser.arth"),
        text: "package phase2.diag module M { public void main() {} }".to_string(),
    };

    let mut r1 = Reporter::new();
    let _ = parse_file(&sf, &mut r1);
    assert!(r1.has_errors(), "expected parser error for missing ';'");

    let d1 = r1
        .diagnostics()
        .iter()
        .find(|d| d.message.contains("expected ';' after package declaration"))
        .expect("expected actionable parser diagnostic");

    assert!(
        d1.span.is_some(),
        "parser diagnostic should have primary span"
    );
    assert!(
        d1.code.is_some(),
        "parser diagnostic should carry a stable code"
    );
    assert!(
        d1.message.contains("expected"),
        "parser message should be actionable"
    );

    // Same input should produce the same diagnostic code.
    let mut r2 = Reporter::new();
    let _ = parse_file(&sf, &mut r2);
    let d2 = r2
        .diagnostics()
        .iter()
        .find(|d| d.message.contains("expected ';' after package declaration"))
        .expect("expected parser diagnostic on second run");

    assert_eq!(
        d1.code, d2.code,
        "diagnostic code should be stable for the same parser error"
    );
}

#[test]
fn typecheck_borrow_diagnostic_includes_suggestion_and_stable_code() {
    let code = r#"
package phase2.diag.borrow;
struct Data { int val; }
module M {
  void main() {
    Data d = Data { val: 0 };
    borrowMut(d);
    d = Data { val: 1 };
  }
}
"#;

    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/phase2/diag/borrow/diag_typeck.arth"),
        text: code.to_string(),
    };

    let run_once = || {
        let mut parse_rep = Reporter::new();
        let ast = parse_file(&sf, &mut parse_rep);
        assert!(
            !parse_rep.has_errors(),
            "fixture parse failed: {:?}",
            parse_rep
                .diagnostics()
                .iter()
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
        );

        let files = vec![(sf.clone(), ast)];
        let mut resolve_rep = Reporter::new();
        let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut resolve_rep);
        assert!(
            !resolve_rep.has_errors(),
            "fixture resolve failed: {:?}",
            resolve_rep
                .diagnostics()
                .iter()
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
        );

        let mut type_rep = Reporter::new();
        typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut type_rep);
        type_rep
            .diagnostics()
            .iter()
            .find(|d| {
                d.message
                    .contains("cannot assign to 'd' while it is exclusively borrowed")
            })
            .expect("expected borrow-assignment diagnostic")
            .clone()
    };

    let d1 = run_once();
    let d2 = run_once();

    assert!(
        d1.suggestion
            .as_ref()
            .map(|s| s.contains("call release(d)"))
            .unwrap_or(false),
        "borrow diagnostic should provide a concrete suggestion when available"
    );
    assert!(
        d1.code.is_some(),
        "borrow diagnostic should have a stable code"
    );
    assert_eq!(
        d1.code, d2.code,
        "same typecheck diagnostic should preserve code stability across runs"
    );
}
