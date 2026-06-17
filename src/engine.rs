use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::backends;
use crate::codegen::{self, GeneratedFile};
use crate::config::{Config, OfflineMode};
use crate::diagnostics::Diagnostic;
use crate::error::{Error, Result};
use crate::fingerprint::Fingerprint;
use crate::ir::ProjectShape;
use crate::parser;

const OFFLINE_METADATA_FORMAT: &str = "queryforge-offline-metadata-v0";

#[derive(Debug, Clone)]
pub struct GenerateOptions {
    pub config_path: PathBuf,
    pub project_root: PathBuf,
    pub out_dir: Option<PathBuf>,
    pub mode: GenerateMode,
}

impl GenerateOptions {
    pub fn from_config_path(path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: path.into(),
            project_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            out_dir: None,
            mode: GenerateMode::WriteFiles,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateMode {
    WriteFiles,
    CheckOnly,
    PrepareOffline,
}

#[derive(Debug, Clone)]
pub struct GenerateReport {
    pub project_fingerprint: String,
    pub files_written: Vec<PathBuf>,
    pub queries_generated: usize,
    pub warnings: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub struct CheckOptions {
    pub config_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CheckReport {
    pub ok: bool,
    pub warnings: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub struct PrepareOptions {
    pub config_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PrepareReport {
    pub metadata_path: PathBuf,
}

pub fn generate(options: GenerateOptions) -> Result<GenerateReport> {
    let out_dir_override = options.out_dir.clone();
    let mode = options.mode;
    let (config, config_base) = load_config_and_base(&options)?;
    crate::type_map::validate_type_mapping_features(&config)?;
    let out_dir =
        out_dir_override.unwrap_or_else(|| resolve_path(&config_base, &config.codegen.out_dir));

    if config.build.offline == OfflineMode::True {
        let metadata = OfflineMetadata::load(config_base.join(".queryforge/metadata.json"))?;
        metadata.validate_config(&config)?;
        metadata.validate_freshness(&config, &config_base)?;
        let files_written = match mode {
            GenerateMode::WriteFiles => codegen::write_files_to_dir(&metadata.files, out_dir)?,
            GenerateMode::CheckOnly | GenerateMode::PrepareOffline => Vec::new(),
        };
        return Ok(GenerateReport {
            project_fingerprint: metadata.project_fingerprint,
            files_written,
            queries_generated: metadata.queries_generated,
            warnings: Vec::new(),
        });
    }

    let (_config, _config_base, project) = inspect_project(options)?;

    let files_written = match mode {
        GenerateMode::WriteFiles => codegen::generate_to_dir(&project, out_dir)?,
        GenerateMode::CheckOnly | GenerateMode::PrepareOffline => Vec::new(),
    };

    Ok(GenerateReport {
        project_fingerprint: project.fingerprint.0,
        files_written,
        queries_generated: project.queries.len(),
        warnings: Vec::new(),
    })
}

fn load_config_and_base(options: &GenerateOptions) -> Result<(Config, PathBuf)> {
    let mut config = Config::from_path(&options.config_path)?;
    let config_base = options
        .config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| options.project_root.clone());
    config.schema.files = config
        .schema
        .files
        .iter()
        .map(|path| resolve_path(&config_base, path))
        .collect();
    config.migrations.paths = config
        .migrations
        .paths
        .iter()
        .map(|path| resolve_path(&config_base, path))
        .collect();
    Ok((config, config_base))
}

fn inspect_project(options: GenerateOptions) -> Result<(Config, PathBuf, ProjectShape)> {
    let (config, config_base) = load_config_and_base(&options)?;
    let query_dir = resolve_path(&config_base, &config.codegen.query_dir);
    let queries = parser::parse_queries_dir(&query_dir)?;
    let project = backends::inspect(&config, queries)?;
    Ok((config, config_base, project))
}

fn resolve_path(base: &std::path::Path, path: &std::path::Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn generate_resolves_paths_relative_to_config_file() {
        let root = temp_dir("queryforge-engine");
        fs::create_dir_all(root.join("queries")).unwrap();
        fs::write(
            root.join("queryforge.toml"),
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"

            [codegen]
            out_dir = "generated"
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

        let report = generate(GenerateOptions::from_config_path(
            root.join("queryforge.toml"),
        ))
        .expect("generation should succeed");

        assert_eq!(report.queries_generated, 1);
        assert!(root.join("generated/mod.rs").exists());
        assert!(root.join("generated/users.rs").exists());

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn prepare_writes_offline_metadata_next_to_config() {
        let root = temp_dir("queryforge-prepare");
        fs::create_dir_all(root.join("queries")).unwrap();
        fs::write(
            root.join("queryforge.toml"),
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"

            [codegen]
            out_dir = "generated"
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

        let report = prepare(PrepareOptions {
            config_path: root.join("queryforge.toml"),
        })
        .expect("prepare should write metadata");

        assert_eq!(report.metadata_path, root.join(".queryforge/metadata.json"));
        let metadata = fs::read_to_string(&report.metadata_path).unwrap();
        assert!(metadata.contains("\"format\": \"queryforge-offline-metadata-v0\""));
        assert!(metadata.contains("\"database_backend\": \"libsql\""));
        assert!(metadata.contains("\"execution_target\": \"libsql-native\""));
        assert!(metadata.contains("\"project_fingerprint\": \"fnv1a64:"));
        assert!(metadata.contains("\"query_source_fingerprint\": \"fnv1a64:"));
        assert!(metadata.contains("\"schema_source_fingerprint\": \"fnv1a64:"));
        assert!(metadata.contains("\"queries_generated\": 1"));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn offline_generate_replays_prepared_files_without_queries_or_schema() {
        let root = temp_dir("queryforge-offline-generate");
        fs::create_dir_all(root.join("queries")).unwrap();
        fs::write(
            root.join("queryforge.toml"),
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"

            [codegen]
            out_dir = "generated"
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
        prepare(PrepareOptions {
            config_path: root.join("queryforge.toml"),
        })
        .unwrap();

        fs::remove_file(root.join("schema.sql")).unwrap();
        fs::remove_dir_all(root.join("queries")).unwrap();
        fs::write(
            root.join("queryforge.toml"),
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"

            [codegen]
            out_dir = "generated"
            execution_target = "libsql-native"
            query_dir = "queries"

            [schema]
            files = ["schema.sql"]

            [build]
            offline = "true"
            "#,
        )
        .unwrap();

        let report = generate(GenerateOptions::from_config_path(
            root.join("queryforge.toml"),
        ))
        .expect("offline generation should use prepared metadata");

        assert_eq!(report.queries_generated, 1);
        assert!(root.join("generated/mod.rs").exists());
        assert!(root.join("generated/users.rs").exists());

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn offline_generate_rejects_stale_metadata_version() {
        let root = temp_dir("queryforge-offline-stale");
        fs::create_dir_all(root.join("queries")).unwrap();
        fs::write(
            root.join("queryforge.toml"),
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"

            [codegen]
            out_dir = "generated"
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
        let report = prepare(PrepareOptions {
            config_path: root.join("queryforge.toml"),
        })
        .unwrap();
        let metadata = fs::read_to_string(&report.metadata_path).unwrap().replace(
            crate::fingerprint::QUERYFORGE_CODEGEN_VERSION,
            "0.0.0-stale",
        );
        fs::write(&report.metadata_path, metadata).unwrap();
        fs::write(
            root.join("queryforge.toml"),
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"

            [codegen]
            out_dir = "generated"
            execution_target = "libsql-native"
            query_dir = "queries"

            [schema]
            files = ["schema.sql"]

            [build]
            offline = "true"
            "#,
        )
        .unwrap();

        let err = generate(GenerateOptions::from_config_path(
            root.join("queryforge.toml"),
        ))
        .expect_err("stale offline metadata should fail");

        assert!(err
            .to_string()
            .contains("stale metadata generated by QueryForge 0.0.0-stale"));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn offline_generate_rejects_backend_and_target_mismatch() {
        let root = temp_dir("queryforge-offline-mismatch");
        fs::create_dir_all(root.join("queries")).unwrap();
        fs::write(
            root.join("queryforge.toml"),
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"

            [codegen]
            out_dir = "generated"
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
        prepare(PrepareOptions {
            config_path: root.join("queryforge.toml"),
        })
        .unwrap();
        fs::write(
            root.join("queryforge.toml"),
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"

            [codegen]
            out_dir = "generated"
            execution_target = "sqlx-sqlite"
            query_dir = "queries"

            [schema]
            files = ["schema.sql"]

            [build]
            offline = "true"
            "#,
        )
        .unwrap();

        let err = generate(GenerateOptions::from_config_path(
            root.join("queryforge.toml"),
        ))
        .expect_err("offline metadata target mismatch should fail");

        assert!(err.to_string().contains(
            "offline metadata execution target mismatch: metadata has `libsql-native`, config has `sqlx-sqlite`"
        ));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn offline_generate_rejects_stale_query_files_when_available() {
        let root = temp_dir("queryforge-offline-stale-query");
        fs::create_dir_all(root.join("queries")).unwrap();
        fs::write(
            root.join("queryforge.toml"),
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"

            [codegen]
            out_dir = "generated"
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
        prepare(PrepareOptions {
            config_path: root.join("queryforge.toml"),
        })
        .unwrap();
        fs::write(
            root.join("queries/users.sql"),
            "--! get_user : one\nSELECT id FROM users WHERE id = :id;\n-- changed",
        )
        .unwrap();
        fs::write(
            root.join("queryforge.toml"),
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"

            [codegen]
            out_dir = "generated"
            execution_target = "libsql-native"
            query_dir = "queries"

            [schema]
            files = ["schema.sql"]

            [build]
            offline = "true"
            "#,
        )
        .unwrap();

        let err = generate(GenerateOptions::from_config_path(
            root.join("queryforge.toml"),
        ))
        .expect_err("stale offline query sources should fail");

        assert!(err
            .to_string()
            .contains("offline metadata query fingerprint mismatch"));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn offline_generate_rejects_stale_schema_files_when_available() {
        let root = temp_dir("queryforge-offline-stale-schema");
        fs::create_dir_all(root.join("queries")).unwrap();
        fs::write(
            root.join("queryforge.toml"),
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"

            [codegen]
            out_dir = "generated"
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
        prepare(PrepareOptions {
            config_path: root.join("queryforge.toml"),
        })
        .unwrap();
        fs::write(
            root.join("schema.sql"),
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
        )
        .unwrap();
        fs::write(
            root.join("queryforge.toml"),
            r#"
            [database]
            backend = "libsql"
            url = "file:test.db"

            [codegen]
            out_dir = "generated"
            execution_target = "libsql-native"
            query_dir = "queries"

            [schema]
            files = ["schema.sql"]

            [build]
            offline = "true"
            "#,
        )
        .unwrap();

        let err = generate(GenerateOptions::from_config_path(
            root.join("queryforge.toml"),
        ))
        .expect_err("stale offline schema sources should fail");

        assert!(err
            .to_string()
            .contains("offline metadata schema fingerprint mismatch"));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn offline_metadata_parser_validates_format_and_version() {
        let metadata = offline_metadata_json(
            "libsql",
            "libsql-native",
            "fnv1a64:test",
            "fnv1a64:queries",
            "fnv1a64:schema",
            0,
            &[GeneratedFile {
                path: PathBuf::from("mod.rs"),
                content: "// @generated\n".to_string(),
            }],
        );

        assert!(parse_offline_metadata(&metadata).is_ok());
        assert!(parse_offline_metadata(
            &metadata.replace(OFFLINE_METADATA_FORMAT, "queryforge-offline-metadata-v999")
        )
        .unwrap_err()
        .contains("unsupported metadata format"));
        assert!(parse_offline_metadata(&metadata.replace(
            crate::fingerprint::QUERYFORGE_CODEGEN_VERSION,
            "0.0.0-stale"
        ))
        .unwrap_err()
        .contains("stale metadata generated by QueryForge 0.0.0-stale"));
    }

    #[test]
    fn offline_metadata_parser_rejects_unsafe_duplicate_and_mismatched_files() {
        let duplicate = offline_metadata_json(
            "libsql",
            "libsql-native",
            "fnv1a64:test",
            "fnv1a64:queries",
            "fnv1a64:schema",
            0,
            &[
                GeneratedFile {
                    path: PathBuf::from("mod.rs"),
                    content: String::new(),
                },
                GeneratedFile {
                    path: PathBuf::from("mod.rs"),
                    content: String::new(),
                },
            ],
        );
        assert!(parse_offline_metadata(&duplicate)
            .unwrap_err()
            .contains("duplicate generated file path"));

        let unsafe_path = offline_metadata_json(
            "libsql",
            "libsql-native",
            "fnv1a64:test",
            "fnv1a64:queries",
            "fnv1a64:schema",
            0,
            &[GeneratedFile {
                path: PathBuf::from("../escape.rs"),
                content: String::new(),
            }],
        );
        assert!(parse_offline_metadata(&unsafe_path)
            .unwrap_err()
            .contains("must not contain `..`"));

        let mismatch = offline_metadata_json(
            "libsql",
            "libsql-native",
            "fnv1a64:test",
            "fnv1a64:queries",
            "fnv1a64:schema",
            1,
            &[GeneratedFile {
                path: PathBuf::from("mod.rs"),
                content: String::new(),
            }],
        );
        assert!(parse_offline_metadata(&mismatch)
            .unwrap_err()
            .contains("metadata contains 0 generated query markers"));
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

pub fn check(options: CheckOptions) -> Result<CheckReport> {
    let opts = GenerateOptions {
        config_path: options.config_path,
        project_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        out_dir: None,
        mode: GenerateMode::CheckOnly,
    };
    let report = generate(opts)?;
    Ok(CheckReport {
        ok: true,
        warnings: report.warnings,
    })
}

pub fn prepare(options: PrepareOptions) -> Result<PrepareReport> {
    let (config, config_base, project) = inspect_project(GenerateOptions {
        config_path: options.config_path,
        project_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        out_dir: None,
        mode: GenerateMode::PrepareOffline,
    })?;
    let files = codegen::render_files(&project);
    let query_source_fingerprint = source_fingerprint_from_existing_paths(&[resolve_path(
        &config_base,
        &config.codegen.query_dir,
    )])?;
    let schema_source_fingerprint = source_fingerprint_from_existing_paths(&config.schema.files)?;
    let metadata_path = config_base.join(".queryforge/metadata.json");
    if let Some(parent) = metadata_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &metadata_path,
        offline_metadata_json(
            &project.backend.to_string(),
            &project.execution_target.to_string(),
            project.fingerprint.as_str(),
            query_source_fingerprint.as_str(),
            schema_source_fingerprint.as_str(),
            project.queries.len(),
            &files,
        ),
    )?;
    Ok(PrepareReport { metadata_path })
}

#[derive(Debug, Clone)]
struct OfflineMetadata {
    database_backend: String,
    execution_target: String,
    project_fingerprint: String,
    query_source_fingerprint: String,
    schema_source_fingerprint: String,
    queries_generated: usize,
    files: Vec<GeneratedFile>,
}

impl OfflineMetadata {
    fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let text = std::fs::read_to_string(&path).map_err(|err| {
            Error::Config(format!(
                "failed to read offline metadata {}: {err}",
                path.display()
            ))
        })?;
        parse_offline_metadata(&text).map_err(|err| {
            Error::Config(format!(
                "invalid offline metadata {}: {err}",
                path.display()
            ))
        })
    }

    fn validate_config(&self, config: &Config) -> Result<()> {
        let backend = config.database.backend.to_string();
        if self.database_backend != backend {
            return Err(Error::Config(format!(
                "offline metadata database backend mismatch: metadata has `{}`, config has `{backend}`; rerun `queryforge prepare`",
                self.database_backend
            )));
        }

        let execution_target = config.codegen.execution_target.to_string();
        if self.execution_target != execution_target {
            return Err(Error::Config(format!(
                "offline metadata execution target mismatch: metadata has `{}`, config has `{execution_target}`; rerun `queryforge prepare`",
                self.execution_target
            )));
        }

        Ok(())
    }

    fn validate_freshness(&self, config: &Config, config_base: &Path) -> Result<()> {
        let query_dir = resolve_path(config_base, &config.codegen.query_dir);
        if query_dir.exists() {
            let current = source_fingerprint_from_existing_paths(&[query_dir])?;
            if self.query_source_fingerprint != current.as_str() {
                return Err(Error::Config(format!(
                    "offline metadata query fingerprint mismatch: metadata has `{}`, current files have `{}`; rerun `queryforge prepare`",
                    self.query_source_fingerprint,
                    current.as_str()
                )));
            }
        }

        if config.schema.files.iter().any(|path| path.exists()) {
            let current = source_fingerprint_from_existing_paths(&config.schema.files)?;
            if self.schema_source_fingerprint != current.as_str() {
                return Err(Error::Config(format!(
                    "offline metadata schema fingerprint mismatch: metadata has `{}`, current files have `{}`; rerun `queryforge prepare`",
                    self.schema_source_fingerprint,
                    current.as_str()
                )));
            }
        }

        Ok(())
    }
}

fn offline_metadata_json(
    database_backend: &str,
    execution_target: &str,
    project_fingerprint: &str,
    query_source_fingerprint: &str,
    schema_source_fingerprint: &str,
    queries_generated: usize,
    files: &[GeneratedFile],
) -> String {
    let mut out = format!(
        concat!(
            "{{\n",
            "  \"format\": \"{}\",\n",
            "  \"queryforge_version\": \"{}\",\n",
            "  \"database_backend\": \"{}\",\n",
            "  \"execution_target\": \"{}\",\n",
            "  \"project_fingerprint\": \"{}\",\n",
            "  \"query_source_fingerprint\": \"{}\",\n",
            "  \"schema_source_fingerprint\": \"{}\",\n",
            "  \"queries_generated\": {},\n",
            "  \"files\": [\n"
        ),
        OFFLINE_METADATA_FORMAT,
        crate::fingerprint::QUERYFORGE_CODEGEN_VERSION,
        json_escape(database_backend),
        json_escape(execution_target),
        json_escape(project_fingerprint),
        json_escape(query_source_fingerprint),
        json_escape(schema_source_fingerprint),
        queries_generated
    );
    for (idx, file) in files.iter().enumerate() {
        let comma = if idx + 1 == files.len() { "" } else { "," };
        out.push_str(&format!(
            "    {{ \"path\": \"{}\", \"content\": \"{}\" }}{}\n",
            json_escape(&file.path.display().to_string()),
            json_escape(&file.content),
            comma
        ));
    }
    out.push_str("  ]\n}\n");
    out
}

fn source_fingerprint_from_existing_paths(paths: &[PathBuf]) -> Result<Fingerprint> {
    let existing: Vec<_> = paths.iter().filter(|path| path.exists()).cloned().collect();
    Fingerprint::from_paths(&existing)
}

fn json_escape(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

fn parse_offline_metadata(text: &str) -> std::result::Result<OfflineMetadata, String> {
    let format = extract_json_string(text, "format")?;
    if format != OFFLINE_METADATA_FORMAT {
        return Err(format!(
            "unsupported metadata format `{format}`; expected `{OFFLINE_METADATA_FORMAT}`"
        ));
    }

    let version = extract_json_string(text, "queryforge_version")?;
    if version != crate::fingerprint::QUERYFORGE_CODEGEN_VERSION {
        return Err(format!(
            "stale metadata generated by QueryForge {version}; regenerate with QueryForge {}",
            crate::fingerprint::QUERYFORGE_CODEGEN_VERSION
        ));
    }

    let project_fingerprint = extract_json_string(text, "project_fingerprint")?;
    let query_source_fingerprint = extract_json_string(text, "query_source_fingerprint")?;
    let schema_source_fingerprint = extract_json_string(text, "schema_source_fingerprint")?;
    let database_backend = extract_json_string(text, "database_backend")?;
    let execution_target = extract_json_string(text, "execution_target")?;
    let queries_generated = extract_json_usize(text, "queries_generated")?;
    let files = extract_generated_files(text)?;
    validate_offline_files(&files, queries_generated)?;
    Ok(OfflineMetadata {
        database_backend,
        execution_target,
        project_fingerprint,
        query_source_fingerprint,
        schema_source_fingerprint,
        queries_generated,
        files,
    })
}

fn validate_offline_files(
    files: &[GeneratedFile],
    queries_generated: usize,
) -> std::result::Result<(), String> {
    let mut seen = BTreeSet::new();
    let mut query_markers = 0;

    for file in files {
        validate_generated_path(&file.path)?;
        let key = file.path.to_string_lossy().to_string();
        if !seen.insert(key.clone()) {
            return Err(format!("duplicate generated file path `{key}`"));
        }
        query_markers += file.content.matches("// queryforge:query:").count();
    }

    if query_markers != queries_generated {
        return Err(format!(
            "`queries_generated` is {queries_generated}, but metadata contains {query_markers} generated query markers"
        ));
    }

    Ok(())
}

fn validate_generated_path(path: &Path) -> std::result::Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("generated file path must not be empty".to_string());
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!(
                    "generated file path `{}` must not contain `..`",
                    path.display()
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "generated file path `{}` must be relative",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn extract_json_string(text: &str, key: &str) -> std::result::Result<String, String> {
    let key_pattern = format!("\"{key}\"");
    let key_start = text
        .find(&key_pattern)
        .ok_or_else(|| format!("missing `{key}`"))?;
    let after_key = &text[key_start + key_pattern.len()..];
    let colon = after_key
        .find(':')
        .ok_or_else(|| format!("missing `:` after `{key}`"))?;
    let after_colon = after_key[colon + 1..].trim_start();
    parse_json_string(after_colon).map(|(value, _)| value)
}

fn extract_json_usize(text: &str, key: &str) -> std::result::Result<usize, String> {
    let key_pattern = format!("\"{key}\"");
    let key_start = text
        .find(&key_pattern)
        .ok_or_else(|| format!("missing `{key}`"))?;
    let after_key = &text[key_start + key_pattern.len()..];
    let colon = after_key
        .find(':')
        .ok_or_else(|| format!("missing `:` after `{key}`"))?;
    let after_colon = after_key[colon + 1..].trim_start();
    let len = after_colon
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(after_colon.len());
    after_colon[..len]
        .parse()
        .map_err(|err| format!("invalid `{key}`: {err}"))
}

fn extract_generated_files(text: &str) -> std::result::Result<Vec<GeneratedFile>, String> {
    let key_start = text
        .find("\"files\"")
        .ok_or_else(|| "missing `files`".to_string())?;
    let after_key = &text[key_start + "\"files\"".len()..];
    let bracket = after_key
        .find('[')
        .ok_or_else(|| "missing `[` after `files`".to_string())?;
    let mut rest = after_key[bracket + 1..].trim_start();
    let mut files = Vec::new();

    loop {
        rest = rest.trim_start();
        if rest.starts_with(']') {
            return Ok(files);
        }
        if !rest.starts_with('{') {
            return Err("expected generated file object".to_string());
        }

        let object_end = find_matching_object_end(rest)?;
        let object = &rest[..=object_end];
        let path = extract_json_string(object, "path")?;
        let content = extract_json_string(object, "content")?;
        files.push(GeneratedFile {
            path: PathBuf::from(path),
            content,
        });

        rest = rest[object_end + 1..].trim_start();
        if rest.starts_with(',') {
            rest = &rest[1..];
        }
    }
}

fn find_matching_object_end(text: &str) -> std::result::Result<usize, String> {
    let mut in_string = false;
    let mut escaped = false;
    for (idx, ch) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
        } else if ch == '}' {
            return Ok(idx);
        }
    }
    Err("unterminated generated file object".to_string())
}

fn parse_json_string(input: &str) -> std::result::Result<(String, &str), String> {
    let mut chars = input.char_indices();
    if chars.next().map(|(_, ch)| ch) != Some('"') {
        return Err("expected JSON string".to_string());
    }

    let mut out = String::new();
    let mut escaped = false;
    for (idx, ch) in chars {
        if escaped {
            match ch {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                other => return Err(format!("unsupported JSON escape `\\{other}`")),
            }
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Ok((out, &input[idx + ch.len_utf8()..]));
        } else {
            out.push(ch);
        }
    }

    Err("unterminated JSON string".to_string())
}
