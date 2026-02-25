//! Runtime Algorithm Benchmarks
//!
//! Criterion benchmarks measuring algorithm execution in the Arth VM.
//! Run with: `cargo bench --bench runtime_benchmarks`
//!
//! These benchmarks complement `vm_benchmarks.rs` by testing classic
//! algorithms that exercise multiple VM features together.
//!
//! Performance Targets:
//! - Fibonacci(30) iterative: < 50µs
//! - Sum of squares(10000): < 500µs
//! - GCD(large numbers): < 10µs

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

use arth_vm::{Op, Program, run_program};

// ============================================================================
// Fibonacci Benchmarks
// ============================================================================

/// Build a program that computes Fibonacci iteratively
fn build_fibonacci_program(n: i64) -> Program {
    // Algorithm:
    //   local 0 = a (fib(i-2))
    //   local 1 = b (fib(i-1))
    //   local 2 = i (counter)
    //   local 3 = temp
    //
    //   a = 0, b = 1, i = 0
    //   while i < n:
    //       temp = a + b
    //       a = b
    //       b = temp
    //       i = i + 1
    //   return a

    let ops = vec![
        // Initialize: a = 0
        Op::PushI64(0),
        Op::LocalSet(0),
        // Initialize: b = 1
        Op::PushI64(1),
        Op::LocalSet(1),
        // Initialize: i = 0
        Op::PushI64(0),
        Op::LocalSet(2),
        // Loop start (index 6)
        // Check: i < n
        Op::LocalGet(2),     // 6
        Op::PushI64(n),      // 7
        Op::LtI64,           // 8
        Op::JumpIfFalse(21), // 9: jump to end if i >= n
        // temp = a + b
        Op::LocalGet(0), // 10
        Op::LocalGet(1), // 11
        Op::AddI64,      // 12
        Op::LocalSet(3), // 13
        // a = b
        Op::LocalGet(1), // 14
        Op::LocalSet(0), // 15
        // b = temp
        Op::LocalGet(3), // 16
        Op::LocalSet(1), // 17
        // i = i + 1
        Op::LocalGet(2), // 18
        Op::PushI64(1),  // 19
        Op::AddI64,      // 20
        Op::LocalSet(2), // 21
        // Jump back to loop start
        Op::Jump(6), // 22
        // End: push result (a) and halt
        Op::LocalGet(0), // 23 (jump target from JumpIfFalse)
        Op::Halt,        // 24
    ];

    Program::new(vec![], ops)
}

fn benchmark_fibonacci(c: &mut Criterion) {
    let mut group = c.benchmark_group("algorithms/fibonacci");

    for n in [10, 20, 30, 40] {
        let program = build_fibonacci_program(n);
        group.bench_with_input(BenchmarkId::new("iterative", n), &program, |b, prog| {
            b.iter(|| run_program(black_box(prog)))
        });
    }

    group.finish();
}

// ============================================================================
// Sum of Squares Benchmarks
// ============================================================================

/// Build a program that computes sum of squares: 1^2 + 2^2 + ... + n^2
fn build_sum_of_squares_program(n: i64) -> Program {
    // Algorithm:
    //   local 0 = sum
    //   local 1 = i
    //
    //   sum = 0, i = 1
    //   while i <= n:
    //       sum = sum + i * i
    //       i = i + 1
    //   return sum

    let ops = vec![
        // Initialize: sum = 0
        Op::PushI64(0),
        Op::LocalSet(0),
        // Initialize: i = 1
        Op::PushI64(1),
        Op::LocalSet(1),
        // Loop start (index 4)
        // Check: i <= n (equivalent to i < n + 1)
        Op::LocalGet(1),     // 4
        Op::PushI64(n + 1),  // 5
        Op::LtI64,           // 6
        Op::JumpIfFalse(18), // 7: jump to end if i > n
        // sum = sum + i * i
        Op::LocalGet(0), // 8
        Op::LocalGet(1), // 9
        Op::LocalGet(1), // 10
        Op::MulI64,      // 11: i * i
        Op::AddI64,      // 12: sum + i*i
        Op::LocalSet(0), // 13
        // i = i + 1
        Op::LocalGet(1), // 14
        Op::PushI64(1),  // 15
        Op::AddI64,      // 16
        Op::LocalSet(1), // 17
        // Jump back to loop start
        Op::Jump(4), // 18
        // End: push result and halt
        Op::LocalGet(0), // 19 (jump target)
        Op::Halt,        // 20
    ];

    Program::new(vec![], ops)
}

