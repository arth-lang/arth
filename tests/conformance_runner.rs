//! Conformance Test Runner
//!
//! This module provides the infrastructure for running conformance tests that validate
//! the Arth compiler against the language specification (docs/spec.md).
//!
//! Conformance tests are organized by specification section:
//! - lexer/: Lexical structure (spec §2)
//! - parser/: Declarations and grammar (spec §4)
//! - types/: Type system and generics (spec §3, §12)
//! - ownership/: Ownership and memory model (spec §8)
//! - exceptions/: Exceptions and error handling (spec §7)
//! - concurrency/: Async/await and concurrency (spec §9)
//! - providers/: Providers and capabilities (spec §11)
//! - modules/: Modules and packages (spec §5, §10)
//! - patterns/: Pattern matching (spec §13)
//!
//! Test file format:
//! ```arth
//! package conformance;
//!
//! // @spec: spec.md §2.3 - Integer literals
//! // @expect: pass | error:<code> | output:<expected>
//! // @description: Brief description of what this test validates
//!
//! module Main {
//!     public static void main() { ... }
//! }
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Backend mode for conformance execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConformanceBackend {
    Vm,
    Llvm,
}

impl ConformanceBackend {
    fn as_label(self) -> &'static str {
        match self {
            ConformanceBackend::Vm => "vm",
            ConformanceBackend::Llvm => "llvm",
        }
    }
}

/// Result of a single conformance test
#[derive(Debug, Clone)]
pub struct ConformanceResult {
    pub name: String,
    pub spec_ref: Option<String>,
    pub status: TestStatus,
    pub duration: Duration,
    pub details: Option<String>,
}

/// Status of a conformance test
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestStatus {
    Pass,
    Fail(String),
    Skip(String),
    Error(String),
}

/// Expected outcome parsed from test file
#[derive(Debug, Clone)]
pub enum Expectation {
    Pass,
    ErrorCode(String),
    Output(Vec<String>),
    CompileError,
}

/// Parsed conformance test metadata
#[derive(Debug)]
pub struct ConformanceTest {
    pub path: PathBuf,
    pub name: String,
    pub spec_ref: Option<String>,
    pub description: Option<String>,
    pub expectation: Expectation,
    pub category: String,
}

/// Parse test file to extract metadata from comments
pub fn parse_test_file(path: &Path) -> Option<ConformanceTest> {
    let content = fs::read_to_string(path).ok()?;
    let name = path.file_stem()?.to_string_lossy().to_string();
    let category = path.parent()?.file_name()?.to_string_lossy().to_string();

    let mut spec_ref = None;
    let mut description = None;
    let mut expectation = Expectation::Pass;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("// @spec:") {
            spec_ref = Some(line.trim_start_matches("// @spec:").trim().to_string());
        } else if line.starts_with("// @expect:") {
            let expect_str = line.trim_start_matches("// @expect:").trim();
            expectation = parse_expectation(expect_str);
        } else if line.starts_with("// @description:") {
            description = Some(
                line.trim_start_matches("// @description:")
                    .trim()
                    .to_string(),
            );
        }
    }

    Some(ConformanceTest {
        path: path.to_path_buf(),
        name,
        spec_ref,
        description,
        expectation,
        category,
    })
}

/// Parse expectation string into Expectation enum
fn parse_expectation(s: &str) -> Expectation {
    if s == "pass" {
        Expectation::Pass
    } else if s == "compile-error" {
        Expectation::CompileError
    } else if let Some(code) = s.strip_prefix("error:") {
        Expectation::ErrorCode(code.trim().to_string())
    } else if let Some(output) = s.strip_prefix("output:") {
        Expectation::Output(output.split('|').map(|s| s.trim().to_string()).collect())
    } else {
        Expectation::Pass
    }
}

/// Discover all conformance tests in a directory
pub fn discover_tests(base_dir: &Path) -> Vec<ConformanceTest> {
    let mut tests = Vec::new();

    if let Ok(entries) = fs::read_dir(base_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                tests.extend(discover_tests(&path));
            } else if path.extension().is_some_and(|e| e == "arth")
                && let Some(test) = parse_test_file(&path)
            {
                tests.push(test);
            }
        }
    }

    tests.sort_by(|a, b| a.name.cmp(&b.name));
    tests
}

