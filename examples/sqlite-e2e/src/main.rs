pub mod db {
    include!(concat!(env!("OUT_DIR"), "/queryforge/mod.rs"));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "sqlite e2e fingerprint: {}",
        db::queryforge_metadata::QUERYFORGE_FINGERPRINT
    );

    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async { run().await })?;
    Ok(())
}

async fn run() -> Result<(), sqlx::Error> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    create_schema(&pool).await?;

    let inserted = db::users::create_user(
        &pool,
        db::users::CreateUserParams {
            id: 1,
            email: "ada@example.com".into(),
            name: "Ada".into(),
            active: true,
        },
    )
    .await?;
    println!("inserted rows: {}", inserted.rows_affected());

    let user = db::users::get_user(&pool, db::users::GetUserParams { id: 1 })
        .await?
        .expect("user exists");
    println!("loaded user: {} ({})", user.email, user.name);

    let mut tx = pool.begin().await?;
    db::users::update_user(
        &mut *tx,
        db::users::UpdateUserParams {
            email: "ada@queryforge.dev".into(),
            name: "Ada Q.".into(),
            active: false,
            id: 1,
        },
    )
    .await?;
    tx.commit().await?;

    let users = db::users::list_users(&pool).await?;
    println!("users after transaction update: {}", users.len());

    let deleted = db::users::delete_user(&pool, db::users::DeleteUserParams { id: 1 }).await?;
    println!("deleted rows: {}", deleted.rows_affected());

    Ok(())
}

async fn create_schema(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE users (
            id INTEGER PRIMARY KEY,
            email TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            active BOOLEAN NOT NULL DEFAULT TRUE
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{create_schema, db};

    #[test]
    fn generated_sqlx_sqlite_functions_execute_against_pool_and_transaction() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();
            create_schema(&pool).await.unwrap();

            let inserted = db::users::create_user(
                &pool,
                db::users::CreateUserParams {
                    id: 1,
                    email: "ada@example.com".into(),
                    name: "Ada".into(),
                    active: true,
                },
            )
            .await
            .unwrap();
            assert_eq!(inserted.rows_affected(), 1);

            let mut tx = pool.begin().await.unwrap();
            db::users::update_user(
                &mut *tx,
                db::users::UpdateUserParams {
                    email: "ada@queryforge.dev".into(),
                    name: "Ada Q.".into(),
                    active: false,
                    id: 1,
                },
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();

            let user = db::users::get_user(&pool, db::users::GetUserParams { id: 1 })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(user.email, "ada@queryforge.dev");
            assert_eq!(user.name, "Ada Q.");
            assert!(!user.active);

            let users = db::users::list_users(&pool).await.unwrap();
            assert_eq!(users.len(), 1);

            let deleted = db::users::delete_user(&pool, db::users::DeleteUserParams { id: 1 })
                .await
                .unwrap();
            assert_eq!(deleted.rows_affected(), 1);
            assert!(
                db::users::get_user(&pool, db::users::GetUserParams { id: 1 })
                    .await
                    .unwrap()
                    .is_none()
            );
        });
    }
}
