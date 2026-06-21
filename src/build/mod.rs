use std::path::PathBuf;

use crate::engine::{generate, GenerateOptions, GenerateReport};
use crate::error::Result;

#[derive(Debug, Clone)]
pub struct BuildGenerate {
    config_path: PathBuf,
    out_dir: Option<PathBuf>,
    watches: Vec<PathBuf>,
}

impl Default for BuildGenerate {
    fn default() -> Self {
        Self {
            config_path: PathBuf::from("queryforge.toml"),
            out_dir: None,
            watches: Vec::new(),
        }
    }
}

impl BuildGenerate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn config(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_path = path.into();
        self
    }

    pub fn out_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.out_dir = Some(path.into());
        self
    }

    pub fn output_dir(self, path: impl Into<PathBuf>) -> Self {
        self.out_dir(path)
    }

    pub fn watch(mut self, path: impl Into<PathBuf>) -> Self {
        self.watches.push(path.into());
        self
    }

    pub fn run(self) -> Result<GenerateReport> {
        println!("cargo:rerun-if-changed={}", self.config_path.display());
        for watch in &self.watches {
            println!("cargo:rerun-if-changed={}", watch.display());
        }

        let mut opts = GenerateOptions::from_config_path(self.config_path);
        opts.out_dir = self.out_dir.or_else(default_out_dir);
        generate(opts)
    }
}

fn default_out_dir() -> Option<PathBuf> {
    std::env::var_os("OUT_DIR").map(|dir| PathBuf::from(dir).join("queryforge"))
}

pub fn generate_builder() -> BuildGenerate {
    BuildGenerate::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, OnceLock};

    #[test]
    fn defaults_to_out_dir_queryforge() {
        let _guard = env_lock().lock().unwrap();
        let root = temp_dir("queryforge-build-default-out-dir");
        let out_dir = root.join("target-out");
        write_fixture(&root);

        let previous = std::env::var_os("OUT_DIR");
        std::env::set_var("OUT_DIR", &out_dir);

        let report = BuildGenerate::new()
            .config(root.join("queryforge.toml"))
            .run()
            .unwrap();

        restore_out_dir(previous);
        assert_eq!(report.queries_generated, 1);
        assert!(out_dir.join("queryforge/mod.rs").exists());
        assert!(out_dir.join("queryforge/users.rs").exists());
        assert!(!root.join("configured-out/mod.rs").exists());

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn output_dir_overrides_out_dir_default() {
        let _guard = env_lock().lock().unwrap();
        let root = temp_dir("queryforge-build-custom-out-dir");
        let out_dir = root.join("target-out");
        let custom_out = root.join("custom-generated");
        write_fixture(&root);

        let previous = std::env::var_os("OUT_DIR");
        std::env::set_var("OUT_DIR", &out_dir);

        let report = BuildGenerate::new()
            .config(root.join("queryforge.toml"))
            .output_dir(&custom_out)
            .run()
            .unwrap();

        restore_out_dir(previous);
        assert_eq!(report.queries_generated, 1);
        assert!(custom_out.join("mod.rs").exists());
        assert!(custom_out.join("users.rs").exists());
        assert!(!out_dir.join("queryforge/mod.rs").exists());
        assert!(!root.join("configured-out/mod.rs").exists());

        fs::remove_dir_all(root).ok();
    }

    fn write_fixture(root: &std::path::Path) {
        fs::create_dir_all(root.join("queries")).unwrap();
        fs::write(
            root.join("queryforge.toml"),
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"

            [codegen]
            out_dir = "configured-out"
            execution_target = "libsql-native"
            query_dir = "queries"

            [schema]
            files = ["schema.sql"]
            "#,
        )
        .unwrap();
        fs::write(
            root.join("schema.sql"),
            "CREATE TABLE users (id INTEGER PRIMARY KEY);",
        )
        .unwrap();
        fs::write(
            root.join("queries/users.sql"),
            "--! get_user : one\nSELECT id FROM users WHERE id = :id;",
        )
        .unwrap();
    }

    fn restore_out_dir(previous: Option<std::ffi::OsString>) {
        if let Some(previous) = previous {
            std::env::set_var("OUT_DIR", previous);
        } else {
            std::env::remove_var("OUT_DIR");
        }
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn temp_dir(name: &str) -> PathBuf {
        let unique = format!(
            "{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
