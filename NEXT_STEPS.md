# Next steps

## Implemented in this milestone

- Nested `serde` + `toml` config is in place, including enum-backed database/backend/codegen/build/inference settings.
- QueryForge SQL files are split into `--! name` / `--! name : cardinality` blocks with a small `nom` parser; omitted cardinality is inferred as `many` for `SELECT`/`WITH` and `exec` for mutation/DDL statements.
- A lightweight `sql_ir` module lowers SQL into common IR (`CREATE TABLE`, `SELECT` with and without `FROM`, CTEs including `WITH RECURSIVE` markers and declared CTE column lists, derived-table subqueries, lateral derived-table joins including inside parenthesized join groups, parenthesized join groups, comma/CROSS-style table references, compound selects, projections, aliases, joined table references, table aliases, qualified columns, and equality params).
- `sql_ir` now uses `sqlparser-rs` for AST-backed `CREATE TABLE` and `SELECT` lowering across PostgreSQL and SQLite dialects, with the older lightweight parser retained as a compatibility fallback for shapes not yet lowered from the AST.
- AST-backed `CREATE TABLE` lowering extracts table names, column names, declared types, column-level `NOT NULL`/`PRIMARY KEY`, and simple table-level primary-key nullability.
- AST-backed `SELECT` lowering extracts named equality params structurally from CTEs, derived tables, scalar subqueries, join constraints, and `WHERE`/`HAVING` expressions instead of relying on raw SQL token scanning when `sqlparser-rs` succeeds.
- QueryForge now has an internal AST visitor/walker over `sqlparser-rs` query, set-expression, select, table-source, join, function-argument, and expression nodes; inference rules plug into that walker instead of each rule owning ad hoc recursion.
- AST-backed expression lowering maps tuple equality and `IN` list predicates such as `(id, org_id) = (:id, :org_id)` and `email IN (:a, :b)` into param-to-column inference pairs.
- AST-backed expression lowering maps range, pattern, and null-safe equality predicates such as `created_at BETWEEN :start AND :end`, `email LIKE :pattern`, and `col IS NOT DISTINCT FROM :value` into param-to-column inference pairs.
- AST-backed mutation lowering extracts target tables and named parameter-to-column mappings for supported `INSERT`, `UPDATE`, and `DELETE` statements, with libSQL inference using that IR before falling back to raw SQL heuristics.
- libSQL mutation inference now relies on AST-backed mutation lowering instead of the older raw SQL mutation token heuristics.
- `Config::from_path` delegates to `Config::load`.
- `ProjectShape` uses the config enums directly; `BackendKind` is now only a compatibility alias.
- `Fingerprint::as_str()` and `Display` are available.
- Postgres inspection connects with `tokio-postgres`, normalizes named params to positional `$n` params, prepares each statement, and fills `QueryParam`/`QueryColumn` from prepared statement metadata.
- Postgres direct-column nullability is inferred from `pg_attribute.attnotnull`.
- Postgres outer-join nullability is handled conservatively for direct columns: columns from nullable sides of `LEFT`, `RIGHT`, and `FULL` joins are generated as nullable even when the underlying table column is `NOT NULL`.
- Postgres expression nullability now has conservative semantic inference for direct column references, bind params, non-null literals, `count(...)`, `coalesce(...)`, `nullif(...)`, `CASE` result arms, casts, parentheses, boolean/comparison/arithmetic expressions, `BETWEEN`, `IN`/`NOT IN`, `LIKE`/`ILIKE`, and `||` concatenation, so expressions like `not_null_text_column || ''` are generated as non-null.
- Postgres scalar-subquery nullability is inferred conservatively: aggregate scalar subqueries such as `SELECT count(*) ...` without top-level `GROUP BY` are non-null, while subqueries that may return zero rows remain nullable or unknown.
- Postgres expression nullability can synthesize relation shapes for simple CTEs and derived-table subqueries, allowing outer projections from those relations to preserve inferred inner nullability.
- Postgres recursive CTEs with declared column lists can propagate anchor-query column nullability to the declared result column names used by outer projections.
- Postgres recursive CTE result nullability is merged across compound branches by position, so nullable recursive-branch expressions make the generated CTE result column nullable.
- Postgres lateral derived-table nullability can resolve dependencies on preceding outer relations, including projection-only lateral subqueries such as `SELECT u.email || '' AS email_expr`.
- Postgres outer-join nullability handles parenthesized join groups conservatively, so every table inside the nullable side of a grouped join is treated as nullable.
- Postgres expression nullability can resolve columns from comma/CROSS-style table references, including when those references appear inside nullable parenthesized join groups.
- Postgres lateral derived-table nullability is covered inside parenthesized join groups, including nullable grouped joins that make lateral output nullable.
- Postgres nullable-table inference is covered for nested mixed outer-join shapes, including grouped `LEFT`/`RIGHT` combinations where nullable status propagates through the nested relation tree.
- `normalize_postgres_params` avoids rewriting params inside single-quoted strings, double-quoted identifiers, dollar-quoted strings, line comments, and block comments, and preserves Postgres casts.
- Basic Postgres type mapping exists for common scalar types, with UUID/JSON/date/time/numeric mapped to `String` for now.
- libSQL consumes the common `sql_ir` and configured schema SQL files to infer direct table columns, nullability, basic SQLite affinities, simple equality parameter types, `*`, `count(*)`, `lower(...)`, `upper(...)`, `coalesce(...)`, `ifnull(...)`, `length(...)`, and `||` string concatenation; ambiguous expressions remain `Unknown`.
- libSQL query inference resolves joined tables, table aliases, qualified projections, qualified `table.*`, and qualified equality params; ambiguous unqualified join columns remain `Unknown`.
- libSQL mutation inference derives `INSERT`, `UPDATE`, and `DELETE` named parameter types from the target table catalog, so mutation blocks do not need `: exec` annotations for normal execution queries.
- libSQL query inference can synthesize table shapes for simple CTEs, declared CTE column lists, derived-table subqueries, and lateral derived-table joins, including propagation of nested named-parameter types to the outer generated query.
- libSQL query inference propagates parameter types and dependencies from compound select branches such as `UNION ALL`, while using the first branch for the generated row shape.
- SQLx Postgres and SQLx SQLite renderers emit executor-style async functions that work with pools, connections, and transactions.
- The tokio-postgres renderer emits `GenericClient`-based async functions that work with clients and transactions.
- Native libSQL generated functions call the `LibsqlExecutor` runtime trait (`execute`, `query_one`, `query_optional`, and `query_many`) so connections and transactions share one generated API.
- The native libSQL runtime module defines `LibsqlValue`, `LibsqlRow`, decode helpers, trait-contract tests, and `libsql-runtime` feature-gated adapters for `libsql::Connection` and `libsql::Transaction`.
- Heavy database client crates are feature-gated: `postgres` pulls `tokio`/`tokio-postgres`, while `libsql-runtime` pulls upstream `libsql`; the default crate and libSQL schema-driven generation path stay dependency-light.
- With `libsql-runtime`, libSQL inspection can read a local live catalog via `sqlite_schema`, `PRAGMA table_xinfo`, `PRAGMA index_list`/`index_info`, and `PRAGMA foreign_key_list`, including primary-key nullability, generated columns, indexes, and foreign keys; schema files remain the fallback when no live catalog data is available.
- With `libsql-remote`, libSQL inspection can read a remote live catalog using the same `sqlite_schema` and `PRAGMA` queries when `[database].auth_token` or `[database].auth_token_env` is configured; schema files remain the fallback when remote credentials are intentionally omitted.
- Remote libSQL URLs without schema files now fail with clear diagnostics that distinguish missing `libsql-remote` support from missing auth-token configuration.
- Generated metadata now includes backend, execution target, project fingerprint, schema fingerprint, migration fingerprint, and type-mapping fingerprint.
- Project and query fingerprints include the QueryForge codegen version, backend, execution target, inference policy, type-mapping fingerprint, schema fingerprint, migration fingerprint, normalized SQL, and inferred query shape inputs.
- Type mapping config has explicit enum-backed choices for UUID, JSON, date/time, and decimal output. Defaults remain dependency-light `String` mappings; opting into external generated types requires the matching zero-dependency QueryForge generator feature and an application dependency on the actual external crate.
- UUID mapping now covers both Postgres `uuid` metadata and SQLite/libSQL columns declared as `UUID` when `uuid = "uuid"` and the `uuid-types` generator feature are enabled.
- libSQL UUID inference recognizes SQLite UUID extension functions (`uuid4`, `gen_random_uuid`, `uuid7`, `uuid_str`, `uuid_blob`, and `uuid7_timestamp_ms`) while leaving extension loading to the application/runtime database connection.
- `queryforge prepare` writes initial offline metadata to `.queryforge/metadata.json` next to the config file.
- Generation can consume prepared offline metadata when `[build].offline = "true"`, replaying generated files without reading query/schema inputs or connecting to Postgres.
- Offline metadata loading validates the format, QueryForge version, database backend, execution target, generated file paths, duplicate paths, query-count consistency, and source freshness when query/schema files are available before replaying files.
- Rust code generation now uses `proc_macro2`, `quote`, `syn`, and `prettyplease` to build and format generated Rust from token streams instead of hand-rendering Rust syntax with string templates.
- Generated row structs use query names for stable row type names and uniquify duplicate result-column field identifiers (`id`, `id_2`, etc.) so join queries with repeated column names still generate valid Rust.
- Generated row decoding uses column indexes for SQLx, tokio-postgres, and native libSQL targets, so duplicate result-column names from joins do not decode the wrong value.
- Generated-code snapshot-style tests cover metadata constants, native libSQL module output, SQLx Postgres, SQLx SQLite, and tokio-postgres output without adding a snapshot-test dependency.
- `examples/usage-app` has an in-memory generated native libSQL execution test that exercises generated functions with both a connection and a transaction.
- `examples/live-libsql-catalog` demonstrates local live libSQL/SQLite catalog introspection from a `catalog.db` created in `build.rs`, without configured schema files.
- `examples/sqlite-e2e` demonstrates generated SQLx SQLite code executing against an in-memory SQLite database, including pool and transaction calls.
- `examples/postgres-e2e` demonstrates generated SQLx Postgres code executing against a Docker/local Postgres database with live prepared-statement introspection.
- `tests/libsql_remote_e2e.rs` provides an opt-in credentialed remote libSQL e2e that creates a temporary remote table, introspects it with `libsql-remote`, and drops it afterward.
- `.github/workflows/ci.yml` runs formatting, all-feature checks, all-feature tests, the Docker-backed Postgres e2e, and the credentialed remote libSQL e2e when `LIBSQL_REMOTE_URL` and `LIBSQL_AUTH_TOKEN` repository secrets are configured.
- `examples/type-mapping-profiles` compiles generated SQLx Postgres, tokio-postgres, and native libSQL modules with external type mappings enabled for UUID, JSON, time/chrono, and decimal profiles where each driver supports them.
- Native libSQL runtime adapters now support feature-gated external type conversion/decoding for `uuid`, `serde_json`, `time`, `chrono`, and `rust_decimal`.
- `queryforge-build` still generates into `OUT_DIR/queryforge` by default.
- `examples/usage-app` demonstrates `queryforge-build`, `queryforge.toml`, SQL blocks, and including generated code from `OUT_DIR`.
- The test suite covers config parsing/display, parser behavior, parameter normalization, codegen idempotency, config-relative generation paths, native libSQL connection/transaction execution adapters, generated native libSQL execution, an opt-in Docker Postgres e2e, and an opt-in credentialed remote libSQL e2e.

## Milestone status

- No blocking hardening items remain from this milestone.
- The credentialed remote libSQL e2e is wired into CI and runs when `LIBSQL_REMOTE_URL` and `LIBSQL_AUTH_TOKEN` repository secrets are configured.
- The current SELECT compatibility fallback is intentionally retained for parser-miss compatibility shapes; future SQL dialect edge cases should move to `sqlparser-rs` AST lowering as they are discovered.
- Future semantic inference additions should be driven by concrete query fixtures and tests; Postgres prepared-statement metadata remains the source of truth for types.

## E2E tests

Normal tests skip Docker work by default:

```bash
cargo test --workspace --all-features
```

Run the Postgres e2e explicitly with:

```bash
QUERYFORGE_E2E_POSTGRES=1 cargo test -p queryforge --features postgres --test postgres_e2e -- --nocapture
```

Run the remote libSQL e2e explicitly with:

```bash
QUERYFORGE_E2E_LIBSQL_REMOTE=1 \
QUERYFORGE_LIBSQL_REMOTE_URL="libsql://..." \
QUERYFORGE_LIBSQL_AUTH_TOKEN="..." \
cargo test -p queryforge --features libsql-remote --test libsql_remote_e2e -- --nocapture
```
