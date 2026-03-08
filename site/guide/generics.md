# Generics

Arth supports generic types, functions, and interfaces with type inference.

## Generic Structs

```java
struct Pair<A, B> {
    final A first;
    final B second;
}

Pair<String, int> entry = Pair { first: "age", second: 30 };
```

## Generic Functions

```java
public <T> T identity(T value) {
    return value;
}

public <T> List<T> repeat(T value, int count) {
    List<T> result = List.empty();
    for (int i = 0; i < count; i++) {
        result = result.append(value);
    }
    return result;
}

// Type is inferred at the call site
String s = identity("hello");          // T = String
List<int> fives = repeat(5, 3);        // T = int → [5, 5, 5]
```

## Generic Interfaces

```java
interface Mapper<In, Out> {
    Out map(In value);
}

interface Repository<T> {
    T findById(int id) throws (NotFoundError);
    List<T> findAll();
    void save(T entity) throws (ValidationError);
}
```

## Bounded Generics

Constrain type parameters with interface bounds:

```java
public <T implements Comparable<T>> T max(T a, T b) {
    if (a.compareTo(b) > 0) {
        return a;
    }
    return b;
}

public <T implements Serializable<T>> String toJson(T value) {
    return value.then(JsonFns.serialize());
}
```

### Multiple Bounds

```java
public <T implements Comparable<T>, Printable<T>> void sortAndPrint(List<T> items) {
    List<T> sorted = items.sort((T a, T b) -> a.compareTo(b));
    for (T item : sorted) {
        println(item.then(PrintFns.format()));
    }
}
```

## Generic Modules

```java
module ListFns {
    public <T> Optional<T> first(List<T> list) {
        if (list.length() == 0) {
            return Optional.empty();
        }
        return Optional.of(list.get(0));
    }

    public <T> List<T> filter(List<T> list, Predicate<T> pred) {
        List<T> result = List.empty();
        for (T item : list) {
            if (pred.test(item)) {
                result = result.append(item);
            }
        }
        return result;
    }

    public <T, R> List<R> map(List<T> list, Mapper<T, R> fn) {
        List<R> result = List.empty();
        for (T item : list) {
            result = result.append(fn.map(item));
        }
        return result;
    }
}
```

## Generic Enums

```java
enum Result<T> {
    Ok(T value),
    Err(String message)
}

enum Tree<T> {
    Leaf(T value),
    Branch(Tree<T> left, Tree<T> right)
}
```

## Type Inference

The compiler infers generic type arguments at call sites. You rarely need to specify them explicitly:

```java
// All types inferred
var pair = Pair { first: "name", second: 42 };   // Pair<String, int>
var items = List.of(1, 2, 3);                     // List<int>
var first = ListFns.first(items);                  // Optional<int>
```
