# QueryForge

QueryForge is a SQL-first Rust code generator inspired by Cornucopia. It parses named SQL blocks, derives query parameter and row shapes from database metadata or catalog/schema inference, and generates typed async Rust functions for Postgres and libSQL/SQLite without turning SQL into an ORM.

The project keeps the public API in the top-level `queryforge` crate. The CLI and `build.rs` helper crates are intentionally thin wrappers.

## Shape

```text
queryforge/              # top-level public library crate
  src/                   # public API + implementation modules
  crates/queryforge-cli  # thin CLI wrapper
  crates/queryforge-build# thin build.rs wrapper
```

Generated APIs are query-shaped. QueryForge emits functions and row DTOs for the SQL you write; application domain models stay in application code.

## Try it

```bash
cargo run -p queryforge-cli -- generate queryforge.toml
```

This writes generated modules into `src/db` by default.

SQL blocks use Cornucopia-style names. Cardinality is optional: `SELECT`/`WITH` defaults to `many`, plain mutation and DDL statements default to `exec`, single-row `INSERT ... RETURNING` defaults to `one`, and `UPDATE`/`DELETE ... RETURNING` defaults to `many`.

```sql
--! insert_author
INSERT INTO authors (first_name, last_name, country)
VALUES (:first_name, :last_name, :country);

--! authors
SELECT first_name, last_name, country FROM authors;
```

For a native libSQL target, call the generated functions like this:

```rust
let inserted = db::authors::insert_author(
    &conn,
    db::authors::InsertAuthorParams {
        first_name: "Octavia".to_string(),
        last_name: "Butler".to_string(),
        country: "US".to_string(),
    },
)
.await?;
assert_eq!(inserted, 1);

let authors = db::authors::authors(&conn).await?;
for author in authors {
    println!("{} {} ({})", author.first_name, author.last_name, author.country);
}
```

Use `INSERT ... RETURNING ...` without `: one` when inserting one row and returning it:

```sql
--! insert_author
INSERT INTO authors (first_name, last_name, country)
VALUES (:first_name, :last_name, :country)
RETURNING id, first_name, last_name, country;
```

```rust
let author = db::authors::insert_author(
    &conn,
    db::authors::InsertAuthorParams {
        first_name: "Octavia".to_string(),
        last_name: "Butler".to_string(),
        country: "US".to_string(),
    },
)
.await?;
println!("inserted author #{}", author.id);
```

Use an explicit cardinality only when the default is not the shape you want, such as `optional` for an update that should affect at most one row or `many` for a bulk insert with `RETURNING`.

Explicit cardinality is still supported when you prefer to document the contract in the SQL block header:

```sql
--! insert_author : one
INSERT INTO authors (first_name, last_name, country)
VALUES (:first_name, :last_name, :country)
RETURNING id, first_name, last_name, country;
```

QueryForge also supports per-query Rust type overrides with `--:` directives when database metadata is not specific enough or when application code has a newtype:

```sql
--! get_author
--: param.id: AuthorId
--: column.email: EmailAddress
SELECT id, email FROM authors WHERE id = :id;
```

Use `param.name` for generated params, `column.name` for returned row fields, or an unscoped `name` to apply to both matching params and columns. Column overrides replace the base Rust type; normal nullability wrapping still applies.

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
cargo run -p queryforge-build-rs-example
cargo check -p queryforge-build-rs-example
```

The build script calls `queryforge-build`, emits Cargo rebuild hints, includes generated code from `OUT_DIR/queryforge`, and the binary executes generated native libSQL functions against an in-memory database using generated `*Params` structs.

### `examples/usage-app`

Binary crate showing generated native libSQL APIs included from `OUT_DIR`.

```bash
cargo run -p queryforge-usage-app
cargo test -p queryforge-usage-app
```

The binary creates an in-memory libSQL database and calls generated functions with named params structs. The test also exercises generated functions with both a connection and a transaction.

### `examples/crud`

Binary crate showing SQL-first CRUD-style mutations without `: exec` annotations.

```bash
cargo run -p queryforge-crud-example
cargo test -p queryforge-crud-example
```

The binary and test create an in-memory libSQL database and exercise generated create, read, update, upsert, list, and delete functions. Parameterized generated functions take named `*Params` structs, so mutation calls use field names instead of positional arguments.

### `examples/sqlite-e2e`

Runnable SQLx SQLite e2e example. It generates SQLx SQLite functions in `build.rs`, creates an in-memory SQLite database at runtime, and executes generated functions with named params structs against a pool and a transaction.

```bash
cargo run -p queryforge-sqlite-e2e-example
cargo test -p queryforge-sqlite-e2e-example
```

This is the quickest example for seeing SQLx-backed generated code actually execute without Docker.

### `examples/postgres-e2e`

Runnable SQLx Postgres e2e example. It is not a workspace member because its build script connects to Postgres for live prepared-statement introspection.

Start Postgres:

```bash
docker run --rm --name queryforge-postgres-e2e-example \
  -e POSTGRES_PASSWORD=postgres \
  -e POSTGRES_DB=queryforge \
  -p 127.0.0.1:55432:5432 \
  postgres:16-alpine