fn benchmark_sum_of_squares(c: &mut Criterion) {
    let mut group = c.benchmark_group("algorithms/sum_of_squares");

    for n in [100, 1000, 10000] {
        let program = build_sum_of_squares_program(n);
        group.bench_with_input(BenchmarkId::new("loop", n), &program, |b, prog| {
            b.iter(|| run_program(black_box(prog)))
        });
    }

    group.finish();
}

// ============================================================================
// GCD Benchmarks (Euclidean Algorithm)
// ============================================================================

/// Build a program that computes GCD using Euclidean algorithm
fn build_gcd_program(a: i64, b: i64) -> Program {
    // Algorithm (iterative):
    //   local 0 = a
    //   local 1 = b
    //   local 2 = temp
    //
    //   while b != 0:
    //       temp = b
    //       b = a % b  (using a - (a / b) * b since we might not have ModI64)
    //       a = temp
    //   return a

    let ops = vec![
        // Initialize a and b
        Op::PushI64(a),
        Op::LocalSet(0),
        Op::PushI64(b),
        Op::LocalSet(1),
        // Loop start (index 4)
        // Check: b > 0 (since b is non-negative in GCD, this means b != 0)
        Op::PushI64(0),      // 4
        Op::LocalGet(1),     // 5: b
        Op::LtI64,           // 6: 0 < b? (true if b > 0)
        Op::JumpIfFalse(21), // 7: jump to end if b == 0
        // temp = b
        Op::LocalGet(1), // 8
        Op::LocalSet(2), // 9
        // b = a % b (computed as a - (a / b) * b)
        Op::LocalGet(0), // 10: a
        Op::LocalGet(0), // 11: a
        Op::LocalGet(1), // 12: b
        Op::DivI64,      // 13: a / b
        Op::LocalGet(1), // 14: b
        Op::MulI64,      // 15: (a / b) * b
        Op::SubI64,      // 16: a - (a / b) * b = a % b
        Op::LocalSet(1), // 17: store in b
        // a = temp
        Op::LocalGet(2), // 18
        Op::LocalSet(0), // 19
        // Jump back to loop
        Op::Jump(4), // 20
        // End: return a
        Op::LocalGet(0), // 21 (jump target)
        Op::Halt,        // 22
    ];

    Program::new(vec![], ops)
}

fn benchmark_gcd(c: &mut Criterion) {
    let mut group = c.benchmark_group("algorithms/gcd");

    let test_cases = [
        ("small", 48, 18),
        ("medium", 1071, 462),
        ("large", 123456789, 987654321),
    ];

    for (name, a, b) in test_cases {
        let program = build_gcd_program(a, b);
        group.bench_with_input(BenchmarkId::new("euclidean", name), &program, |b, prog| {
            b.iter(|| run_program(black_box(prog)))
        });
    }

    group.finish();
}

// ============================================================================
// Prime Check Benchmark
// ============================================================================

