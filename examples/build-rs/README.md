# QueryForge build.rs Example

Small library crate showing `queryforge-build` from a Cargo build script.

The build script:

- reads `queryforge.toml`
- watches `queryforge.toml`, `schema.sql`, and `queries`
- generates code into `OUT_DIR/queryforge`
- exposes generated code through `include!`
- runs generated native libSQL functions against an in-memory database

To write somewhere else, add `.output_dir("src/db")` to the builder. The default intentionally avoids dirtying source files.

## Run

From the workspace root:

```bash
cargo run -p queryforge-build-rs-example
cargo check -p queryforge-build-rs-example
cargo test -p queryforge-build-rs-example
```

The binary prints generated SQL constants, inserts a row through the generated `insert_user` function, and reads it back with generated query functions.

## Key Files

- `build.rs` shows the build helper API.
- `src/lib.rs` includes generated code from `OUT_DIR`.
- `src/main.rs` runs generated functions against an in-memory libSQL database.
- `queries/users.sql` contains QueryForge `--! name` / `--! name : cardinality` blocks.
