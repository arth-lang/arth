# Functions & Modules

## Functions

### Basic Functions

```java
int add(int a, int b) {
    return a + b;
}

void greet(String name) {
    println("Hello, " + name);
}
```

### Visibility

Functions are package-private by default. Use `public` for external access:

```java
public int add(int a, int b) {    // visible outside package
    return a + b;
}

int helper(int x) {               // package-private
    return x * 2;
}
```

### Generic Functions

```java
public <T> T identity(T value) {
    return value;
}

public <T> Optional<T> firstWhere(List<T> list, Predicate<T> pred) {
    for (T item : list) {
        if (pred.test(item)) {
            return Optional.of(item);
        }
    }
    return Optional.empty();
}
```

## Modules

Modules are Arth's primary unit of behavior. They group related functions and can implement interfaces.

### Basic Module

```java
struct Circle {
    final double radius;
}

module CircleFns {
    public Circle create(double radius) {
        return Circle { radius: radius };
    }

    public double area(Circle self) {
        return Math.PI * self.radius * self.radius;
    }

    public double circumference(Circle self) {
        return 2.0 * Math.PI * self.radius;
    }
}
```

### The `self` Parameter

When a module function takes a struct as its first parameter named `self`, it can be called with `.then()` syntax:

```java
Circle c = CircleFns.create(5.0);

// Direct call
double a1 = CircleFns.area(c);

// .then() sugar — same thing
double a2 = c.then(CircleFns.area());
```

### Why Modules?

In Java or Kotlin, you'd put `area()` inside the `Circle` class. Arth separates data from behavior for several reasons:

1. **Multiple implementations** — Different modules can operate on the same struct
2. **Interface conformance** — A module can implement multiple interfaces for a type
3. **Testability** — Functions are stateless and easy to test in isolation
4. **No inheritance** — No fragile base class problems

### Implementing Interfaces

```java
interface Printable<T> {
    String format(T self);
}

interface Measurable<T> {
    double measure(T self);
}

module CircleFns implements Printable<Circle>, Measurable<Circle> {
    public String format(Circle self) {
        return "Circle(r=" + self.radius + ")";
    }

    public double measure(Circle self) {
        return self.radius * 2.0;    // diameter
    }
}
```

### Module Conformance

The compiler checks that a module implements all required interface methods with correct signatures. Missing or mismatched methods are compile errors.

## Interfaces

### Defining Interfaces

```java
interface Serializable<T> {
    String serialize(T self);
    T deserialize(String data) throws (ParseError);
}

interface Comparable<T> {
    int compareTo(T self, T other);
}
```

### Generic Interfaces

```java
interface Transform<In, Out> {
    Out apply(In value);
}

interface CacheKey<T> {
    String key(T self);
}
```

## Closures and Lambdas

```java
// Lambda syntax
var double_it = (int x) -> x * 2;

// Closures capture their environment
int multiplier = 3;
var scale = (int x) -> x * multiplier;

// Passing to higher-order functions
List<int> nums = List.of(1, 2, 3, 4, 5);
List<int> evens = nums.filter((int n) -> n % 2 == 0);
List<int> doubled = nums.map((int n) -> n * 2);
```

## Packages and Imports

Every file starts with a package declaration:

```java
package app.services;

import app.models.User;
import app.utils.*;              // star import

public module UserService {
    // ...
}
```

Package names map to directory structure: `app.services` → `src/app/services/`.
