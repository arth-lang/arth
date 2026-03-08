# Providers

Providers replace global mutable state with explicit, lifetime-managed containers.

## The Problem with Globals

In most languages, shared state is managed through global variables, singletons, or dependency injection frameworks. These are hard to reason about, test, and debug.

## Defining a Provider

A provider declares its fields with explicit mutability:

```java
provider AppConfig {
    public final String env;              // immutable after init
    public final int port;
    public shared Map<String, String> settings;  // shared mutable state
}
```

- **`final`** — Set once at creation, never changes
- **`shared`** — Thread-safe mutable state (uses atomic operations internally)

## Creating and Managing Providers

Modules handle provider lifecycle:

```java
module AppConfigFns {
    public AppConfig create(String env, int port) {
        return AppConfig {
            env: env,
            port: port,
            settings: Map.empty()
        };
    }

    public void deinit(AppConfig self) {
        // cleanup logic
    }

    public void set(AppConfig self, String key, String value) {
        self.settings.put(key, value);
    }

    public Optional<String> get(AppConfig self, String key) {
        return self.settings.get(key);
    }
}
```

## Using Providers

```java
public void main() {
    AppConfig config = AppConfigFns.create("production", 8080);
    config.then(AppConfigFns.set("db_url", "postgres://localhost/app"));

    Optional<String> url = config.then(AppConfigFns.get("db_url"));
    println(url.orElse("not configured"));
}
```

## Shared Mutable State

The `shared` keyword marks fields that can be safely mutated across threads:

```java
provider Counter {
    public shared Atomic<int> count;
}

module CounterFns {
    public Counter create() {
        return Counter { count: Atomic.of(0) };
    }

    public void increment(Counter self) {
        self.count.incrementAndGet();
    }

    public int get(Counter self) {
        return self.count.get();
    }
}
```

## Why Providers?

| Approach | Testable? | Thread-safe? | Explicit? |
|----------|-----------|-------------|-----------|
| Global variables | No | No | No |
| Singletons | Hard | Manual | Somewhat |
| DI frameworks | Yes | Varies | Hidden (magic) |
| **Providers** | **Yes** | **Built-in** | **Yes** |

Providers make state visible in the type system. You can see exactly what state a piece of code depends on, swap providers in tests, and trust the compiler to enforce thread safety.
