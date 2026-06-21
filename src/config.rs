use std::path::PathBuf;

use crate::error::{Error, Result};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Config {
    pub database: DatabaseConfig,
    pub codegen: CodegenConfig,

    #[serde(default)]
    pub schema: SchemaConfig,

    #[serde(default)]
    pub migrations: MigrationsConfig,

    #[serde(default)]
    pub build: BuildConfig,

    #[serde(default)]
    pub inference: InferenceConfig,

    #[serde(default)]
    pub type_mapping: TypeMappingConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionTarget {
    SqlxPostgres,
    TokioPostgres,

    #[serde(alias = "libsql")]
    LibsqlNative,

    SqlxSqlite,
}

impl std::fmt::Display for ExecutionTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SqlxPostgres => "sqlx-postgres",
            Self::TokioPostgres => "tokio-postgres",
            Self::LibsqlNative => "libsql-native",
            Self::SqlxSqlite => "sqlx-sqlite",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DatabaseConfig {
    pub backend: DatabaseBackend,
    pub url: String,

    #[serde(default)]
    pub auth_token: Option<String>,

    #[serde(default)]
    pub auth_token_env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseBackend {
    Postgres,
    Libsql,
}

impl std::fmt::Display for DatabaseBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DatabaseBackend::Postgres => write!(f, "postgres"),
            DatabaseBackend::Libsql => write!(f, "libsql"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CodegenConfig {
    pub out_dir: PathBuf,
    pub execution_target: ExecutionTarget,

    #[serde(default = "default_query_dir")]
    pub query_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct SchemaConfig {
    #[serde(default)]
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct MigrationsConfig {
    #[serde(default)]
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TypeMappingConfig {
    #[serde(default = "default_type_mapping_profile")]
    pub profile: TypeMappingProfile,

    #[serde(default)]
    pub uuid: UuidTypeMapping,

    #[serde(default)]
    pub json: JsonTypeMapping,

    #[serde(default)]
    pub time: TimeTypeMapping,

    #[serde(default)]
    pub decimal: DecimalTypeMapping,
}

impl Default for TypeMappingConfig {
    fn default() -> Self {
        Self {
            profile: default_type_mapping_profile(),
            uuid: UuidTypeMapping::default(),
            json: JsonTypeMapping::default(),
            time: TimeTypeMapping::default(),
            decimal: DecimalTypeMapping::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TypeMappingProfile {
    Default,
}

impl std::fmt::Display for TypeMappingProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Default => "default",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum UuidTypeMapping {
    #[default]
    String,
    Uuid,
}

impl std::fmt::Display for UuidTypeMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::String => "string",
            Self::Uuid => "uuid",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum JsonTypeMapping {
    #[default]
    String,
    SerdeJson,
}

impl std::fmt::Display for JsonTypeMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::String => "string",
            Self::SerdeJson => "serde-json",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TimeTypeMapping {
    #[default]
    String,
    Chrono,
    Time,
}

impl std::fmt::Display for TimeTypeMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::String => "string",
            Self::Chrono => "chrono",
            Self::Time => "time",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DecimalTypeMapping {
    #[default]
    String,
    RustDecimal,
}

impl std::fmt::Display for DecimalTypeMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::String => "string",
            Self::RustDecimal => "rust-decimal",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BuildConfig {
    #[serde(default = "default_build_output")]
    pub default_output: BuildOutput,

    #[serde(default = "default_offline_mode")]
    pub offline: OfflineMode,

    #[serde(default)]
    pub watch: Vec<PathBuf>,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            default_output: default_build_output(),
            offline: default_offline_mode(),
            watch: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildOutput {
    OutDir,
    Source,
}

impl std::fmt::Display for BuildOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::OutDir => "out-dir",
            Self::Source => "source",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OfflineMode {
    Auto,
    True,
    False,
}

impl std::fmt::Display for OfflineMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::True => "true",
            Self::False => "false",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InferenceConfig {
    #[serde(default = "default_unknown_expression_policy")]
    pub unknown_expression_policy: UnknownExpressionPolicy,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            unknown_expression_policy: default_unknown_expression_policy(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnknownExpressionPolicy {
    Error,
    String,
    OptionString,
}

impl std::fmt::Display for UnknownExpressionPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Error => "error",
            Self::String => "string",
            Self::OptionString => "option-string",
        })
    }
}

impl Config {
    pub fn from_path(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::load(path)
    }

    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let path = path.as_ref();

        let contents = std::fs::read_to_string(path)
            .map_err(|err| Error::Config(format!("failed to read {}: {err}", path.display())))?;

        toml::from_str(&contents)
            .map_err(|err| Error::Config(format!("failed to parse {}: {err}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_config_with_defaults_and_aliases() {
        let config: Config = toml::from_str(
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"
            auth_token_env = "LIBSQL_AUTH_TOKEN"

            [codegen]
            out_dir = "src/db"
            execution_target = "libsql"

            [inference]
            unknown_expression_policy = "option-string"

            [type_mapping]
            uuid = "uuid"
            json = "serde-json"
            time = "chrono"
            decimal = "rust-decimal"
            "#,
        )
        .unwrap();

        assert_eq!(config.database.backend, DatabaseBackend::Libsql);
        assert_eq!(config.database.auth_token, None);
        assert_eq!(
            config.database.auth_token_env.as_deref(),
            Some("LIBSQL_AUTH_TOKEN")
        );
        assert_eq!(
            config.codegen.execution_target,
            ExecutionTarget::LibsqlNative
        );
        assert_eq!(config.codegen.query_dir, PathBuf::from("queries"));
        assert_eq!(config.build.default_output, BuildOutput::OutDir);
        assert_eq!(config.build.offline, OfflineMode::Auto);
        assert_eq!(
            config.inference.unknown_expression_policy,
            UnknownExpressionPolicy::OptionString
        );
        assert_eq!(config.type_mapping.profile, TypeMappingProfile::Default);
        assert_eq!(config.type_mapping.uuid, UuidTypeMapping::Uuid);
        assert_eq!(config.type_mapping.json, JsonTypeMapping::SerdeJson);
        assert_eq!(config.type_mapping.time, TimeTypeMapping::Chrono);
        assert_eq!(config.type_mapping.decimal, DecimalTypeMapping::RustDecimal);
    }

    #[test]
    fn display_uses_stable_kebab_case_values() {
        assert_eq!(DatabaseBackend::Postgres.to_string(), "postgres");
        assert_eq!(DatabaseBackend::Libsql.to_string(), "libsql");
        assert_eq!(ExecutionTarget::SqlxPostgres.to_string(), "sqlx-postgres");
        assert_eq!(ExecutionTarget::TokioPostgres.to_string(), "tokio-postgres");
        assert_eq!(ExecutionTarget::LibsqlNative.to_string(), "libsql-native");
        assert_eq!(ExecutionTarget::SqlxSqlite.to_string(), "sqlx-sqlite");
        assert_eq!(BuildOutput::OutDir.to_string(), "out-dir");
        assert_eq!(OfflineMode::False.to_string(), "false");
        assert_eq!(
            UnknownExpressionPolicy::OptionString.to_string(),
            "option-string"
        );
        assert_eq!(TypeMappingProfile::Default.to_string(), "default");
        assert_eq!(UuidTypeMapping::Uuid.to_string(), "uuid");
        assert_eq!(JsonTypeMapping::SerdeJson.to_string(), "serde-json");
        assert_eq!(TimeTypeMapping::Chrono.to_string(), "chrono");
        assert_eq!(TimeTypeMapping::Time.to_string(), "time");
        assert_eq!(DecimalTypeMapping::RustDecimal.to_string(), "rust-decimal");
    }
}

fn default_query_dir() -> PathBuf {
    PathBuf::from("queries")
}

fn default_build_output() -> BuildOutput {
    BuildOutput::OutDir
}

fn default_offline_mode() -> OfflineMode {
    OfflineMode::Auto
}

fn default_unknown_expression_policy() -> UnknownExpressionPolicy {
    UnknownExpressionPolicy::Error
}

fn default_type_mapping_profile() -> TypeMappingProfile {
    TypeMappingProfile::Default
}
