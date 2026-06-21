const DATABASE_URL: &str = "postgres://postgres:postgres@127.0.0.1:55432/queryforge";

pub mod db {
    include!(concat!(env!("OUT_DIR"), "/queryforge/mod.rs"));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "postgres e2e fingerprint: {}",
        db::queryforge_metadata::QUERYFORGE_FINGERPRINT
    );

    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async { run().await })?;
    Ok(())
}

async fn run() -> Result<(), sqlx::Error> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(DATABASE_URL)
        .await?;

    sqlx::query("TRUNCATE TABLE users")
        .execute(&pool)
        .await?;

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