/// Run a single conformance test
pub fn run_test(
    test: &ConformanceTest,
    arth_bin: &Path,
    backend: ConformanceBackend,
) -> ConformanceResult {
    let start = Instant::now();

    // First, try to compile/check/build the file
    let test_path = test.path.to_str().unwrap();
    let mut compile_cmd = Command::new(arth_bin);
    compile_cmd.current_dir(env!("CARGO_MANIFEST_DIR"));
    match backend {
        ConformanceBackend::Vm => {
            compile_cmd.args(["check", test_path]);
        }
        ConformanceBackend::Llvm => {
            compile_cmd.args(["build", "--backend", "llvm", test_path]);
        }
    };

    let compile_output = compile_cmd.output();

    let compile_result = match compile_output {
        Ok(output) => output,
        Err(e) => {
            return ConformanceResult {
                name: test.name.clone(),
                spec_ref: test.spec_ref.clone(),
                status: TestStatus::Error(format!(
                    "Failed to run compiler ({backend}): {e}",
                    backend = backend.as_label()
                )),
                duration: start.elapsed(),
                details: None,
            };
        }
    };

    // Handle based on expectation
    match &test.expectation {
        Expectation::CompileError => {
            if !compile_result.status.success() {
                ConformanceResult {
                    name: test.name.clone(),
                    spec_ref: test.spec_ref.clone(),
                    status: TestStatus::Pass,
                    duration: start.elapsed(),
                    details: Some(format!(
                        "Compilation failed as expected ({})",
                        backend.as_label()
                    )),
                }
            } else {
                ConformanceResult {
                    name: test.name.clone(),
                    spec_ref: test.spec_ref.clone(),
                    status: TestStatus::Fail(
                        "Expected compile error, but compilation succeeded".to_string(),
                    ),
                    duration: start.elapsed(),
                    details: None,
                }
            }
        }
        Expectation::ErrorCode(code) => {
            let stderr = String::from_utf8_lossy(&compile_result.stderr);
            if stderr.contains(code) {
                ConformanceResult {
                    name: test.name.clone(),
                    spec_ref: test.spec_ref.clone(),
                    status: TestStatus::Pass,
                    duration: start.elapsed(),
                    details: Some(format!(
                        "Found expected error code '{}' on {}",
                        code,
                        backend.as_label()
                    )),
                }
            } else {
                ConformanceResult {
                    name: test.name.clone(),
                    spec_ref: test.spec_ref.clone(),
                    status: TestStatus::Fail(format!(
                        "Expected error code '{}' not found in: {}",
                        code, stderr
                    )),
                    duration: start.elapsed(),
                    details: None,
                }
            }
        }
        Expectation::Pass | Expectation::Output(_) => {
            if !compile_result.status.success() {
                let stderr = String::from_utf8_lossy(&compile_result.stderr);
                return ConformanceResult {
                    name: test.name.clone(),
                    spec_ref: test.spec_ref.clone(),
                    status: TestStatus::Fail(format!("Compilation failed: {}", stderr)),
                    duration: start.elapsed(),
                    details: None,
                };
            }

            // If we need output verification, run the program
            if let Expectation::Output(expected) = &test.expectation {
                let run_output = match backend {
                    ConformanceBackend::Vm => Command::new(arth_bin)
                        .current_dir(env!("CARGO_MANIFEST_DIR"))
                        .args(["run", test_path])
                        .output(),
                    ConformanceBackend::Llvm => {
                        let native_bin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                            .join("target")
                            .join("arth-out")
                            .join("app");
                        Command::new(native_bin).output()
                    }
                };

                match run_output {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let actual: Vec<&str> = stdout.lines().collect();
                        let expected_refs: Vec<&str> =
                            expected.iter().map(|s| s.as_str()).collect();

                        if actual == expected_refs {
                            ConformanceResult {
                                name: test.name.clone(),
                                spec_ref: test.spec_ref.clone(),
                                status: TestStatus::Pass,
                                duration: start.elapsed(),
                                details: None,
                            }
                        } else {
                            ConformanceResult {
                                name: test.name.clone(),
                                spec_ref: test.spec_ref.clone(),
                                status: TestStatus::Fail(format!(
                                    "Output mismatch:\nExpected: {:?}\nActual: {:?}",
                                    expected_refs, actual
                                )),
                                duration: start.elapsed(),
                                details: None,
                            }
                        }
                    }
                    Err(e) => ConformanceResult {
                        name: test.name.clone(),
                        spec_ref: test.spec_ref.clone(),
                        status: TestStatus::Error(format!("Failed to run program: {}", e)),
                        duration: start.elapsed(),
                        details: None,
                    },
                }
            } else {
                ConformanceResult {
                    name: test.name.clone(),
                    spec_ref: test.spec_ref.clone(),
                    status: TestStatus::Pass,
                    duration: start.elapsed(),
                    details: None,
                }
            }
        }
    }
}

/// Summary statistics for conformance test run
#[derive(Debug, Default)]
pub struct ConformanceSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: usize,
    pub by_category: HashMap<String, CategoryStats>,
    pub by_spec_section: HashMap<String, CategoryStats>,
    pub total_duration: Duration,
}

#[derive(Debug, Default, Clone)]
pub struct CategoryStats {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
}