/// Build a program that checks if n is prime (trial division)
fn build_is_prime_program(n: i64) -> Program {
    // Algorithm:
    //   if n <= 1: return 0 (not prime)
    //   if n <= 3: return 1 (prime)
    //   if n % 2 == 0: return 0 (not prime)
    //   i = 3
    //   while i * i <= n:
    //       if n % i == 0: return 0
    //       i = i + 2
    //   return 1 (prime)
    //
    //   local 0 = n
    //   local 1 = i
    //   local 2 = temp (for computations)

    let ops = vec![
        // Store n
        Op::PushI64(n),
        Op::LocalSet(0), // 1
        // Check n <= 1
        Op::LocalGet(0),    // 2
        Op::PushI64(2),     // 3
        Op::LtI64,          // 4: n < 2
        Op::JumpIfFalse(8), // 5: if n >= 2, continue
        Op::PushI64(0),     // 6: return false (not prime)
        Op::Halt,           // 7
        // Check n <= 3 (already know n >= 2)
        Op::LocalGet(0),     // 8
        Op::PushI64(4),      // 9
        Op::LtI64,           // 10: n < 4 (i.e., n is 2 or 3)
        Op::JumpIfFalse(14), // 11
        Op::PushI64(1),      // 12: return true (2 and 3 are prime)
        Op::Halt,            // 13
        // Check n % 2 == 0 (even check)
        Op::LocalGet(0),     // 14
        Op::PushI64(2),      // 15
        Op::LocalGet(0),     // 16
        Op::PushI64(2),      // 17
        Op::DivI64,          // 18: n / 2
        Op::MulI64,          // 19: (n / 2) * 2
        Op::SubI64,          // 20: n - (n / 2) * 2 = n % 2
        Op::PushI64(0),      // 21
        Op::EqI64,           // 22: n % 2 == 0?
        Op::JumpIfFalse(26), // 23
        Op::PushI64(0),      // 24: return false (even, not prime)
        Op::Halt,            // 25
        // Initialize i = 3
        Op::PushI64(3),  // 26
        Op::LocalSet(1), // 27
        // Loop: while i * i <= n
        Op::LocalGet(1), // 28: i
        Op::LocalGet(1), // 29: i
        Op::MulI64,      // 30: i * i
        Op::LocalGet(0), // 31: n
        Op::LtI64,       // 32: i * i < n (we want <=, so also check ==)
        // Simplified: if i*i > n, exit
        Op::LocalGet(1),     // 33
        Op::LocalGet(1),     // 34
        Op::MulI64,          // 35: i * i
        Op::LocalGet(0),     // 36: n
        Op::SubI64,          // 37: i*i - n
        Op::PushI64(0),      // 38
        Op::LtI64,           // 39: i*i - n < 0 means i*i < n
        Op::JumpIfFalse(56), // 40: if i*i >= n, exit loop
        // Check n % i == 0
        Op::LocalGet(0),     // 41: n
        Op::LocalGet(0),     // 42: n
        Op::LocalGet(1),     // 43: i
        Op::DivI64,          // 44: n / i
        Op::LocalGet(1),     // 45: i
        Op::MulI64,          // 46: (n / i) * i
        Op::SubI64,          // 47: n % i
        Op::PushI64(0),      // 48
        Op::EqI64,           // 49: n % i == 0?
        Op::JumpIfFalse(53), // 50
        Op::PushI64(0),      // 51: return false (divisible)
        Op::Halt,            // 52
        // i = i + 2
        Op::LocalGet(1), // 53
        Op::PushI64(2),  // 54
        Op::AddI64,      // 55
        Op::LocalSet(1), // 56
        Op::Jump(28),    // 57: back to loop
        // Return true (prime)
        Op::PushI64(1), // 58 (jump target)
        Op::Halt,       // 59
    ];

    Program::new(vec![], ops)
}

fn benchmark_is_prime(c: &mut Criterion) {
    let mut group = c.benchmark_group("algorithms/is_prime");

    let test_cases = [
        ("small_prime", 97),
        ("medium_prime", 7919),
        ("large_prime", 104729),
        ("non_prime", 100000),
    ];

    for (name, n) in test_cases {
        let program = build_is_prime_program(n);
        group.bench_with_input(
            BenchmarkId::new("trial_division", name),
            &program,
            |b, prog| b.iter(|| run_program(black_box(prog))),
        );
    }

    group.finish();
}

// ============================================================================
// Factorial Benchmark
// ============================================================================

/// Build a program that computes n! iteratively
fn build_factorial_program(n: i64) -> Program {
    // Algorithm:
    //   local 0 = result
    //   local 1 = i
    //
    //   result = 1, i = 2
    //   while i <= n:
    //       result = result * i
    //       i = i + 1
    //   return result

    let ops = vec![
        // Initialize: result = 1
        Op::PushI64(1),
        Op::LocalSet(0),
        // Initialize: i = 2
        Op::PushI64(2),
        Op::LocalSet(1),
        // Loop start (index 4)
        // Check: i <= n (i < n + 1)
        Op::LocalGet(1),     // 4
        Op::PushI64(n + 1),  // 5
        Op::LtI64,           // 6
        Op::JumpIfFalse(15), // 7
        // result = result * i
        Op::LocalGet(0), // 8
        Op::LocalGet(1), // 9
        Op::MulI64,      // 10
        Op::LocalSet(0), // 11
        // i = i + 1
        Op::LocalGet(1), // 12
        Op::PushI64(1),  // 13
        Op::AddI64,      // 14
        Op::LocalSet(1), // 15
        Op::Jump(4),     // 16
        // Return result
        Op::LocalGet(0), // 17 (jump target)
        Op::Halt,        // 18
    ];

    Program::new(vec![], ops)
}

fn benchmark_factorial(c: &mut Criterion) {
    let mut group = c.benchmark_group("algorithms/factorial");

    for n in [10, 15, 20] {
        let program = build_factorial_program(n);
        group.bench_with_input(BenchmarkId::new("iterative", n), &program, |b, prog| {
            b.iter(|| run_program(black_box(prog)))
        });
    }

    group.finish();
}

// ============================================================================
// Main Benchmark Groups
// ============================================================================

criterion_group!(
    benches,
    benchmark_fibonacci,
    benchmark_sum_of_squares,
    benchmark_gcd,
    benchmark_is_prime,
    benchmark_factorial,
);

criterion_main!(benches);
