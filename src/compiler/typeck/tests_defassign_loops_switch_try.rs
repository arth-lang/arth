use crate::compiler::diagnostics::Reporter;
use crate::compiler::parser::parse_file;
use crate::compiler::resolve::resolve_project;
use crate::compiler::source::SourceFile;
use crate::compiler::typeck::typecheck_project;

#[test]
fn definite_assignment_for_loop_init_and_body() {
    let code = r#"
package defassign;

module M {
    void main() {
        int x;
        for (x = 0; x < 10; x = x + 1) {
            // x is initialized in init and may be reassigned in body
        }
        println(x); // OK: x initialized in for-init
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/for_ok.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    if r2.has_errors() {
        r2.drain_to_stderr();
    }
    assert!(!r2.has_errors());
}

#[test]
fn definite_assignment_for_loop_body_only_is_error() {
    let code = r#"
package defassign;

module M {
    void main() {
        int x;
        int i = 0;
        for (; i < 10; i = i + 1) {
            x = 1;
        }
        println(x); // ERROR: x may not be initialized if loop body never runs
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/for_err.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    assert!(r2.has_errors());
    assert!(
        r2.diagnostics()
            .iter()
            .any(|d| d.message.contains("before definite initialization"))
    );
}

#[test]
fn definite_assignment_switch_with_default_all_branches_ok() {
    let code = r#"
package defassign;

module M {
    void main() {
        int x;
        int tag = 1;
        switch (tag) {
            case 0: {
                x = 1;
            }
            default: {
                x = 2;
            }
        }
        println(x); // OK: all switch paths initialize x
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/switch_ok.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    if r2.has_errors() {
        r2.drain_to_stderr();
    }
    assert!(!r2.has_errors());
}

#[test]
fn definite_assignment_switch_missing_default_is_error() {
    let code = r#"
package defassign;

module M {
    void main() {
        int x;
        int tag = 1;
        switch (tag) {
            case 0: {
                x = 1;
            }
        }
        println(x); // ERROR: tag may not match any case, x not definitely initialized
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/switch_err.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    assert!(r2.has_errors());
    assert!(
        r2.diagnostics()
            .iter()
            .any(|d| d.message.contains("before definite initialization"))
    );
}

#[test]
fn definite_assignment_try_catch_all_paths_ok() {
    let code = r#"
package defassign;

struct Error {}

module M {
    void mayThrow(bool flag) throws (Error) {
        if (flag) {
            throw Error {};
        }
    }

    void main() throws (Error) {
        int x;
        try {
            mayThrow(false);
            x = 1;
        } catch (Error e) {
            x = 2;
        }
        println(x); // OK: x initialized in both try-normal and catch paths
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/trycatch_ok.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    if r2.has_errors() {
        r2.drain_to_stderr();
    }
    assert!(!r2.has_errors());
}

#[test]
fn definite_assignment_try_catch_missing_init_is_error() {
    let code = r#"
package defassign;

struct Error {}

module M {
    void mayThrow(bool flag) throws (Error) {
        if (flag) {
            throw Error {};
        }
    }

    void main() throws (Error) {
        int x;
        try {
            mayThrow(false);
            // x NOT initialized on normal path
        } catch (Error e) {
            x = 2;
        }
        println(x); // ERROR: x not definitely initialized on all try/catch paths
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/trycatch_err.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    assert!(r2.has_errors());
    assert!(
        r2.diagnostics()
            .iter()
            .any(|d| d.message.contains("before definite initialization"))
    );
}

// ============================================================================
// Additional Definite Assignment Edge Cases
// ============================================================================

#[test]
fn definite_assignment_while_loop_body_only_is_error() {
    // Variable initialized only inside while loop body should error after loop
    let code = r#"
package defassign;

module M {
    void main() {
        int x;
        int i = 0;
        while (i < 10) {
            x = 1;
            i = i + 1;
        }
        println(x); // ERROR: while body may never execute (loop may be skipped)
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/while_body_err.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    assert!(r2.has_errors());
    assert!(
        r2.diagnostics()
            .iter()
            .any(|d| d.message.contains("before definite initialization"))
    );
}

#[test]
fn definite_assignment_while_initialized_before_ok() {
    // Variable initialized before while should be usable after
    let code = r#"
package defassign;

module M {
    void main() {
        int x = 42;
        int i = 0;
        while (i < 10) {
            x = x + 1;
            i = i + 1;
        }
        println(x); // OK: x initialized before while
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/while_ok.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    if r2.has_errors() {
        r2.drain_to_stderr();
    }
    assert!(!r2.has_errors());
}

#[test]
fn definite_assignment_nested_loops_ok() {
    // Nested loops with initialization in outer for-init
    let code = r#"
package defassign;

module M {
    void main() {
        int x;
        for (x = 0; x < 5; x = x + 1) {
            int j = 0;
            while (j < 3) {
                j = j + 1;
            }
        }
        println(x); // OK: x initialized in for-init
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/nested_loops_ok.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    if r2.has_errors() {
        r2.drain_to_stderr();
    }
    assert!(!r2.has_errors());
}

#[test]
fn definite_assignment_switch_multiple_cases_all_init_ok() {
    // Switch with multiple cases all initializing variable
    let code = r#"
package defassign;

module M {
    void main() {
        int x;
        int tag = 2;
        switch (tag) {
            case 0: {
                x = 10;
            }
            case 1: {
                x = 20;
            }
            case 2: {
                x = 30;
            }
            default: {
                x = 40;
            }
        }
        println(x); // OK: all cases initialize x
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/switch_multi_ok.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    if r2.has_errors() {
        r2.drain_to_stderr();
    }
    assert!(!r2.has_errors());
}

#[test]
fn definite_assignment_switch_one_case_missing_init_error() {
    // Switch with one case NOT initializing should error
    let code = r#"
package defassign;

module M {
    void main() {
        int x;
        int tag = 2;
        switch (tag) {
            case 0: {
                x = 10;
            }
            case 1: {
                // x NOT initialized in case 1
            }
            default: {
                x = 40;
            }
        }
        println(x); // ERROR: case 1 does not initialize x
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/switch_case_missing_err.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    assert!(r2.has_errors());
    assert!(
        r2.diagnostics()
            .iter()
            .any(|d| d.message.contains("before definite initialization"))
    );
}

#[test]
fn definite_assignment_try_finally_init_ok() {
    // Initialization in finally block should be visible after try
    let code = r#"
package defassign;

struct Error {}

module M {
    void mayThrow(bool flag) throws (Error) {
        if (flag) {
            throw Error {};
        }
    }

    void main() throws (Error) {
        int x;
        try {
            mayThrow(false);
        } catch (Error e) {
            // catch doesn't init x
        } finally {
            x = 99;
        }
        println(x); // OK: finally always runs and initializes x
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/try_finally_ok.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    if r2.has_errors() {
        r2.drain_to_stderr();
    }
    assert!(!r2.has_errors());
}

#[test]
fn definite_assignment_if_else_chain_all_init_ok() {
    // else-if chain with all branches initializing
    let code = r#"
package defassign;

module M {
    void main() {
        int x;
        int n = 5;
        if (n < 0) {
            x = -1;
        } else if (n == 0) {
            x = 0;
        } else if (n < 10) {
            x = 1;
        } else {
            x = 2;
        }
        println(x); // OK: all branches initialize x
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/ifelse_chain_ok.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    if r2.has_errors() {
        r2.drain_to_stderr();
    }
    assert!(!r2.has_errors());
}

#[test]
fn definite_assignment_if_else_chain_one_missing_error() {
    // else-if chain with one branch not initializing
    let code = r#"
package defassign;

module M {
    void main() {
        int x;
        int n = 5;
        if (n < 0) {
            x = -1;
        } else if (n == 0) {
            // x NOT initialized
        } else {
            x = 2;
        }
        println(x); // ERROR: else-if branch does not initialize x
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/ifelse_chain_err.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    assert!(r2.has_errors());
    assert!(
        r2.diagnostics()
            .iter()
            .any(|d| d.message.contains("before definite initialization"))
    );
}

#[test]
fn definite_assignment_multiple_vars_independent() {
    // Multiple variables with independent initialization paths
    let code = r#"
package defassign;

module M {
    void main() {
        int x;
        int y;
        if (true) {
            x = 1;
        } else {
            x = 2;
        }
        // x is definitely init, y is not
        println(x); // OK

        if (false) {
            y = 10;
        } else {
            y = 20;
        }
        println(y); // OK: y now definitely init
    }
}
"#;
    let sf = SourceFile {
        path: std::path::PathBuf::from("/mem/defassign/multi_vars.arth"),
        text: code.to_string(),
    };
    let mut rep = Reporter::new();
    let ast = parse_file(&sf, &mut rep);
    assert!(!rep.has_errors());
    let files = vec![(sf.clone(), ast.clone())];
    let mut rr = Reporter::new();
    let rp = resolve_project(std::path::Path::new("/mem"), &files, &mut rr);
    assert!(!rr.has_errors());
    let mut r2 = Reporter::new();
    typecheck_project(std::path::Path::new("/mem"), &files, &rp, &mut r2);
    if r2.has_errors() {
        r2.drain_to_stderr();
    }
    assert!(!r2.has_errors());
}
