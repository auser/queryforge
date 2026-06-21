# QueryForge Usage App Example

Runnable app showing native libSQL generated APIs.

This example demonstrates:

- `build.rs` integration through `queryforge-build`
- generated code included from `OUT_DIR/queryforge`
- generated SQL constants
- generated native libSQL functions
- per-query type overrides with `queryforge::scalar_newtype!`
- transaction-compatible generated APIs

## Run

From the workspace root:

```bash
cargo run -p queryforge-usage-app
```

The binary prints generated SQL constants, creates an in-memory libSQL database, and calls generated `get_user` and `list_users` functions. `get_user` uses `UserId` and `EmailAddress` newtypes generated through SQL `--:` overrides and implemented once with `queryforge::scalar_newtype!`.

Run its execution test with:

```bash
cargo test -p queryforge-usage-app
```

The test creates an in-memory libSQL database and calls generated functions with both `libsql::Connection` and `libsql::Transaction`.

## Key Files

- `build.rs` invokes `queryforge-build`.
- `src/main.rs` includes generated code, defines `UserId`/`EmailAddress`, and contains the native libSQL execution test.
- `queryforge.toml` selects `libsql-native`.
- `queries/users.sql` defines `get_user` with type overrides and `list_users`.
