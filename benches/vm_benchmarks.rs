//! VM Performance Benchmarks
//!
//! Criterion benchmarks for the Arth VM runtime.
//! Run with: `cargo bench --bench vm_benchmarks`

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

use arth_vm::{Op, Program, decode_program, encode_program, run_program};

// ============================================================================
// Bytecode Encoding/Decoding Benchmarks
// ============================================================================

fn benchmark_encode_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("bytecode");

    // Small program
    let small_program = Program::new(
        vec!["hello".to_string()],
        vec![Op::PushI64(42), Op::PushI64(1), Op::AddI64, Op::Halt],
    );

    // Medium program (100 instructions)
    let medium_ops: Vec<Op> = (0..99)
        .map(|i| {
            if i % 3 == 0 {
                Op::PushI64(i as i64)
            } else if i % 3 == 1 {
                Op::AddI64
            } else {
                Op::Pop
            }
        })
        .chain(std::iter::once(Op::Halt))
        .collect();
    let medium_strings: Vec<String> = (0..10).map(|i| format!("string_{}", i)).collect();
    let medium_program = Program::new(medium_strings, medium_ops);

    // Large program (1000 instructions)
    let large_ops: Vec<Op> = (0..999)
        .map(|i| {
            if i % 4 == 0 {
                Op::PushI64(i as i64)
            } else if i % 4 == 1 {
                Op::PushF64(i as f64 * 0.5)
            } else if i % 4 == 2 {
                Op::AddI64
            } else {
                Op::Pop
            }
        })
        .chain(std::iter::once(Op::Halt))
        .collect();
    let large_strings: Vec<String> = (0..100).map(|i| format!("string_{}", i)).collect();
    let large_program = Program::new(large_strings, large_ops);

    // Encode benchmarks
    group.bench_function("encode_small", |b| {
        b.iter(|| encode_program(black_box(&small_program)))
    });

    group.bench_function("encode_medium", |b| {
        b.iter(|| encode_program(black_box(&medium_program)))
    });

    group.bench_function("encode_large", |b| {
        b.iter(|| encode_program(black_box(&large_program)))
    });

    // Pre-encode for decode benchmarks
    let small_bytes = encode_program(&small_program);
    let medium_bytes = encode_program(&medium_program);
    let large_bytes = encode_program(&large_program);

    group.throughput(Throughput::Bytes(small_bytes.len() as u64));
    group.bench_function("decode_small", |b| {
        b.iter(|| decode_program(black_box(&small_bytes)))
    });

    group.throughput(Throughput::Bytes(medium_bytes.len() as u64));
    group.bench_function("decode_medium", |b| {
        b.iter(|| decode_program(black_box(&medium_bytes)))
    });

    group.throughput(Throughput::Bytes(large_bytes.len() as u64));
    group.bench_function("decode_large", |b| {
        b.iter(|| decode_program(black_box(&large_bytes)))
    });

    group.finish();
}

// ============================================================================
// Instruction Dispatch Benchmarks
// ============================================================================

fn benchmark_instruction_dispatch(c: &mut Criterion) {
    let mut group = c.benchmark_group("dispatch");

    // Arithmetic loop: push, add, pop (measures basic dispatch overhead)
    let arith_ops: Vec<Op> =
        std::iter::repeat_with(|| vec![Op::PushI64(1), Op::PushI64(2), Op::AddI64, Op::Pop])
            .take(100)
            .flatten()
            .chain(std::iter::once(Op::Halt))
            .collect();
    let arith_program = Program::new(vec![], arith_ops);

    group.bench_function("arithmetic_loop_100", |b| {
        b.iter(|| run_program(black_box(&arith_program)))
    });

    // Float operations (push/pop since no float arithmetic ops)
    let float_ops: Vec<Op> =
        std::iter::repeat_with(|| vec![Op::PushF64(1.5), Op::PushF64(2.5), Op::Pop, Op::Pop])
            .take(100)
            .flatten()
            .chain(std::iter::once(Op::Halt))
            .collect();
    let float_program = Program::new(vec![], float_ops);

    group.bench_function("float_push_pop_100", |b| {
        b.iter(|| run_program(black_box(&float_program)))
    });

    // Comparison operations
    let cmp_ops: Vec<Op> =
        std::iter::repeat_with(|| vec![Op::PushI64(42), Op::PushI64(43), Op::LtI64, Op::Pop])
            .take(100)
            .flatten()
            .chain(std::iter::once(Op::Halt))
            .collect();
    let cmp_program = Program::new(vec![], cmp_ops);

    group.bench_function("comparison_100", |b| {
        b.iter(|| run_program(black_box(&cmp_program)))
    });

    group.finish();
}

