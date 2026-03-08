# Concurrency

Arth provides actors, channels, and async/await for safe concurrent programming.

## Actors

Actors encapsulate mutable state behind message-passing. No shared memory, no locks:

```java
actor ShoppingCart {
    private List<String> items = List.empty();

    public void addItem(String item) {
        items = items.append(item);
    }

    public List<String> getItems() {
        return items;
    }

    public int count() {
        return items.length();
    }
}
```

### Using Actors

```java
ShoppingCart cart = ShoppingCart.spawn();
cart.addItem("Laptop");
cart.addItem("Mouse");
int n = cart.count();    // 2
```

Each actor runs in its own logical thread. Method calls are serialized — no data races.

## Channels

For producer-consumer patterns:

```java
Channel<String> ch = Channel.create();

// Producer
spawn {
    ch.send("hello");
    ch.send("world");
    ch.close();
}

// Consumer
for (String msg : ch) {
    println(msg);
}
```

### Buffered Channels

```java
Channel<int> ch = Channel.buffered(100);    // buffer up to 100 items
```

## Async / Await

For IO-bound work:

```java
public async Response fetchData(String url) throws (HttpError) {
    Response res = await Http.get(url);
    return res;
}

public async void processAll(List<String> urls) throws (HttpError) {
    // Concurrent requests
    List<Task<Response>> tasks = urls.map(
        (String url) -> fetchData(url)
    );

    for (Task<Response> task : tasks) {
        Response res = await task;
        println(res.body);
    }
}
```

### Tasks

`async` functions return `Task<T>`:

```java
Task<Response> task = fetchData("https://api.example.com/data");
// ... do other work ...
Response res = await task;    // wait for completion
```

## Sendable and Shareable

The type system enforces what can cross thread boundaries:

- **`Sendable`** — Can be moved to another thread (ownership transfers)
- **`Shareable`** — Can be shared between threads (requires thread-safe access)

```java
// Structs with only final fields are automatically Sendable
struct Message {
    final String text;
    final int priority;
}

// The compiler rejects unsafe sharing:
// List<int> is not Shareable — use Atomic or shared fields instead
```

## Structured Concurrency

Tasks are scoped — child tasks cannot outlive their parent:

```java
public async void handleRequest(Request req) throws (HttpError) {
    // Both tasks are bounded by this function's lifetime
    Task<User> userTask = fetchUser(req.userId);
    Task<List<Order>> ordersTask = fetchOrders(req.userId);

    User user = await userTask;
    List<Order> orders = await ordersTask;

    respond(user, orders);
}
// Both tasks guaranteed complete before function returns
```
