//! Test runner for the Arth language.
//!
//! This module provides functionality to discover and run tests and benchmarks
//! annotated with `@test` and `@bench` attributes.
//!
//! # Example
//!
//! ```ignore
//! use arth::compiler::test_runner::{discover_tests, run_tests, TestConfig};
//!
//! let tests = discover_tests(&analysis.tests);
//! let results = run_tests(&tests, &TestConfig::default());
//! ```

mod report;
mod runner;

pub use report::{ReportConfig, ReportFormat, TestReport, format_results};
pub use runner::{ExecutionContext, TestResult, TestStatus, run_test, run_tests};

use std::path::PathBuf;
use std::time::Duration;

use crate::compiler::typeck::attrs::TestEntry;

/// Configuration for the test runner.
#[derive(Clone, Debug)]
pub struct TestConfig {
    /// Filter pattern for test names.
    pub filter: Option<String>,
    /// Whether to run tests in parallel.
    pub parallel: bool,
    /// Maximum time per test before timeout.
    pub timeout: Duration,
    /// Whether to capture stdout/stderr.
    pub capture: bool,
    /// Whether to show passed tests.
    pub show_passed: bool,
    /// Whether to run benchmarks.
    pub run_benchmarks: bool,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            filter: None,
            parallel: true,
            timeout: Duration::from_secs(60),
            capture: true,
            show_passed: true,
            run_benchmarks: false,
        }
    }
}

impl TestConfig {
    /// Create a new test config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a filter pattern.
    pub fn with_filter(mut self, filter: impl Into<String>) -> Self {
        self.filter = Some(filter.into());
        self
    }

    /// Set whether to run in parallel.
    pub fn with_parallel(mut self, parallel: bool) -> Self {
        self.parallel = parallel;
        self
    }

    /// Set the timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set whether to capture output.
    pub fn with_capture(mut self, capture: bool) -> Self {
        self.capture = capture;
        self
    }

    /// Set whether to run benchmarks.
    pub fn with_benchmarks(mut self, run: bool) -> Self {
        self.run_benchmarks = run;
        self
    }
}

/// Information about a discovered test.
#[derive(Clone, Debug)]
pub struct TestInfo {
    /// Full qualified name: `pkg.module.function` or `pkg.function`.
    pub name: String,
    /// Package name.
    pub package: String,
    /// Module name (if inside a module).
    pub module: Option<String>,
    /// Function name.
    pub function: String,
    /// Source file path.
    pub file: PathBuf,
    /// Whether this is a benchmark.
    pub is_benchmark: bool,
}

impl From<&TestEntry> for TestInfo {
    fn from(entry: &TestEntry) -> Self {
        let name = if let Some(ref module) = entry.module {
            format!("{}.{}.{}", entry.package, module, entry.function)
        } else {
            format!("{}.{}", entry.package, entry.function)
        };

        Self {
            name,
            package: entry.package.clone(),
            module: entry.module.clone(),
            function: entry.function.clone(),
            file: entry.file.clone(),
            is_benchmark: false,
        }
    }
}

/// Discover tests from the test collection.
pub fn discover_tests(
    tests: &crate::compiler::typeck::attrs::TestCollection,
    config: &TestConfig,
) -> Vec<TestInfo> {
    let mut discovered = Vec::new();

    // Add tests
    for entry in &tests.tests {
        let info = TestInfo::from(entry);
        if should_include(&info, config) {
            discovered.push(info);
        }
    }

    // Add benchmarks if configured
    if config.run_benchmarks {
        for entry in &tests.benches {
            let mut info = TestInfo::from(entry);
            info.is_benchmark = true;
            if should_include(&info, config) {
                discovered.push(info);
            }
        }
    }

    discovered
}

/// Check if a test should be included based on the filter.
fn should_include(test: &TestInfo, config: &TestConfig) -> bool {
    if let Some(ref filter) = config.filter {
        test.name.contains(filter) || test.function.contains(filter)
    } else {
        true
    }
}

/// Summary of test results.
#[derive(Clone, Debug, Default)]
pub struct TestSummary {
    /// Total number of tests.
    pub total: usize,
    /// Number of passed tests.
    pub passed: usize,
    /// Number of failed tests.
    pub failed: usize,
    /// Number of skipped tests.
    pub skipped: usize,
    /// Total duration.
    pub duration: Duration,
}

impl TestSummary {
    /// Check if all tests passed.
    pub fn all_passed(&self) -> bool {
        self.failed == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::typeck::attrs::TestCollection;

    #[test]
    fn test_config_default() {
        let config = TestConfig::default();
        assert!(config.parallel);
        assert!(config.capture);
        assert!(config.filter.is_none());
    }

    #[test]
    fn test_config_builder() {
        let config = TestConfig::new()
            .with_filter("my_test")
            .with_parallel(false)
            .with_benchmarks(true);

        assert_eq!(config.filter, Some("my_test".to_string()));
        assert!(!config.parallel);
        assert!(config.run_benchmarks);
    }

    #[test]
    fn test_discover_empty() {
        let tests = TestCollection::default();
        let config = TestConfig::default();
        let discovered = discover_tests(&tests, &config);
        assert!(discovered.is_empty());
    }

    #[test]
    fn test_discover_with_filter() {
        let mut tests = TestCollection::default();
        tests.tests.push(TestEntry {
            package: "pkg".to_string(),
            module: None,
            function: "test_foo".to_string(),
            file: PathBuf::from("test.arth"),
        });
        tests.tests.push(TestEntry {
            package: "pkg".to_string(),
            module: None,
            function: "test_bar".to_string(),
            file: PathBuf::from("test.arth"),
        });

        let config = TestConfig::new().with_filter("foo");
        let discovered = discover_tests(&tests, &config);

        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].function, "test_foo");
    }

    #[test]
    fn test_info_from_entry() {
        let entry = TestEntry {
            package: "my.pkg".to_string(),
            module: Some("MyModule".to_string()),
            function: "test_something".to_string(),
            file: PathBuf::from("test.arth"),
        };

        let info = TestInfo::from(&entry);
        assert_eq!(info.name, "my.pkg.MyModule.test_something");
        assert_eq!(info.package, "my.pkg");
        assert_eq!(info.module, Some("MyModule".to_string()));
        assert!(!info.is_benchmark);
    }

    #[test]
    fn test_summary_all_passed() {
        let summary = TestSummary {
            total: 5,
            passed: 5,
            failed: 0,
            skipped: 0,
            duration: Duration::from_secs(1),
        };
        assert!(summary.all_passed());

        let summary_failed = TestSummary {
            total: 5,
            passed: 4,
            failed: 1,
            skipped: 0,
            duration: Duration::from_secs(1),
        };
        assert!(!summary_failed.all_passed());
    }
}
