//! Test result reporting and formatting.
//!
//! Formats test results for display in the terminal.

use std::fmt::Write;

use super::{TestConfig, TestResult, TestStatus, TestSummary};

/// Report format options.
#[derive(Clone, Debug, Default)]
pub enum ReportFormat {
    /// Human-readable format with colors.
    #[default]
    Human,
    /// JSON format for machine consumption.
    Json,
    /// Compact format (one line per test).
    Compact,
}

/// Configuration for report formatting.
#[derive(Clone, Debug)]
pub struct ReportConfig {
    /// Output format.
    pub format: ReportFormat,
    /// Whether to use colors.
    pub color: bool,
    /// Whether to show passed tests.
    pub show_passed: bool,
    /// Whether to show captured output.
    pub show_output: bool,
}

impl Default for ReportConfig {
    fn default() -> Self {
        Self {
            format: ReportFormat::Human,
            color: true,
            show_passed: true,
            show_output: true,
        }
    }
}

/// A formatted test report.
#[derive(Clone, Debug)]
pub struct TestReport {
    /// The formatted output.
    pub output: String,
    /// Summary of results.
    pub summary: TestSummary,
}

/// Format test results into a report.
pub fn format_results(
    results: &[TestResult],
    summary: &TestSummary,
    config: &ReportConfig,
) -> TestReport {
    let output = match config.format {
        ReportFormat::Human => format_human(results, summary, config),
        ReportFormat::Json => format_json(results, summary),
        ReportFormat::Compact => format_compact(results, summary, config),
    };

    TestReport {
        output,
        summary: summary.clone(),
    }
}

/// Format results in human-readable format.
fn format_human(results: &[TestResult], summary: &TestSummary, config: &ReportConfig) -> String {
    let mut out = String::new();

    // Header
    let _ = writeln!(&mut out, "Running {} tests...", summary.total);
    let _ = writeln!(&mut out);

    // Individual results
    for result in results {
        let should_show = match &result.status {
            TestStatus::Passed => config.show_passed,
            _ => true,
        };

        if should_show {
            write_result_human(&mut out, result, config);
        }
    }

    // Summary
    let _ = writeln!(&mut out);
    write_summary_human(&mut out, summary, config);

    out
}

/// Write a single result in human format.
fn write_result_human(out: &mut String, result: &TestResult, config: &ReportConfig) {
    let status_str = match &result.status {
        TestStatus::Passed => {
            if config.color {
                "\x1b[32mPASS\x1b[0m"
            } else {
                "PASS"
            }
        }
        TestStatus::Failed(_) => {
            if config.color {
                "\x1b[31mFAIL\x1b[0m"
            } else {
                "FAIL"
            }
        }
        TestStatus::Skipped(_) => {
            if config.color {
                "\x1b[33mSKIP\x1b[0m"
            } else {
                "SKIP"
            }
        }
        TestStatus::TimedOut => {
            if config.color {
                "\x1b[31mTIME\x1b[0m"
            } else {
                "TIME"
            }
        }
        TestStatus::Panicked(_) => {
            if config.color {
                "\x1b[31mPANIC\x1b[0m"
            } else {
                "PANIC"
            }
        }
    };

    let duration_ms = result.duration.as_millis();
    let _ = writeln!(
        out,
        "{} {} ({}ms)",
        status_str, result.test.name, duration_ms
    );

    // Show failure message or panic info
    match &result.status {
        TestStatus::Failed(msg) => {
            let _ = writeln!(out, "    Error: {}", msg);
        }
        TestStatus::Panicked(msg) => {
            let _ = writeln!(out, "    Panic: {}", msg);
        }
        TestStatus::Skipped(reason) => {
            let _ = writeln!(out, "    Skipped: {}", reason);
        }
        _ => {}
    }

    // Show captured output
    if config.show_output && (!result.stdout.is_empty() || !result.stderr.is_empty()) {
        if !result.stdout.is_empty() {
            let _ = writeln!(out, "    stdout:");
            for line in result.stdout.lines() {
                let _ = writeln!(out, "      {}", line);
            }
        }
        if !result.stderr.is_empty() {
            let _ = writeln!(out, "    stderr:");
            for line in result.stderr.lines() {
                let _ = writeln!(out, "      {}", line);
            }
        }
    }
}

/// Write summary in human format.
fn write_summary_human(out: &mut String, summary: &TestSummary, config: &ReportConfig) {
    let (pass_color, fail_color, reset) = if config.color {
        ("\x1b[32m", "\x1b[31m", "\x1b[0m")
    } else {
        ("", "", "")
    };

    if summary.all_passed() {
        let _ = writeln!(
            out,
            "{}All {} tests passed{} in {:?}",
            pass_color, summary.passed, reset, summary.duration
        );
    } else {
        let _ = writeln!(out, "Test Results:");
        let _ = writeln!(out, "  {}Passed:  {}{}", pass_color, summary.passed, reset);
        let _ = writeln!(out, "  {}Failed:  {}{}", fail_color, summary.failed, reset);
        if summary.skipped > 0 {
            let _ = writeln!(out, "  Skipped: {}", summary.skipped);
        }
        let _ = writeln!(out, "  Duration: {:?}", summary.duration);
    }
}

