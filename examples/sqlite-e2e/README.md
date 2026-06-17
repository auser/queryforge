# SQLite SQLx e2e example

This package demonstrates QueryForge generated SQLx SQLite code executing against a real in-memory SQLite database.

Run it from the workspace root:

```bash
cargo run -p queryforge-sqlite-e2e-example
cargo test -p queryforge-sqlite-e2e-example
```

The build script runs QueryForge, includes generated modules from `OUT_DIR/queryforge`, creates an in-memory SQLite database at runtime, and calls generated `create_user`, `get_user`, `list_users`, `update_user`, and `delete_user` functions.

The example also opens a SQLx transaction and calls the same generated `update_user` function through `&mut *tx`, which is the transaction-compatible executor shape QueryForge generates for SQLx targets.
