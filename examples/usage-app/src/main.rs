pub mod db {
    include!(concat!(env!("OUT_DIR"), "/queryforge/mod.rs"));
}

fn main() {
    println!(
        "fingerprint: {}",
        db::queryforge_metadata::QUERYFORGE_FINGERPRINT
    );
    println!("get_user:\n{}", db::users::GET_USER_SQL);
    println!("list_users:\n{}", db::users::LIST_USERS_SQL);

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
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL)",
        (),
    )
    .await
    .unwrap();
    libsql::Connection::execute(
        &conn,
        "INSERT INTO users (id, email) VALUES (?1, ?2)",
        (1_i64, "a@example.com"),
    )
    .await
    .unwrap();

    let user = db::users::get_user(&conn, 1).await.unwrap();
    println!("user: {}", user.email);

    let users = db::users::list_users(&conn).await.unwrap();
    println!("users: {}", users.len());
}

#[cfg(test)]
mod tests {
    use super::db;
    use queryforge::runtime::libsql_executor::{LibsqlExecutor, LibsqlValue};

    #[test]
    fn generated_native_libsql_functions_execute_with_connection_and_transaction() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let database = libsql::Builder::new_local(":memory:")
                .build()
                .await
                .unwrap();
            let conn = database.connect().unwrap();

            libsql::Connection::execute(
                &conn,
                "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL)",
                (),
            )
            .await
            .unwrap();
            libsql::Connection::execute(
                &conn,
                "INSERT INTO users (id, email) VALUES (?1, ?2)",
                (1_i64, "a@example.com"),
            )
            .await
            .unwrap();

            let user = db::users::get_user(&conn, 1).await.unwrap();
            assert_eq!(user.id, 1);
            assert_eq!(user.email, "a@example.com");

            let tx = conn.transaction().await.unwrap();
            LibsqlExecutor::execute(
                &tx,
                "INSERT INTO users (id, email) VALUES (?1, ?2)",
                &[
                    LibsqlValue::Integer(2),
                    LibsqlValue::Text("b@example.com".to_string()),
                ],
            )
            .await
            .unwrap();
            let tx_user = db::users::get_user(&tx, 2).await.unwrap();
            assert_eq!(tx_user.email, "b@example.com");
            tx.rollback().await.unwrap();

            let users = db::users::list_users(&conn).await.unwrap();
            assert_eq!(users.len(), 1);
            assert_eq!(users[0].id, 1);
        });
    }
}
