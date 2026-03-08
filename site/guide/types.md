# Types & Variables

## Primitive Types

| Type | Description | Example |
|------|-------------|---------|
| `int` | 64-bit signed integer | `42`, `0xFF`, `0b1010`, `1_000_000` |
| `double` | 64-bit floating point | `3.14`, `1.0e-5` |
| `boolean` | True or false | `true`, `false` |
| `char` | Unicode character | `'A'`, `'\n'` |
| `String` | UTF-8 text | `"hello"` |
| `void` | No value | (return type only) |

### Numeric Literals

```java
int decimal = 42;
int hex = 0xFF;
int binary = 0b1010;
int readable = 1_000_000;    // underscores for readability

double pi = 3.14159;
double tiny = 1.0e-10;
```

### Type Aliases

Arth provides shorter aliases for common types:

| Alias | Full Type |
|-------|-----------|
| `i8`, `i16`, `i32`, `i64` | Sized signed integers |
| `u8`, `u16`, `u32`, `u64` | Sized unsigned integers |
| `f32`, `f64` | Sized floats |

## Variables

### Immutable by Default

Use `final` for values that never change:

```java
final String name = "Arth";
final int version = 1;
// name = "other";  // compile error
```

### Mutable Variables

Omit `final` for mutable bindings:

```java
int counter = 0;
counter = counter + 1;    // OK
counter += 1;             // also OK
```

### Type Inference

Use `var` when the type is obvious from context:

```java
var count = 42;          // inferred as int
var name = "Alice";      // inferred as String
var list = List.of(1, 2, 3);  // inferred as List<int>
```

## Structs

Plain data containers with named fields:

```java
struct Point {
    final double x;
    final double y;
}

struct MutableCounter {
    int value;    // mutable field (no final)
}
```

### Construction

```java
Point origin = Point { x: 0.0, y: 0.0 };
Point p = Point { x: 3.0, y: 4.0 };
```

### Field Access

```java
double px = p.x;
double py = p.y;
```

## Enums

Tagged unions — each variant can carry data:

```java
enum Color {
    Red,
    Green,
    Blue,
    Custom(int r, int g, int b)
}

enum Result<T> {
    Ok(T value),
    Err(String message)
}
```

### Pattern Matching

```java
String describe(Color c) {
    match (c) {
        Red => return "red";
        Green => return "green";
        Blue => return "blue";
        Custom(r, g, b) => return "rgb(" + r + "," + g + "," + b + ")";
    }
}
```

## Optional

Arth has no `null`. Use `Optional<T>` to represent values that might be absent:

```java
Optional<String> findName(int id) {
    if (exists(id)) {
        return Optional.of(getName(id));
    }
    return Optional.empty();
}

// Consuming optionals
Optional<String> name = findName(42);
String display = name.orElse("unknown");

// Chaining
Optional<int> len = name.map(StringFns.length());
```

## Collections

### Lists

```java
List<int> nums = List.of(1, 2, 3);
int first = nums.get(0);
int size = nums.length();
```

### Maps

```java
Map<String, int> ages = Map.of("Alice", 30, "Bob", 25);
Optional<int> age = ages.get("Alice");
```

## Type Casting

Arth is strongly typed. Conversions between numeric types are explicit:

```java
int x = 42;
double d = x.toDouble();    // explicit widening
int back = d.toInt();       // explicit narrowing (truncates)
```
