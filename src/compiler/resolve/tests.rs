use super::*;
use crate::compiler::parser::parse_file;

fn sf(path: &str, text: &str) -> SourceFile {
    SourceFile {
        path: PathBuf::from(path),
        text: text.to_string(),
    }
}

fn parse_all(files: &[SourceFile], reporter: &mut Reporter) -> Vec<(SourceFile, FileAst)> {
    files
        .iter()
        .map(|f| {
            let ast = parse_file(f, reporter);
            (f.clone(), ast)
        })
        .collect()
}

#[test]
fn package_dir_matches() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/app/http/Client.arth",
        "package app.http; module Client { public void main() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "expected no errors");
}

#[test]
fn package_dir_mismatch() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/mismatch/Client.arth",
        "package app.http; module Client { public void main() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    assert!(r.has_errors(), "expected errors for package dir mismatch");
}

#[test]
fn import_cycle_detected() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/a/A.arth",
            "package a; import b.*; module A { public void main() {} }",
        ),
        sf(
            "/mem/project/src/b/B.arth",
            "package b; import a.*; module B { public void main() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    assert!(r.has_errors(), "expected cycle error");
}

#[test]
fn stdlib_log_is_resolvable() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/demo/M.arth",
        "package demo; import log.*; module M { public void main() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "expected log.* to resolve via stdlib stubs"
    );
}

#[test]
fn stdlib_math_is_resolvable() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/demo/M.arth",
        "package demo; import math.*; module M { public void main() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "expected math.* to resolve via stdlib stubs"
    );
}

#[test]
fn stdlib_net_http_is_resolvable() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/demo/M.arth",
        "package demo; import net.http.*; module M { public void main() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "expected net.http.* to resolve via stdlib stubs"
    );
}

#[test]
fn stdlib_net_ws_is_resolvable() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/demo/M.arth",
        "package demo; import net.ws.*; module M { public void main() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "expected net.ws.* to resolve via stdlib stubs"
    );
}

#[test]
fn stdlib_net_sse_is_resolvable() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/demo/M.arth",
        "package demo; import net.sse.*; module M { public void main() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "expected net.sse.* to resolve via stdlib stubs"
    );
}

#[test]
fn stdlib_mail_is_resolvable() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/demo/M.arth",
        "package demo; import mail.*; module M { public void main() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "expected mail.* to resolve via stdlib stubs"
    );
}

#[test]
fn stdlib_mail_smtp_is_resolvable() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/demo/M.arth",
        "package demo; import mail.smtp.*; module M { public void main() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "expected mail.smtp.* to resolve via stdlib stubs"
    );
}

#[test]
fn stdlib_mail_imap_is_resolvable() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/demo/M.arth",
        "package demo; import mail.imap.*; module M { public void main() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "expected mail.imap.* to resolve via stdlib stubs"
    );
}

#[test]
fn stdlib_mail_pop3_is_resolvable() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/demo/M.arth",
        "package demo; import mail.pop3.*; module M { public void main() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "expected mail.pop3.* to resolve via stdlib stubs"
    );
}

#[test]
fn redeclare_and_use_before_declare() {
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; module Main { public void main() { int x; int x; x = 1; y = 2; } }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    assert!(r.has_errors(), "expected local scope errors");
}

#[test]
fn import_visibility_specific_private_denied() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/p1/A.arth",
            "package p1; private void privfun() {}",
        ),
        sf(
            "/mem/project/src/p2/B.arth",
            "package p2; import p1.privfun; module M { public void main() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    assert!(r.has_errors(), "expected private import error");
}

#[test]
fn top_level_free_functions_rejected() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/p1/A.arth",
        "package p1; public void freeFunc() {}",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    assert!(r.has_errors(), "expected error for top-level free function");
}

