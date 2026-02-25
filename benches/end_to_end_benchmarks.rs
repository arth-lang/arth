//! End-to-End Compilation Benchmarks
//!
//! Criterion benchmarks measuring full compilation pipeline performance.
//! Run with: `cargo bench --bench end_to_end_benchmarks`
//!
//! Performance Targets (documented baselines):
//! - Lexer: > 10 MB/s throughput
//! - Parser: > 5 MB/s throughput
//! - Full pipeline: > 1 MB/s throughput

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::path::PathBuf;

use arth::compiler::diagnostics::Reporter;
use arth::compiler::lexer::lex_all;
use arth::compiler::parser::parse_file;
use arth::compiler::source::SourceFile;

/// Create a SourceFile from a string (for benchmarking)
fn source_from_string(name: &str, content: &str) -> SourceFile {
    SourceFile {
        path: PathBuf::from(name),
        text: content.to_string(),
    }
}

// ============================================================================
// Sample Sources of Various Sizes
// ============================================================================

const TINY_SOURCE: &str = r#"
package bench;
module Main {
    public static void main() {
        Int x = 1;
    }
}
"#;

const SMALL_SOURCE: &str = r#"
package bench;

public struct Point {
    public Int x;
    public Int y;
}

module PointFns {
    public Point new(Int x, Int y) {
        return Point { x: x, y: y };
    }

    public Int sum(Point p) {
        return p.x + p.y;
    }
}

module Main {
    public static void main() {
        Point p = PointFns.new(10, 20);
        Int s = PointFns.sum(p);
    }
}
"#;

const MEDIUM_SOURCE: &str = r#"
package bench;

public struct ValidationError {
    public String message;
}

public struct Point {
    public Int x;
    public Int y;
}

public struct Rectangle {
    public Point topLeft;
    public Point bottomRight;
}

public interface Shape {
    Int area();
    Bool contains(Point p);
}

public enum Result<T, E> {
    Ok(T),
    Err(E)
}

module PointFns {
    public Point new(Int x, Int y) {
        return Point { x: x, y: y };
    }

    public Int distanceSquared(Point a, Point b) {
        Int dx = b.x - a.x;
        Int dy = b.y - a.y;
        return dx * dx + dy * dy;
    }

    public Point add(Point a, Point b) {
        return Point { x: a.x + b.x, y: a.y + b.y };
    }

    public Point scale(Point p, Int factor) {
        return Point { x: p.x * factor, y: p.y * factor };
    }
}

module RectangleFns implements Shape {
    public Rectangle new(Point tl, Point br) {
        return Rectangle { topLeft: tl, bottomRight: br };
    }

    public Int area(Rectangle r) {
        Int width = r.bottomRight.x - r.topLeft.x;
        Int height = r.bottomRight.y - r.topLeft.y;
        if (width < 0) { width = 0 - width; }
        if (height < 0) { height = 0 - height; }
        return width * height;
    }

    public Bool contains(Rectangle r, Point p) {
        Bool xOk = p.x >= r.topLeft.x && p.x <= r.bottomRight.x;
        Bool yOk = p.y >= r.topLeft.y && p.y <= r.bottomRight.y;
        return xOk && yOk;
    }

    public Rectangle validate(Rectangle r) throws (ValidationError) {
        if (r.topLeft.x > r.bottomRight.x) {
            throw ValidationError { message: "invalid x bounds" };
        }
        if (r.topLeft.y > r.bottomRight.y) {
            throw ValidationError { message: "invalid y bounds" };
        }
        return r;
    }
}

module Main {
    public static void main() {
        Point p1 = PointFns.new(0, 0);
        Point p2 = PointFns.new(100, 100);
        Rectangle rect = RectangleFns.new(p1, p2);

        Int area = RectangleFns.area(rect);
        Point test = PointFns.new(50, 50);
        Bool inside = RectangleFns.contains(rect, test);

        try {
            Rectangle validated = RectangleFns.validate(rect);
        } catch (ValidationError e) {
            String msg = e.message;
        }
    }
}
"#;

/// Generate a large source file with specified number of functions
fn generate_large_source(num_functions: usize) -> String {
    let mut source = String::from("package bench;\n\n");
    source.push_str("public struct Data { public Int value; }\n\n");

    for i in 0..num_functions {
        source.push_str(&format!(
            r#"
module Mod{} {{
    public Int compute{}(Int a, Int b, Int c) {{
        Int x = a + b;
        Int y = b * c;
        Int z = x - y;
        if (z > 0) {{
            for (Int i = 0; i < 5; i = i + 1) {{
                z = z + i;
            }}
            return z;
        }} else {{
            while (z < 0) {{
                z = z + 1;
            }}
            return z;
        }}
    }}
}}
"#,
            i, i
        ));
    }

    source.push_str("\nmodule Main {\n    public static void main() {\n");
    for i in 0..num_functions.min(20) {
        source.push_str(&format!(
            "        Int r{} = Mod{}.compute{}(1, 2, 3);\n",
            i, i, i
        ));
    }
    source.push_str("    }\n}\n");

    source
}

