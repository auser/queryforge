fn main() {
    queryforge_build::generate()
        .config("queryforge.toml")
        .watch("queries")
        .watch("schema.sql")
        .run()
        .expect("queryforge generation failed");
}