#[test]
fn export_module_visible_from_other_package() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/p1/A.arth",
            "package p1; export module ExportedMod { public void func() {} }",
        ),
        sf(
            "/mem/project/src/p2/B.arth",
            "package p2; import p1.ExportedMod; module M { public void main() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "export module should be visible from other packages"
    );
}

#[test]
fn non_export_module_not_visible_from_other_package() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/p1/A.arth",
            "package p1; module InternalMod { public void func() {} }",
        ),
        sf(
            "/mem/project/src/p2/B.arth",
            "package p2; import p1.InternalMod; module M { public void main() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    assert!(
        r.has_errors(),
        "non-export module should not be visible from other packages"
    );
}

#[test]
fn non_export_module_visible_within_same_top_level_package() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/p1/sub1/A.arth",
            "package p1.sub1; module InternalMod { public void func() {} }",
        ),
        sf(
            "/mem/project/src/p1/sub2/B.arth",
            "package p1.sub2; import p1.sub1.InternalMod; module M { public void main() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "non-export module should be visible within same top-level package"
    );
}

// ===== Package Registry Tests =====

#[test]
fn registry_tracks_file_to_package_mapping() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/app/http/Client.arth",
            "package app.http; module Client { public void get() {} }",
        ),
        sf(
            "/mem/project/src/app/http/Server.arth",
            "package app.http; module Server { public void serve() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    assert!(!r.has_errors());

    // Test file-to-package mapping
    let pkg = rp.registry.get_package_for_file(std::path::Path::new(
        "/mem/project/src/app/http/Client.arth",
    ));
    assert_eq!(pkg, Some("app.http"));
}

#[test]
fn registry_has_package_info() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/demo/Main.arth",
        "package demo; module Main { public void run() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    assert!(!r.has_errors());

    // Test package info exists
    let info = rp.registry.get_package("demo");
    assert!(info.is_some());
    let info = info.unwrap();
    assert_eq!(info.name, "demo");
    assert!(!info.is_stdlib);
    assert_eq!(info.files.len(), 1);
}

#[test]
fn registry_tracks_stdlib_packages() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/demo/Main.arth",
        "package demo; import log.*; module Main { public void run() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    assert!(!r.has_errors());

    // Stdlib packages should be registered
    assert!(rp.registry.has_package("log"));
    let log_info = rp.registry.get_package("log");
    assert!(log_info.is_some());
    assert!(log_info.unwrap().is_stdlib);
}

// ===== Qualified Name Resolution Tests =====

#[test]
fn resolve_qualified_name_single_segment_from_current_package() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/demo/Main.arth",
        "package demo; struct User { } module Main { public void run() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    assert!(!r.has_errors());

    // Single segment should resolve from current package
    let result = resolve_qualified_name(&rp, "demo", None, &["User".to_string()]);
    assert!(result.is_some());
    let resolved = result.unwrap();
    assert_eq!(resolved.package, "demo");
    assert_eq!(resolved.name, "User");
    assert_eq!(resolved.kind, ResolvedKind::Struct);
}

#[test]
fn resolve_qualified_name_multi_segment_cross_package() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/model/User.arth",
            "package model; struct User { }",
        ),
        sf(
            "/mem/project/src/app/Main.arth",
            "package app; module Main { public void run() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    assert!(!r.has_errors());

    // Qualified name should resolve cross-package
    let result =
        resolve_qualified_name(&rp, "app", None, &["model".to_string(), "User".to_string()]);
    assert!(result.is_some());
    let resolved = result.unwrap();
    assert_eq!(resolved.package, "model");
    assert_eq!(resolved.name, "User");
    assert_eq!(resolved.kind, ResolvedKind::Struct);
}

#[test]
fn resolve_qualified_name_from_imports() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/model/User.arth",
            "package model; struct User { }",
        ),
        sf(
            "/mem/project/src/app/Main.arth",
            "package app; import model.User; module Main { public void run() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    assert!(!r.has_errors());

    // Single segment should resolve via imports
    let file_path = PathBuf::from("/mem/project/src/app/Main.arth");
    let result = resolve_qualified_name(&rp, "app", Some(&file_path), &["User".to_string()]);
    assert!(result.is_some());
    let resolved = result.unwrap();
    assert_eq!(resolved.package, "model");
    assert_eq!(resolved.name, "User");
}

