use crate::config::{
    Config, DecimalTypeMapping, JsonTypeMapping, TimeTypeMapping, TypeMappingConfig,
    UuidTypeMapping,
};
use crate::error::{Error, Result};
use crate::fingerprint::{Fingerprint, QUERYFORGE_CODEGEN_VERSION};
use crate::ir::RustType;

pub fn validate_type_mapping_features(config: &Config) -> Result<()> {
    let mut missing = Vec::new();

    if config.type_mapping.uuid == UuidTypeMapping::Uuid && !cfg!(feature = "uuid-types") {
        missing.push("uuid-types");
    }
    if config.type_mapping.json == JsonTypeMapping::SerdeJson && !cfg!(feature = "serde-json-types")
    {
        missing.push("serde-json-types");
    }
    if config.type_mapping.time == TimeTypeMapping::Chrono && !cfg!(feature = "chrono-types") {
        missing.push("chrono-types");
    }
    if config.type_mapping.time == TimeTypeMapping::Time && !cfg!(feature = "time-types") {
        missing.push("time-types");
    }
    if config.type_mapping.decimal == DecimalTypeMapping::RustDecimal
        && !cfg!(feature = "decimal-types")
    {
        missing.push("decimal-types");
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "type mapping config requires enabling QueryForge feature(s): {}",
            missing.join(", ")
        )))
    }
}

pub fn type_mapping_fingerprint(config: &Config) -> Fingerprint {
    Fingerprint::from_text(&format!(
        "queryforge-version={}\nbackend={}\nexecution-target={}\ntype-mapping-profile={}\nuuid={}\njson={}\ntime={}\ndecimal={}\n",
        QUERYFORGE_CODEGEN_VERSION,
        config.database.backend,
        config.codegen.execution_target,
        config.type_mapping.profile,
        config.type_mapping.uuid,
        config.type_mapping.json,
        config.type_mapping.time,
        config.type_mapping.decimal
    ))
}

pub fn sqlite_declared_type_to_rust(declared: &str) -> RustType {
    sqlite_declared_type_to_rust_with_config(declared, &TypeMappingConfig::default())
}

pub fn sqlite_declared_type_to_rust_with_config(
    declared: &str,
    mapping: &TypeMappingConfig,
) -> RustType {
    let upper = declared.to_ascii_uppercase();
    if upper.contains("INT") {
        RustType("i64".to_string())
    } else if upper.contains("REAL") || upper.contains("FLOA") || upper.contains("DOUB") {
        RustType("f64".to_string())
    } else if upper.contains("BLOB") {
        RustType("Vec<u8>".to_string())
    } else if upper.contains("BOOL") {
        RustType("bool".to_string())
    } else if upper.contains("JSON") && mapping.json == JsonTypeMapping::SerdeJson {
        RustType("serde_json::Value".to_string())
    } else if (upper.contains("DECIMAL") || upper.contains("NUMERIC"))
        && mapping.decimal == DecimalTypeMapping::RustDecimal
    {
        RustType("rust_decimal::Decimal".to_string())
    } else {
        RustType("String".to_string())
    }
}

pub fn postgres_type_to_rust(name: &str) -> RustType {
    postgres_type_to_rust_with_config(name, &TypeMappingConfig::default())
}