```

Then run the example from another terminal:

```bash
cargo run --manifest-path examples/postgres-e2e/Cargo.toml
```

The build script creates the schema, QueryForge introspects it through `tokio-postgres`, and the binary executes generated SQLx Postgres functions with named params structs against a pool and transaction.

### `examples/type-mapping-profiles`

Compile fixture for external generated type mappings.

```bash
cargo check -p queryforge-type-mapping-profiles-example
cargo test -p queryforge-type-mapping-profiles-example
```

The build script writes generated modules for SQLx Postgres, tokio-postgres, and native libSQL into `OUT_DIR`, then the crate compiles those modules with real application dependencies such as `uuid`, `serde_json`, `time`, `chrono`, and `rust_decimal`.

### `examples/live-libsql-catalog`

Binary crate showing live local libSQL/SQLite catalog introspection. Its `build.rs` creates `catalog.db`, then QueryForge reads table metadata from `sqlite_schema` and `PRAGMA table_xinfo` instead of `[schema].files`.

```bash
cargo run -p queryforge-live-libsql-catalog-example
```

This example requires the generator-side `libsql-runtime` feature through `queryforge-build`.

### `examples/with-config`

Binary plus library crate with a slightly larger config and `exec` query.

```bash
cargo run -p queryforge-with-config-example
cargo test -p queryforge-with-config-example
```

It prints generated SQL constants, creates an in-memory libSQL database, inserts a row, and reads it back.

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

By default, `queryforge-build` writes to `OUT_DIR/queryforge`, which keeps generated files out of source control and works with:

```rust
pub mod db {
    include!(concat!(env!("OUT_DIR"), "/queryforge/mod.rs"));
}
```

Use `.output_dir(...)` when a build script should write somewhere else:

```rust
fn main() {
    queryforge_build::generate()
        .config("queryforge.toml")
        .output_dir("src/db")
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

## Generated params, rows, and domain models

QueryForge generates query-specific parameter DTOs from named SQL params and row DTOs from returned columns. For example, a `--! get_user : one` query with `WHERE id = :id` generates `GetUserParams`, while the selected columns generate `GetUserRow`.

Parameterized generated functions take the params struct instead of positional arguments:

```rust
let user = db::users::get_user(
    &conn,
    db::users::GetUserParams {
        id: 42,
    },
)
.await?;
```

This makes call sites easier to read, gives IDEs concrete field names to autocomplete, and prevents same-type positional argument swaps.

When needed, per-query `--:` overrides can give those generated DTO fields application-specific Rust types:

```sql
--! get_user
--: param.id: UserId
--: column.email: Email
SELECT id, email FROM users WHERE id = :id;
```

The generated `GetUserParams` will contain `pub id: UserId`, and `GetUserRow` will contain `pub email: Email` unless that column is nullable, in which case the generated field becomes `Option<Email>`.

Use scoped overrides when a parameter and returned column share a name but need different Rust types:

```sql
--! find_author
--: param.id: AuthorId
--: column.id: i64
SELECT id, name
FROM authors
WHERE id = :id;
```

Use an unscoped override when the same Rust type should apply to both a matching param and matching column:

```sql
--! get_author
--: id: AuthorId
SELECT id, name
FROM authors
WHERE id = :id;
```

Overrides are validated against the generated shape. If `--: column.emali: Email` does not match a returned column or generated Rust field name, generation fails instead of silently ignoring the typo.

QueryForge-owned scalar traits let application newtypes stay backend-neutral. Generated code calls `QueryForgeEncode` for params and `QueryForgeDecode` for returned columns, then uses the associated `Storage` type with the selected database driver.

```rust
#[derive(Debug, Clone, Copy)]
pub struct AuthorId(pub i64);
queryforge::scalar_newtype!(AuthorId, i64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailAddress(pub String);
queryforge::scalar_newtype!(EmailAddress, String);
```

The macro is shorthand for implementing both directions once:

```rust
impl queryforge::QueryForgeEncode for AuthorId {
    type Storage = i64;

    fn queryforge_encode(self) -> i64 {
        self.0
    }
}

impl queryforge::QueryForgeDecode for AuthorId {
    type Storage = i64;

    fn queryforge_decode(value: i64) -> queryforge::Result<Self> {
        Ok(Self(value))
    }
}
```

With those impls available, the same SQL override works for supported execution targets:

```rust
let author = db::authors::get_author(
    &pool,
    db::authors::GetAuthorParams {
        id: AuthorId(42),
    },
)
.await?;

let email: EmailAddress = author.email;
```

The associated `Storage` type is what must be supported by the active driver. For example, `AuthorId` stores as `i64`, so SQLx, tokio-postgres, and native libSQL bind/decode `i64`; `EmailAddress` stores as `String`, so the drivers bind/decode `String`. If the storage type is not supported by the selected backend, Rust compilation fails in generated code.

Use manual trait impls instead of `scalar_newtype!` when decoding can fail or validation belongs at the database boundary:

```rust
impl queryforge::QueryForgeDecode for EmailAddress {
    type Storage = String;

    fn queryforge_decode(value: String) -> queryforge::Result<Self> {
        EmailAddress::parse(value)
            .map_err(|err| queryforge::Error::Backend(format!("invalid email: {err}")))
    }
}
```

Generated rows are API boundary types, not ORM models: they do not own persistence behavior, relations, validation, or business methods.

Query names drive generated API names: `--! get_user_with_org : one` generates a `get_user_with_org(...)` function, `GetUserWithOrgParams` when params exist, and `GetUserWithOrgRow` row type in the module derived from the SQL file name. Join queries can select duplicate column names; QueryForge keeps generated Rust valid by suffixing duplicate field identifiers (`id`, `id_2`, etc.) and decoding rows by column position. Prefer explicit SQL aliases such as `u.id AS user_id` and `o.id AS org_id` when you want stable semantic field names.

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
db::users::get_user(&pool, db::users::GetUserParams { id }).await?;

let mut tx = pool.begin().await?;
db::users::get_user(&mut *tx, db::users::GetUserParams { id }).await?;
tx.commit().await?;
```

Generated tokio-postgres functions accept `tokio_postgres::GenericClient`, which is implemented by both `Client` and `Transaction`:

```rust
db::users::get_user(&client, db::users::GetUserParams { id }).await?;

let tx = client.transaction().await?;
db::users::get_user(&tx, db::users::GetUserParams { id }).await?;
tx.commit().await?;
```

Native libSQL generated functions use a `queryforge::runtime::libsql_executor::LibsqlExecutor` bound. With the `libsql-runtime` feature enabled, QueryForge implements that trait for `libsql::Connection` and `libsql::Transaction`, so the same generated functions can run inside or outside transactions.

```rust
db::users::get_user(&conn, db::users::GetUserParams { id }).await?;

let tx = conn.transaction().await?;
db::users::get_user(&tx, db::users::GetUserParams { id }).await?;
tx.commit().await?;
```

## Features

The default `queryforge` crate stays dependency-light and does not compile database client crates.

- `postgres` enables live Postgres introspection through `tokio` and `tokio-postgres`.
- `libsql` is reserved for the dependency-light libSQL/SQLite schema-driven generation path.
- `libsql-runtime` enables adapters for upstream `libsql::Connection` and `libsql::Transaction`.
- `libsql-remote` enables live remote libSQL catalog introspection in addition to `libsql-runtime`; remote databases also require `[database].auth_token` or `[database].auth_token_env`.
- `queryforge-cli` and `queryforge-build` forward these features; enable `postgres` there only when a CLI/build script needs live Postgres introspection.
- External generated type paths and native libSQL runtime adapters are feature-gated: `uuid-types`, `chrono-types`, `time-types`, `serde-json-types`, and `decimal-types`.

When an external mapping is enabled in `queryforge.toml`, enable the matching QueryForge feature on the generator and add the actual external crate to the application that compiles the generated code. QueryForge still keeps these dependencies out of the default build.

```toml
[type_mapping]
uuid = "uuid"
json = "serde-json"
time = "chrono"
decimal = "rust-decimal"
```

`uuid = "uuid"` maps Postgres `uuid` metadata and SQLite/libSQL columns declared with `UUID` affinity to `uuid::Uuid`. QueryForge also recognizes common SQLite UUID extension functions such as `uuid4()`, `gen_random_uuid()`, `uuid7()`, `uuid_str(...)`, `uuid_blob(...)`, and `uuid7_timestamp_ms(...)` during libSQL inference. Without that setting, UUID values stay dependency-light as `String`.

QueryForge does not load SQLite extensions for the application. If generated SQL calls SQLite UUID functions, the target libSQL/SQLite connection must have the UUID extension available at runtime.

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

QueryForge parses named SQL blocks, infers default cardinality for common statement shapes including `INSERT ... RETURNING`, loads nested TOML config, normalizes named parameters, applies per-query Rust type overrides, computes fingerprints, writes initial offline metadata with `queryforge prepare`, and generates Rust modules.

The parser boundary is intentional: `nom` parses QueryForge block headers, while `sqlparser-rs` provides AST-backed lowering for supported PostgreSQL/SQLite `CREATE TABLE`, `SELECT`, and mutation shapes into the shared lightweight `sql_ir`. QueryForge has an internal AST visitor over nested query, table, join, function, and expression paths so inference rules can inspect SQL structurally instead of reimplementing grammar. QueryForge keeps a compatibility fallback for SQL shapes not yet lowered from the AST, and it still relies on database metadata or conservative `Unknown` results rather than trying to become a full SQL engine.

Postgres inspection uses `tokio-postgres` prepared statement metadata for parameter and column types. Direct table-column nullability is inferred from `pg_attribute`; conservative expression nullability handles direct columns, bind params, common expressions such as `CASE`, `nullif`, comparisons, arithmetic and boolean expressions, `BETWEEN`, `IN`/`NOT IN`, `LIKE`/`ILIKE`, outer joins, nullable parenthesized join groups, comma/CROSS-style table references, simple CTEs, declared recursive CTE result columns, recursive CTE branch nullability merging, derived-table subqueries, and lateral derived tables that depend on preceding outer relations, including lateral derived tables inside parenthesized join groups.

libSQL inspection currently consumes `sql_ir` plus configured schema SQL files for dependency-free inference. With `libsql-runtime`, it can also inspect a local database catalog through `sqlite_schema`, `PRAGMA table_xinfo`, indexes, and foreign keys. With `libsql-remote`, the same catalog queries can run against a remote libSQL database when an auth token is configured. It handles direct columns, joins, simple CTEs, declared CTE column lists, derived tables, lateral derived-table joins, compound select branch parameter types, `*`, simple equality parameter types, and basic expressions such as `count(*)`, `lower(...)`, `upper(...)`, `coalesce(...)`, `ifnull(...)`, `length(...)`, and `||` string concatenation.

SQLx Postgres and SQLx SQLite renderers emit executor-style async functions that can be called with pools, connections, or transactions. The tokio-postgres renderer emits `GenericClient`-based async functions for clients and transactions. Native libSQL rendering calls the QueryForge runtime executor trait and has concrete adapters for the upstream `libsql` crate.

Generated metadata embeds backend, execution target, project, schema, migration, and type-mapping fingerprints. Query and project fingerprints include the QueryForge codegen version, backend, execution target, inference policy, type-mapping fingerprint, schema and migration fingerprints, normalized SQL, and inferred query shape inputs.

## Tests

```bash
cargo fmt
cargo check --workspace --all-features
cargo test --workspace --all-features
```

CI runs the same all-feature formatting/check/test path, plus opt-in e2e jobs.

The Docker-backed Postgres e2e is opt-in:

```bash
QUERYFORGE_E2E_POSTGRES=1 cargo test -p queryforge --features postgres --test postgres_e2e -- --nocapture
```

The credentialed remote libSQL e2e is also opt-in:

```bash
QUERYFORGE_E2E_LIBSQL_REMOTE=1 \
QUERYFORGE_LIBSQL_REMOTE_URL="libsql://..." \
QUERYFORGE_LIBSQL_AUTH_TOKEN="..." \
cargo test -p queryforge --features libsql-remote --test libsql_remote_e2e -- --nocapture
```

The GitHub Actions workflow runs the remote libSQL e2e automatically when repository secrets named `LIBSQL_REMOTE_URL` and `LIBSQL_AUTH_TOKEN` are configured; otherwise that job records an explicit skip.

Runnable generated-code e2e examples are also available:

```bash
cargo run -p queryforge-sqlite-e2e-example
cargo test -p queryforge-sqlite-e2e-example
```

```bash
docker run --rm --name queryforge-postgres-e2e-example \
  -e POSTGRES_PASSWORD=postgres \
  -e POSTGRES_DB=queryforge \
  -p 127.0.0.1:55432:5432 \
  postgres:16-alpine
```

```bash
cargo run --manifest-path examples/postgres-e2e/Cargo.toml
```
