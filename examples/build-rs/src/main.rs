use queryforge_build_rs_example::db;

fn main() {
    println!(
        "fingerprint: {}",
        db::queryforge_metadata::QUERYFORGE_FINGERPRINT
    );
    println!("get_user:\n{}", db::users::GET_USER_SQL);
    println!("list_users:\n{}", db::users::LIST_USERS_SQL);
    println!("insert_user:\n{}", db::users::INSERT_USER_SQL);

    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async { run().await });
}

async fn run() {
    let database = libsql::Builder::new_local(":memory:")
        .build()
        .await
        .unwrap();
    let conn = database.connect().unwrap();

    libsql::Connection::execute(
        &conn,
        "CREATE TABLE users (
            id INTEGER PRIMARY KEY,
            email TEXT NOT NULL,
            created_at TEXT NOT NULL
        ) STRICT",
        (),
    )
    .await
    .unwrap();

    db::users::insert_user(
        &conn,
        db::users::InsertUserParams {
            id: 1,
            email: "a@example.com".to_string(),
            created_at: "2026-06-17".to_string(),
        },
    )
    .await
    .unwrap();

    let user = db::users::get_user(&conn, db::users::GetUserParams { id: 1 })
        .await
        .unwrap();
    println!("inserted user: {} at {}", user.email, user.created_at);

    let users = db::users::list_users(&conn).await.unwrap();
    println!("users: {}", users.len());
}
