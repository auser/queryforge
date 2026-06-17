# QueryForge build.rs Example

Small library crate showing `queryforge-build` from a Cargo build script.

The build script:

- reads `queryforge.toml`
- watches `queryforge.toml`, `schema.sql`, and `queries`
- generates code into `OUT_DIR/queryforge`
- exposes generated code through `include!`

## Run

From the workspace root:

```bash
cargo check -p queryforge-build-rs-example
cargo test -p queryforge-build-rs-example
```

This crate has no binary target, so `cargo run -p queryforge-build-rs-example` is not expected to work.

## Key Files

- `build.rs` shows the build helper API.
- `src/lib.rs` includes generated code from `OUT_DIR`.
- `queries/users.sql` contains QueryForge `--! name : cardinality` blocks.

