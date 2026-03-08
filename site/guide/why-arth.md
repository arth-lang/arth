# Why Arth?

## The Problem

Developers face a tradeoff:

- **Java, Kotlin, Go** — Familiar syntax, productive, but garbage-collected. GC pauses hurt latency-sensitive workloads. No control over memory layout.
- **Rust** — Memory safe, fast, no GC. But the learning curve is steep — lifetimes, trait bounds, and borrow checker errors slow down teams.
- **C, C++** — Fast, full control. But memory bugs are the #1 source of security vulnerabilities.

## Arth's Answer

Arth takes the syntax developers already know (Java/TypeScript) and backs it with Rust-grade compile-time safety — without exposing lifetime annotations.

| | Java | Rust | Arth |
|---|---|---|---|
| **Familiar syntax** | Yes | No | Yes |
| **Memory safe** | GC | Compile-time | Compile-time |
| **No GC pauses** | No | Yes | Yes |
| **Lifetime annotations** | N/A | Required | Inferred |
| **Null safety** | `@Nullable` | `Option<T>` | `Optional<T>` |
| **Error handling** | Checked exceptions | `Result<T, E>` | Typed `throws` |

## Design Choices

### Modules, Not Methods

In Arth, structs hold data. Modules hold behavior. This isn't just stylistic — it enables cleaner composition, easier testing, and interface-based polymorphism without inheritance hierarchies.

```java
// Data is plain
struct Point { final double x; final double y; }

// Behavior is separate
module PointFns implements Printable<Point> {
    public String format(Point self) {
        return "(" + self.x + ", " + self.y + ")";
    }

    public double distance(Point a, Point b) {
        double dx = a.x - b.x;
        double dy = a.y - b.y;
        return Math.sqrt(dx * dx + dy * dy);
    }
}
```

### Providers, Not Globals

Global mutable state causes bugs that are hard to trace. Arth's providers make shared state explicit, with clear ownership and lifetime semantics.

### Typed Exceptions, Not Unchecked Panics

Every function declares what it can throw. The compiler ensures exhaustive handling. Unlike Java's checked exceptions, Arth's exception types are lightweight and composable.

### TypeScript as a First-Class Frontend

TypeScript files compile through the same pipeline as Arth. Share types between your TS frontend and Arth backend. One compiler, one type system, one binary.

## Who Is Arth For?

- **Java/Kotlin developers** who want native performance without learning Rust's syntax
- **Systems programmers** who want memory safety with less ceremony
- **Teams** who need both TypeScript frontends and safe backend code in one toolchain
- **Anyone** building latency-sensitive applications where GC pauses are unacceptable
