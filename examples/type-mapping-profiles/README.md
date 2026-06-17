# Type mapping profiles compile fixture

This package verifies that generated Rust compiles when external type mappings are enabled.

It does not connect to a database. The build script constructs representative `ProjectShape` values and writes generated modules for:

- SQLx Postgres
- tokio-postgres
- native libSQL

The generated code uses these external types:

- `uuid::Uuid`
- `serde_json::Value`
- `time::OffsetDateTime`
- `chrono::NaiveDate`, `chrono::NaiveTime`, `chrono::DateTime<chrono::Utc>`
- `rust_decimal::Decimal`

SQLx and native libSQL fixtures cover the full list. The tokio-postgres fixture covers `serde_json::Value` and `chrono`, which are the external mappings available through `tokio-postgres` 0.7 feature flags in this workspace.

Run from the workspace root:

```bash
cargo check -p queryforge-type-mapping-profiles-example
cargo test -p queryforge-type-mapping-profiles-example
```

This fixture catches missing SQLx/tokio-postgres feature flags and missing native libSQL runtime conversions without requiring live database infrastructure.