/// Generate source with deeply nested control flow
fn generate_nested_source(depth: usize) -> String {
    let mut source =
        String::from("package bench;\nmodule Main {\n    public static void main() {\n");
    source.push_str("        Int x = 0;\n");

    for i in 0..depth {
        source.push_str(&format!(
            "{}if (x < {}) {{\n",
            "        ".repeat(i + 2),
            100 - i
        ));
        source.push_str(&format!("{}x = x + 1;\n", "        ".repeat(i + 3)));
    }

    for i in (0..depth).rev() {
        source.push_str(&format!("{}}}\n", "        ".repeat(i + 2)));
    }

    source.push_str("    }\n}\n");
    source
}

/// Generate source with many generic types
fn generate_generic_source(num_types: usize) -> String {
    let mut source = String::from("package bench;\n\n");

    for i in 0..num_types {
        source.push_str(&format!(
            "public struct Box{}<T> {{ public T value; }}\n",
            i
        ));
    }

    source.push_str("\nmodule Main {\n    public static void main() {\n");
    for i in 0..num_types.min(10) {
        source.push_str(&format!(
            "        Box{}<Int> b{} = Box{} {{ value: {} }};\n",
            i, i, i, i
        ));
    }
    source.push_str("    }\n}\n");

    source
}

// ============================================================================
// Lexer Throughput Benchmarks
// ============================================================================

fn benchmark_lexer_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("lexer_throughput");

    // Test different source sizes
    for (name, source) in [
        ("tiny", TINY_SOURCE.to_string()),
        ("small", SMALL_SOURCE.to_string()),
        ("medium", MEDIUM_SOURCE.to_string()),
        ("large_100", generate_large_source(100)),
        ("large_500", generate_large_source(500)),
    ] {
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_with_input(BenchmarkId::new("lex", name), &source, |b, src| {
            b.iter(|| {
                let tokens = lex_all(black_box(src));
                black_box(tokens)
            })
        });
    }

    group.finish();
}

// ============================================================================
// Parser Throughput Benchmarks
// ============================================================================

fn benchmark_parser_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("parser_throughput");

    for (name, source) in [
        ("tiny", TINY_SOURCE.to_string()),
        ("small", SMALL_SOURCE.to_string()),
        ("medium", MEDIUM_SOURCE.to_string()),
        ("large_100", generate_large_source(100)),
    ] {
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_with_input(BenchmarkId::new("parse", name), &source, |b, src| {
            let source_file = source_from_string("bench.arth", src);

            b.iter(|| {
                let mut reporter = Reporter::new();
                let result = parse_file(&source_file, &mut reporter);
                black_box(result)
            })
        });
    }

    group.finish();
}

// ============================================================================
// Stress Test Benchmarks
// ============================================================================

fn benchmark_stress_tests(c: &mut Criterion) {
    let mut group = c.benchmark_group("stress");

    // Deep nesting stress test
    for depth in [10, 25, 50] {
        let source = generate_nested_source(depth);
        group.bench_with_input(
            BenchmarkId::new("nested_depth", depth),
            &source,
            |b, src| {
                let source_file = source_from_string("bench.arth", src);

                b.iter(|| {
                    let mut reporter = Reporter::new();
                    let result = parse_file(&source_file, &mut reporter);
                    black_box(result)
                })
            },
        );
    }

    // Many generic types stress test
    for num_types in [10, 50, 100] {
        let source = generate_generic_source(num_types);
        group.bench_with_input(
            BenchmarkId::new("generic_types", num_types),
            &source,
            |b, src| {
                let source_file = source_from_string("bench.arth", src);

                b.iter(|| {
                    let mut reporter = Reporter::new();
                    let result = parse_file(&source_file, &mut reporter);
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// Scaling Benchmarks
// ============================================================================

fn benchmark_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("scaling");

    // Test how performance scales with source size
    for num_funcs in [10, 25, 50, 100, 200, 500] {
        let source = generate_large_source(num_funcs);
        group.throughput(Throughput::Elements(num_funcs as u64));

        group.bench_with_input(
            BenchmarkId::new("full_parse", num_funcs),
            &source,
            |b, src| {
                let source_file = source_from_string("bench.arth", src);

                b.iter(|| {
                    let mut reporter = Reporter::new();
                    let result = parse_file(&source_file, &mut reporter);
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// Main Benchmark Groups
// ============================================================================

criterion_group!(
    benches,
    benchmark_lexer_throughput,
    benchmark_parser_throughput,
    benchmark_stress_tests,
    benchmark_scaling,
);

criterion_main!(benches);