// ============================================================================
// Function Call Benchmarks
// ============================================================================

fn benchmark_function_calls(c: &mut Criterion) {
    let mut group = c.benchmark_group("function_calls");

    // Simple function that returns immediately
    // Layout: [0: Call 3, 1: Pop, 2: Halt, 3: Ret]
    let simple_call = Program::new(vec![], vec![Op::Call(3), Op::Pop, Op::Halt, Op::Ret]);

    group.bench_function("simple_call", |b| {
        b.iter(|| run_program(black_box(&simple_call)))
    });

    // Multiple calls in sequence
    let multi_call_ops: Vec<Op> = (0..10)
        .flat_map(|_| vec![Op::Call(22), Op::Pop])
        .chain(vec![Op::Halt, Op::Ret])
        .collect();
    let multi_call = Program::new(vec![], multi_call_ops);

    group.bench_function("multi_call_10", |b| {
        b.iter(|| run_program(black_box(&multi_call)))
    });

    // Nested calls (depth 5)
    // Each function calls the next, then returns
    let nested_call = Program::new(
        vec![],
        vec![
            // 0: main entry
            Op::Call(3), // call f1
            Op::Pop,
            Op::Halt,
            // 3: f1
            Op::Call(6), // call f2
            Op::Pop,
            Op::Ret,
            // 6: f2
            Op::Call(9), // call f3
            Op::Pop,
            Op::Ret,
            // 9: f3
            Op::Call(12), // call f4
            Op::Pop,
            Op::Ret,
            // 12: f4
            Op::Call(15), // call f5
            Op::Pop,
            Op::Ret,
            // 15: f5 (leaf)
            Op::Ret,
        ],
    );

    group.bench_function("nested_call_depth_5", |b| {
        b.iter(|| run_program(black_box(&nested_call)))
    });

    group.finish();
}

// ============================================================================
// Control Flow Benchmarks
// ============================================================================

fn benchmark_control_flow(c: &mut Criterion) {
    let mut group = c.benchmark_group("control_flow");

    // Tight loop: count from 0 to 100
    // Using local variables for counter
    // 0: PushI64 0       ; initialize counter
    // 1: LocalSet 0      ; store in local 0
    // 2: LocalGet 0      ; get counter
    // 3: PushI64 100     ; limit
    // 4: LtI64           ; counter < 100?
    // 5: JumpIfFalse 12  ; if not, exit
    // 6: LocalGet 0      ; get counter
    // 7: PushI64 1       ; increment
    // 8: AddI64
    // 9: LocalSet 0      ; store back
    // 10: Jump 2         ; loop
    // 11: Halt
    // 12: Halt
    let loop_program = Program::new(
        vec![],
        vec![
            Op::PushI64(0),      // 0
            Op::LocalSet(0),     // 1
            Op::LocalGet(0),     // 2
            Op::PushI64(100),    // 3
            Op::LtI64,           // 4
            Op::JumpIfFalse(12), // 5
            Op::LocalGet(0),     // 6
            Op::PushI64(1),      // 7
            Op::AddI64,          // 8
            Op::LocalSet(0),     // 9
            Op::Jump(2),         // 10
            Op::Halt,            // 11
            Op::Halt,            // 12
        ],
    );

    group.bench_function("loop_100_iterations", |b| {
        b.iter(|| run_program(black_box(&loop_program)))
    });

    // Branching: alternating true/false
    let mut branch_ops: Vec<Op> = Vec::new();
    for i in 0..50 {
        branch_ops.push(Op::PushBool(if i % 2 == 0 { 1 } else { 0 }));
        branch_ops.push(Op::JumpIfFalse((branch_ops.len() + 3) as u32));
        branch_ops.push(Op::PushI64(1));
        branch_ops.push(Op::Pop);
    }
    branch_ops.push(Op::Halt);
    let branch_program = Program::new(vec![], branch_ops);

    group.bench_function("branch_50", |b| {
        b.iter(|| run_program(black_box(&branch_program)))
    });

    group.finish();
}

