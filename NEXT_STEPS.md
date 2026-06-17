# Next steps

## Implemented in this milestone

- Nested `serde` + `toml` config is in place, including enum-backed database/backend/codegen/build/inference settings.
- QueryForge SQL files are split into `--! name : cardinality` blocks with a small `nom` parser.
- A lightweight `sql_ir` module uses `nom` to parse the supported schema/query shape subset into common IR (`CREATE TABLE`, `SELECT`, CTEs, derived-table subqueries, compound selects, projections, aliases, joined table references, table aliases, qualified columns, and equality params).
- `Config::from_path` delegates to `Config::load`.
- `ProjectShape` uses the config enums directly; `BackendKind` is now only a compatibility alias.
- `Fingerprint::as_str()` and `Display` are available.
- Postgres inspection connects with `tokio-postgres`, normalizes named params to positional `$n` params, prepares each statement, and fills `QueryParam`/`QueryColumn` from prepared statement metadata.
- Postgres direct-column nullability is inferred from `pg_attribute.attnotnull`.
- Postgres outer-join nullability is handled conservatively for direct columns: columns from nullable sides of `LEFT`, `RIGHT`, and `FULL` joins are generated as nullable even when the underlying table column is `NOT NULL`.
- Postgres expression nullability now has conservative semantic inference for direct column references, non-null literals, `count(...)`, `coalesce(...)`, casts, parentheses, and `||` concatenation, so expressions like `not_null_text_column || ''` are generated as non-null.
- Postgres expression nullability can synthesize relation shapes for simple CTEs and derived-table subqueries, allowing outer projections from those relations to preserve inferred inner nullability.
- `normalize_postgres_params` avoids rewriting params inside single-quoted strings, double-quoted identifiers, dollar-quoted strings, line comments, and block comments, and preserves Postgres casts.
- Basic Postgres type mapping exists for common scalar types, with UUID/JSON/date/time/numeric mapped to `String` for now.
- libSQL consumes the common `sql_ir` and configured schema SQL files to infer direct table columns, nullability, basic SQLite affinities, simple equality parameter types, `*`, `count(*)`, `lower(...)`, and `upper(...)`; ambiguous expressions remain `Unknown`.
- libSQL query inference resolves joined tables, table aliases, qualified projections, qualified `table.*`, and qualified equality params; ambiguous unqualified join columns remain `Unknown`.
- libSQL query inference can synthesize table shapes for simple CTEs and derived-table subqueries, including propagation of nested named-parameter types to the outer generated query.
- libSQL query inference propagates parameter types and dependencies from compound select branches such as `UNION ALL`, while using the first branch for the generated row shape.
- SQLx Postgres and SQLx SQLite renderers emit executor-style async functions that work with pools, connections, and transactions.
- The tokio-postgres renderer emits `GenericClient`-based async functions that work with clients and transactions.
- Native libSQL generated functions call the `LibsqlExecutor` runtime trait (`execute`, `query_one`, `query_optional`, and `query_many`) so connections and transactions share one generated API.
- The native libSQL runtime module defines `LibsqlValue`, `LibsqlRow`, decode helpers, trait-contract tests, and `libsql-runtime` feature-gated adapters for `libsql::Connection` and `libsql::Transaction`.
- Heavy database client crates are feature-gated: `postgres` pulls `tokio`/`tokio-postgres`, while `libsql-runtime` pulls upstream `libsql`; the default crate and libSQL schema-driven generation path stay dependency-light.
- With `libsql-runtime`, libSQL inspection can read a local live catalog via `sqlite_schema`, `PRAGMA table_xinfo`, `PRAGMA index_list`/`index_info`, and `PRAGMA foreign_key_list`, including primary-key nullability, generated columns, indexes, and foreign keys; schema files remain the fallback when no live catalog data is available.
- Remote libSQL URLs without schema files now fail with a clear diagnostic explaining that remote live catalog introspection is not supported yet and `[schema].files` should be provided for offline inference.
- Generated metadata now includes backend, execution target, project fingerprint, schema fingerprint, migration fingerprint, and type-mapping fingerprint.
- Project and query fingerprints include the QueryForge codegen version, backend, execution target, inference policy, type-mapping fingerprint, schema fingerprint, migration fingerprint, normalized SQL, and inferred query shape inputs.
- Type mapping config has explicit enum-backed choices for UUID, JSON, date/time, and decimal output. Defaults remain dependency-light `String` mappings; opting into external generated types requires the matching zero-dependency QueryForge generator feature and an application dependency on the actual external crate.
- `queryforge prepare` writes initial offline metadata to `.queryforge/metadata.json` next to the config file.
- Generation can consume prepared offline metadata when `[build].offline = "true"`, replaying generated files without reading query/schema inputs or connecting to Postgres.
- Offline metadata loading validates the format, QueryForge version, database backend, execution target, generated file paths, duplicate paths, query-count consistency, and source freshness when query/schema files are available before replaying files.
- Rust code generation now uses `proc_macro2`, `quote`, `syn`, and `prettyplease` to build and format generated Rust from token streams instead of hand-rendering Rust syntax with string templates.
- Generated-code snapshot-style tests cover metadata constants, native libSQL module output, SQLx Postgres, SQLx SQLite, and tokio-postgres output without adding a snapshot-test dependency.
- `examples/usage-app` has an in-memory generated native libSQL execution test that exercises generated functions with both a connection and a transaction.
- `queryforge-build` still generates into `OUT_DIR/queryforge` by default.
- `examples/usage-app` demonstrates `queryforge-build`, `queryforge.toml`, SQL blocks, and including generated code from `OUT_DIR`.
- The test suite covers config parsing/display, parser behavior, parameter normalization, codegen idempotency, config-relative generation paths, native libSQL connection/transaction execution adapters, generated native libSQL execution, and an opt-in Docker Postgres e2e.

## Remaining work

- Add deeper semantic Postgres nullability for scalar subqueries, recursive CTEs, richer expressions, lateral joins, and more complex join shapes.
- Expand live libSQL/SQLite catalog support beyond local databases by adding actual remote catalog introspection.
- Expand the common `sql_ir` beyond the current CTE/derived-table/compound-select/join/alias subset, especially richer expressions, recursive CTEs, lateral joins, and more SQL dialect edge cases.
- Add e2e coverage and generated-code compile fixtures for external type mapping profiles across SQLx, tokio-postgres, and native libSQL targets.

## E2E tests

Normal tests skip Docker work by default:

```bash
cargo test --workspace --all-features
```

Run the Postgres e2e explicitly with:

```bash
QUERYFORGE_E2E_POSTGRES=1 cargo test -p queryforge --features postgres --test postgres_e2e -- --nocapture
```