pub fn postgres_type_to_rust_with_config(name: &str, mapping: &TypeMappingConfig) -> RustType {
    match name {
        "int2" | "smallint" => RustType("i16".to_string()),
        "int4" | "integer" => RustType("i32".to_string()),
        "int8" | "bigint" => RustType("i64".to_string()),
        "bool" | "boolean" => RustType("bool".to_string()),
        "float4" | "real" => RustType("f32".to_string()),
        "float8" | "double precision" => RustType("f64".to_string()),
        "bytea" => RustType("Vec<u8>".to_string()),
        "uuid" if mapping.uuid == UuidTypeMapping::Uuid => RustType("uuid::Uuid".to_string()),
        "json" | "jsonb" if mapping.json == JsonTypeMapping::SerdeJson => {
            RustType("serde_json::Value".to_string())
        }
        "date" if mapping.time == TimeTypeMapping::Chrono => {
            RustType("chrono::NaiveDate".to_string())
        }
        "time" if mapping.time == TimeTypeMapping::Chrono => {
            RustType("chrono::NaiveTime".to_string())
        }
        "timestamp" if mapping.time == TimeTypeMapping::Chrono => {
            RustType("chrono::NaiveDateTime".to_string())
        }
        "timestamptz" if mapping.time == TimeTypeMapping::Chrono => {
            RustType("chrono::DateTime<chrono::Utc>".to_string())
        }
        "date" if mapping.time == TimeTypeMapping::Time => RustType("time::Date".to_string()),
        "time" if mapping.time == TimeTypeMapping::Time => RustType("time::Time".to_string()),
        "timestamp" if mapping.time == TimeTypeMapping::Time => {
            RustType("time::PrimitiveDateTime".to_string())
        }
        "timestamptz" if mapping.time == TimeTypeMapping::Time => {
            RustType("time::OffsetDateTime".to_string())
        }
        "numeric" | "decimal" if mapping.decimal == DecimalTypeMapping::RustDecimal => {
            RustType("rust_decimal::Decimal".to_string())
        }
        _ => RustType("String".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        BuildConfig, CodegenConfig, DatabaseBackend, DatabaseConfig, ExecutionTarget,
        InferenceConfig, MigrationsConfig, SchemaConfig, TypeMappingProfile,
    };
    use std::path::PathBuf;

    #[test]
    fn default_mappings_stay_dependency_light() {
        assert_eq!(postgres_type_to_rust("uuid").0, "String");
        assert_eq!(postgres_type_to_rust("jsonb").0, "String");
        assert_eq!(postgres_type_to_rust("timestamptz").0, "String");
        assert_eq!(postgres_type_to_rust("numeric").0, "String");
        assert_eq!(sqlite_declared_type_to_rust("JSON").0, "String");
        assert_eq!(sqlite_declared_type_to_rust("NUMERIC").0, "String");
    }

    #[test]
    fn explicit_profiles_emit_external_type_paths() {
        let mapping = TypeMappingConfig {
            profile: TypeMappingProfile::Default,
            uuid: UuidTypeMapping::Uuid,
            json: JsonTypeMapping::SerdeJson,
            time: TimeTypeMapping::Time,
            decimal: DecimalTypeMapping::RustDecimal,
        };

        assert_eq!(
            postgres_type_to_rust_with_config("uuid", &mapping).0,
            "uuid::Uuid"
        );
        assert_eq!(
            postgres_type_to_rust_with_config("jsonb", &mapping).0,
            "serde_json::Value"
        );
        assert_eq!(
            postgres_type_to_rust_with_config("timestamptz", &mapping).0,
            "time::OffsetDateTime"
        );
        assert_eq!(
            postgres_type_to_rust_with_config("numeric", &mapping).0,
            "rust_decimal::Decimal"
        );
        assert_eq!(
            sqlite_declared_type_to_rust_with_config("JSON", &mapping).0,
            "serde_json::Value"
        );
        assert_eq!(
            sqlite_declared_type_to_rust_with_config("DECIMAL", &mapping).0,
            "rust_decimal::Decimal"
        );
    }

    #[test]
    fn chrono_time_mapping_uses_chrono_type_paths() {
        let mapping = TypeMappingConfig {
            time: TimeTypeMapping::Chrono,
            ..TypeMappingConfig::default()
        };

        assert_eq!(
            postgres_type_to_rust_with_config("date", &mapping).0,
            "chrono::NaiveDate"
        );
        assert_eq!(
            postgres_type_to_rust_with_config("time", &mapping).0,
            "chrono::NaiveTime"
        );
        assert_eq!(
            postgres_type_to_rust_with_config("timestamp", &mapping).0,
            "chrono::NaiveDateTime"
        );
        assert_eq!(
            postgres_type_to_rust_with_config("timestamptz", &mapping).0,
            "chrono::DateTime<chrono::Utc>"
        );
    }

    #[test]
    fn type_mapping_fingerprint_changes_with_mapping_choices() {
        let mut config = Config {
            database: DatabaseConfig {
                backend: DatabaseBackend::Postgres,
                url: "postgres://localhost/queryforge".to_string(),
            },
            codegen: CodegenConfig {
                out_dir: PathBuf::from("generated"),
                execution_target: ExecutionTarget::TokioPostgres,
                query_dir: PathBuf::from("queries"),
            },
            schema: SchemaConfig::default(),
            migrations: MigrationsConfig::default(),
            build: BuildConfig::default(),
            inference: InferenceConfig::default(),
            type_mapping: TypeMappingConfig::default(),
        };
        let default = type_mapping_fingerprint(&config);
        config.type_mapping.uuid = UuidTypeMapping::Uuid;

        assert_ne!(type_mapping_fingerprint(&config), default);
    }

    #[test]
    fn validates_required_generator_features_for_external_mappings() {
        let config = Config {
            database: DatabaseConfig {
                backend: DatabaseBackend::Postgres,
                url: "postgres://localhost/queryforge".to_string(),
            },
            codegen: CodegenConfig {
                out_dir: PathBuf::from("generated"),
                execution_target: ExecutionTarget::TokioPostgres,
                query_dir: PathBuf::from("queries"),
            },
            schema: SchemaConfig::default(),
            migrations: MigrationsConfig::default(),
            build: BuildConfig::default(),
            inference: InferenceConfig::default(),
            type_mapping: TypeMappingConfig {
                uuid: UuidTypeMapping::Uuid,
                json: JsonTypeMapping::SerdeJson,
                time: TimeTypeMapping::Chrono,
                decimal: DecimalTypeMapping::RustDecimal,
                ..TypeMappingConfig::default()
            },
        };

        let result = validate_type_mapping_features(&config);
        if cfg!(all(
            feature = "uuid-types",
            feature = "serde-json-types",
            feature = "chrono-types",
            feature = "decimal-types"
        )) {
            assert!(result.is_ok());
        } else {
            let err = result.unwrap_err().to_string();
            assert!(err.contains("type mapping config requires enabling QueryForge feature"));
        }
    }
}
