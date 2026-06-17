use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set");
    let db_path = Path::new(&manifest_dir).join("catalog.db");

    create_catalog(&db_path).expect("failed to create live libSQL catalog");

    queryforge_build::generate()
        .config("queryforge.toml")
        .watch("queryforge.toml")
        .watch("build.rs")
        .watch("queries")
        .run()
        .expect("queryforge generation failed");
}

fn create_catalog(db_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = db_path.to_string_lossy().into_owned();
    tokio::runtime::Runtime::new()?.block_on(async move {
        let database = libsql::Builder::new_local(db_path).build().await?;
        let conn = database.connect()?;

        libsql::Connection::execute(&conn, "PRAGMA foreign_keys = ON", ()).await?;
        libsql::Connection::execute(&conn, "DROP TABLE IF EXISTS users", ()).await?;
        libsql::Connection::execute(&conn, "DROP TABLE IF EXISTS organizations", ()).await?;
        libsql::Connection::execute(
            &conn,
            "CREATE TABLE organizations (
                id INTEGER PRIMARY KEY,
                slug TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL
            )",
            (),
        )
        .await?;
        libsql::Connection::execute(
            &conn,
            "CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                org_id INTEGER NOT NULL REFERENCES organizations(id),
                email TEXT NOT NULL,
                display_name TEXT,
                active BOOLEAN NOT NULL DEFAULT TRUE,
                profile_json JSON,
                balance DECIMAL
            )",
            (),
        )
        .await?;
        libsql::Connection::execute(&conn, "CREATE INDEX users_org_id_idx ON users(org_id)", ())
            .await?;
        libsql::Connection::execute(
            &conn,
            "INSERT INTO organizations (id, slug, name) VALUES (?1, ?2, ?3)",
            (1_i64, "acme", "Acme Corp"),
        )
        .await?;
        libsql::Connection::execute(
            &conn,
            r#"INSERT INTO users (id, org_id, email, display_name, active, profile_json, balance)
               VALUES (1, 1, 'a@example.com', 'Ada', 1, '{"role":"admin"}', '42.50')"#,
            (),
        )
        .await?;

        Ok::<_, Box<dyn std::error::Error>>(())
    })
}