#[test]
fn resolve_qualified_name_visibility_enforced() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/core/Impl.arth",
            "package core; module InternalMod { public void work() {} }",
        ),
        sf(
            "/mem/project/src/other/Main.arth",
            "package other; module Main { public void run() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors());

    // Internal module should not be visible from different top-level package
    let result = resolve_qualified_name(
        &rp,
        "other",
        None,
        &["core".to_string(), "InternalMod".to_string()],
    );
    assert!(result.is_none(), "internal module should not be visible");
}

#[test]
fn file_imports_tracked() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/utils/StringUtil.arth",
            "package utils; export module StringUtil { public void trim() {} }",
        ),
        sf(
            "/mem/project/src/app/Main.arth",
            "package app; import utils.StringUtil; module Main { public void run() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    assert!(!r.has_errors());

    // File imports should be tracked
    let file_path = PathBuf::from("/mem/project/src/app/Main.arth");
    let imports = rp.file_imports.get(&file_path);
    assert!(imports.is_some());
    let imports = imports.unwrap();
    assert!(imports.symbols.contains_key("StringUtil"));
    let (pkg, _original_name, _kind) = imports.symbols.get("StringUtil").unwrap();
    assert_eq!(pkg, "utils");
}

#[test]
fn nested_package_resolution() {
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/app/http/client/Client.arth",
            "package app.http.client; struct HttpClient { }",
        ),
        sf(
            "/mem/project/src/app/Main.arth",
            "package app; module Main { public void run() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    assert!(!r.has_errors());

    // Deep nested package should resolve
    let result = resolve_qualified_name(
        &rp,
        "app",
        None,
        &[
            "app".to_string(),
            "http".to_string(),
            "client".to_string(),
            "HttpClient".to_string(),
        ],
    );
    assert!(result.is_some());
    let resolved = result.unwrap();
    assert_eq!(resolved.package, "app.http.client");
    assert_eq!(resolved.name, "HttpClient");
}

#[test]
fn import_aliasing_basic() {
    // Test: import model.User as U; should allow using U to reference User
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/model/User.arth",
            "package model; struct User { int id; }",
        ),
        sf(
            "/mem/project/src/app/Main.arth",
            "package app; import model.User as U; module Main { public void run() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "import alias should parse and resolve");

    // The alias "U" should be in the imports, pointing to model.User
    let file_path = PathBuf::from("/mem/project/src/app/Main.arth");
    let result = resolve_qualified_name(&rp, "app", Some(&file_path), &["U".to_string()]);
    assert!(result.is_some(), "alias U should resolve");
    let resolved = result.unwrap();
    assert_eq!(resolved.package, "model");
    assert_eq!(resolved.name, "User");
    assert_eq!(resolved.kind, ResolvedKind::Struct);
}

#[test]
fn import_aliasing_module() {
    // Test: import model.UserFns as UF; should allow using UF to reference UserFns
    // Note: 'export module' makes it publicly visible from other packages
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/model/User.arth",
            "package model; export module UserFns { public void create() {} }",
        ),
        sf(
            "/mem/project/src/app/Main.arth",
            "package app; import model.UserFns as UF; module Main { public void run() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "import module alias should parse and resolve"
    );

    // The alias "UF" should be in the imports, pointing to model.UserFns
    let file_path = PathBuf::from("/mem/project/src/app/Main.arth");
    let result = resolve_qualified_name(&rp, "app", Some(&file_path), &["UF".to_string()]);
    assert!(result.is_some(), "alias UF should resolve");
    let resolved = result.unwrap();
    assert_eq!(resolved.package, "model");
    assert_eq!(resolved.name, "UserFns");
    assert_eq!(resolved.kind, ResolvedKind::Module);
}

