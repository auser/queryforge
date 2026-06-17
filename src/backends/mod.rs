pub mod libsql;
#[cfg(feature = "postgres")]
pub mod postgres;

use crate::config::{Config, DatabaseBackend};
use crate::error::Result;
use crate::ir::{ParsedQuery, ProjectShape};

pub fn inspect(config: &Config, parsed: Vec<ParsedQuery>) -> Result<ProjectShape> {
    crate::type_map::validate_type_mapping_features(config)?;

    match config.database.backend {
        DatabaseBackend::Postgres => inspect_postgres(config, parsed),
        DatabaseBackend::Libsql => libsql::inspect(config, parsed),
    }
}

#[cfg(feature = "postgres")]
fn inspect_postgres(config: &Config, parsed: Vec<ParsedQuery>) -> Result<ProjectShape> {
    let runtime = tokio::runtime::Runtime::new().map_err(crate::error::Error::Io)?;
    runtime.block_on(postgres::inspect(config, parsed))
}

#[cfg(not(feature = "postgres"))]
fn inspect_postgres(_config: &Config, _parsed: Vec<ParsedQuery>) -> Result<ProjectShape> {
    Err(crate::error::Error::Unsupported(
        "postgres inspection requires the `postgres` feature".to_string(),
    ))
}
