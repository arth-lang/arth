# Arth

A systems programming language with Java-like syntax and Rust-grade memory safety.

> No garbage collector. No lifetime annotations. Just write code.

```arth
struct User {
    final String name;
    final int age;
}

module UserFns {
    public User create(String name, int age) {
        return User { name: name, age: age };
    }

    public String greet(User self) {
        return "Hello, " + self.name;
    }
}

public void main() {
    User user = UserFns.create("Alice", 30);
    String msg = user.then(UserFns.greet());
    println(msg);
}
```

## Why Arth?

- **Java-familiar syntax** — If you know Java, you can read Arth on day one
- **Rust-grade memory safety** — Ownership and borrowing, enforced at compile time
- **No lifetime annotations** — The compiler infers lifetimes for you
- **No garbage collector** — Deterministic memory reclamation, zero pauses
- **Modules over methods** — Behavior lives in modules, not inside structs
- **Typed exceptions** — `throws` clauses checked at compile time
- **No null** — `Optional<T>` everywhere, enforced by the type system
- **TypeScript interop** — Mixed Arth + TS codebases, single compilation pipeline

## Quick Start

```bash
# Build the compiler
cargo build

# Run a program
arth run examples/arth-sample/src/demo/Hello.arth

# Build native binary (LLVM backend)
arth build --backend llvm examples/arth-sample/src/demo/Hello.arth
```

## Language Highlights

### Providers for Shared State

No globals. Long-lived state is explicit and managed:

```arth
provider AppConfig {
    public final String env;
    public shared Map<String, Atomic<int>> counters;
}
```

### Typed Exceptions

Every throwable path is declared and checked:

```arth
public Response fetch(Request r) throws (IoError, TimeoutError) {
    Connection conn = connect(r.host) catch (IoError e) {
        return Response.error(e);
    };
    return conn.send(r);
}
```

### Concurrency with Actors and Channels

```arth
actor Counter {
    private int count = 0;

    public void increment() { count += 1; }
    public int get() { return count; }
}
```

## Backends

| Backend | Description |
|---------|-------------|
| **VM** | Portable bytecode (`.abc`), fast iteration, optional Cranelift JIT |
| **LLVM** | Native AOT compilation with full debug info |

## Tooling

- **LSP server** — Diagnostics, completions, go-to-definition
- **VSCode extension** — Syntax highlighting included
- **Formatter** — `arth fmt` for consistent style

## Documentation

Full documentation at [arth-lang.github.io/arth](https://arth-lang.github.io/arth)

## Contributing

```bash
cargo test                    # Run tests
cargo fmt --all               # Format
cargo clippy --all-targets -- -D warnings  # Lint
```

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/).

## License

[Apache License 2.0](LICENSE)
