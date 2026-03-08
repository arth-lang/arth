# Arth

> A systems programming language with Java-like syntax and Rust-grade memory safety.

## Write like Java. Run like Rust.

Arth gives you the syntax millions of developers already know, backed by compile-time ownership and borrowing — without lifetime annotations or a garbage collector.

```java
struct Task {
    final String name;
    final int priority;
}

module TaskFns {
    public Task create(String name, int priority) {
        return Task { name: name, priority: priority };
    }

    public boolean isUrgent(Task self) {
        return self.priority > 8;
    }
}

public void main() {
    Task task = TaskFns.create("Deploy", 9);

    if (task.then(TaskFns.isUrgent())) {
        println("Handle immediately: " + task.name);
    }
}
```

## Key Features

### Familiar Syntax, Modern Safety

If you've written Java, Kotlin, or TypeScript — you can read Arth immediately. But under the hood, the compiler enforces Rust-grade memory safety: ownership tracking, borrow checking, and deterministic cleanup. No GC pauses, no dangling pointers, no data races.

### Modules Over Methods

Behavior doesn't live inside structs. Instead, **modules** group related functions and implement interfaces. This keeps data and logic separate, making code easier to test, compose, and reason about.

```java
module JsonFns implements Serializable<User> {
    public String serialize(User self) {
        return "{\"name\": \"" + self.name + "\"}";
    }
}
```

### Providers for State

Global mutable state is a common source of bugs. Arth replaces globals with **providers** — explicitly declared, lifetime-managed containers for shared state.

```java
provider Database {
    public final String url;
    public shared ConnectionPool pool;
}
```

### Typed Exceptions

Every function declares what it can throw. The compiler ensures callers handle every case. No unchecked surprises at runtime.

```java
public User findUser(int id) throws (NotFoundError, DbError) {
    Row row = db.query("SELECT * FROM users WHERE id = ?", id);
    return UserFns.fromRow(row);
}
```

### No Null

`null` doesn't exist in Arth. Use `Optional<T>` instead — the type system ensures you handle the empty case.

```java
Optional<User> user = findById(42);
String name = user.map(UserFns.name()).orElse("anonymous");
```

### TypeScript Interop

Write frontend logic in TypeScript, backend in Arth. Both compile through the same pipeline and share types.

## Backends

| Backend | Best For |
|---------|----------|
| **VM** | Development, scripting, fast iteration. Portable `.abc` bytecode. |
| **Cranelift** | JIT compilation for hot functions in the VM. Feature-gated (`--features cranelift`). |
| **LLVM** | Production. Native AOT binaries with full debug info and optimizations. |

## Get Started

```bash
# Build the compiler
cargo build

# Run your first program
arth run examples/arth-sample/src/demo/Hello.arth

# Native compilation
arth build --backend llvm your_program.arth
```

[Installation Guide](guide/installation.md) · [Hello World Tutorial](guide/hello-world.md) · [Language Overview](guide/language-overview.md)