impl ConformanceSummary {
    pub fn from_results(results: &[ConformanceResult], tests: &[ConformanceTest]) -> Self {
        let mut summary = ConformanceSummary::default();

        for (result, test) in results.iter().zip(tests.iter()) {
            summary.total += 1;
            summary.total_duration += result.duration;

            let passed = matches!(result.status, TestStatus::Pass);

            match &result.status {
                TestStatus::Pass => summary.passed += 1,
                TestStatus::Fail(_) => summary.failed += 1,
                TestStatus::Skip(_) => summary.skipped += 1,
                TestStatus::Error(_) => summary.errors += 1,
            }

            // Track by category
            let cat_stats = summary
                .by_category
                .entry(test.category.clone())
                .or_default();
            cat_stats.total += 1;
            if passed {
                cat_stats.passed += 1;
            } else {
                cat_stats.failed += 1;
            }

            // Track by spec section
            if let Some(ref spec_ref) = test.spec_ref {
                let section = spec_ref
                    .split_whitespace()
                    .next()
                    .unwrap_or("unknown")
                    .to_string();
                let spec_stats = summary.by_spec_section.entry(section).or_default();
                spec_stats.total += 1;
                if passed {
                    spec_stats.passed += 1;
                } else {
                    spec_stats.failed += 1;
                }
            }
        }

        summary
    }

    pub fn print_report(&self) {
        println!("\n=== Conformance Test Report ===\n");
        println!(
            "Total: {} | Passed: {} | Failed: {} | Skipped: {} | Errors: {}",
            self.total, self.passed, self.failed, self.skipped, self.errors
        );
        println!("Duration: {:?}\n", self.total_duration);

        if !self.by_category.is_empty() {
            println!("By Category:");
            let mut categories: Vec<_> = self.by_category.iter().collect();
            categories.sort_by_key(|(k, _)| *k);
            for (cat, stats) in categories {
                let pct = if stats.total > 0 {
                    (stats.passed as f64 / stats.total as f64) * 100.0
                } else {
                    0.0
                };
                println!("  {}: {}/{} ({:.1}%)", cat, stats.passed, stats.total, pct);
            }
        }

        if !self.by_spec_section.is_empty() {
            println!("\nBy Spec Section:");
            let mut sections: Vec<_> = self.by_spec_section.iter().collect();
            sections.sort_by_key(|(k, _)| *k);
            for (section, stats) in sections {
                let pct = if stats.total > 0 {
                    (stats.passed as f64 / stats.total as f64) * 100.0
                } else {
                    0.0
                };
                println!(
                    "  {}: {}/{} ({:.1}%)",
                    section, stats.passed, stats.total, pct
                );
            }
        }

        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arth::compiler::codegen::linker::detect_available_linker;
    use std::env;

    fn get_arth_bin() -> PathBuf {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        PathBuf::from(manifest_dir)
            .join("target")
            .join("debug")
            .join("arth")
    }

    fn get_conformance_dir() -> PathBuf {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        PathBuf::from(manifest_dir)
            .join("tests")
            .join("conformance")
    }

    #[test]
    fn test_discover_conformance_tests() {
        let dir = get_conformance_dir();
        if dir.exists() {
            let tests = discover_tests(&dir);
            // We expect at least some tests if the directory exists
            println!("Discovered {} conformance tests", tests.len());
            for test in &tests {
                println!(
                    "  [{}/{}] {}",
                    test.category,
                    test.name,
                    test.spec_ref.as_deref().unwrap_or("(no spec ref)")
                );
            }
        }
    }

    fn run_all_conformance_tests_for(backend: ConformanceBackend) {
        let arth_bin = get_arth_bin();
        let conformance_dir = get_conformance_dir();

        if !arth_bin.exists() {
            panic!(
                "Arth binary not found at {:?}. Run `cargo build` first.",
                arth_bin
            );
        }

        if matches!(backend, ConformanceBackend::Llvm) && detect_available_linker().is_err() {
            println!("Skipping LLVM conformance run: no native linker toolchain available");
            return;
        }

        let tests = discover_tests(&conformance_dir);
        if tests.is_empty() {
            println!("No conformance tests found in {:?}", conformance_dir);
            return;
        }

        let mut results = Vec::new();
        for test in &tests {
            let result = run_test(test, &arth_bin, backend);
            let status_str = match &result.status {
                TestStatus::Pass => "PASS".to_string(),
                TestStatus::Fail(msg) => format!("FAIL: {}", msg),
                TestStatus::Skip(reason) => format!("SKIP: {}", reason),
                TestStatus::Error(err) => format!("ERROR: {}", err),
            };
            println!(
                "[{}][{}] {} ({:?})",
                status_str,
                backend.as_label(),
                test.name,
                result.duration
            );
            results.push(result);
        }

        let summary = ConformanceSummary::from_results(&results, &tests);
        summary.print_report();

        // Fail if any tests failed
        assert_eq!(
            summary.failed,
            0,
            "{} {} conformance tests failed",
            summary.failed,
            backend.as_label()
        );
    }

    #[test]
    #[ignore] // Run with: cargo test --test conformance_runner run_all_conformance_tests_vm -- --ignored
    fn run_all_conformance_tests_vm() {
        run_all_conformance_tests_for(ConformanceBackend::Vm);
    }

    #[test]
    #[ignore] // Run with: cargo test --test conformance_runner run_all_conformance_tests_llvm -- --ignored
    fn run_all_conformance_tests_llvm() {
        run_all_conformance_tests_for(ConformanceBackend::Llvm);
    }
}
