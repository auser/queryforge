# QueryForge Basic Example

Minimal CLI-driven QueryForge fixture.

This directory is not a Cargo package. It demonstrates the files a user project needs before wiring generated code into an application:

- `queryforge.toml`
- `schema.sql`
- `queries/users.sql`
- `.queryforge/metadata.json` from `queryforge prepare`

## Run

From the workspace root:

```bash
cargo run -p queryforge-cli -- generate examples/basic/queryforge.toml
```

This writes generated Rust to:

```text
examples/basic/src/db
```

Refresh offline metadata with:

```bash
cargo run -p queryforge-cli -- prepare examples/basic/queryforge.toml
```

That updates:

```text
examples/basic/.queryforge/metadata.json
```