#[test]
fn import_aliasing_conflict_detection() {
    // Test: importing the same symbol under two different aliases should work
    // But importing two different symbols with the same alias should conflict
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/model/User.arth",
            "package model; struct User { }",
        ),
        sf(
            "/mem/project/src/data/Account.arth",
            "package data; struct Account { }",
        ),
        sf(
            "/mem/project/src/app/Main.arth",
            "package app; import model.User as T; import data.Account as T; module Main { public void run() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    // Should have conflict error because both are imported as T
    assert!(r.has_errors(), "import alias conflict should be detected");
}

#[test]
fn star_import_conflict_detection_for_same_symbol_name() {
    // Test: importing two packages with the same symbol name via star imports must error.
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/model/User.arth",
            "package model; public struct User { }",
        ),
        sf(
            "/mem/project/src/data/User.arth",
            "package data; public struct User { }",
        ),
        sf(
            "/mem/project/src/app/Main.arth",
            "package app; import model.*; import data.*; module Main { public void run() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);

    assert!(
        r.has_errors(),
        "star import conflict for duplicate symbol names should be reported"
    );
    assert!(
        r.diagnostics()
            .iter()
            .any(|d| d.message.contains("import conflict")),
        "expected import conflict diagnostic for ambiguous star imports"
    );
}

// ============================================================================
// Closure Capture Tests
// ============================================================================

#[test]
fn lambda_capture_valid_local_ok() {
    // Test that a lambda capturing a valid local variable is accepted
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; module Main { public void run() { int x = 10; fn(int y) { return x + y; }; } }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "valid lambda capture should be accepted");
}

#[test]
fn lambda_capture_undefined_variable_error() {
    // Test that a lambda capturing an undefined variable is rejected
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; module Main { public void run() { fn(int y) { return undefined_var + y; }; } }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    assert!(
        r.has_errors(),
        "lambda capturing undefined variable should error"
    );
}

#[test]
fn lambda_capture_multiple_variables_ok() {
    // Test that a lambda capturing multiple valid variables is accepted
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; module Main { public void run() { int a = 1; int b = 2; String c = \"hello\"; fn(int x) { return a + b + x; }; } }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "capturing multiple valid variables should be ok"
    );
}

#[test]
fn lambda_capture_parameter_ok() {
    // Test that a lambda capturing a function parameter is accepted
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; module Main { public void run(int multiplier) { fn(int x) { return x * multiplier; }; } }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "capturing function parameter should be ok");
}

#[test]
fn lambda_uses_own_parameter_not_capture() {
    // Test that lambda using its own parameter is not considered a capture
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; module Main { public void run() { fn(int x) { return x * 2; }; } }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "lambda using its own parameter should be ok"
    );
}

#[test]
fn lambda_capture_from_enclosing_block_ok() {
    // Test that a lambda can capture from an enclosing block scope
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; module Main { public void run() { { int outer = 5; fn(int x) { return x + outer; }; } } }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "capturing from enclosing block should be ok"
    );
}

#[test]
fn nested_lambda_capture_outer_ok() {
    // Test that nested lambdas can capture from their respective enclosing scopes
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; module Main { public void run() { int a = 1; fn(int x) { fn(int y) { return a + x + y; }; }; } }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "nested lambda captures should be ok");
}

#[test]
fn lambda_capture_shadowed_variable_uses_inner() {
    // Test that a lambda captures the correct (innermost) shadowed variable
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; module Main { public void run() { int x = 1; { int x = 2; fn(int y) { return x + y; }; } } }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "shadowed variable capture should be ok");
}

#[test]
fn lambda_capture_in_if_block_ok() {
    // Test that lambdas in if blocks can capture outer variables
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; module Main { public void run() { int outer = 5; if (true) { fn(int x) { return x + outer; }; } } }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "capture from if block should be ok");
}

