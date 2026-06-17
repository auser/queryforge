fn main() {
    queryforge_build::generate()
        .config("queryforge.toml")
        .watch("queryforge.toml")
        .watch("schema.sql")
        .watch("queries")
        .run()
        .expect("queryforge generation failed");
}
