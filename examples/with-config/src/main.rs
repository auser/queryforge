use queryforge_with_config_example::db;

fn main() {
    println!("QueryForge usage app");

    // These names match the starter renderer's intended output shape.
    // If your generated module names differ, inspect:
    //   target/debug/build/queryforge-usage-app-*/out/queryforge/mod.rs
    println!(
        "fingerprint: {}",
        db::queryforge_metadata::QUERYFORGE_FINGERPRINT
    );

    println!("\nGenerated SQL constants:\n");
    println!("get_user:\n{}", db::users::GET_USER_SQL);
    println!("list_users:\n{}", db::users::LIST_USERS_SQL);
    println!("create_user:\n{}", db::users::CREATE_USER_SQL);
}
