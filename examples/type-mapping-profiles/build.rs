use std::path::PathBuf;

use queryforge::codegen;
use queryforge::config::{DatabaseBackend, ExecutionTarget};
use queryforge::fingerprint::Fingerprint;
use queryforge::ir::{
    Cardinality, InferenceConfidence, Nullability, ProjectShape, QueryColumn, QueryDependencies,
    QueryParam, QueryShape, RustType, TypeSource,
};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    render_project(
        &out_dir.join("sqlx_postgres"),
        ExecutionTarget::SqlxPostgres,
        "$1",
    );
    render_project(
        &out_dir.join("tokio_postgres"),
        ExecutionTarget::TokioPostgres,
        "$1",
    );
    render_project(
        &out_dir.join("libsql_native"),
        ExecutionTarget::LibsqlNative,
        "?1",
    );
}

fn render_project(out_dir: &std::path::Path, execution_target: ExecutionTarget, placeholder: &str) {
    let queries = match execution_target {
        ExecutionTarget::TokioPostgres => vec![tokio_supported_row_query()],
        _ => vec![
            external_row_query(placeholder),
            external_exec_query(placeholder),
            chrono_row_query(placeholder),
        ],
    };

    let project = ProjectShape {
        backend: match execution_target {
            ExecutionTarget::LibsqlNative | ExecutionTarget::SqlxSqlite => DatabaseBackend::Libsql,
            ExecutionTarget::SqlxPostgres | ExecutionTarget::TokioPostgres => {
                DatabaseBackend::Postgres
            }
        },
        execution_target,
        schema_fingerprint: Fingerprint::from_text("external-type-schema"),
        migration_fingerprint: Fingerprint::from_text("external-type-migrations"),
        type_mapping_fingerprint: Fingerprint::from_text("external-type-mapping"),
        queries,
        fingerprint: Fingerprint::from_text("external-type-project"),
    };

    codegen::generate_to_dir(&project, out_dir).expect("fixture generation succeeds");
}

fn tokio_supported_row_query() -> QueryShape {
    QueryShape {
        name: "get_tokio_supported_values".to_string(),
        module_path: vec!["profiles".to_string()],
        source_file: PathBuf::from("queries/profiles.sql"),
        original_sql: "SELECT payload, happened_at FROM external_values".to_string(),
        normalized_sql: "SELECT payload, happened_at FROM external_values".to_string(),
        cardinality: Cardinality::One,
        params: Vec::new(),
        columns: vec![
            column(
                "payload",
                "serde_json::Value",
                "jsonb",
                Nullability::NonNull,
            ),
            column(
                "happened_at",
                "chrono::DateTime<chrono::Utc>",
                "timestamptz",
                Nullability::NonNull,
            ),
        ],
        dependencies: QueryDependencies::default(),
        fingerprint: Fingerprint::from_text("get-tokio-supported-values"),
    }
}

fn external_row_query(placeholder: &str) -> QueryShape {
    QueryShape {
        name: "get_external_values".to_string(),
        module_path: vec!["profiles".to_string()],
        source_file: PathBuf::from("queries/profiles.sql"),
        original_sql: format!("SELECT {placeholder} AS id"),
        normalized_sql: format!(
            "SELECT id, payload, happened_at, amount FROM external_values WHERE id = {placeholder}"
        ),
        cardinality: Cardinality::One,
        params: vec![param("id", 1, "uuid::Uuid", "uuid")],
        columns: vec![
            column("id", "uuid::Uuid", "uuid", Nullability::NonNull),
            column(
                "payload",
                "serde_json::Value",
                "jsonb",
                Nullability::NonNull,
            ),
            column(
                "happened_at",
                "time::OffsetDateTime",
                "timestamptz",
                Nullability::NonNull,
            ),
            column(
                "amount",
                "rust_decimal::Decimal",
                "numeric",
                Nullability::Nullable,
            ),
        ],
        dependencies: QueryDependencies::default(),
        fingerprint: Fingerprint::from_text("get-external-values"),
    }
}

fn external_exec_query(placeholder: &str) -> QueryShape {
    let placeholders = numbered_placeholders(placeholder, 4);
    QueryShape {
        name: "insert_external_values".to_string(),
        module_path: vec!["profiles".to_string()],
        source_file: PathBuf::from("queries/profiles.sql"),
        original_sql: "INSERT INTO external_values (...) VALUES (...)".to_string(),
        normalized_sql: format!(
            "INSERT INTO external_values (id, payload, happened_at, amount) VALUES ({}, {}, {}, {})",
            placeholders[0], placeholders[1], placeholders[2], placeholders[3]
        ),
        cardinality: Cardinality::Exec,
        params: vec![
            param("id", 1, "uuid::Uuid", "uuid"),
            param("payload", 2, "serde_json::Value", "jsonb"),
            param("happened_at", 3, "time::OffsetDateTime", "timestamptz"),
            param("amount", 4, "rust_decimal::Decimal", "numeric"),
        ],
        columns: Vec::new(),
        dependencies: QueryDependencies::default(),
        fingerprint: Fingerprint::from_text("insert-external-values"),
    }
}

fn chrono_row_query(placeholder: &str) -> QueryShape {
    QueryShape {
        name: "get_chrono_values".to_string(),
        module_path: vec!["profiles".to_string()],
        source_file: PathBuf::from("queries/profiles.sql"),
        original_sql: format!("SELECT {placeholder} AS id"),
        normalized_sql: format!(
            "SELECT day, at_time, happened_at FROM chrono_values WHERE id = {placeholder}"
        ),
        cardinality: Cardinality::One,
        params: vec![param("id", 1, "uuid::Uuid", "uuid")],
        columns: vec![
            column("day", "chrono::NaiveDate", "date", Nullability::NonNull),
            column("at_time", "chrono::NaiveTime", "time", Nullability::NonNull),
            column(
                "happened_at",
                "chrono::DateTime<chrono::Utc>",
                "timestamptz",
                Nullability::NonNull,
            ),
        ],
        dependencies: QueryDependencies::default(),
        fingerprint: Fingerprint::from_text("get-chrono-values"),
    }
}

fn numbered_placeholders(first: &str, count: usize) -> Vec<String> {
    let prefix = if first.starts_with('$') { "$" } else { "?" };
    (1..=count)
        .map(|position| format!("{prefix}{position}"))
        .collect()
}

fn param(name: &str, position: usize, rust_type: &str, db_type: &str) -> QueryParam {
    QueryParam {
        name: name.to_string(),
        position,
        db_type: Some(format!("db:{db_type}")),
        rust_type: RustType::new(rust_type),
        source: TypeSource::DatabaseMetadata,
        confidence: InferenceConfidence::Exact,
    }
}

fn column(name: &str, rust_type: &str, db_type: &str, nullable: Nullability) -> QueryColumn {
    QueryColumn {
        name: name.to_string(),
        rust_name: name.to_string(),
        db_type: Some(format!("db:{db_type}")),
        rust_type: RustType::new(rust_type),
        nullable,
        source: TypeSource::DatabaseMetadata,
        confidence: InferenceConfidence::Exact,
    }
}
