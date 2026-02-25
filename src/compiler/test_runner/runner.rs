//! Test execution engine.
//!
//! Runs tests using the VM backend and collects results.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use arth_vm as vm;

use super::{TestConfig, TestInfo, TestSummary};

/// Status of a test execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestStatus {
    /// Test passed.
    Passed,
    /// Test failed with a message.
    Failed(String),
    /// Test was skipped.
    Skipped(String),
    /// Test timed out.
    TimedOut,
    /// Test panicked.
    Panicked(String),
}

impl TestStatus {
    /// Check if the test passed.
    pub fn is_pass(&self) -> bool {
        matches!(self, TestStatus::Passed)
    }

    /// Check if the test failed.
    pub fn is_fail(&self) -> bool {
        matches!(
            self,
            TestStatus::Failed(_) | TestStatus::TimedOut | TestStatus::Panicked(_)
        )
    }
}

/// Result of running a single test.
#[derive(Clone, Debug)]
pub struct TestResult {
    /// Test information.
    pub test: TestInfo,
    /// Test status.
    pub status: TestStatus,
    /// Execution duration.
    pub duration: Duration,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

impl TestResult {
    /// Check if the test passed.
    pub fn passed(&self) -> bool {
        self.status.is_pass()
    }

    /// Check if the test failed.
    pub fn failed(&self) -> bool {
        self.status.is_fail()
    }
}

/// Execution context holding a compiled program and its function offsets.
///
/// When provided to `run_test`, tests are executed via the VM.
/// When `None`, tests are skipped with a "no compiled program" reason.
pub struct ExecutionContext {
    /// The compiled VM program.
    pub program: vm::Program,
    /// Map from qualified function name (e.g. "Module.func") to bytecode offset.
    pub func_offsets: HashMap<String, u32>,
}

/// Run a single test using the VM if an execution context is provided.
pub fn run_test(
    test: &TestInfo,
    _config: &TestConfig,
    ctx: Option<&ExecutionContext>,
) -> TestResult {
    let start = Instant::now();

    let status = match ctx {
        Some(exec) => execute_test_in_vm(test, exec),
        None => TestStatus::Skipped("no compiled program available".into()),
    };

    let duration = start.elapsed();

    TestResult {
        test: test.clone(),
        status,
        duration,
        stdout: String::new(),
        stderr: String::new(),
    }
}

/// Execute a single test function in the VM.
fn execute_test_in_vm(test: &TestInfo, ctx: &ExecutionContext) -> TestStatus {
    // Build the qualified name the codegen uses: "Module.function"
    let lookup_name = if let Some(ref module) = test.module {
        format!("{}.{}", module, test.function)
    } else {
        test.function.clone()
    };

    let offset = match ctx.func_offsets.get(&lookup_name) {
        Some(&off) => off,
        None => {
            return TestStatus::Skipped(format!(
                "function '{}' not found in compiled program",
                lookup_name
            ));
        }
    };

    // Test functions take no arguments and return void
    match vm::call_export(&ctx.program, offset, 0, &[], None) {
        vm::CallExportResult::Success(_) => TestStatus::Passed,
        vm::CallExportResult::Failed(code) => {
            TestStatus::Failed(format!("test exited with code {}", code))
        }
        vm::CallExportResult::ExportNotFound => {
            TestStatus::Skipped(format!("offset {} not found in bytecode", offset))
        }
        vm::CallExportResult::InvalidArgument(msg) => {
            TestStatus::Failed(format!("invalid argument: {}", msg))
        }
    }
}

/// Run all tests and return results.
pub fn run_tests(
    tests: &[TestInfo],
    config: &TestConfig,
    ctx: Option<&ExecutionContext>,
) -> (Vec<TestResult>, TestSummary) {
    let start = Instant::now();
    let mut results = Vec::with_capacity(tests.len());

    for test in tests {
        let result = run_test(test, config, ctx);
        results.push(result);
    }

    let summary = calculate_summary(&results, start.elapsed());

    (results, summary)
}

/// Calculate summary from results.
fn calculate_summary(results: &[TestResult], duration: Duration) -> TestSummary {
    let mut summary = TestSummary {
        total: results.len(),
        duration,
        ..Default::default()
    };

    for result in results {
        match &result.status {
            TestStatus::Passed => summary.passed += 1,
            TestStatus::Failed(_) | TestStatus::TimedOut | TestStatus::Panicked(_) => {
                summary.failed += 1
            }
            TestStatus::Skipped(_) => summary.skipped += 1,
        }
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_test_info(name: &str) -> TestInfo {
        TestInfo {
            name: name.to_string(),
            package: "test".to_string(),
            module: None,
            function: name.to_string(),
            file: PathBuf::from("test.arth"),
            is_benchmark: false,
        }
    }

    #[test]
    fn test_run_single_without_context_skips() {
        let test = make_test_info("test_example");
        let config = TestConfig::default();
        let result = run_test(&test, &config, None);

        assert!(!result.passed());
        assert!(matches!(result.status, TestStatus::Skipped(_)));
        assert_eq!(result.test.name, "test_example");
    }

    #[test]
    fn test_run_multiple_without_context() {
        let tests = vec![
            make_test_info("test_one"),
            make_test_info("test_two"),
            make_test_info("test_three"),
        ];
        let config = TestConfig::default();
        let (results, summary) = run_tests(&tests, &config, None);

        assert_eq!(results.len(), 3);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.skipped, 3);
    }

    #[test]
    fn test_run_with_empty_context_skips_missing() {
        let test = TestInfo {
            name: "pkg.Mod.test_x".to_string(),
            package: "pkg".to_string(),
            module: Some("Mod".to_string()),
            function: "test_x".to_string(),
            file: PathBuf::from("test.arth"),
            is_benchmark: false,
        };
        let config = TestConfig::default();
        let ctx = ExecutionContext {
            program: vm::Program::new(vec![], vec![]),
            func_offsets: HashMap::new(),
        };
        let result = run_test(&test, &config, Some(&ctx));

        assert!(matches!(result.status, TestStatus::Skipped(_)));
    }

    #[test]
    fn test_status_predicates() {
        assert!(TestStatus::Passed.is_pass());
        assert!(!TestStatus::Passed.is_fail());

        assert!(!TestStatus::Failed("error".to_string()).is_pass());
        assert!(TestStatus::Failed("error".to_string()).is_fail());

        assert!(TestStatus::TimedOut.is_fail());
        assert!(TestStatus::Panicked("panic".to_string()).is_fail());

        assert!(!TestStatus::Skipped("reason".to_string()).is_pass());
        assert!(!TestStatus::Skipped("reason".to_string()).is_fail());
    }
}
