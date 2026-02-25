//! Performance Benchmarks: VM Mode
//!
//! These benchmarks measure VM execution performance.
//! Native benchmarks are pending on LLVM backend improvements for loop constructs.
//!
//! Run with: cargo test --test benchmark_test -- --ignored --nocapture

use std::process::Command;
use std::time::{Duration, Instant};

/// Number of iterations for each benchmark
const ITERATIONS: u32 = 5;

/// Benchmark result
#[derive(Debug)]
struct BenchResult {
    name: String,
    times: Vec<Duration>,
}

impl BenchResult {
    fn avg_ms(&self) -> f64 {
        if self.times.is_empty() {
            return 0.0;
        }
        let total: Duration = self.times.iter().sum();
        total.as_secs_f64() * 1000.0 / self.times.len() as f64
    }

    fn min_ms(&self) -> f64 {
        self.times
            .iter()
            .min()
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    }

    fn max_ms(&self) -> f64 {
        self.times
            .iter()
            .max()
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    }
}

/// Run an Arth file with the VM backend and measure time.
fn run_vm_timed(source_path: &str) -> Result<Duration, String> {
    let start = Instant::now();

    let output = Command::new("cargo")
        .args(["run", "--quiet", "--release", "--", "run", source_path])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .map_err(|e| format!("Failed to run VM: {}", e))?;

    let elapsed = start.elapsed();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("VM execution failed: {}", stderr));
    }

    Ok(elapsed)
}

/// Run a benchmark for a given source file.
fn run_benchmark(name: &str, source_path: &str) -> Result<BenchResult, String> {
    eprintln!("\n=== Benchmark: {} ===", name);
    eprintln!("Source: {}", source_path);

    let mut times = Vec::new();

    eprintln!("\nRunning VM ({} iterations)...", ITERATIONS);
    for i in 0..ITERATIONS {
        match run_vm_timed(source_path) {
            Ok(duration) => {
                eprintln!("  Run {}: {:.2}ms", i + 1, duration.as_secs_f64() * 1000.0);
                times.push(duration);
            }
            Err(e) => {
                eprintln!("  Run {} failed: {}", i + 1, e);
                return Err(e);
            }
        }
    }

    Ok(BenchResult {
        name: name.to_string(),
        times,
    })
}

/// Print benchmark summary table
fn print_summary(results: &[BenchResult]) {
    eprintln!("\n");
    eprintln!("╔═══════════════════════════════════════════════════════════╗");
    eprintln!("║              VM BENCHMARK RESULTS SUMMARY                 ║");
    eprintln!("╠═══════════════════╦═════════════╦═════════════╦═══════════╣");
    eprintln!("║ Benchmark         ║ Avg (ms)    ║ Min (ms)    ║ Max (ms)  ║");
    eprintln!("╠═══════════════════╬═════════════╬═════════════╬═══════════╣");

    for result in results {
        eprintln!(
            "║ {:17} ║ {:11.2} ║ {:11.2} ║ {:9.2} ║",
            result.name,
            result.avg_ms(),
            result.min_ms(),
            result.max_ms()
        );
    }

    eprintln!("╚═══════════════════╩═════════════╩═════════════╩═══════════╝");
    eprintln!();
    eprintln!("Note: Native benchmarks pending on LLVM backend improvements.");
    eprintln!("      See docs/hybrid-vm-native-architecture.md for status.");
}

// =============================================================================
// Benchmark Tests
// =============================================================================

#[test]
#[ignore] // Run with: cargo test --test benchmark_test -- --ignored --nocapture
fn benchmark_all() {
    let benchmarks = [
        ("arithmetic", "tests/benchmarks/arithmetic.arth"),
        ("fibonacci", "tests/benchmarks/fibonacci.arth"),
        ("loop", "tests/benchmarks/loop.arth"),
    ];

    let mut results = Vec::new();
    let mut failures = Vec::new();

    for (name, path) in &benchmarks {
        match run_benchmark(name, path) {
            Ok(result) => results.push(result),
            Err(e) => {
                failures.push(format!("{}: {}", name, e));
            }
        }
    }

    print_summary(&results);

    if !failures.is_empty() {
        eprintln!("\nFailures:");
        for f in &failures {
            eprintln!("  - {}", f);
        }
    }

    // Test passes if at least one benchmark succeeded
    assert!(!results.is_empty(), "All benchmarks failed");
}

#[test]
#[ignore]
fn benchmark_arithmetic() {
    let result = run_benchmark("arithmetic", "tests/benchmarks/arithmetic.arth");
    assert!(
        result.is_ok(),
        "Arithmetic benchmark failed: {:?}",
        result.err()
    );
    let r = result.unwrap();
    eprintln!(
        "\nArithmetic: avg={:.2}ms, min={:.2}ms, max={:.2}ms",
        r.avg_ms(),
        r.min_ms(),
        r.max_ms()
    );
}

#[test]
#[ignore]
fn benchmark_fibonacci() {
    let result = run_benchmark("fibonacci", "tests/benchmarks/fibonacci.arth");
    assert!(
        result.is_ok(),
        "Fibonacci benchmark failed: {:?}",
        result.err()
    );
    let r = result.unwrap();
    eprintln!(
        "\nFibonacci: avg={:.2}ms, min={:.2}ms, max={:.2}ms",
        r.avg_ms(),
        r.min_ms(),
        r.max_ms()
    );
}

#[test]
#[ignore]
fn benchmark_loop() {
    let result = run_benchmark("loop", "tests/benchmarks/loop.arth");
    assert!(result.is_ok(), "Loop benchmark failed: {:?}", result.err());
    let r = result.unwrap();
    eprintln!(
        "\nLoop: avg={:.2}ms, min={:.2}ms, max={:.2}ms",
        r.avg_ms(),
        r.min_ms(),
        r.max_ms()
    );
}

// =============================================================================
// Unit Tests
// =============================================================================

#[test]
fn test_bench_result_calculations() {
    let result = BenchResult {
        name: "test".to_string(),
        times: vec![
            Duration::from_millis(100),
            Duration::from_millis(200),
            Duration::from_millis(150),
        ],
    };

    assert!((result.avg_ms() - 150.0).abs() < 0.1);
    assert!((result.min_ms() - 100.0).abs() < 0.1);
    assert!((result.max_ms() - 200.0).abs() < 0.1);
}
