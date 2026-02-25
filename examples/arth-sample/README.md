# Arth Scaffolded Project (Sample)

This is a scaffolded sample project demonstrating Arth’s recommended layout, providers, modules, encoding, logging, and HTTP fetch usage.

Structure
- `arth.toml` — manifest (TOML)
- `arth.lock.json` — lockfile (JSON; illustrative)
- `src/` — source code (packages map 1:1 to directories)
- `tests/` — test packages, discovered via `@test`

Highlights
- Providers: explicit `provider` with state, behavior in a companion module.
- Encoding: reflection‑free `Json.serialize/deserialize` with an explicit codec provider module.
- HTTP: `Http.fetch` with explicit throws for errors.
- Logging: low‑noise structured logs.

This sample is illustrative; the current repository doesn’t ship the Arth toolchain.

Additional examples
- `src/demo/AsyncChannels.arth` — channels + tasks with a `Message` enum.
- `src/demo/SealedEnum.arth` — exhaustive `switch` over an enum.
- `src/demo/RetryFetch.arth` — retry with typed exceptions and backoff.
- `src/demo/EncodingAttrs.arth` — `@derive(JsonCodec)`, `@rename`, `@default` usage.
- `src/actor/Counter.arth` — provider‑backed counter actor using a message loop.
- `src/demo/logging/Logging.arth` — terse structured logging with fields.
