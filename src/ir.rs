use std::path::PathBuf;

use crate::config::{DatabaseBackend, ExecutionTarget};
use crate::error::{Error, Result};
use crate::fingerprint::Fingerprint;

pub type BackendKind = DatabaseBackend;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cardinality {
    One,
    Optional,
    Many,
    Exec,
    Stream,
    Scalar,
    Batch,
}

impl Cardinality {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "one" => Ok(Self::One),
            "optional" => Ok(Self::Optional),
            "many" => Ok(Self::Many),
            "exec" => Ok(Self::Exec),
            "stream" => Ok(Self::Stream),
            "scalar" => Ok(Self::Scalar),
            "batch" => Ok(Self::Batch),
            other => Err(Error::Parse(format!("unknown cardinality `{other}`"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nullability {
    NonNull,
    Nullable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeSource {
    DatabaseMetadata,
    SchemaCatalog,
    BuiltinFunctionRule,
    ExpressionInference,
    UserOverride,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceConfidence {
    Exact,
    Strong,
    Weak,
    UserOverride,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustType(pub String);

impl RustType {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn string() -> Self {
        Self("String".to_string())
    }
    pub fn unit() -> Self {
        Self("()".to_string())
    }
}

#[derive(Debug, Clone)]
pub struct QueryParam {
    pub name: String,
    pub position: usize,
    pub db_type: Option<String>,
    pub rust_type: RustType,
    pub source: TypeSource,
    pub confidence: InferenceConfidence,
}

#[derive(Debug, Clone)]
pub struct QueryColumn {
    pub name: String,
    pub rust_name: String,
    pub db_type: Option<String>,
    pub rust_type: RustType,
    pub nullable: Nullability,
    pub source: TypeSource,
    pub confidence: InferenceConfidence,
}

#[derive(Debug, Clone, Default)]
pub struct QueryDependencies {
    pub tables: Vec<String>,
    pub functions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ParsedQuery {
    pub name: String,
    pub source_file: PathBuf,
    pub original_sql: String,
    pub cardinality: Cardinality,
}

#[derive(Debug, Clone)]
pub struct QueryShape {
    pub name: String,
    pub module_path: Vec<String>,
    pub source_file: PathBuf,
    pub original_sql: String,
    pub normalized_sql: String,
    pub cardinality: Cardinality,
    pub params: Vec<QueryParam>,
    pub columns: Vec<QueryColumn>,
    pub dependencies: QueryDependencies,
    pub fingerprint: Fingerprint,
}

#[derive(Debug, Clone)]
pub struct ProjectShape {
    pub backend: DatabaseBackend,
    pub execution_target: ExecutionTarget,
    pub schema_fingerprint: Fingerprint,
    pub migration_fingerprint: Fingerprint,
    pub type_mapping_fingerprint: Fingerprint,
    pub queries: Vec<QueryShape>,
    pub fingerprint: Fingerprint,
}
