# Language Specification

> This is a summary of the Arth language specification. For the full normative spec, see [spec.md](https://github.com/splentainc/arth/blob/main/docs/reference/spec.md) in the repository.

## Lexical Structure

### Keywords

```
abstract  actor     as        async     await     boolean
break     byte      case      catch     char      class
const     continue  default   do        double    else
enum      extends   false     final     finally   float
for       if        implements import   instanceof int
interface long      match     module   new       null
optional  package   private   protected provider  public
return    sealed    shared    short    static    string
struct    super     switch    synchronized this   throw
throws    true      try       var      void      while
```

### Operators

```
+  -  *  /  %  **          // arithmetic
== != < > <= >=             // comparison
&& || !                     // logical
& | ^ ~ << >>              // bitwise
= += -= *= /= %= **=       // assignment
-> =>                       // arrow, fat arrow
:: .                        // scope, member access
```

### Literals

```java
42              // int
0xFF            // hex int
0b1010          // binary int
1_000_000       // underscored int
3.14            // double
'A'             // char
"hello"         // string
true / false    // boolean
```

## Declarations

### Structs
```java
struct Name {
    [final] Type field;
    ...
}
```

### Enums
```java
enum Name {
    Variant,
    Variant(Type field, ...),
    ...
}
```

### Interfaces
```java
interface Name<T> {
    ReturnType method(T self, ...);
    ...
}
```

### Modules
```java
module Name [implements Interface<T>, ...] {
    [public] ReturnType function(params) [throws (E1, E2)] { ... }
    ...
}
```

### Providers
```java
provider Name {
    [public] final Type field;
    [public] shared Type field;
    ...
}
```

## Control Flow

```java
// If/else
if (condition) { ... } else if (other) { ... } else { ... }

// For loop
for (Type item : collection) { ... }
for (int i = 0; i < n; i++) { ... }

// While
while (condition) { ... }

// Match (pattern matching)
match (value) {
    Pattern => expression;
    Pattern(bindings) => { block };
}

// Try/catch/finally
try { ... } catch (ErrorType e) { ... } finally { ... }
```

## Memory Model

- Every value has exactly one owner
- Assignment moves ownership (original becomes invalid)
- `borrow` for read access without ownership transfer
- `borrow mut` for mutable borrows
- One mutable borrow XOR any number of immutable borrows
- Lifetimes inferred by the compiler (never written explicitly)
- Deterministic destruction at scope exit

## Concurrency Model

- `actor` — isolated mutable state with message-passing
- `async` / `await` — cooperative async functions returning `Task<T>`
- `Channel<T>` — typed communication between threads
- `Sendable` — types safe to move between threads
- `Shareable` — types safe to share between threads
- `shared` fields — thread-safe mutable state in providers
