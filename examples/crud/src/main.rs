pub mod db {
    include!(concat!(env!("OUT_DIR"), "/queryforge/mod.rs"));
}

fn main() {
    println!(
        "fingerprint: {}",
        db::queryforge_metadata::QUERYFORGE_FINGERPRINT
    );
    println!("create_user:\n{}", db::users::CREATE_USER_SQL);
    println!("update_user:\n{}", db::users::UPDATE_USER_SQL);
    println!("upsert_user:\n{}", db::users::UPSERT_USER_SQL);
    println!("delete_user:\n{}", db::users::DELETE_USER_SQL);

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
    create_schema(&conn).await;

    db::users::create_user(&conn, 1, "a@example.com".into(), "Ada".into(), true)
        .await
        .unwrap();
    db::users::upsert_user(&conn, 1, "ada@queryforge.dev".into(), "Ada Q.".into(), true)
        .await
        .unwrap();

    let user = db::users::get_user(&conn, 1).await.unwrap().unwrap();
    println!("user after upsert: {} ({})", user.email, user.name);

    db::users::delete_user(&conn, 1).await.unwrap();
    println!(
        "users after delete: {}",
        db::users::list_users(&conn).await.unwrap().len()
    );
}

async fn create_schema(conn: &libsql::Connection) {
    libsql::Connection::execute(
        conn,
        "CREATE TABLE users (
            id INTEGER PRIMARY KEY,
            email TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            active BOOLEAN NOT NULL DEFAULT TRUE
        )",
        (),
    )
    .await
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::{create_schema, db};

    #[test]
    fn generated_crud_functions_execute() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let database = libsql::Builder::new_local(":memory:")
                .build()
                .await
                .unwrap();
            let conn = database.connect().unwrap();
            create_schema(&conn).await;

            let inserted =
                db::users::create_user(&conn, 1, "a@example.com".into(), "Ada".into(), true)
                    .await
                    .unwrap();
            assert_eq!(inserted, 1);

            let user = db::users::get_user(&conn, 1).await.unwrap().unwrap();
            assert_eq!(user.email, "a@example.com");
            assert_eq!(user.name, "Ada");
            assert!(user.active);

            let updated = db::users::update_user(
                &conn,
                "ada@example.com".into(),
                "Ada Lovelace".into(),
                false,
                1,
            )
            .await
            .unwrap();
            assert_eq!(updated, 1);
            let user = db::users::get_user(&conn, 1).await.unwrap().unwrap();
            assert_eq!(user.email, "ada@example.com");
            assert_eq!(user.name, "Ada Lovelace");
            assert!(!user.active);

            let upserted = db::users::upsert_user(
                &conn,
                1,
                "ada@queryforge.dev".into(),
                "Ada Q.".into(),
                true,
            )
            .await
            .unwrap();
            assert_eq!(upserted, 1);
            let user = db::users::get_user(&conn, 1).await.unwrap().unwrap();
            assert_eq!(user.email, "ada@queryforge.dev");
            assert_eq!(user.name, "Ada Q.");
            assert!(user.active);

            db::users::create_user(&conn, 2, "b@example.com".into(), "Babbage".into(), true)
                .await
                .unwrap();
            let users = db::users::list_users(&conn).await.unwrap();
            assert_eq!(users.len(), 2);

            let deleted = db::users::delete_user(&conn, 1).await.unwrap();
            assert_eq!(deleted, 1);
            assert!(db::users::get_user(&conn, 1).await.unwrap().is_none());
        });
    }
}
