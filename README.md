# QueryForge starter

A dependency-light starter for a Cornucopia-inspired Rust code generator that targets Postgres and libSQL.

## Shape

```text
queryforge/              # top-level public library crate
  src/                   # public API + implementation modules
  crates/queryforge-cli  # thin CLI wrapper
  crates/queryforge-build# thin build.rs wrapper
```

The CLI and build helper both call the top-level `queryforge` API.

## Try it

```bash
cargo run -p queryforge-cli -- generate queryforge.toml
```

This writes generated modules into `src/db` by default.

## Examples

Run commands from the workspace root unless an example README says otherwise.

### `examples/basic`

Minimal CLI-driven fixture. It is not a Cargo package; it contains only `queryforge.toml`, `schema.sql`, SQL query files, and prepared offline metadata.

```bash
cargo run -p queryforge-cli -- generate examples/basic/queryforge.toml
cargo run -p queryforge-cli -- prepare examples/basic/queryforge.toml
```

`generate` writes generated Rust under `examples/basic/src/db`. `prepare` refreshes `examples/basic/.queryforge/metadata.json`.

### `examples/build-rs`

Library crate showing the smallest `build.rs` integration.

```bash
cargo check -p queryforge-build-rs-example
```

The build script calls `queryforge-build`, emits Cargo rebuild hints, and includes generated code from `OUT_DIR/queryforge`.

### `examples/usage-app`

Binary crate showing generated native libSQL APIs included from `OUT_DIR`.

```bash
cargo run -p queryforge-usage-app
cargo test -p queryforge-usage-app
```

The test creates an in-memory libSQL database and exercises generated functions with both a connection and a transaction.

### `examples/with-config`

Binary plus library crate with a slightly larger config and `exec` query.

```bash
cargo run -p queryforge-with-config-example
cargo test -p queryforge-with-config-example
```

It prints generated SQL constants and the generated project fingerprint.

## build.rs API

```rust
fn main() {
    queryforge_build::generate()
        .config("queryforge.toml")
        .watch("queries")
        .watch("schema.sql")
        .run()
        .expect("queryforge generation failed");
}
```

Applications that include generated native libSQL code also need the top-level runtime crate:

```toml
[dependencies]
queryforge = "0.1"
```

## Generated rows and domain models

QueryForge generates query-specific row DTOs from the columns returned by each SQL block. For example, a `--! get_user : one` query can generate a `GetUserRow` with fields that match the selected database columns. These generated rows are API boundary types, not ORM models: they do not own persistence behavior, relations, validation, or business methods.

Keep domain models in application code and convert from generated rows:

```rust
pub struct User {
    pub id: UserId,
    pub email: Email,
}

impl TryFrom<db::users::GetUserRow> for User {
    type Error = EmailError;

    fn try_from(row: db::users::GetUserRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: UserId(row.id),
            email: Email::parse(row.email)?,
        })
    }
}
```

For infallible mappings, implement `From<GetUserRow>` instead. This keeps generated code disposable and schema-shaped, while the application keeps control over domain invariants.

## Transactions

Generated SQLx functions accept `sqlx::Executor`, so the same function works with pools, connections, and transactions:

```rust
db::users::get_user(&pool, id).await?;

let mut tx = pool.begin().await?;
db::users::get_user(&mut *tx, id).await?;
tx.commit().await?;
```

Generated tokio-postgres functions accept `tokio_postgres::GenericClient`, which is implemented by both `Client` and `Transaction`:

```rust
db::users::get_user(&client, id).await?;

let tx = client.transaction().await?;
db::users::get_user(&tx, id).await?;
tx.commit().await?;
```

Native libSQL generated functions use a `queryforge::runtime::libsql_executor::LibsqlExecutor` bound. With the `libsql-runtime` feature enabled, QueryForge implements that trait for `libsql::Connection` and `libsql::Transaction`, so the same generated functions can run inside or outside transactions.

## Features

The default `queryforge` crate stays dependency-light and does not compile database client crates.

- `postgres` enables live Postgres introspection through `tokio` and `tokio-postgres`.
- `libsql` is reserved for the dependency-light libSQL/SQLite schema-driven generation path.
- `libsql-runtime` enables adapters for upstream `libsql::Connection` and `libsql::Transaction`.
- `queryforge-cli` and `queryforge-build` forward these features; enable `postgres` there only when a CLI/build script needs live Postgres introspection.
- External generated type paths are feature-gated without adding dependencies to QueryForge: `uuid-types`, `chrono-types`, `time-types`, `serde-json-types`, and `decimal-types`.

When an external mapping is enabled in `queryforge.toml`, enable the matching QueryForge feature on the generator and add the actual external crate to the application that compiles the generated code:

```toml
[type_mapping]
uuid = "uuid"
json = "serde-json"
time = "chrono"
decimal = "rust-decimal"
```

```toml
[build-dependencies]
queryforge-build = { path = "../../crates/queryforge-build", features = ["uuid-types", "serde-json-types", "chrono-types", "decimal-types"] }

[dependencies]
uuid = "1"
serde_json = "1"
chrono = "0.4"
rust_decimal = "1"
```

## Current implementation level

QueryForge parses named SQL blocks, loads nested TOML config, normalizes named parameters, computes fingerprints, writes initial offline metadata with `queryforge prepare`, and generates Rust modules.

The parser boundary is intentionally narrow: `nom` parses QueryForge block headers and the supported SQL-shape subset into a shared lightweight `sql_ir`. QueryForge does not try to be a full SQL parser.

Postgres inspection uses `tokio-postgres` prepared statement metadata for parameter and column types. Direct table-column nullability is inferred from `pg_attribute`; conservative expression nullability handles direct columns, common expressions, outer joins, simple CTEs, and derived-table subqueries.

libSQL inspection currently consumes `sql_ir` plus configured schema SQL files for dependency-free inference. It handles direct columns, joins, simple CTEs and derived tables, compound select branch parameter types, `*`, simple equality parameter types, and basic expressions such as `count(*)`, `lower(...)`, and `upper(...)`.

SQLx Postgres and SQLx SQLite renderers emit executor-style async functions that can be called with pools, connections, or transactions. The tokio-postgres renderer emits `GenericClient`-based async functions for clients and transactions. Native libSQL rendering calls the QueryForge runtime executor trait and has concrete adapters for the upstream `libsql` crate.

Generated metadata embeds backend, execution target, project, schema, migration, and type-mapping fingerprints. Query and project fingerprints include the QueryForge codegen version, backend, execution target, inference policy, type-mapping fingerprint, schema and migration fingerprints, normalized SQL, and inferred query shape inputs.

## Tests

```bash
cargo fmt
cargo check --workspace --all-features
cargo test --workspace --all-features
```

The Docker-backed Postgres e2e is opt-in:

```bash
QUERYFORGE_E2E_POSTGRES=1 cargo test -p queryforge --features postgres --test postgres_e2e -- --nocapture
```
