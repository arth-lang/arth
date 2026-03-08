# Hello World

## Your First Program

Create a file called `Hello.arth`:

```java
package hello;

public void main() {
    println("Hello, world!");
}
```

Run it:

```bash
arth run Hello.arth
```

## A Step Further

Let's use structs, modules, and typed exceptions:

```java
package greeter;

struct Person {
    final String name;
    final int age;
}

module PersonFns {
    public Person create(String name, int age) throws (ValidationError) {
        if (age < 0) {
            throw ValidationError { message: "Age cannot be negative" };
        }
        return Person { name: name, age: age };
    }

    public String greet(Person self) {
        return "Hi, I'm " + self.name + " (" + self.age + ")";
    }
}

struct ValidationError {
    final String message;
}

public void main() {
    try {
        Person alice = PersonFns.create("Alice", 30);
        println(alice.then(PersonFns.greet()));

        Person bob = PersonFns.create("Bob", -1);  // throws!
    } catch (ValidationError e) {
        println("Error: " + e.message);
    }
}
```

```bash
arth run greeter.arth
# Output:
# Hi, I'm Alice (30)
# Error: Age cannot be negative
```

## What Just Happened?

1. **`struct Person`** — Plain data. No methods, no inheritance. Fields are `final` (immutable by default).
2. **`module PersonFns`** — Behavior grouped in a module. The `self` parameter makes `greet` callable via `.then()` syntax.
3. **`throws (ValidationError)`** — The function declares it can throw. The compiler forces callers to handle it.
4. **No null anywhere** — `Person` fields are always initialized. If something might be absent, you'd use `Optional<Person>`.

## Next Steps

- [Language Overview](guide/language-overview.md) — Tour of all major features
- [Functions & Modules](guide/functions-modules.md) — Deep dive into modules and interfaces
- [Ownership & Borrowing](guide/ownership.md) — How Arth manages memory
