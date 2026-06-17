# Postgres SQLx e2e example

This package demonstrates QueryForge generated SQLx Postgres code executing against a real Postgres database.

It is intentionally not a workspace member because its build script performs live Postgres introspection. Normal workspace checks should not require a running database.

Start Postgres:

```bash
docker run --rm --name queryforge-postgres-e2e-example \
  -e POSTGRES_PASSWORD=postgres \
  -e POSTGRES_DB=queryforge \
  -p 127.0.0.1:55432:5432 \
  postgres:16-alpine
```

In another terminal, run the example:

```bash
cargo run --manifest-path examples/postgres-e2e/Cargo.toml
```

The build script creates the schema, runs QueryForge against live `tokio-postgres` prepared statement metadata, includes generated modules from `OUT_DIR/queryforge`, and the binary calls generated `create_user`, `get_user`, `list_users`, `update_user`, and `delete_user` functions through SQLx.

The binary also opens a SQLx transaction and calls the same generated `update_user` function through `&mut *tx`, which verifies the transaction-compatible executor shape.