#[test]
fn lambda_capture_in_while_loop_ok() {
    // Test that lambdas in while loops can capture outer variables
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; module Main { public void run() { int count = 0; while (count < 10) { fn(int x) { return x + count; }; count = count + 1; } } }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "capture from while loop should be ok");
}

#[test]
fn lambda_capture_for_loop_variable_ok() {
    // Test that lambdas can capture for loop variables
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; module Main { public void run() { for (int i = 0; i < 10; i += 1) { fn(int x) { return x + i; }; } } }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "capture of for loop variable should be ok");
}

// ============================================================================
// Interface Conformance Tests
// ============================================================================

#[test]
fn implements_valid_interface_ok() {
    // Test that implementing a valid interface is accepted
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        r#"package M;
            interface Display { String show(); }
            module MyFns implements Display {
                public String show(int x) { return "value"; }
            }"#,
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "implementing valid interface should succeed"
    );
}

#[test]
fn implements_unknown_interface_error() {
    // Test that implementing an unknown interface is rejected
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; module Fns implements UnknownInterface {}",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    assert!(r.has_errors(), "implementing unknown interface should fail");
}

#[test]
fn implements_struct_instead_of_interface_error() {
    // Test that implementing a struct (not an interface) is rejected
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        "package M; struct Foo {} module Fns implements Foo {}",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    assert!(r.has_errors(), "implementing a struct should fail");
}

#[test]
fn implements_generic_interface_ok() {
    // Test that implementing a generic interface with type args is accepted
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        r#"package M;
        interface Container<T> { T get(); }
        struct Box {}
        module BoxFns implements Container<Box> {
            public Box get(Box b) { return b; }
        }"#,
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "implementing generic interface should succeed"
    );
}

#[test]
fn implements_cross_package_interface_ok() {
    // Test that implementing an interface from another package works
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/iface/Reader.arth",
            "package iface; public interface Reader { String read(); }",
        ),
        sf(
            "/mem/project/src/impl/Impl.arth",
            r#"package impl;
            struct File {}
            module FileFns implements iface.Reader {
                public String read(File f) { return ""; }
            }"#,
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "cross-package interface implementation should succeed"
    );
}

