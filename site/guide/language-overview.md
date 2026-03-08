# Language Overview

Arth is a statically typed, compiled language. This page tours the major features.

## Primitives and Variables

```java
int count = 42;
double pi = 3.14159;
boolean active = true;
String name = "Arth";
char letter = 'A';

// Type inference
var x = 10;       // inferred as int
var msg = "hi";   // inferred as String
```

Variables are immutable by default when declared `final`:

```java
final int max = 100;    // cannot reassign
int counter = 0;        // mutable
counter = counter + 1;  // OK
```

## Structs

Plain data containers. No methods, no inheritance.

```java
struct Rectangle {
    final double width;
    final double height;
}

// Construction
Rectangle r = Rectangle { width: 10.0, height: 5.0 };
```

## Modules

Modules group behavior and can implement interfaces:

```java
interface Area<T> {
    double area(T self);
}

module RectFns implements Area<Rectangle> {
    public double area(Rectangle self) {
        return self.width * self.height;
    }

    public double perimeter(Rectangle self) {
        return 2.0 * (self.width + self.height);
    }
}

// Call via module
double a = RectFns.area(r);

// Or via .then() sugar
double a = r.then(RectFns.area());
```

## Enums

Tagged unions with associated data:

```java
enum Shape {
    Circle(double radius),
    Rect(double w, double h),
    Point
}

// Pattern matching
String describe(Shape s) {
    match (s) {
        Circle(r) => return "Circle r=" + r;
        Rect(w, h) => return "Rect " + w + "x" + h;
        Point => return "Point";
    }
}
```

## Optional

No null. Use `Optional<T>`:

```java
Optional<String> find(String key) {
    if (map.contains(key)) {
        return Optional.of(map.get(key));
    }
    return Optional.empty();
}

// Usage
Optional<String> val = find("name");
String name = val.orElse("unknown");
```

## Error Handling

### Typed Exceptions

```java
struct ParseError { final String message; final int line; }
struct IoError { final String cause; }

public Config load(String path) throws (IoError, ParseError) {
    String content = readFile(path);    // may throw IoError
    return parse(content);              // may throw ParseError
}

// Caller must handle all declared exceptions
try {
    Config cfg = load("app.toml");
} catch (IoError e) {
    println("IO failed: " + e.cause);
} catch (ParseError e) {
    println("Parse error at line " + e.line + ": " + e.message);
}
```

## Providers

Explicit, managed shared state:

```java
provider Logger {
    public final String prefix;
    public shared List<String> entries;
}

module LoggerFns {
    public Logger create(String prefix) {
        return Logger { prefix: prefix, entries: List.empty() };
    }

    public void log(Logger self, String msg) {
        self.entries.add(self.prefix + ": " + msg);
    }
}
```

## Concurrency

### Actors

```java
actor BankAccount {
    private double balance = 0.0;

    public void deposit(double amount) {
        balance += amount;
    }

    public double getBalance() {
        return balance;
    }
}
```

### Async/Await

```java
public async Response fetchData(String url) throws (HttpError) {
    Response res = await Http.get(url);
    return res;
}
```

## Generics

```java
struct Pair<A, B> {
    final A first;
    final B second;
}

module PairFns {
    public <A, B> Pair<A, B> of(A a, B b) {
        return Pair { first: a, second: b };
    }

    public <A, B> A first(Pair<A, B> self) {
        return self.first;
    }
}
```

## Ownership

Arth tracks ownership at compile time. When a value is moved, the original binding becomes invalid:

```java
String a = "hello";
String b = a;          // a is moved to b
// println(a);         // compile error: a was moved

// Borrowing for read access
void printLength(borrow String s) {
    println(s.length());
}

printLength(borrow b);  // b is borrowed, not moved
println(b);             // b is still valid
```

The compiler infers lifetimes — you never write lifetime annotations.

## Next Steps

- [Functions & Modules](guide/functions-modules.md) — Interfaces, `.then()` chaining, module conformance
- [Ownership & Borrowing](guide/ownership.md) — Deep dive into the memory model
- [Error Handling](guide/error-handling.md) — Exception hierarchies and recovery patterns
- [Providers](guide/providers.md) — State management patterns
