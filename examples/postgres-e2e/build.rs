use std::time::{Duration, Instant};

const DATABASE_URL: &str = "postgres://postgres:postgres@127.0.0.1:55432/queryforge";

fn main() {
    println!("cargo:rerun-if-changed=queryforge.toml");
    println!("cargo:rerun-if-changed=schema.sql");
    println!("cargo:rerun-if-changed=queries");

    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async { prepare_database().await });

    queryforge_build::generate()
        .config("queryforge.toml")
        .watch("queryforge.toml")
        .watch("schema.sql")
        .watch("queries")
        .run()
        .expect("queryforge generation failed");
}

async fn prepare_database() {
    let deadline = Instant::now() + Duration::from_secs(30);

    loop {
        match tokio_postgres::connect(DATABASE_URL, tokio_postgres::NoTls).await {
            Ok((client, connection)) => {
                tokio::spawn(async move {
                    let _ = connection.await;
                });
                client.batch_execute(include_str!("schema.sql")).await.unwrap();
                return;
            }
            Err(err) if Instant::now() < deadline => {
                println!("cargo:warning=waiting for Postgres example database: {err}");
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(err) => {
                panic!(
                    "Postgres e2e example needs a database at {DATABASE_URL}; \
                     start it with the docker command from examples/postgres-e2e/README.md: {err}"
                );
            }
        }
    }
}