/// Format results in JSON format.
fn format_json(results: &[TestResult], summary: &TestSummary) -> String {
    let mut out = String::new();

    out.push_str("{\n");
    out.push_str("  \"results\": [\n");

    for (i, result) in results.iter().enumerate() {
        let status = match &result.status {
            TestStatus::Passed => "passed",
            TestStatus::Failed(_) => "failed",
            TestStatus::Skipped(_) => "skipped",
            TestStatus::TimedOut => "timeout",
            TestStatus::Panicked(_) => "panicked",
        };

        let message = match &result.status {
            TestStatus::Failed(m) | TestStatus::Panicked(m) | TestStatus::Skipped(m) => {
                format!(",\n      \"message\": {:?}", m)
            }
            _ => String::new(),
        };

        out.push_str(&format!(
            "    {{\n      \"name\": {:?},\n      \"status\": {:?},\n      \"duration_ms\": {}{}",
            result.test.name,
            status,
            result.duration.as_millis(),
            message
        ));

        out.push_str("\n    }");
        if i < results.len() - 1 {
            out.push(',');
        }
        out.push('\n');
    }

    out.push_str("  ],\n");
    out.push_str(&format!(
        "  \"summary\": {{\n    \"total\": {},\n    \"passed\": {},\n    \"failed\": {},\n    \"skipped\": {},\n    \"duration_ms\": {}\n  }}\n",
        summary.total,
        summary.passed,
        summary.failed,
        summary.skipped,
        summary.duration.as_millis()
    ));
    out.push('}');

    out
}

/// Format results in compact format.
fn format_compact(results: &[TestResult], summary: &TestSummary, config: &ReportConfig) -> String {
    let mut out = String::new();

    for result in results {
        let symbol = match &result.status {
            TestStatus::Passed => {
                if config.color {
                    "\x1b[32m.\x1b[0m"
                } else {
                    "."
                }
            }
            TestStatus::Failed(_) => {
                if config.color {
                    "\x1b[31mF\x1b[0m"
                } else {
                    "F"
                }
            }
            TestStatus::Skipped(_) => {
                if config.color {
                    "\x1b[33mS\x1b[0m"
                } else {
                    "S"
                }
            }
            TestStatus::TimedOut => {
                if config.color {
                    "\x1b[31mT\x1b[0m"
                } else {
                    "T"
                }
            }
            TestStatus::Panicked(_) => {
                if config.color {
                    "\x1b[31mP\x1b[0m"
                } else {
                    "P"
                }
            }
        };
        out.push_str(symbol);
    }

    out.push('\n');
    let _ = writeln!(
        out,
        "{} tests, {} passed, {} failed in {:?}",
        summary.total, summary.passed, summary.failed, summary.duration
    );

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    use crate::compiler::test_runner::TestInfo;

    fn make_passed_result(name: &str) -> TestResult {
        TestResult {
            test: TestInfo {
                name: name.to_string(),
                package: "test".to_string(),
                module: None,
                function: name.to_string(),
                file: PathBuf::from("test.arth"),
                is_benchmark: false,
            },
            status: TestStatus::Passed,
            duration: Duration::from_millis(10),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn make_failed_result(name: &str, msg: &str) -> TestResult {
        TestResult {
            test: TestInfo {
                name: name.to_string(),
                package: "test".to_string(),
                module: None,
                function: name.to_string(),
                file: PathBuf::from("test.arth"),
                is_benchmark: false,
            },
            status: TestStatus::Failed(msg.to_string()),
            duration: Duration::from_millis(15),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    #[test]
    fn test_format_human_all_passed() {
        let results = vec![
            make_passed_result("test_one"),
            make_passed_result("test_two"),
        ];
        let summary = TestSummary {
            total: 2,
            passed: 2,
            failed: 0,
            skipped: 0,
            duration: Duration::from_millis(25),
        };
        let config = ReportConfig {
            color: false,
            ..Default::default()
        };

        let report = format_results(&results, &summary, &config);

        assert!(report.output.contains("PASS test_one"));
        assert!(report.output.contains("PASS test_two"));
        assert!(report.output.contains("All 2 tests passed"));
    }

    #[test]
    fn test_format_human_with_failure() {
        let results = vec![
            make_passed_result("test_one"),
            make_failed_result("test_two", "assertion failed"),
        ];
        let summary = TestSummary {
            total: 2,
            passed: 1,
            failed: 1,
            skipped: 0,
            duration: Duration::from_millis(25),
        };
        let config = ReportConfig {
            color: false,
            ..Default::default()
        };

        let report = format_results(&results, &summary, &config);

        assert!(report.output.contains("PASS test_one"));
        assert!(report.output.contains("FAIL test_two"));
        assert!(report.output.contains("assertion failed"));
        assert!(report.output.contains("Failed:  1"));
    }

    #[test]
    fn test_format_json() {
        let results = vec![make_passed_result("test_example")];
        let summary = TestSummary {
            total: 1,
            passed: 1,
            failed: 0,
            skipped: 0,
            duration: Duration::from_millis(10),
        };
        let config = ReportConfig {
            format: ReportFormat::Json,
            ..Default::default()
        };

        let report = format_results(&results, &summary, &config);

        assert!(report.output.contains("\"name\": \"test_example\""));
        assert!(report.output.contains("\"status\": \"passed\""));
        assert!(report.output.contains("\"total\": 1"));
    }

    #[test]
    fn test_format_compact() {
        let results = vec![
            make_passed_result("test_one"),
            make_passed_result("test_two"),
            make_failed_result("test_three", "error"),
        ];
        let summary = TestSummary {
            total: 3,
            passed: 2,
            failed: 1,
            skipped: 0,
            duration: Duration::from_millis(35),
        };
        let config = ReportConfig {
            format: ReportFormat::Compact,
            color: false,
            ..Default::default()
        };

        let report = format_results(&results, &summary, &config);

        // Should contain ..F for two passes and one fail
        assert!(report.output.contains("..F"));
        assert!(report.output.contains("3 tests"));
    }
}
