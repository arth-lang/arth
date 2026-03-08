# Standard Library

## Core Types

### `Optional<T>`

```java
Optional.of(value)          // wrap a value
Optional.empty()            // no value
opt.get()                   // unwrap (throws if empty)
opt.orElse(default)         // unwrap or use default
opt.map(fn)                 // transform if present
opt.isEmpty()               // check absence
opt.isPresent()             // check presence
```

### `List<T>`

```java
List.of(1, 2, 3)           // create from values
List.empty()                // empty list
list.get(index)             // access by index
list.length()               // size
list.append(value)          // add to end (returns new list)
list.filter(predicate)      // filter elements
list.map(fn)                // transform elements
list.contains(value)        // membership check
```

### `Map<K, V>`

```java
Map.of("a", 1, "b", 2)     // create from pairs
Map.empty()                 // empty map
map.get(key)                // returns Optional<V>
map.put(key, value)         // insert/update
map.contains(key)           // key existence
map.keys()                  // all keys
map.values()                // all values
```

### `String`

```java
str.length()                // character count
str.substring(start, end)   // slice
str.contains(other)         // search
str.split(delimiter)        // split to List<String>
str.trim()                  // strip whitespace
str.toUpperCase()           // case conversion
str.toLowerCase()
str.startsWith(prefix)
str.endsWith(suffix)
```

## I/O

### Console

```java
println(value)              // print with newline
print(value)                // print without newline
String line = readLine()    // read from stdin
```

### File System

```java
String content = readFile(path) throws (IoError);
writeFile(path, content) throws (IoError);
boolean exists = fileExists(path);
```

## Concurrency

### `Task<T>`

```java
Task<T> task = asyncFn();   // start async work
T result = await task;      // wait for result
```

### `Channel<T>`

```java
Channel.create()            // unbuffered
Channel.buffered(size)      // buffered
ch.send(value)              // send (blocks if full)
T value = ch.receive()      // receive (blocks if empty)
ch.close()                  // close channel
```

### `Atomic<T>`

```java
Atomic.of(value)            // create
atomic.get()                // read
atomic.set(value)           // write
atomic.compareAndSwap(expected, new)
atomic.incrementAndGet()    // for numeric types
```

## Math

```java
Math.PI                     // 3.14159...
Math.E                      // 2.71828...
Math.sqrt(x)
Math.abs(x)
Math.min(a, b)
Math.max(a, b)
Math.pow(base, exp)
Math.floor(x)
Math.ceil(x)
Math.round(x)
Math.random()               // [0.0, 1.0)
```

## Networking

### HTTP Client

```java
Response res = await Http.get(url) throws (HttpError);
Response res = await Http.post(url, body) throws (HttpError);
```

### HTTP Server

```java
HttpServer server = HttpServer.bind("0.0.0.0", 8080);
server.route("/api/users", handler);
server.start();
```
