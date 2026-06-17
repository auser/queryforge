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
        ParsedQuery {
            name: "get_user_expressions".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql:
                "SELECT CASE WHEN active THEN email ELSE 'inactive@example.com' END AS display_email, CASE WHEN parent_id IS NULL THEN email END AS maybe_email, NULLIF(email, '') AS nullified_email, active AND email <> '' AS valid_user, parent_id = :parent_id AS parent_matches, id BETWEEN :min_id AND :max_id AS id_between, email LIKE '%@example.com' AS email_like, parent_id IN (1, 2) AS parent_in, parent_id + 1 AS parent_plus_one FROM users WHERE id = :id"
                    .to_string(),
            cardinality: Cardinality::One,
        },
        ParsedQuery {
            name: "walk_nodes".to_string(),
            source_file: PathBuf::from("queries/nodes.sql"),
            original_sql:
                "WITH RECURSIVE tree(node_id, parent_node_id, node_label) AS (SELECT id, parent_id, label FROM nodes WHERE id = :root_id UNION ALL SELECT n.id, n.parent_id, n.label FROM nodes n JOIN tree t ON n.parent_id = t.node_id) SELECT tree.node_id, tree.parent_node_id, tree.node_label, tree.node_label || '' AS label_expr FROM tree"
                    .to_string(),
            cardinality: Cardinality::Many,
        },
        ParsedQuery {
            name: "walk_nodes_recursive_branch_nullable".to_string(),
            source_file: PathBuf::from("queries/nodes.sql"),
            original_sql:
                "WITH RECURSIVE nullable_tree(node_id, maybe_parent_id) AS (SELECT id, id FROM nodes WHERE id = :root_id UNION ALL SELECT n.id, n.parent_id FROM nodes n JOIN nullable_tree t ON n.parent_id = t.node_id) SELECT nullable_tree.node_id, nullable_tree.maybe_parent_id FROM nullable_tree"
                    .to_string(),
            cardinality: Cardinality::Many,
        },
        ParsedQuery {
            name: "get_user_lateral".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql:
                "SELECT u.id, lateral_user.email_expr, lateral_user.parent_expr FROM users u JOIN LATERAL (SELECT u.email || '' AS email_expr, u.parent_id || '' AS parent_expr) lateral_user ON true WHERE u.id = :id"
                    .to_string(),
            cardinality: Cardinality::One,
        },
        ParsedQuery {
            name: "get_user_join_group".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql:
                "SELECT u.id, o.name AS org_name, a.slug AS account_slug FROM users u LEFT JOIN (organizations o JOIN accounts a ON a.org_id = o.id) ON o.id = u.org_id WHERE u.id = :id"
                    .to_string(),
            cardinality: Cardinality::One,
        },
        ParsedQuery {
            name: "get_user_comma_join".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql:
                "SELECT u.id, o.name || '' AS org_expr FROM users u, organizations o WHERE u.org_id = o.id AND u.id = :id"
                    .to_string(),
            cardinality: Cardinality::One,
        },
        ParsedQuery {
            name: "get_user_lateral_group".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql:
                "SELECT u.id, org_meta.org_expr FROM users u LEFT JOIN (organizations o JOIN LATERAL (SELECT o.name || '' AS org_expr) org_meta ON true) ON o.id = u.org_id WHERE u.id = :id"
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

    let expression_query = &project.queries[2];
    assert_eq!(expression_query.params.len(), 4);
    assert_eq!(expression_query.params[0].name, "parent_id");
    assert_eq!(
        expression_query.params[0].db_type.as_deref(),
        Some("postgres:int8")
    );
    assert_eq!(expression_query.params[1].name, "min_id");
    assert_eq!(
        expression_query.params[1].db_type.as_deref(),
        Some("postgres:int8")
    );
    assert_eq!(expression_query.params[2].name, "max_id");
    assert_eq!(
        expression_query.params[2].db_type.as_deref(),
        Some("postgres:int8")
    );
    assert_eq!(expression_query.params[3].name, "id");
    assert_eq!(
        expression_query.params[3].db_type.as_deref(),
        Some("postgres:int8")
    );
    assert_eq!(expression_query.columns.len(), 9);
    assert_eq!(expression_query.columns[0].name, "display_email");
    assert_eq!(expression_query.columns[0].nullable, Nullability::NonNull);
    assert_eq!(expression_query.columns[1].name, "maybe_email");
    assert_eq!(expression_query.columns[1].nullable, Nullability::Nullable);
    assert_eq!(expression_query.columns[2].name, "nullified_email");
    assert_eq!(expression_query.columns[2].nullable, Nullability::Nullable);
    assert_eq!(expression_query.columns[3].name, "valid_user");
    assert_eq!(expression_query.columns[3].nullable, Nullability::NonNull);
    assert_eq!(expression_query.columns[4].name, "parent_matches");
    assert_eq!(expression_query.columns[4].nullable, Nullability::Nullable);
    assert_eq!(expression_query.columns[5].name, "id_between");
    assert_eq!(expression_query.columns[5].nullable, Nullability::NonNull);
    assert_eq!(expression_query.columns[6].name, "email_like");
    assert_eq!(expression_query.columns[6].nullable, Nullability::NonNull);
    assert_eq!(expression_query.columns[7].name, "parent_in");
    assert_eq!(expression_query.columns[7].nullable, Nullability::Nullable);
    assert_eq!(expression_query.columns[8].name, "parent_plus_one");
    assert_eq!(expression_query.columns[8].nullable, Nullability::Nullable);

    let recursive_query = &project.queries[3];
    assert_eq!(recursive_query.params.len(), 1);
    assert_eq!(recursive_query.params[0].name, "root_id");
    assert_eq!(
        recursive_query.params[0].db_type.as_deref(),
        Some("postgres:int8")
    );
    assert_eq!(recursive_query.columns.len(), 4);
    assert_eq!(recursive_query.columns[0].name, "node_id");
    assert_eq!(recursive_query.columns[0].nullable, Nullability::NonNull);
    assert_eq!(recursive_query.columns[1].name, "parent_node_id");
    assert_eq!(recursive_query.columns[1].nullable, Nullability::Nullable);
    assert_eq!(recursive_query.columns[2].name, "node_label");
    assert_eq!(recursive_query.columns[2].nullable, Nullability::NonNull);
    assert_eq!(recursive_query.columns[3].name, "label_expr");
    assert_eq!(recursive_query.columns[3].nullable, Nullability::NonNull);

    let recursive_branch_query = &project.queries[4];
    assert_eq!(recursive_branch_query.params.len(), 1);
    assert_eq!(recursive_branch_query.params[0].name, "root_id");
    assert_eq!(recursive_branch_query.columns.len(), 2);
    assert_eq!(recursive_branch_query.columns[0].name, "node_id");
    assert_eq!(
        recursive_branch_query.columns[0].nullable,
        Nullability::NonNull
    );
    assert_eq!(recursive_branch_query.columns[1].name, "maybe_parent_id");
    assert_eq!(
        recursive_branch_query.columns[1].nullable,
        Nullability::Nullable
    );

    let lateral_query = &project.queries[5];
    assert_eq!(lateral_query.params.len(), 1);
    assert_eq!(lateral_query.params[0].name, "id");
    assert_eq!(
        lateral_query.params[0].db_type.as_deref(),
        Some("postgres:int8")
    );
    assert_eq!(lateral_query.columns.len(), 3);
    assert_eq!(lateral_query.columns[0].name, "id");
    assert_eq!(lateral_query.columns[0].nullable, Nullability::NonNull);
    assert_eq!(lateral_query.columns[1].name, "email_expr");
    assert_eq!(lateral_query.columns[1].nullable, Nullability::NonNull);
    assert_eq!(lateral_query.columns[2].name, "parent_expr");
    assert_eq!(lateral_query.columns[2].nullable, Nullability::Nullable);

    let join_group_query = &project.queries[6];
    assert_eq!(join_group_query.params.len(), 1);
    assert_eq!(join_group_query.params[0].name, "id");
    assert_eq!(join_group_query.columns.len(), 3);
    assert_eq!(join_group_query.columns[0].name, "id");
    assert_eq!(join_group_query.columns[0].nullable, Nullability::NonNull);
    assert_eq!(join_group_query.columns[1].name, "org_name");
    assert_eq!(join_group_query.columns[1].nullable, Nullability::Nullable);
    assert_eq!(join_group_query.columns[2].name, "account_slug");
    assert_eq!(join_group_query.columns[2].nullable, Nullability::Nullable);

    let comma_join_query = &project.queries[7];
    assert_eq!(comma_join_query.params.len(), 1);
    assert_eq!(comma_join_query.params[0].name, "id");
    assert_eq!(comma_join_query.columns.len(), 2);
    assert_eq!(comma_join_query.columns[0].name, "id");
    assert_eq!(comma_join_query.columns[0].nullable, Nullability::NonNull);
    assert_eq!(comma_join_query.columns[1].name, "org_expr");
    assert_eq!(comma_join_query.columns[1].nullable, Nullability::NonNull);

    let lateral_group_query = &project.queries[8];
    assert_eq!(lateral_group_query.params.len(), 1);
    assert_eq!(lateral_group_query.params[0].name, "id");
    assert_eq!(lateral_group_query.columns.len(), 2);
    assert_eq!(lateral_group_query.columns[0].name, "id");
    assert_eq!(
        lateral_group_query.columns[0].nullable,
        Nullability::NonNull
    );
    assert_eq!(lateral_group_query.columns[1].name, "org_expr");
    assert_eq!(
        lateral_group_query.columns[1].nullable,
        Nullability::Nullable
    );
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
                CREATE TABLE accounts (
                    id BIGINT PRIMARY KEY,
                    org_id BIGINT NOT NULL REFERENCES organizations(id),
                    slug TEXT NOT NULL
                );
                CREATE TABLE nodes (
                    id BIGINT PRIMARY KEY,
                    parent_id BIGINT,
                    label TEXT NOT NULL
                );
                ",
            )
            .await
            .unwrap();
    });
}
