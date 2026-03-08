# Ownership & Borrowing

Arth's memory model gives you Rust-grade safety with zero lifetime annotations.

## The Core Idea

Every value has exactly one **owner**. When the owner goes out of scope, the value is destroyed. No garbage collector needed.

```java
void example() {
    String s = "hello";    // s owns the string
    // ...
}                          // s goes out of scope, memory freed
```

## Moves

Assigning a value to a new variable **moves** ownership. The original binding becomes invalid:

```java
String a = "hello";
String b = a;          // ownership moves from a to b
// println(a);         // compile error: a was moved
println(b);            // OK: b owns the value
```

This prevents double-free bugs and use-after-free — at compile time.

### Moves in Function Calls

Passing a value to a function also moves it:

```java
void consume(String s) {
    println(s);
}

String msg = "hello";
consume(msg);          // msg is moved into consume()
// println(msg);       // compile error: msg was moved
```

## Borrowing

Sometimes you want to use a value without taking ownership. That's **borrowing**:

```java
void printLength(borrow String s) {
    println(s.length());    // read access only
}

String msg = "hello";
printLength(borrow msg);   // msg is borrowed, not moved
println(msg);              // OK: msg is still valid
```

### Mutable Borrows

To modify a borrowed value:

```java
void increment(borrow mut int[] counter) {
    counter[0] += 1;
}
```

### Borrowing Rules

The compiler enforces these rules at compile time:

1. **One mutable borrow OR any number of immutable borrows** — never both at the same time
2. **Borrows cannot outlive the owner** — no dangling references
3. **No mutation through immutable borrows** — data races prevented

```java
String s = "hello";
borrow String r1 = borrow s;     // OK: immutable borrow
borrow String r2 = borrow s;     // OK: multiple immutable borrows
// borrow mut String r3 = borrow mut s;  // ERROR: can't mix mutable + immutable
```

## No Lifetime Annotations

Unlike Rust, you never write lifetime parameters. The compiler infers them:

```java
// Rust would require: fn first<'a>(list: &'a [String]) -> &'a String
// Arth infers this automatically:
borrow String first(borrow List<String> list) {
    return list.get(0);
}
```

The compiler tracks lifetimes internally to ensure safety, but you don't need to reason about them explicitly.

## Structs and Ownership

Struct fields follow the same ownership rules:

```java
struct Document {
    final String title;
    final String content;
}

// Moving a struct moves all its fields
Document doc = Document { title: "README", content: "..." };
Document copy = doc;    // doc is moved
// doc.title;           // compile error
```

### Final vs Mutable Fields

```java
struct Config {
    final String name;     // immutable after construction
    int retries;           // mutable
}

Config cfg = Config { name: "app", retries: 3 };
cfg.retries = 5;          // OK: mutable field
// cfg.name = "other";    // compile error: final field
```

## Deterministic Cleanup

Values are destroyed at the end of their scope, in reverse order of creation:

```java
void process() {
    File f = File.open("data.txt");     // opened first
    Buffer b = Buffer.allocate(1024);   // allocated second

    // ... use f and b ...

}   // b destroyed first, then f — reverse order, deterministic
```

No finalizers, no GC pauses, no unpredictable cleanup timing.
