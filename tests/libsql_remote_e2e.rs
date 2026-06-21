#![cfg(feature = "libsql-remote")]

use std::path::PathBuf;

use queryforge::config::{
    BuildConfig, CodegenConfig, DatabaseBackend, DatabaseConfig, ExecutionTarget, InferenceConfig,
    MigrationsConfig, SchemaConfig, TypeMappingConfig,
};
use queryforge::ir::{Cardinality, Nullability, ParsedQuery};
use queryforge::Config;

#[test]
fn remote_libsql_live_catalog_e2e() {
    if std::env::var("QUERYFORGE_E2E_LIBSQL_REMOTE").as_deref() != Ok("1") {
        eprintln!(
            "skipping remote libSQL e2e; set QUERYFORGE_E2E_LIBSQL_REMOTE=1, \
             QUERYFORGE_LIBSQL_REMOTE_URL, and QUERYFORGE_LIBSQL_AUTH_TOKEN to run"
        );
        return;
    }

    let url = std::env::var("QUERYFORGE_LIBSQL_REMOTE_URL")
        .expect("QUERYFORGE_LIBSQL_REMOTE_URL must be set for remote libSQL e2e");
    let token = std::env::var("QUERYFORGE_LIBSQL_AUTH_TOKEN")
        .expect("QUERYFORGE_LIBSQL_AUTH_TOKEN must be set for remote libSQL e2e");
    let table_name = unique_table_name();

    tokio::runtime::Runtime::new().unwrap().block_on(async {
        setup_remote_table(&url, &token, &table_name).await;
    });

    let _cleanup = RemoteTableCleanup {
        url: url.clone(),
        token: token.clone(),
        table_name: table_name.clone(),
    };

    let config = Config {
        database: DatabaseConfig {
            backend: DatabaseBackend::Libsql,
            url,
            auth_token: Some(token),
            auth_token_env: None,
        },
        codegen: CodegenConfig {
            out_dir: PathBuf::from("generated"),
            execution_target: ExecutionTarget::LibsqlNative,
            query_dir: PathBuf::from("queries"),
        },
        schema: SchemaConfig::default(),
        migrations: MigrationsConfig::default(),
        build: BuildConfig::default(),
        inference: InferenceConfig::default(),
        type_mapping: TypeMappingConfig::default(),
    };
    let parsed = vec![ParsedQuery {
        name: "get_remote_user".to_string(),
        source_file: PathBuf::from("queries/users.sql"),
        original_sql: format!(
            "SELECT id, email, parent_id, active FROM {table_name} WHERE id = :id"
        ),
        cardinality: Cardinality::One,
    }];

    let project = queryforge::backends::libsql::inspect(&config, parsed).unwrap();
    let query = &project.queries[0];

    assert_eq!(query.columns.len(), 4);
    assert_eq!(query.columns[0].name, "id");
    assert_eq!(query.columns[0].rust_type.0, "i64");
    assert_eq!(query.columns[0].nullable, Nullability::NonNull);
    assert_eq!(query.columns[1].name, "email");
    assert_eq!(query.columns[1].rust_type.0, "String");
    assert_eq!(query.columns[1].nullable, Nullability::NonNull);
    assert_eq!(query.columns[2].name, "parent_id");
    assert_eq!(query.columns[2].nullable, Nullability::Nullable);
    assert_eq!(query.columns[3].name, "active");
    assert_eq!(query.columns[3].rust_type.0, "bool");
    assert_eq!(query.params.len(), 1);
    assert_eq!(query.params[0].name, "id");
    assert_eq!(query.params[0].rust_type.0, "i64");
    assert_eq!(query.params[0].db_type.as_deref(), Some("sqlite:INTEGER"));
}

async fn setup_remote_table(url: &str, token: &str, table_name: &str) {
    let db = libsql::Builder::new_remote(url.to_string(), token.to_string())
        .build()
        .await
        .expect("failed to open remote libSQL database");
    let conn = db
        .connect()
        .expect("failed to connect to remote libSQL database");
    let drop_sql = format!("DROP TABLE IF EXISTS {table_name}");
    libsql::Connection::execute(&conn, &drop_sql, ())
        .await
        .expect("failed to drop stale remote e2e table");
    let create_sql = format!(
        "CREATE TABLE {table_name} (
            id INTEGER PRIMARY KEY,
            email TEXT NOT NULL,
            parent_id INTEGER,
            active BOOLEAN NOT NULL
        )"
    );
    libsql::Connection::execute(&conn, &create_sql, ())
        .await
        .expect("failed to create remote e2e table");
}

struct RemoteTableCleanup {
    url: String,
    token: String,
    table_name: String,
}

impl Drop for RemoteTableCleanup {
    fn drop(&mut self) {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let db = libsql::Builder::new_remote(self.url.clone(), self.token.clone())
                .build()
                .await;
            let Ok(db) = db else {
                return;
            };
            let Ok(conn) = db.connect() else {
                return;
            };
            let sql = format!("DROP TABLE IF EXISTS {}", self.table_name);
            let _ = libsql::Connection::execute(&conn, &sql, ()).await;
        });
    }
}

fn unique_table_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("queryforge_remote_e2e_{}_{}", std::process::id(), nanos)
}
