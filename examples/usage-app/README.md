# QueryForge Usage App Example

Runnable app showing native libSQL generated APIs.

This example demonstrates:

- `build.rs` integration through `queryforge-build`
- generated code included from `OUT_DIR/queryforge`
- generated SQL constants
- generated native libSQL functions
- transaction-compatible generated APIs

## Run

From the workspace root:

```bash
cargo run -p queryforge-usage-app
```

The binary prints generated SQL constants, creates an in-memory libSQL database, and calls generated `get_user` and `list_users` functions.

Run its execution test with:

```bash
cargo test -p queryforge-usage-app
```

The test creates an in-memory libSQL database and calls generated functions with both `libsql::Connection` and `libsql::Transaction`.

## Key Files

- `build.rs` invokes `queryforge-build`.
- `src/main.rs` includes generated code and contains the native libSQL execution test.
- `queryforge.toml` selects `libsql-native`.
- `queries/users.sql` defines `get_user` and `list_users`.