#[test]
fn implements_interface_from_unknown_package_error() {
    // Test that implementing an interface from an unknown package is rejected
    let root = PathBuf::from("/mem/project/src");
    let files = vec![sf(
        "/mem/project/src/impl/Impl.arth",
        r#"package impl;
        struct File {}
        module FileFns implements nonexistent.Reader {
            public String read(File f) { return ""; }
        }"#,
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    assert!(
        r.has_errors(),
        "implementing interface from unknown package should fail"
    );
}

#[test]
fn coherence_duplicate_impl_same_package_error() {
    // Test that duplicate implementations in the same package are rejected
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        r#"package M;
        interface Display { String show(); }
        struct Point {}
        module PointFns implements Display {
            public String show(Point p) { return "point"; }
        }
        module PointFns2 implements Display {
            public String show(Point p) { return "point2"; }
        }"#,
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    assert!(
        r.has_errors(),
        "duplicate interface implementations should fail"
    );
}

#[test]
fn coherence_duplicate_impl_cross_package_error() {
    // Test that duplicate implementations across packages are rejected
    let root = PathBuf::from("/mem/project/src");
    let files = vec![
        sf(
            "/mem/project/src/iface/Iface.arth",
            "package iface; public interface Display { String show(); }",
        ),
        sf(
            "/mem/project/src/types/Point.arth",
            "package types; public struct Point {}",
        ),
        sf(
            "/mem/project/src/impl1/Impl1.arth",
            r#"package impl1;
            module PointFns implements iface.Display {
                public String show(types.Point p) { return "impl1"; }
            }"#,
        ),
        sf(
            "/mem/project/src/impl2/Impl2.arth",
            r#"package impl2;
            module PointFns implements iface.Display {
                public String show(types.Point p) { return "impl2"; }
            }"#,
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    assert!(
        r.has_errors(),
        "duplicate implementations across packages should fail"
    );
}

#[test]
fn coherence_different_type_args_ok() {
    // Test that different type arguments for same interface is allowed
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        r#"package M;
        interface Container<T> { T get(); }
        struct Box {}
        struct Bag {}
        module BoxFns implements Container<Box> {
            public Box get(Box b) { return b; }
        }
        module BagFns implements Container<Bag> {
            public Bag get(Bag b) { return b; }
        }"#,
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "different type args should be allowed");
}

#[test]
fn coherence_same_type_args_different_types_ok() {
    // Test that same interface with same type args but different implementing types is OK
    // e.g., Comparable<int> for Point vs Comparable<int> for Line
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        r#"package M;
        interface Comparable<T> { int compare(T other); }
        struct Point {}
        struct Line {}
        module PointFns implements Comparable<int> {
            public int compare(Point p, int other) { return 0; }
        }
        module LineFns implements Comparable<int> {
            public int compare(Line l, int other) { return 0; }
        }"#,
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "same interface for different types should be allowed"
    );
}

#[test]
fn implements_multiple_interfaces_ok() {
    // Test that a module can implement multiple interfaces
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/M.arth",
        r#"package M;
        interface Display { String show(); }
        interface Debug { String debug(); }
        struct Point {}
        module PointFns implements Display, Debug {
            public String show(Point p) { return "show"; }
            public String debug(Point p) { return "debug"; }
        }"#,
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let _rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(
        !r.has_errors(),
        "implementing multiple interfaces should succeed"
    );
}

// =============================================================================
// Module Initialization Order Tests
// =============================================================================

#[test]
fn module_init_order_single_package() {
    // A single package with no dependencies has itself in the init order
    let root = PathBuf::from("/mem/project");
    let files = vec![sf(
        "/mem/project/A/M.arth",
        "package A; module M { public void main() {} }",
    )];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "expected no errors");
    assert!(
        rp.module_init_order.init_order.contains(&"A".to_string()),
        "init order should contain package A"
    );
}

#[test]
fn module_init_order_linear_chain() {
    // A imports B, B imports C => init order is [C, B, A]
    let root = PathBuf::from("/mem/project");
    let files = vec![
        sf(
            "/mem/project/C/M.arth",
            "package C; module M { public void helper() {} }",
        ),
        sf(
            "/mem/project/B/M.arth",
            "package B; import C.*; module M { public void use() {} }",
        ),
        sf(
            "/mem/project/A/M.arth",
            "package A; import B.*; module M { public void main() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "expected no errors");

    let order = &rp.module_init_order.init_order;
    // Order includes stdlib packages, so just check our packages are present
    assert!(order.contains(&"A".to_string()), "should contain A");
    assert!(order.contains(&"B".to_string()), "should contain B");
    assert!(order.contains(&"C".to_string()), "should contain C");

    // C must come before B, B must come before A
    let pos_c = order.iter().position(|p| p == "C").unwrap();
    let pos_b = order.iter().position(|p| p == "B").unwrap();
    let pos_a = order.iter().position(|p| p == "A").unwrap();

    assert!(
        pos_c < pos_b,
        "C should be initialized before B (C imports nothing, B imports C)"
    );
    assert!(
        pos_b < pos_a,
        "B should be initialized before A (A imports B)"
    );
}

#[test]
fn module_init_order_diamond_dependency() {
    // A imports B and C, both B and C import D
    // => D must come first, then B and C (in some order), then A last
    let root = PathBuf::from("/mem/project");
    let files = vec![
        sf(
            "/mem/project/D/M.arth",
            "package D; module M { public void base() {} }",
        ),
        sf(
            "/mem/project/B/M.arth",
            "package B; import D.*; module M { public void b() {} }",
        ),
        sf(
            "/mem/project/C/M.arth",
            "package C; import D.*; module M { public void c() {} }",
        ),
        sf(
            "/mem/project/A/M.arth",
            "package A; import B.*; import C.*; module M { public void main() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "expected no errors");

    let order = &rp.module_init_order.init_order;
    // Order includes stdlib packages, so just check our packages are present
    assert!(order.contains(&"A".to_string()), "should contain A");
    assert!(order.contains(&"B".to_string()), "should contain B");
    assert!(order.contains(&"C".to_string()), "should contain C");
    assert!(order.contains(&"D".to_string()), "should contain D");

    let pos_d = order.iter().position(|p| p == "D").unwrap();
    let pos_b = order.iter().position(|p| p == "B").unwrap();
    let pos_c = order.iter().position(|p| p == "C").unwrap();
    let pos_a = order.iter().position(|p| p == "A").unwrap();

    // D must come before B and C
    assert!(pos_d < pos_b, "D should be initialized before B");
    assert!(pos_d < pos_c, "D should be initialized before C");
    // B and C must come before A
    assert!(pos_b < pos_a, "B should be initialized before A");
    assert!(pos_c < pos_a, "C should be initialized before A");
}

#[test]
fn module_init_order_independent_packages() {
    // Two independent packages have no ordering constraint between them
    let root = PathBuf::from("/mem/project");
    let files = vec![
        sf(
            "/mem/project/X/M.arth",
            "package X; module M { public void x() {} }",
        ),
        sf(
            "/mem/project/Y/M.arth",
            "package Y; module M { public void y() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "expected no errors");

    let order = &rp.module_init_order.init_order;
    // Order includes stdlib packages, so just check our packages are present
    assert!(order.contains(&"X".to_string()), "should contain X");
    assert!(order.contains(&"Y".to_string()), "should contain Y");
}

#[test]
fn module_init_order_deinit_is_reverse_of_init() {
    // deinit order should be the reverse of init order
    let root = PathBuf::from("/mem/project");
    let files = vec![
        sf(
            "/mem/project/C/M.arth",
            "package C; module M { public void helper() {} }",
        ),
        sf(
            "/mem/project/B/M.arth",
            "package B; import C.*; module M { public void use() {} }",
        ),
        sf(
            "/mem/project/A/M.arth",
            "package A; import B.*; module M { public void main() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "expected no errors");

    let init_order = &rp.module_init_order.init_order;
    let deinit_order = &rp.module_init_order.deinit_order;

    // deinit should be reverse of init
    let reversed: Vec<String> = init_order.iter().rev().cloned().collect();
    assert_eq!(
        *deinit_order, reversed,
        "deinit order should be reverse of init order"
    );
}

#[test]
fn module_init_order_dependencies_tracked() {
    // Verify that the dependencies map is correctly populated
    let root = PathBuf::from("/mem/project");
    let files = vec![
        sf(
            "/mem/project/Base/M.arth",
            "package Base; module M { public void base() {} }",
        ),
        sf(
            "/mem/project/App/M.arth",
            "package App; import Base.*; module M { public void main() {} }",
        ),
    ];
    let mut r = Reporter::new();
    let fas = parse_all(&files, &mut r);
    let rp = resolve_project(&root, &fas, &mut r);
    if r.has_errors() {
        r.drain_to_stderr();
    }
    assert!(!r.has_errors(), "expected no errors");

    // App should depend on Base
    let deps = &rp.module_init_order.dependencies;
    let app_deps = deps.get("App");
    assert!(app_deps.is_some(), "App should have dependencies tracked");
    assert!(
        app_deps.unwrap().contains("Base"),
        "App should depend on Base"
    );

    // Base should have no dependencies (or empty set)
    let base_deps = deps.get("Base");
    if let Some(bd) = base_deps {
        assert!(bd.is_empty(), "Base should have no dependencies");
    }
    // Note: Base might not be in the map at all if it has no imports, which is also valid
}
