//! Compiler Performance Benchmarks
//!
//! Criterion benchmarks for the Arth compiler pipeline.
//! Run with: `cargo bench --bench compiler_benchmarks`

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

use arth::compiler::lexer::lex_all;

// ============================================================================
// Sample Source Code
// ============================================================================

const MINIMAL_SOURCE: &str = r#"
package test;

public void main() {
    Int x = 42;
}
"#;

const SMALL_SOURCE: &str = r#"
package test;

public Int add(Int a, Int b) {
    return a + b;
}

public Int factorial(Int n) {
    if (n <= 1) {
        return 1;
    }
    return n * factorial(n - 1);
}

public void main() {
    Int result = add(10, 20);
    Int fact = factorial(5);
}
"#;

const MEDIUM_SOURCE: &str = r#"
package test;

struct Point {
    Int x;
    Int y;
}

struct Rectangle {
    Point topLeft;
    Point bottomRight;
}

module PointOps {
    public Point new(Int x, Int y) {
        return Point { x: x, y: y };
    }

    public Int distance(Point a, Point b) {
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

module RectOps {
    public Rectangle new(Point tl, Point br) {
        return Rectangle { topLeft: tl, bottomRight: br };
    }

    public Int area(Rectangle r) {
        Int width = r.bottomRight.x - r.topLeft.x;
        Int height = r.bottomRight.y - r.topLeft.y;
        return width * height;
    }

    public Bool contains(Rectangle r, Point p) {
        return p.x >= r.topLeft.x &&
               p.x <= r.bottomRight.x &&
               p.y >= r.topLeft.y &&
               p.y <= r.bottomRight.y;
    }
}

public void main() {
    Point p1 = PointOps.new(0, 0);
    Point p2 = PointOps.new(10, 10);

    Rectangle rect = RectOps.new(p1, p2);
    Int a = RectOps.area(rect);

    Point test = PointOps.new(5, 5);
    Bool inside = RectOps.contains(rect, test);
}
"#;

fn generate_large_source(num_functions: usize) -> String {
    let mut source = String::from("package test;\n\n");

    for i in 0..num_functions {
        source.push_str(&format!(
            r#"
public Int func_{}(Int a, Int b, Int c) {{
    Int x = a + b;
    Int y = b + c;
    Int z = x * y;
    if (z > 100) {{
        return z - 100;
    }} else {{
        return z + 100;
    }}
}}
"#,
            i
        ));
    }

    source.push_str("\npublic void main() {\n");
    for i in 0..num_functions.min(10) {
        source.push_str(&format!("    Int r{} = func_{}(1, 2, 3);\n", i, i));
    }
    source.push_str("}\n");

    source
}

// ============================================================================
// Lexer Benchmarks
// ============================================================================

fn benchmark_lexer(c: &mut Criterion) {
    let mut group = c.benchmark_group("lexer");

    // Minimal source
    group.throughput(Throughput::Bytes(MINIMAL_SOURCE.len() as u64));
    group.bench_function("minimal", |b| {
        b.iter(|| {
            let tokens = lex_all(black_box(MINIMAL_SOURCE));
            black_box(tokens)
        })
    });

    // Small source
    group.throughput(Throughput::Bytes(SMALL_SOURCE.len() as u64));
    group.bench_function("small", |b| {
        b.iter(|| {
            let tokens = lex_all(black_box(SMALL_SOURCE));
            black_box(tokens)
        })
    });

    // Medium source
    group.throughput(Throughput::Bytes(MEDIUM_SOURCE.len() as u64));
    group.bench_function("medium", |b| {
        b.iter(|| {
            let tokens = lex_all(black_box(MEDIUM_SOURCE));
            black_box(tokens)
        })
    });

    // Large source (50 functions)
    let large_source = generate_large_source(50);
    group.throughput(Throughput::Bytes(large_source.len() as u64));
    group.bench_function("large_50_funcs", |b| {
        b.iter(|| {
            let tokens = lex_all(black_box(&large_source));
            black_box(tokens)
        })
    });

    group.finish();
}

// ============================================================================
// Scaling Benchmarks
// ============================================================================

fn benchmark_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("scaling");

    for num_funcs in [10, 25, 50, 100].iter() {
        let source = generate_large_source(*num_funcs);

        group.throughput(Throughput::Elements(*num_funcs as u64));

        group.bench_with_input(
            BenchmarkId::new("lex_functions", num_funcs),
            &source,
            |b, src| {
                b.iter(|| {
                    let tokens = lex_all(black_box(src));
                    black_box(tokens)
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// Token-Specific Benchmarks
// ============================================================================

fn benchmark_token_types(c: &mut Criterion) {
    let mut group = c.benchmark_group("token_types");

    // Number-heavy source
    let number_source: String = (0..100)
        .map(|i| format!("Int x{} = {};\n", i, i * 12345))
        .collect::<Vec<_>>()
        .join("");
    let number_source = format!(
        "package test;\npublic void main() {{\n{}}}\n",
        number_source
    );

    group.bench_function("numbers", |b| {
        b.iter(|| {
            let tokens = lex_all(black_box(&number_source));
            black_box(tokens)
        })
    });

    // String-heavy source
    let string_source: String = (0..100)
        .map(|i| format!("String s{} = \"hello world string number {}\";\n", i, i))
        .collect::<Vec<_>>()
        .join("");
    let string_source = format!(
        "package test;\npublic void main() {{\n{}}}\n",
        string_source
    );

    group.bench_function("strings", |b| {
        b.iter(|| {
            let tokens = lex_all(black_box(&string_source));
            black_box(tokens)
        })
    });

    // Identifier-heavy source
    let ident_source: String = (0..100)
        .map(|i| format!("someLongVariableName{} = anotherLongName{};\n", i, i))
        .collect::<Vec<_>>()
        .join("");
    let ident_source = format!("package test;\npublic void main() {{\n{}}}\n", ident_source);

    group.bench_function("identifiers", |b| {
        b.iter(|| {
            let tokens = lex_all(black_box(&ident_source));
            black_box(tokens)
        })
    });

    // Operator-heavy source
    let op_source: String = (0..50)
        .map(|i| format!("x = a{} + b{} - c{} * d{} / e{} % f{};\n", i, i, i, i, i, i))
        .collect::<Vec<_>>()
        .join("");
    let op_source = format!(
        "package test;\npublic void main() {{\nInt x;\n{}}}\n",
        op_source
    );

    group.bench_function("operators", |b| {
        b.iter(|| {
            let tokens = lex_all(black_box(&op_source));
            black_box(tokens)
        })
    });

    group.finish();
}

// ============================================================================
// Main Benchmark Groups
// ============================================================================

criterion_group!(
    benches,
    benchmark_lexer,
    benchmark_scaling,
    benchmark_token_types,
);

criterion_main!(benches);
