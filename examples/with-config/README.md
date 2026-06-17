# QueryForge With Config Example

Runnable app showing a fuller `queryforge.toml` setup.

This example demonstrates:

- app-owned `queryforge.toml`
- `queryforge-build` integration
- generated code included from `OUT_DIR/queryforge`
- generated SQL constants for explicit `one`/`many` query blocks and an inferred `exec` mutation block
- generated native libSQL functions running against an in-memory database

## Run

From the workspace root:

```bash
cargo run -p queryforge-with-config-example
cargo test -p queryforge-with-config-example
```

The binary prints the generated project fingerprint, SQL constants, inserts a row, and reads it back. It includes SQL constants for:

- `get_user`
- `list_users`
- `create_user`

## Key Files

- `build.rs` invokes `queryforge-build`.
- `src/lib.rs` includes generated code.
- `src/main.rs` prints generated metadata and SQL constants.
- `queries/users.sql` contains the query blocks.
