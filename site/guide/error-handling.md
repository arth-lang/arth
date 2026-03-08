# Error Handling

Arth uses **typed exceptions** — every function declares what it can throw, and the compiler ensures callers handle every case.

## Declaring Exceptions

Exception types are just structs:

```java
struct NotFoundError {
    final String resource;
    final int id;
}

struct ValidationError {
    final String field;
    final String message;
}
```

## Throwing Exceptions

Use `throws` in the function signature to declare throwable types:

```java
public User findUser(int id) throws (NotFoundError) {
    Optional<User> user = db.find(id);
    if (user.isEmpty()) {
        throw NotFoundError { resource: "User", id: id };
    }
    return user.get();
}
```

### Multiple Exception Types

```java
public Config loadConfig(String path) throws (IoError, ParseError) {
    String content = readFile(path);         // may throw IoError
    Config cfg = parseToml(content);         // may throw ParseError
    return cfg;
}
```

## Catching Exceptions

The compiler ensures you handle every declared exception type:

```java
try {
    Config cfg = loadConfig("app.toml");
    println("Loaded: " + cfg.name);
} catch (IoError e) {
    println("Could not read file: " + e.cause);
} catch (ParseError e) {
    println("Invalid config at line " + e.line);
}
```

### Missing a catch is a compile error:

```java
try {
    Config cfg = loadConfig("app.toml");
} catch (IoError e) {
    // handle IO
}
// compile error: ParseError is not caught
```

## Finally Blocks

`finally` runs regardless of whether an exception was thrown:

```java
File f = File.open("data.txt");
try {
    process(f);
} catch (ProcessError e) {
    log("Processing failed: " + e.message);
} finally {
    f.close();    // always runs
}
```

## Propagation

If a function calls something that throws, it must either catch or declare:

```java
// Option 1: Catch it
public void handleRequest() {
    try {
        User u = findUser(42);
    } catch (NotFoundError e) {
        respond(404, e.resource + " not found");
    }
}

// Option 2: Propagate it
public User getUser(int id) throws (NotFoundError) {
    return findUser(id);    // NotFoundError propagates to caller
}
```

## Inline Catch

For concise error handling in expressions:

```java
User user = findUser(id) catch (NotFoundError e) {
    return defaultUser();
};
```

## Why Not Result Types?

Languages like Rust use `Result<T, E>`. Arth chose typed exceptions because:

1. **Familiar to Java/Kotlin developers** — same `try`/`catch`/`throws` patterns
2. **Compiler-checked** — unlike Java's unchecked exceptions, Arth enforces exhaustive handling
3. **Cleaner call sites** — no `.unwrap()` or `?` operator needed; errors flow naturally through `throws` declarations
4. **Structured cleanup** — `finally` blocks provide deterministic resource cleanup tied to exception flow
