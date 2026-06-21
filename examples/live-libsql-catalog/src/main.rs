use std::path::PathBuf;

pub mod db {
    include!(concat!(env!("OUT_DIR"), "/queryforge/mod.rs"));
}

fn main() {
    println!(
        "fingerprint: {}",
        db::queryforge_metadata::QUERYFORGE_FINGERPRINT
    );
    println!(
        "schema fingerprint: {}",
        db::queryforge_metadata::QUERYFORGE_SCHEMA_FINGERPRINT
    );

    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async { run().await });
}

async fn run() {
    let db_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("catalog.db");
    let database = libsql::Builder::new_local(db_path.to_string_lossy().into_owned())
        .build()
        .await
        .unwrap();
    let conn = database.connect().unwrap();

    let user = db::users::get_user(&conn, db::users::GetUserParams { id: 1 })
        .await
        .unwrap();
    println!("user: {} ({})", user.email, user.org_slug);

    let users = db::users::list_users(&conn).await.unwrap();
    println!("users: {}", users.len());
}
