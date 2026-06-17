# Live libSQL Catalog Example

This example shows QueryForge reading schema metadata from a real local libSQL/SQLite database instead of `[schema].files`.

The build script creates `catalog.db` with normal SQL DDL, then calls `queryforge-build` with the `libsql-runtime` feature enabled. QueryForge opens the local database and introspects:

- `sqlite_schema`
- `PRAGMA table_xinfo`
- `PRAGMA index_list` / `PRAGMA index_info`
- `PRAGMA foreign_key_list`

Run it from the workspace root:

```bash
cargo run -p queryforge-live-libsql-catalog-example
```

The generated code is included from `OUT_DIR/queryforge`. The `catalog.db` file is created locally by `build.rs` and ignored by git.

The important config detail is that `queryforge.toml` does not define `[schema].files`:

```toml
[database]
backend = "libsql"
url = "file:catalog.db"

[codegen]
execution_target = "libsql-native"
query_dir = "queries"
```

Remote libSQL catalog introspection is not implemented yet. For remote databases, provide `[schema].files` for offline inference.