// ============================================================================
// Stack Operations Benchmarks
// ============================================================================

fn benchmark_stack_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("stack");

    // Push/pop stress test
    let push_pop_ops: Vec<Op> = (0..100)
        .flat_map(|i| vec![Op::PushI64(i)])
        .chain((0..100).map(|_| Op::Pop))
        .chain(std::iter::once(Op::Halt))
        .collect();
    let push_pop = Program::new(vec![], push_pop_ops);

    group.bench_function("push_pop_100", |b| {
        b.iter(|| run_program(black_box(&push_pop)))
    });

    // Local variable access
    let local_ops: Vec<Op> = vec![
        Op::PushI64(1), // local 0
        Op::LocalSet(0),
        Op::PushI64(2), // local 1
        Op::LocalSet(1),
        Op::PushI64(3), // local 2
        Op::LocalSet(2),
    ]
    .into_iter()
    .chain((0..100).flat_map(|_| vec![Op::LocalGet(0), Op::LocalGet(1), Op::AddI64, Op::Pop]))
    .chain(vec![Op::Halt])
    .collect();
    let local_program = Program::new(vec![], local_ops);

    group.bench_function("local_access_100", |b| {
        b.iter(|| run_program(black_box(&local_program)))
    });

    group.finish();
}

// ============================================================================
// String Operations Benchmarks
// ============================================================================

fn benchmark_string_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("strings");

    // String concatenation
    let strings = vec!["hello".to_string(), " ".to_string(), "world".to_string()];
    let concat_ops: Vec<Op> = (0..10)
        .flat_map(|_| {
            vec![
                Op::PushStr(0),
                Op::PushStr(1),
                Op::ConcatStr,
                Op::PushStr(2),
                Op::ConcatStr,
                Op::Pop,
            ]
        })
        .chain(std::iter::once(Op::Halt))
        .collect();
    let concat_program = Program::new(strings, concat_ops);

    group.bench_function("concat_10", |b| {
        b.iter(|| run_program(black_box(&concat_program)))
    });

    // String comparison
    let cmp_strings = vec!["alpha".to_string(), "alpha".to_string(), "beta".to_string()];
    let str_cmp_ops: Vec<Op> = (0..50)
        .flat_map(|i| {
            vec![
                Op::PushStr(0),
                Op::PushStr(if i % 2 == 0 { 1 } else { 2 }),
                Op::EqStr,
                Op::Pop,
            ]
        })
        .chain(std::iter::once(Op::Halt))
        .collect();
    let str_cmp_program = Program::new(cmp_strings, str_cmp_ops);

    group.bench_function("str_compare_50", |b| {
        b.iter(|| run_program(black_box(&str_cmp_program)))
    });

    group.finish();
}

// ============================================================================
// Scaling Benchmarks
// ============================================================================

fn benchmark_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("scaling");

    for size in [10, 100, 1000, 10000].iter() {
        let ops: Vec<Op> = (0..*size)
            .flat_map(|_| vec![Op::PushI64(1), Op::PushI64(2), Op::AddI64, Op::Pop])
            .chain(std::iter::once(Op::Halt))
            .collect();
        let program = Program::new(vec![], ops);

        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::new("arithmetic_ops", size), size, |b, _| {
            b.iter(|| run_program(black_box(&program)))
        });
    }

    group.finish();
}

// ============================================================================
// Main Benchmark Groups
// ============================================================================

criterion_group!(
    benches,
    benchmark_encode_decode,
    benchmark_instruction_dispatch,
    benchmark_function_calls,
    benchmark_control_flow,
    benchmark_stack_operations,
    benchmark_string_operations,
    benchmark_scaling,
);

criterion_main!(benches);
