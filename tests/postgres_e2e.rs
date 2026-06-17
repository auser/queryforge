#![cfg(feature = "postgres")]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use queryforge::config::{
    BuildConfig, CodegenConfig, DatabaseBackend, DatabaseConfig, ExecutionTarget, InferenceConfig,
    MigrationsConfig, SchemaConfig, TypeMappingConfig,
};
use queryforge::ir::{Cardinality, Nullability, ParsedQuery};
use queryforge::Config;

#[test]
fn postgres_prepared_statement_metadata_e2e() {
    if std::env::var("QUERYFORGE_E2E_POSTGRES").as_deref() != Ok("1") {
        eprintln!("skipping Postgres e2e; set QUERYFORGE_E2E_POSTGRES=1 to run");
        return;
    }

    let container = PostgresContainer::start();
    let url = format!(
        "postgres://postgres:postgres@127.0.0.1:{}/queryforge",
        container.port
    );

    wait_for_postgres(&url);
    setup_schema(&url);

    let config = Config {
        database: DatabaseConfig {
            backend: DatabaseBackend::Postgres,
            url,
        },
        codegen: CodegenConfig {
            out_dir: PathBuf::from("generated"),
            execution_target: ExecutionTarget::SqlxPostgres,
            query_dir: PathBuf::from("queries"),
        },
        schema: SchemaConfig::default(),
        migrations: MigrationsConfig::default(),
        build: BuildConfig::default(),
        inference: InferenceConfig::default(),
        type_mapping: TypeMappingConfig::default(),
    };
    let parsed = vec![
        ParsedQuery {
            name: "get_user".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql:
                "SELECT u.id, u.parent_id, u.email, u.active, o.name AS org_name, u.email || '' AS email_expr FROM users u LEFT JOIN organizations o ON o.id = u.org_id WHERE u.id = :id OR u.parent_id = :id"
                    .to_string(),
            cardinality: Cardinality::One,
        },
        ParsedQuery {
            name: "get_active_user".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql:
                "WITH active_users AS (SELECT id, email, parent_id FROM users WHERE active = true) SELECT au.id, au.email, au.parent_id, au.email || '' AS email_expr FROM active_users au WHERE au.id = :id"
                    .to_string(),
            cardinality: Cardinality::One,
        },
    ];

    let project = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(queryforge::backends::postgres::inspect(&config, parsed))
        .unwrap();
    let query = &project.queries[0];

    assert_eq!(
        query.normalized_sql,
        "SELECT u.id, u.parent_id, u.email, u.active, o.name AS org_name, u.email || '' AS email_expr FROM users u LEFT JOIN organizations o ON o.id = u.org_id WHERE u.id = $1 OR u.parent_id = $1"
    );
    assert_eq!(query.params.len(), 1);
    assert_eq!(query.params[0].name, "id");
    assert_eq!(query.params[0].position, 1);
    assert_eq!(query.params[0].db_type.as_deref(), Some("postgres:int8"));
    assert_eq!(query.params[0].rust_type.0, "i64");

    assert_eq!(query.columns.len(), 6);
    assert_eq!(query.columns[0].name, "id");
    assert_eq!(query.columns[0].rust_type.0, "i64");
    assert_eq!(query.columns[0].nullable, Nullability::NonNull);
    assert_eq!(query.columns[1].name, "parent_id");
    assert_eq!(query.columns[1].rust_type.0, "i64");
    assert_eq!(query.columns[1].nullable, Nullability::Nullable);
    assert_eq!(query.columns[2].name, "email");
    assert_eq!(query.columns[2].rust_type.0, "String");
    assert_eq!(query.columns[2].nullable, Nullability::NonNull);
    assert_eq!(query.columns[3].name, "active");
    assert_eq!(query.columns[3].rust_type.0, "bool");
    assert_eq!(query.columns[3].nullable, Nullability::NonNull);
    assert_eq!(query.columns[4].name, "org_name");
    assert_eq!(query.columns[4].rust_type.0, "String");
    assert_eq!(query.columns[4].nullable, Nullability::Nullable);
    assert_eq!(query.columns[5].name, "email_expr");
    assert_eq!(query.columns[5].nullable, Nullability::NonNull);

    let cte_query = &project.queries[1];
    assert_eq!(cte_query.columns.len(), 4);
    assert_eq!(cte_query.columns[0].name, "id");
    assert_eq!(cte_query.columns[0].nullable, Nullability::NonNull);
    assert_eq!(cte_query.columns[1].name, "email");
    assert_eq!(cte_query.columns[1].nullable, Nullability::NonNull);
    assert_eq!(cte_query.columns[2].name, "parent_id");
    assert_eq!(cte_query.columns[2].nullable, Nullability::Nullable);
    assert_eq!(cte_query.columns[3].name, "email_expr");
    assert_eq!(cte_query.columns[3].nullable, Nullability::NonNull);
}

struct PostgresContainer {
    name: String,
    port: u16,
}

impl PostgresContainer {
    fn start() -> Self {
        let name = format!(
            "queryforge-postgres-e2e-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        let output = Command::new("docker")
            .args([
                "run",
                "--rm",
                "-d",
                "--name",
                &name,
                "-e",
                "POSTGRES_PASSWORD=postgres",
                "-e",
                "POSTGRES_DB=queryforge",
                "-p",
                "127.0.0.1::5432",
                "postgres:16-alpine",
            ])
            .output()
            .expect("failed to start postgres container");

        assert!(
            output.status.success(),
            "docker run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let port_output = Command::new("docker")
            .args(["port", &name, "5432/tcp"])
            .output()
            .expect("failed to inspect postgres container port");
        assert!(
            port_output.status.success(),
            "docker port failed: {}",
            String::from_utf8_lossy(&port_output.stderr)
        );
        let port_text = String::from_utf8(port_output.stdout).unwrap();
        let port = port_text
            .trim()
            .rsplit_once(':')
            .and_then(|(_, port)| port.parse::<u16>().ok())
            .expect("docker port output should contain host port");

        Self { name, port }
    }
}

impl Drop for PostgresContainer {
    fn drop(&mut self) {
        Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output()
            .ok();
    }
}

fn wait_for_postgres(url: &str) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);

    loop {
        let result = runtime.block_on(async {
            let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls).await?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            client.simple_query("SELECT 1").await?;
            Ok::<_, tokio_postgres::Error>(())
        });

        if result.is_ok() {
            return;
        }

        assert!(
            Instant::now() < deadline,
            "postgres container did not become ready in time"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn setup_schema(url: &str) {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client
            .batch_execute(
                "
                CREATE TABLE users (
                    id BIGINT PRIMARY KEY,
                    parent_id BIGINT,
                    org_id BIGINT NOT NULL,
                    email TEXT NOT NULL,
                    active BOOLEAN NOT NULL
                );
                CREATE TABLE organizations (
                    id BIGINT PRIMARY KEY,
                    name TEXT NOT NULL
                );
                ",
            )
            .await
            .unwrap();
    });
}
