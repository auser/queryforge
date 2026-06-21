use std::collections::BTreeMap;

use crate::config::{Config, TypeMappingConfig, UuidTypeMapping};
use crate::error::{Error, Result};
use crate::fingerprint::{Fingerprint, QUERYFORGE_CODEGEN_VERSION};
use crate::ir::{
    InferenceConfidence, Nullability, ParsedQuery, ProjectShape, QueryColumn, QueryDependencies,
    QueryParam, QueryShape, RustType, TypeSource,
};
use crate::names::to_snake_case;
use crate::sql_ir::{self, ColumnNullability, SelectProjection, SqlStatement};
use crate::type_map::{sqlite_declared_type_to_rust_with_config, type_mapping_fingerprint};

pub fn inspect(config: &Config, parsed: Vec<ParsedQuery>) -> Result<ProjectShape> {
    crate::type_map::validate_type_mapping_features(config)?;

    let catalog = SchemaCatalog::load(config)?;
    let schema_fp = catalog.fingerprint.clone();
    let migration_fp = Fingerprint::from_paths(&config.migrations.paths)?;
    let type_mapping_fp = type_mapping_fingerprint(config);
    let queries: Vec<QueryShape> = parsed
        .into_iter()
        .map(|q| {
            shape_query(
                config,
                &catalog,
                &schema_fp,
                &migration_fp,
                &type_mapping_fp,
                q,
            )
        })
        .collect::<Result<_>>()?;
    let mut project_text = format!(
        "queryforge-version={}\nbackend={}\nexecution-target={}\ninference-policy={}\ntype-mapping={}\nschema={}\nmigrations={}\n",
        QUERYFORGE_CODEGEN_VERSION,
        config.database.backend,
        config.codegen.execution_target,
        config.inference.unknown_expression_policy,
        type_mapping_fp,
        schema_fp,
        migration_fp
    );
    for q in &queries {
        project_text.push_str(q.fingerprint.as_str());
        project_text.push('\n');
    }
    Ok(ProjectShape {
        backend: config.database.backend.clone(),
        execution_target: config.codegen.execution_target.clone(),
        schema_fingerprint: schema_fp,
        migration_fingerprint: migration_fp,
        type_mapping_fingerprint: type_mapping_fp,
        queries,
        fingerprint: Fingerprint::from_text(&project_text),
    })
}

fn shape_query(
    config: &Config,
    catalog: &SchemaCatalog,
    schema_fp: &Fingerprint,
    migration_fp: &Fingerprint,
    type_mapping_fp: &Fingerprint,
    q: ParsedQuery,
) -> Result<QueryShape> {
    let normalized = normalize_named_params(&q.original_sql, "?");
    let query_analysis = analyze_query(&q.original_sql, catalog, &config.type_mapping)?;
    let params = normalized
        .param_names
        .into_iter()
        .enumerate()
        .map(|(idx, name)| QueryParam {
            rust_type: query_analysis
                .param_types
                .get(&name)
                .cloned()
                .unwrap_or_else(RustType::string),
            db_type: query_analysis
                .param_db_types
                .get(&name)
                .cloned()
                .unwrap_or(None),
            source: if query_analysis.param_types.contains_key(&name) {
                TypeSource::SchemaCatalog
            } else {
                TypeSource::Unknown
            },
            confidence: if query_analysis.param_types.contains_key(&name) {
                InferenceConfidence::Strong
            } else {
                InferenceConfidence::Weak
            },
            name,
            position: idx + 1,
        })
        .collect::<Vec<_>>();

    let fp = Fingerprint::from_text(&format!(
        "queryforge-version={}\nbackend={}\nexecution-target={}\ninference-policy={}\ntype-mapping={}\nschema={}\nmigrations={}\nquery={}\ncardinality={:?}\nsql={}\nparams={:?}\ncolumns={:?}\n",
        QUERYFORGE_CODEGEN_VERSION,
        config.database.backend,
        config.codegen.execution_target,
        config.inference.unknown_expression_policy,
        type_mapping_fp,
        schema_fp,
        migration_fp,
        q.name,
        q.cardinality,
        normalized.sql,
        params,
        query_analysis.columns
    ));

    Ok(QueryShape {
        name: q.name,
        module_path: module_path(&q.source_file),
        source_file: q.source_file,
        original_sql: q.original_sql,
        normalized_sql: normalized.sql,
        cardinality: q.cardinality,
        params,
        columns: query_analysis.columns,
        dependencies: query_analysis.dependencies,
        fingerprint: fp,
    })
}

#[cfg(test)]
fn find_named_params(sql: &str) -> Vec<String> {
    normalize_named_params(sql, "?").param_names
}

fn module_path(file: &std::path::Path) -> Vec<String> {
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("queries");
    vec![to_snake_case(stem)]
}

#[derive(Debug, Clone)]
struct SchemaCatalog {
    tables: BTreeMap<String, TableSchema>,
    fingerprint: Fingerprint,
}

impl SchemaCatalog {
    fn load(config: &Config) -> Result<Self> {
        #[cfg(feature = "libsql-runtime")]
        if let Some(live) = Self::load_live_or_fallback(config)? {
            return Ok(live);
        }

        Self::load_schema_files(config)
    }

    fn load_schema_files(config: &Config) -> Result<Self> {
        let mut schema_sql = String::new();
        let mut fingerprint_text = String::new();
        for path in &config.schema.files {
            let contents = std::fs::read_to_string(path).map_err(|err| {
                Error::Config(format!("failed to read schema {}: {err}", path.display()))
            })?;
            fingerprint_text.push_str(&format!("{}\n", path.display()));
            fingerprint_text.push_str(&contents);
            fingerprint_text.push('\n');
            schema_sql.push_str(&contents);
            schema_sql.push('\n');
        }

        Ok(Self {
            tables: parse_schema_catalog_with_config(&schema_sql, &config.type_mapping)?,
            fingerprint: Fingerprint::from_text(&fingerprint_text),
        })
    }

    #[cfg(feature = "libsql-runtime")]
    fn load_live_or_fallback(config: &Config) -> Result<Option<Self>> {
        let catalog = if let Some(path) = local_libsql_path(&config.database.url) {
            load_live_catalog(path.to_string(), &config.type_mapping)
        } else if is_remote_libsql_url(&config.database.url) {
            #[cfg(feature = "libsql-remote")]
            {
                let Some(auth_token) = libsql_auth_token(config)? else {
                    if config.schema.files.is_empty() {
                        return Err(Error::Config(format!(
                            "remote libSQL catalog introspection for `{}` requires [database].auth_token or [database].auth_token_env; provide [schema].files for offline inference if remote introspection is not desired",
                            config.database.url
                        )));
                    }
                    return Ok(None);
                };
                load_remote_live_catalog(
                    config.database.url.clone(),
                    auth_token,
                    &config.type_mapping,
                )
            }

            #[cfg(not(feature = "libsql-remote"))]
            {
                if config.schema.files.is_empty() {
                    return Err(Error::Unsupported(format!(
                        "remote libSQL catalog introspection for `{}` requires enabling the QueryForge `libsql-remote` feature; provide [schema].files for offline inference if remote introspection is not desired",
                        config.database.url
                    )));
                }
                return Ok(None);
            }
        } else {
            return Ok(None);
        };

        match catalog {
            Ok(catalog) if !catalog.tables.is_empty() || config.schema.files.is_empty() => {
                Ok(Some(catalog))
            }
            Ok(_) => Ok(None),
            Err(err) if config.schema.files.is_empty() => Err(err),
            Err(_) => Ok(None),
        }
    }
}

#[derive(Debug, Clone)]
struct TableSchema {
    name: String,
    columns: BTreeMap<String, ColumnSchema>,
    #[cfg(feature = "libsql-runtime")]
    indexes: Vec<IndexSchema>,
    #[cfg(feature = "libsql-runtime")]
    foreign_keys: Vec<ForeignKeySchema>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ColumnSchema {
    name: String,
    declared_type: String,
    rust_type: RustType,
    nullable: Nullability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "libsql-runtime")]
struct IndexSchema {
    name: String,
    unique: bool,
    columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "libsql-runtime")]
struct ForeignKeySchema {
    id: i64,
    seq: i64,
    from_column: String,
    table: String,
    to_column: Option<String>,
    on_update: Option<String>,
    on_delete: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct QueryAnalysis {
    columns: Vec<QueryColumn>,
    param_types: BTreeMap<String, RustType>,
    param_db_types: BTreeMap<String, Option<String>>,
    dependencies: QueryDependencies,
}

#[cfg(test)]
fn parse_schema_catalog(schema: &str) -> Result<BTreeMap<String, TableSchema>> {
    parse_schema_catalog_with_config(schema, &crate::config::TypeMappingConfig::default())
}

fn parse_schema_catalog_with_config(
    schema: &str,
    type_mapping: &crate::config::TypeMappingConfig,
) -> Result<BTreeMap<String, TableSchema>> {
    let mut tables = BTreeMap::new();

    for statement in sql_ir::parse_statements(schema)? {
        let SqlStatement::CreateTable(create_table) = statement else {
            continue;
        };
        let table = TableSchema {
            name: create_table.table,
            columns: create_table
                .columns
                .into_iter()
                .map(|column| {
                    let nullable = match column.nullable {
                        ColumnNullability::NonNull => Nullability::NonNull,
                        ColumnNullability::Nullable => Nullability::Nullable,
                    };
                    let schema = ColumnSchema {
                        name: column.name,
                        rust_type: sqlite_declared_type_to_rust_with_config(
                            &column.declared_type,
                            type_mapping,
                        ),
                        declared_type: column.declared_type,
                        nullable,
                    };
                    (normalize_ident(&schema.name), schema)
                })
                .collect(),
            #[cfg(feature = "libsql-runtime")]
            indexes: Vec::new(),
            #[cfg(feature = "libsql-runtime")]
            foreign_keys: Vec::new(),
        };
        tables.insert(normalize_ident(&table.name), table);
    }

    Ok(tables)
}

#[cfg(feature = "libsql-runtime")]
fn load_live_catalog(
    path: String,
    type_mapping: &crate::config::TypeMappingConfig,
) -> Result<SchemaCatalog> {
    let type_mapping = type_mapping.clone();
    block_on_libsql(async move {
        let db = libsql::Builder::new_local(path.clone())
            .build()
            .await
            .map_err(map_libsql_error)?;
        let conn = db.connect().map_err(map_libsql_error)?;
        load_live_catalog_from_connection(
            &conn,
            format!("libsql-live-catalog-v0\nsource=local\npath={path}\n"),
            &type_mapping,
        )
        .await
    })
}

#[cfg(all(feature = "libsql-runtime", feature = "libsql-remote"))]
fn load_remote_live_catalog(
    url: String,
    auth_token: String,
    type_mapping: &crate::config::TypeMappingConfig,
) -> Result<SchemaCatalog> {
    let type_mapping = type_mapping.clone();
    block_on_libsql(async move {
        let db = libsql::Builder::new_remote(url.clone(), auth_token)
            .build()
            .await
            .map_err(map_libsql_error)?;
        let conn = db.connect().map_err(map_libsql_error)?;
        load_live_catalog_from_connection(
            &conn,
            format!("libsql-live-catalog-v0\nsource=remote\nurl={url}\n"),
            &type_mapping,
        )
        .await
    })
}

#[cfg(feature = "libsql-runtime")]
async fn load_live_catalog_from_connection(
    conn: &libsql::Connection,
    mut fingerprint_text: String,
    type_mapping: &crate::config::TypeMappingConfig,
) -> Result<SchemaCatalog> {
    let mut table_rows = libsql::Connection::query(
        conn,
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        (),
    )
    .await
    .map_err(map_libsql_error)?;

    let mut table_names = Vec::new();
    while let Some(row) = table_rows.next().await.map_err(map_libsql_error)? {
        let value = row.get_value(0).map_err(map_libsql_error)?;
        if let Some(name) = libsql_value_to_string(value) {
            table_names.push(name);
        }
    }

    let mut tables = BTreeMap::new();
    for table_name in table_names {
        let table = load_live_table_schema(conn, &table_name, type_mapping).await?;
        fingerprint_text.push_str(&format!("table={}\n", table.name));
        for column in table.columns.values() {
            fingerprint_text.push_str(&format!(
                "column={}:{}:{:?}\n",
                column.name, column.declared_type, column.nullable
            ));
        }
        for index in &table.indexes {
            fingerprint_text.push_str(&format!(
                "index={}:{}:{}\n",
                index.name,
                index.unique,
                index.columns.join(",")
            ));
        }
        for foreign_key in &table.foreign_keys {
            fingerprint_text.push_str(&format!(
                "foreign-key={}:{}:{}:{}:{:?}:{:?}:{:?}\n",
                foreign_key.id,
                foreign_key.seq,
                foreign_key.from_column,
                foreign_key.table,
                foreign_key.to_column,
                foreign_key.on_update,
                foreign_key.on_delete
            ));
        }
        tables.insert(normalize_ident(&table.name), table);
    }

    Ok(SchemaCatalog {
        tables,
        fingerprint: Fingerprint::from_text(&fingerprint_text),
    })
}

#[cfg(feature = "libsql-runtime")]
async fn load_live_table_schema(
    conn: &libsql::Connection,
    table_name: &str,
    type_mapping: &crate::config::TypeMappingConfig,
) -> Result<TableSchema> {
    let sql = format!("PRAGMA table_xinfo({})", quote_sqlite_ident(table_name));
    let mut rows = libsql::Connection::query(conn, &sql, ())
        .await
        .map_err(map_libsql_error)?;
    let mut columns = BTreeMap::new();

    while let Some(row) = rows.next().await.map_err(map_libsql_error)? {
        let name = row
            .get_value(1)
            .map_err(map_libsql_error)
            .ok()
            .and_then(libsql_value_to_string)
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }

        let declared_type = row
            .get_value(2)
            .map_err(map_libsql_error)
            .ok()
            .and_then(libsql_value_to_string)
            .unwrap_or_default();
        let notnull = row
            .get_value(3)
            .map_err(map_libsql_error)
            .ok()
            .and_then(libsql_value_to_i64)
            .unwrap_or(0);
        let pk = row
            .get_value(5)
            .map_err(map_libsql_error)
            .ok()
            .and_then(libsql_value_to_i64)
            .unwrap_or(0);
        let hidden = row
            .get_value(6)
            .map_err(map_libsql_error)
            .ok()
            .and_then(libsql_value_to_i64)
            .unwrap_or(0);
        if hidden == 1 {
            continue;
        }

        let nullable = if notnull != 0 || pk != 0 {
            Nullability::NonNull
        } else {
            Nullability::Nullable
        };
        let column = ColumnSchema {
            name,
            rust_type: sqlite_declared_type_to_rust_with_config(&declared_type, type_mapping),
            declared_type,
            nullable,
        };
        columns.insert(normalize_ident(&column.name), column);
    }

    Ok(TableSchema {
        name: table_name.to_string(),
        columns,
        indexes: load_live_table_indexes(conn, table_name).await?,
        foreign_keys: load_live_table_foreign_keys(conn, table_name).await?,
    })
}

#[cfg(feature = "libsql-runtime")]
async fn load_live_table_indexes(
    conn: &libsql::Connection,
    table_name: &str,
) -> Result<Vec<IndexSchema>> {
    let sql = format!("PRAGMA index_list({})", quote_sqlite_ident(table_name));
    let mut rows = libsql::Connection::query(conn, &sql, ())
        .await
        .map_err(map_libsql_error)?;
    let mut indexes = Vec::new();

    while let Some(row) = rows.next().await.map_err(map_libsql_error)? {
        let name = row
            .get_value(1)
            .map_err(map_libsql_error)
            .ok()
            .and_then(libsql_value_to_string)
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let unique = row
            .get_value(2)
            .map_err(map_libsql_error)
            .ok()
            .and_then(libsql_value_to_i64)
            .unwrap_or(0)
            != 0;
        indexes.push(IndexSchema {
            columns: load_live_index_columns(conn, &name).await?,
            name,
            unique,
        });
    }

    indexes.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(indexes)
}

#[cfg(feature = "libsql-runtime")]
async fn load_live_index_columns(
    conn: &libsql::Connection,
    index_name: &str,
) -> Result<Vec<String>> {
    let sql = format!("PRAGMA index_info({})", quote_sqlite_ident(index_name));
    let mut rows = libsql::Connection::query(conn, &sql, ())
        .await
        .map_err(map_libsql_error)?;
    let mut columns = Vec::new();

    while let Some(row) = rows.next().await.map_err(map_libsql_error)? {
        if let Some(name) = row
            .get_value(2)
            .map_err(map_libsql_error)
            .ok()
            .and_then(libsql_value_to_string)
        {
            columns.push(name);
        }
    }

    Ok(columns)
}

#[cfg(feature = "libsql-runtime")]
async fn load_live_table_foreign_keys(
    conn: &libsql::Connection,
    table_name: &str,
) -> Result<Vec<ForeignKeySchema>> {
    let sql = format!(
        "PRAGMA foreign_key_list({})",
        quote_sqlite_ident(table_name)
    );
    let mut rows = libsql::Connection::query(conn, &sql, ())
        .await
        .map_err(map_libsql_error)?;
    let mut foreign_keys = Vec::new();

    while let Some(row) = rows.next().await.map_err(map_libsql_error)? {
        foreign_keys.push(ForeignKeySchema {
            id: row
                .get_value(0)
                .map_err(map_libsql_error)
                .ok()
                .and_then(libsql_value_to_i64)
                .unwrap_or(0),
            seq: row
                .get_value(1)
                .map_err(map_libsql_error)
                .ok()
                .and_then(libsql_value_to_i64)
                .unwrap_or(0),
            table: row
                .get_value(2)
                .map_err(map_libsql_error)
                .ok()
                .and_then(libsql_value_to_string)
                .unwrap_or_default(),
            from_column: row
                .get_value(3)
                .map_err(map_libsql_error)
                .ok()
                .and_then(libsql_value_to_string)
                .unwrap_or_default(),
            to_column: row
                .get_value(4)
                .map_err(map_libsql_error)
                .ok()
                .and_then(libsql_value_to_string),
            on_update: row
                .get_value(5)
                .map_err(map_libsql_error)
                .ok()
                .and_then(libsql_value_to_string),
            on_delete: row
                .get_value(6)
                .map_err(map_libsql_error)
                .ok()
                .and_then(libsql_value_to_string),
        });
    }

    foreign_keys.sort_by(|left, right| (left.id, left.seq).cmp(&(right.id, right.seq)));
    Ok(foreign_keys)
}

#[cfg(feature = "libsql-runtime")]
fn block_on_libsql<F, T>(future: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>> + Send + 'static,
    T: Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .map_err(|err| Error::Backend(format!("failed to create tokio runtime: {err}")))?
                .block_on(future)
        })
        .join()
        .map_err(|_| Error::Backend("libSQL catalog thread panicked".to_string()))?
    } else {
        tokio::runtime::Runtime::new()
            .map_err(|err| Error::Backend(format!("failed to create tokio runtime: {err}")))?
            .block_on(future)
    }
}

#[cfg(feature = "libsql-runtime")]
fn local_libsql_path(url: &str) -> Option<&str> {
    if url == ":memory:" {
        return Some(url);
    }
    if let Some(path) = url.strip_prefix("file:") {
        return (!path.is_empty()).then_some(path);
    }
    if url.contains("://") || url.starts_with("libsql:") {
        return None;
    }
    (!url.is_empty()).then_some(url)
}

#[cfg(feature = "libsql-runtime")]
fn is_remote_libsql_url(url: &str) -> bool {
    url.contains("://") || url.starts_with("libsql:")
}

#[cfg(all(feature = "libsql-runtime", feature = "libsql-remote"))]
fn libsql_auth_token(config: &Config) -> Result<Option<String>> {
    if let Some(token) = config
        .database
        .auth_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        return Ok(Some(token.to_string()));
    }

    let Some(env_name) = config
        .database
        .auth_token_env
        .as_deref()
        .map(str::trim)
        .filter(|env_name| !env_name.is_empty())
    else {
        return Ok(None);
    };

    std::env::var(env_name).map(Some).map_err(|err| {
        Error::Config(format!(
            "failed to read libSQL auth token from environment variable `{env_name}`: {err}"
        ))
    })
}

#[cfg(feature = "libsql-runtime")]
fn quote_sqlite_ident(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

#[cfg(feature = "libsql-runtime")]
fn libsql_value_to_string(value: libsql::Value) -> Option<String> {
    match value {
        libsql::Value::Text(value) => Some(value),
        libsql::Value::Integer(value) => Some(value.to_string()),
        libsql::Value::Real(value) => Some(value.to_string()),
        libsql::Value::Null | libsql::Value::Blob(_) => None,
    }
}

#[cfg(feature = "libsql-runtime")]
fn libsql_value_to_i64(value: libsql::Value) -> Option<i64> {
    match value {
        libsql::Value::Integer(value) => Some(value),
        libsql::Value::Text(value) => value.parse().ok(),
        _ => None,
    }
}

#[cfg(feature = "libsql-runtime")]
fn map_libsql_error(error: libsql::Error) -> Error {
    Error::Backend(format!("libSQL error: {error}"))
}

fn analyze_query(
    sql: &str,
    catalog: &SchemaCatalog,
    type_mapping: &TypeMappingConfig,
) -> Result<QueryAnalysis> {
    if sql_ir::parse_select(sql).is_some() {
        return analyze_select(sql, catalog, type_mapping);
    }

    analyze_mutation(sql, catalog)
}

fn analyze_select(
    sql: &str,
    catalog: &SchemaCatalog,
    type_mapping: &TypeMappingConfig,
) -> Result<QueryAnalysis> {
    let Some(select) = sql_ir::parse_select(sql) else {
        return Ok(QueryAnalysis::default());
    };

    let catalog = catalog_with_query_relations(catalog, &select, type_mapping)?;
    let resolved_tables = resolve_tables(&select, &catalog);
    if resolved_tables.is_empty() && !select.table_refs.is_empty() {
        return Ok(QueryAnalysis {
            dependencies: QueryDependencies {
                tables: select
                    .table_refs
                    .into_iter()
                    .map(|table_ref| table_ref.name)
                    .collect(),
                functions: Vec::new(),
            },
            ..QueryAnalysis::default()
        });
    }

    let mut analysis = QueryAnalysis {
        dependencies: QueryDependencies {
            tables: resolved_tables
                .iter()
                .map(|resolved| resolved.table.name.clone())
                .collect(),
            functions: Vec::new(),
        },
        ..QueryAnalysis::default()
    };
    let (nested_param_types, nested_param_db_types) =
        query_relation_param_types(&catalog, &select, type_mapping)?;
    analysis.param_types.extend(nested_param_types);
    analysis.param_db_types.extend(nested_param_db_types);
    merge_compound_analysis(&catalog, &select, &mut analysis, type_mapping)?;

    for projection in select.projections {
        infer_projection(&projection, &resolved_tables, &mut analysis, type_mapping);
    }
    infer_param_types(
        sql,
        &select.equality_params,
        &resolved_tables,
        &mut analysis,
    );

    Ok(analysis)
}

fn analyze_mutation(sql: &str, catalog: &SchemaCatalog) -> Result<QueryAnalysis> {
    let mut analysis = QueryAnalysis::default();
    if let Some(mutation) = sql_ir::parse_mutation(sql) {
        analysis.dependencies.tables.push_unique(&mutation.table);
        let Some(table) = catalog.tables.get(&normalize_ident(&mutation.table)) else {
            return Ok(analysis);
        };

        infer_param_types_by_name(sql, table, &mut analysis);
        infer_mutation_column_param_types(&mutation.column_params, table, &mut analysis);
        infer_mutation_equality_param_types(&mutation.equality_params, table, &mut analysis);
        return Ok(analysis);
    }

    Ok(analysis)
}

fn infer_mutation_column_param_types(
    column_params: &[sql_ir::MutationColumnParam],
    table: &TableSchema,
    analysis: &mut QueryAnalysis,
) {
    for column_param in column_params {
        if let Some(column) = table.columns.get(&normalize_ident(&column_param.column)) {
            insert_param_type(&column_param.param, column, analysis);
        }
    }
}

fn infer_mutation_equality_param_types(
    equality_params: &[sql_ir::EqualityParam],
    table: &TableSchema,
    analysis: &mut QueryAnalysis,
) {
    for equality in equality_params {
        if let Some(column) = table.columns.get(&normalize_ident(&equality.column)) {
            insert_param_type(&equality.param, column, analysis);
        }
    }
}

fn infer_param_types_by_name(sql: &str, table: &TableSchema, analysis: &mut QueryAnalysis) {
    for param in normalize_named_params(sql, "?").param_names {
        if let Some(column) = table.columns.get(&normalize_ident(&param)) {
            insert_param_type(&param, column, analysis);
        }
    }
}

fn insert_param_type(param: &str, column: &ColumnSchema, analysis: &mut QueryAnalysis) {
    analysis
        .param_types
        .insert(param.to_string(), column.rust_type.clone());
    analysis.param_db_types.insert(
        param.to_string(),
        Some(format!("sqlite:{}", column.declared_type)),
    );
}

fn find_matching_simple_paren(input: &str, open: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    let mut idx = open;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' => skip_single_quoted_bytes(bytes, &mut idx),
            b'"' => skip_double_quoted_bytes(bytes, &mut idx),
            b'(' => {
                depth += 1;
                idx += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(idx);
                }
                idx += 1;
            }
            _ => idx += 1,
        }
    }
    None
}

fn skip_single_quoted_bytes(bytes: &[u8], idx: &mut usize) {
    *idx += 1;
    while *idx < bytes.len() {
        if bytes[*idx] == b'\'' {
            *idx += 1;
            if bytes.get(*idx) == Some(&b'\'') {
                *idx += 1;
                continue;
            }
            break;
        }
        *idx += 1;
    }
}

fn skip_double_quoted_bytes(bytes: &[u8], idx: &mut usize) {
    *idx += 1;
    while *idx < bytes.len() {
        if bytes[*idx] == b'"' {
            *idx += 1;
            if bytes.get(*idx) == Some(&b'"') {
                *idx += 1;
                continue;
            }
            break;
        }
        *idx += 1;
    }
}

fn merge_compound_analysis(
    catalog: &SchemaCatalog,
    select: &sql_ir::SelectStatement,
    analysis: &mut QueryAnalysis,
    type_mapping: &TypeMappingConfig,
) -> Result<()> {
    for compound in &select.compound {
        let branch = analyze_select(&compound.query, catalog, type_mapping)?;
        for table in branch.dependencies.tables {
            analysis.dependencies.tables.push_unique(&table);
        }
        for function in branch.dependencies.functions {
            analysis.dependencies.functions.push_unique(&function);
        }
        analysis.param_types.extend(branch.param_types);
        analysis.param_db_types.extend(branch.param_db_types);
    }

    Ok(())
}

fn query_relation_param_types(
    catalog: &SchemaCatalog,
    select: &sql_ir::SelectStatement,
    type_mapping: &TypeMappingConfig,
) -> Result<(BTreeMap<String, RustType>, BTreeMap<String, Option<String>>)> {
    let mut param_types = BTreeMap::new();
    let mut param_db_types = BTreeMap::new();

    for cte in &select.ctes {
        let analysis = analyze_select(&cte.query, catalog, type_mapping)?;
        param_types.extend(analysis.param_types);
        param_db_types.extend(analysis.param_db_types);
    }

    for table_ref in &select.table_refs {
        let Some(query) = &table_ref.derived_query else {
            continue;
        };
        let analysis = analyze_select(query, catalog, type_mapping)?;
        param_types.extend(analysis.param_types);
        param_db_types.extend(analysis.param_db_types);
    }

    Ok((param_types, param_db_types))
}

fn catalog_with_query_relations(
    catalog: &SchemaCatalog,
    select: &sql_ir::SelectStatement,
    type_mapping: &TypeMappingConfig,
) -> Result<SchemaCatalog> {
    let mut catalog = catalog.clone();

    for cte in &select.ctes {
        if let Some(table) =
            table_schema_from_select(&cte.name, &cte.query, &cte.columns, &catalog, type_mapping)?
        {
            catalog.tables.insert(normalize_ident(&cte.name), table);
        }
    }

    for table_ref in &select.table_refs {
        let Some(query) = &table_ref.derived_query else {
            continue;
        };
        if let Some(table) =
            table_schema_from_select(&table_ref.name, query, &[], &catalog, type_mapping)?
        {
            catalog
                .tables
                .insert(normalize_ident(&table_ref.name), table);
        }
    }

    Ok(catalog)
}

fn table_schema_from_select(
    name: &str,
    sql: &str,
    declared_names: &[String],
    catalog: &SchemaCatalog,
    type_mapping: &TypeMappingConfig,
) -> Result<Option<TableSchema>> {
    let analysis = analyze_select(sql, catalog, type_mapping)?;
    if analysis.columns.is_empty() {
        return Ok(None);
    }

    let columns = analysis
        .columns
        .into_iter()
        .enumerate()
        .map(|column| {
            let (idx, column) = column;
            let declared_type = column
                .db_type
                .as_deref()
                .and_then(|db_type| db_type.strip_prefix("sqlite:"))
                .unwrap_or("TEXT")
                .to_string();
            let name = declared_names
                .get(idx)
                .cloned()
                .unwrap_or_else(|| column.name);
            let schema = ColumnSchema {
                name,
                rust_type: column.rust_type,
                declared_type,
                nullable: column.nullable,
            };
            (normalize_ident(&schema.name), schema)
        })
        .collect();

    Ok(Some(TableSchema {
        name: name.to_string(),
        columns,
        #[cfg(feature = "libsql-runtime")]
        indexes: Vec::new(),
        #[cfg(feature = "libsql-runtime")]
        foreign_keys: Vec::new(),
    }))
}

#[derive(Debug)]
struct ResolvedTable<'a> {
    qualifier: String,
    alias: Option<String>,
    table: &'a TableSchema,
}

fn resolve_tables<'a>(
    select: &sql_ir::SelectStatement,
    catalog: &'a SchemaCatalog,
) -> Vec<ResolvedTable<'a>> {
    select
        .table_refs
        .iter()
        .filter_map(|table_ref| {
            let table = catalog.tables.get(&normalize_ident(&table_ref.name))?;
            Some(ResolvedTable {
                qualifier: normalize_ident(&table_ref.name),
                alias: table_ref.alias.as_ref().map(|alias| normalize_ident(alias)),
                table,
            })
        })
        .collect()
}

fn infer_projection(
    projection: &SelectProjection,
    tables: &[ResolvedTable<'_>],
    analysis: &mut QueryAnalysis,
    type_mapping: &TypeMappingConfig,
) {
    let expr = projection.expr.trim();
    if expr == "*" {
        for table in tables {
            for column in table.table.columns.values() {
                analysis.columns.push(query_column_from_schema(column));
            }
        }
        return;
    }

    if let Some((qualifier, star)) = expr.rsplit_once('.') {
        if star == "*" {
            if let Some(table) = resolve_table_by_qualifier(tables, qualifier) {
                for column in table.table.columns.values() {
                    analysis.columns.push(query_column_from_schema(column));
                }
            }
            return;
        }
    }

    let alias = projection.alias.as_deref();
    if let Some(column) = resolve_column(tables, expr) {
        let mut query_column = query_column_from_schema(column);
        if let Some(alias) = alias {
            query_column.name = alias.to_string();
            query_column.rust_name = to_snake_case(alias);
        }
        analysis.columns.push(query_column);
        return;
    }

    if let Some(column) = infer_builtin_expression(expr, alias, tables, analysis, type_mapping) {
        analysis.columns.push(column);
    } else if let Some(alias) = alias {
        analysis.columns.push(QueryColumn {
            name: alias.to_string(),
            rust_name: to_snake_case(alias),
            db_type: None,
            rust_type: RustType::string(),
            nullable: Nullability::Unknown,
            source: TypeSource::ExpressionInference,
            confidence: InferenceConfidence::Weak,
        });
    }
}

fn infer_builtin_expression(
    expr: &str,
    alias: Option<&str>,
    tables: &[ResolvedTable<'_>],
    analysis: &mut QueryAnalysis,
    type_mapping: &TypeMappingConfig,
) -> Option<QueryColumn> {
    let lower = expr.trim().to_ascii_lowercase();
    if lower == "count(*)" {
        analysis.dependencies.functions.push_unique("count");
        return Some(QueryColumn {
            name: alias.unwrap_or("count").to_string(),
            rust_name: to_snake_case(alias.unwrap_or("count")),
            db_type: Some("sqlite:INTEGER".to_string()),
            rust_type: RustType::new("i64"),
            nullable: Nullability::NonNull,
            source: TypeSource::BuiltinFunctionRule,
            confidence: InferenceConfidence::Strong,
        });
    }

    if matches!(
        lower.as_str(),
        "uuid()" | "uuid4()" | "uuid7()" | "gen_random_uuid()"
    ) {
        let function = lower
            .strip_suffix("()")
            .unwrap_or(lower.as_str())
            .to_string();
        analysis.dependencies.functions.push_unique(&function);
        return Some(QueryColumn {
            name: alias.unwrap_or(expr).to_string(),
            rust_name: to_snake_case(alias.unwrap_or(expr)),
            db_type: Some("sqlite:UUID".to_string()),
            rust_type: uuid_rust_type(type_mapping),
            nullable: Nullability::NonNull,
            source: TypeSource::BuiltinFunctionRule,
            confidence: InferenceConfidence::Strong,
        });
    }

    if let Some(args) = function_args(expr, "uuid_str") {
        analysis.dependencies.functions.push_unique("uuid_str");
        return Some(QueryColumn {
            name: alias.unwrap_or(expr).to_string(),
            rust_name: to_snake_case(alias.unwrap_or(expr)),
            db_type: Some("sqlite:UUID".to_string()),
            rust_type: uuid_rust_type(type_mapping),
            nullable: expression_nullability(&args, tables),
            source: TypeSource::BuiltinFunctionRule,
            confidence: InferenceConfidence::Strong,
        });
    }

    if let Some(args) = function_args(expr, "uuid_blob") {
        analysis.dependencies.functions.push_unique("uuid_blob");
        return Some(QueryColumn {
            name: alias.unwrap_or(expr).to_string(),
            rust_name: to_snake_case(alias.unwrap_or(expr)),
            db_type: Some("sqlite:BLOB".to_string()),
            rust_type: uuid_rust_type(type_mapping),
            nullable: expression_nullability(&args, tables),
            source: TypeSource::BuiltinFunctionRule,
            confidence: InferenceConfidence::Strong,
        });
    }

    if function_args(expr, "uuid7_timestamp_ms").is_some() {
        analysis
            .dependencies
            .functions
            .push_unique("uuid7_timestamp_ms");
        return Some(QueryColumn {
            name: alias.unwrap_or(expr).to_string(),
            rust_name: to_snake_case(alias.unwrap_or(expr)),
            db_type: Some("sqlite:INTEGER".to_string()),
            rust_type: RustType::new("i64"),
            nullable: Nullability::Nullable,
            source: TypeSource::BuiltinFunctionRule,
            confidence: InferenceConfidence::Strong,
        });
    }

    for coalesce_function in ["coalesce", "ifnull"] {
        let Some(args) = function_args(expr, coalesce_function) else {
            continue;
        };
        analysis
            .dependencies
            .functions
            .push_unique(coalesce_function);
        let (rust_type, db_type) = expression_type(&args, tables)
            .unwrap_or_else(|| (RustType::string(), Some("sqlite:TEXT".to_string())));
        return Some(QueryColumn {
            name: alias.unwrap_or(expr).to_string(),
            rust_name: to_snake_case(alias.unwrap_or(expr)),
            db_type,
            rust_type,
            nullable: coalesce_expression_nullability(&args, tables),
            source: TypeSource::BuiltinFunctionRule,
            confidence: InferenceConfidence::Strong,
        });
    }

    if let Some(args) = function_args(expr, "length") {
        analysis.dependencies.functions.push_unique("length");
        return Some(QueryColumn {
            name: alias.unwrap_or(expr).to_string(),
            rust_name: to_snake_case(alias.unwrap_or(expr)),
            db_type: Some("sqlite:INTEGER".to_string()),
            rust_type: RustType::new("i64"),
            nullable: expression_nullability(&args, tables),
            source: TypeSource::BuiltinFunctionRule,
            confidence: InferenceConfidence::Strong,
        });
    }

    let concat_parts = split_top_level_operator(expr, "||");
    if concat_parts.len() > 1 {
        return Some(QueryColumn {
            name: alias.unwrap_or(expr).to_string(),
            rust_name: to_snake_case(alias.unwrap_or(expr)),
            db_type: Some("sqlite:TEXT".to_string()),
            rust_type: RustType::string(),
            nullable: expression_nullability(&concat_parts, tables),
            source: TypeSource::BuiltinFunctionRule,
            confidence: InferenceConfidence::Strong,
        });
    }

    for function in ["lower", "upper"] {
        let prefix = format!("{function}(");
        if lower.starts_with(&prefix) && lower.ends_with(')') {
            let inner = expr[prefix.len()..expr.len() - 1].trim();
            let column = resolve_column(tables, inner)?;
            analysis.dependencies.functions.push_unique(function);
            return Some(QueryColumn {
                name: alias.unwrap_or(expr).to_string(),
                rust_name: to_snake_case(alias.unwrap_or(expr)),
                db_type: Some(format!("sqlite:{}", column.declared_type)),
                rust_type: RustType::string(),
                nullable: column.nullable.clone(),
                source: TypeSource::BuiltinFunctionRule,
                confidence: InferenceConfidence::Strong,
            });
        }
    }

    None
}

fn uuid_rust_type(type_mapping: &TypeMappingConfig) -> RustType {
    if type_mapping.uuid == UuidTypeMapping::Uuid {
        RustType::new("uuid::Uuid")
    } else {
        RustType::string()
    }
}

fn function_args<'a>(expr: &'a str, expected_name: &str) -> Option<Vec<&'a str>> {
    let expr = expr.trim();
    let open = expr.find('(')?;
    let name = expr[..open].trim();
    if !name.eq_ignore_ascii_case(expected_name) {
        return None;
    }
    let close = find_matching_simple_paren(expr, open)?;
    if !expr[close + 1..].trim().is_empty() {
        return None;
    }
    Some(sql_ir::split_comma_separated(&expr[open + 1..close]))
}

fn expression_type(
    exprs: &[&str],
    tables: &[ResolvedTable<'_>],
) -> Option<(RustType, Option<String>)> {
    exprs.iter().find_map(|expr| {
        resolve_column(tables, expr).map(|column| {
            (
                column.rust_type.clone(),
                Some(format!("sqlite:{}", column.declared_type)),
            )
        })
    })
}

fn expression_nullability(exprs: &[&str], tables: &[ResolvedTable<'_>]) -> Nullability {
    let mut saw_nullable = false;
    for expr in exprs {
        match single_expression_nullability(expr, tables) {
            Nullability::NonNull => {}
            Nullability::Nullable => saw_nullable = true,
            Nullability::Unknown => return Nullability::Unknown,
        }
    }
    if saw_nullable {
        Nullability::Nullable
    } else {
        Nullability::NonNull
    }
}

fn coalesce_expression_nullability(exprs: &[&str], tables: &[ResolvedTable<'_>]) -> Nullability {
    let mut saw_unknown = false;
    for expr in exprs {
        match single_expression_nullability(expr, tables) {
            Nullability::NonNull => return Nullability::NonNull,
            Nullability::Unknown => saw_unknown = true,
            Nullability::Nullable => {}
        }
    }
    if saw_unknown {
        Nullability::Unknown
    } else {
        Nullability::Nullable
    }
}

fn single_expression_nullability(expr: &str, tables: &[ResolvedTable<'_>]) -> Nullability {
    let expr = expr.trim();
    if expr.eq_ignore_ascii_case("null") {
        return Nullability::Nullable;
    }
    if is_non_null_literal(expr) {
        return Nullability::NonNull;
    }
    resolve_column(tables, expr)
        .map(|column| column.nullable.clone())
        .unwrap_or(Nullability::Unknown)
}

fn is_non_null_literal(expr: &str) -> bool {
    let trimmed = expr.trim();
    if trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2 {
        return true;
    }
    trimmed.parse::<i64>().is_ok() || trimmed.parse::<f64>().is_ok()
}

fn split_top_level_operator<'a>(expr: &'a str, operator: &str) -> Vec<&'a str> {
    let bytes = expr.as_bytes();
    let op = operator.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut idx = 0;

    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' => {
                skip_single_quoted_bytes(bytes, &mut idx);
                continue;
            }
            b'"' => {
                skip_double_quoted_bytes(bytes, &mut idx);
                continue;
            }
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            _ => {}
        }

        if depth == 0
            && bytes
                .get(idx..idx + op.len())
                .is_some_and(|candidate| candidate == op)
        {
            parts.push(expr[start..idx].trim());
            idx += op.len();
            start = idx;
            continue;
        }

        idx += 1;
    }

    if start == 0 {
        vec![expr.trim()]
    } else {
        parts.push(expr[start..].trim());
        parts
    }
}

fn resolve_table_by_qualifier<'a>(
    tables: &'a [ResolvedTable<'a>],
    qualifier: &str,
) -> Option<&'a ResolvedTable<'a>> {
    let qualifier = normalize_ident(qualifier);
    tables.iter().find(|table| {
        table.qualifier == qualifier
            || table
                .alias
                .as_deref()
                .is_some_and(|alias| alias == qualifier)
    })
}

fn resolve_column<'a>(tables: &'a [ResolvedTable<'a>], expr: &str) -> Option<&'a ColumnSchema> {
    let (qualifier, column) = sql_ir::split_qualified_name(expr);
    let column = normalize_ident(&column);

    if let Some(qualifier) = qualifier {
        return resolve_table_by_qualifier(tables, &qualifier)?
            .table
            .columns
            .get(&column);
    }

    let mut matches = tables
        .iter()
        .filter_map(|table| table.table.columns.get(&column));
    let first = matches.next()?;
    if matches.next().is_none() {
        Some(first)
    } else {
        None
    }
}

trait PushUnique {
    fn push_unique(&mut self, value: &str);
}

impl PushUnique for Vec<String> {
    fn push_unique(&mut self, value: &str) {
        if !self.iter().any(|existing| existing == value) {
            self.push(value.to_string());
        }
    }
}

fn query_column_from_schema(column: &ColumnSchema) -> QueryColumn {
    QueryColumn {
        name: column.name.clone(),
        rust_name: to_snake_case(&column.name),
        db_type: Some(format!("sqlite:{}", column.declared_type)),
        rust_type: column.rust_type.clone(),
        nullable: column.nullable.clone(),
        source: TypeSource::SchemaCatalog,
        confidence: InferenceConfidence::Strong,
    }
}

fn infer_param_types(
    sql: &str,
    equality_params: &[sql_ir::EqualityParam],
    tables: &[ResolvedTable<'_>],
    analysis: &mut QueryAnalysis,
) {
    let params = normalize_named_params(sql, "?").param_names;
    for param in params {
        if let Some(column) = resolve_column(tables, &param) {
            analysis
                .param_types
                .insert(param.clone(), column.rust_type.clone());
            analysis
                .param_db_types
                .insert(param, Some(format!("sqlite:{}", column.declared_type)));
        }
    }

    for equality in equality_params {
        let expr = if let Some(qualifier) = &equality.qualifier {
            format!("{qualifier}.{}", equality.column)
        } else {
            equality.column.clone()
        };
        if let Some(column) = resolve_column(tables, &expr) {
            analysis
                .param_types
                .insert(equality.param.clone(), column.rust_type.clone());
            analysis.param_db_types.insert(
                equality.param.clone(),
                Some(format!("sqlite:{}", column.declared_type)),
            );
        }
    }
}

fn normalize_ident(input: &str) -> String {
    sql_ir::strip_identifier_quotes(input).to_ascii_lowercase()
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}
fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedNamedParams {
    sql: String,
    param_names: Vec<String>,
}

fn normalize_named_params(sql: &str, prefix: &str) -> NormalizedNamedParams {
    let mut out = String::with_capacity(sql.len());
    let mut names = Vec::<String>::new();
    let bytes = sql.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' => copy_single_quoted(sql, &mut idx, &mut out),
            b'"' => copy_double_quoted(sql, &mut idx, &mut out),
            b'-' if bytes.get(idx + 1).copied() == Some(b'-') => {
                copy_line_comment(sql, &mut idx, &mut out);
            }
            b'/' if bytes.get(idx + 1).copied() == Some(b'*') => {
                copy_block_comment(sql, &mut idx, &mut out);
            }
            b':' if bytes.get(idx + 1).copied().is_some_and(is_ident_start) => {
                let start = idx + 1;
                let mut end = start + 1;
                while end < bytes.len() && is_ident_continue(bytes[end]) {
                    end += 1;
                }

                let name = sql[start..end].to_string();
                let position = names
                    .iter()
                    .position(|existing| existing == &name)
                    .map(|position| position + 1)
                    .unwrap_or_else(|| {
                        names.push(name);
                        names.len()
                    });
                out.push_str(&positional_param(prefix, position));
                idx = end;
            }
            _ => {
                out.push(bytes[idx] as char);
                idx += 1;
            }
        }
    }

    NormalizedNamedParams {
        sql: out,
        param_names: names,
    }
}

fn positional_param(prefix: &str, position: usize) -> String {
    if prefix == "$" {
        format!("${position}")
    } else {
        format!("?{position}")
    }
}

fn copy_single_quoted(sql: &str, idx: &mut usize, out: &mut String) {
    let bytes = sql.as_bytes();
    out.push('\'');
    *idx += 1;

    while *idx < bytes.len() {
        out.push(bytes[*idx] as char);
        if bytes[*idx] == b'\'' {
            *idx += 1;
            if bytes.get(*idx).copied() == Some(b'\'') {
                out.push('\'');
                *idx += 1;
                continue;
            }
            break;
        }
        *idx += 1;
    }
}

fn copy_double_quoted(sql: &str, idx: &mut usize, out: &mut String) {
    let bytes = sql.as_bytes();
    out.push('"');
    *idx += 1;

    while *idx < bytes.len() {
        out.push(bytes[*idx] as char);
        if bytes[*idx] == b'"' {
            *idx += 1;
            if bytes.get(*idx).copied() == Some(b'"') {
                out.push('"');
                *idx += 1;
                continue;
            }
            break;
        }
        *idx += 1;
    }
}

fn copy_line_comment(sql: &str, idx: &mut usize, out: &mut String) {
    let bytes = sql.as_bytes();
    while *idx < bytes.len() {
        out.push(bytes[*idx] as char);
        *idx += 1;
        if bytes[*idx - 1] == b'\n' {
            break;
        }
    }
}

fn copy_block_comment(sql: &str, idx: &mut usize, out: &mut String) {
    let bytes = sql.as_bytes();
    while *idx < bytes.len() {
        out.push(bytes[*idx] as char);
        *idx += 1;
        if *idx >= 2 && bytes[*idx - 2] == b'*' && bytes[*idx - 1] == b'/' {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        BuildConfig, CodegenConfig, DatabaseBackend, DatabaseConfig, ExecutionTarget,
        InferenceConfig, MigrationsConfig, SchemaConfig,
    };
    use crate::ir::Cardinality;
    use std::path::PathBuf;

    #[test]
    fn finds_named_params_once_in_order() {
        assert_eq!(
            find_named_params("WHERE id = :id OR parent_id = :id AND org_id = :org_id"),
            vec!["id".to_string(), "org_id".to_string()]
        );
    }

    #[test]
    fn normalizes_repeated_params() {
        assert_eq!(
            normalize_named_params("WHERE id = :id OR parent_id = :id", "?"),
            NormalizedNamedParams {
                sql: "WHERE id = ?1 OR parent_id = ?1".to_string(),
                param_names: vec!["id".to_string()],
            }
        );
    }

    #[test]
    fn ignores_params_in_strings_identifiers_and_comments() {
        assert_eq!(
            normalize_named_params(
                "SELECT ':literal', \":identifier\", :id -- :comment\n/* :block */",
                "?"
            )
            .sql,
            "SELECT ':literal', \":identifier\", ?1 -- :comment\n/* :block */"
        );
    }

    #[test]
    fn parses_create_table_schema_catalog() {
        let tables = parse_schema_catalog(
            "
            CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY,
                email TEXT NOT NULL,
                parent_id INTEGER,
                active BOOLEAN NOT NULL,
                body BLOB,
                score REAL,
                CONSTRAINT users_email_unique UNIQUE (email)
            );
            ",
        )
        .unwrap();
        let users = tables.get("users").unwrap();

        assert_eq!(users.columns["id"].rust_type.0, "i64");
        assert_eq!(users.columns["id"].nullable, Nullability::NonNull);
        assert_eq!(users.columns["email"].rust_type.0, "String");
        assert_eq!(users.columns["email"].nullable, Nullability::NonNull);
        assert_eq!(users.columns["parent_id"].nullable, Nullability::Nullable);
        assert_eq!(users.columns["active"].rust_type.0, "bool");
        assert_eq!(users.columns["body"].rust_type.0, "Vec<u8>");
        assert_eq!(users.columns["score"].rust_type.0, "f64");
    }

    #[test]
    fn infers_direct_columns_and_param_types_from_schema() {
        let catalog = catalog();
        let config = test_config(Vec::new());
        let query = ParsedQuery {
            name: "get_user".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql:
                "SELECT id, email, parent_id FROM users WHERE id = :user_id AND email = :email"
                    .to_string(),
            cardinality: Cardinality::One,
        };

        let migration_fingerprint = Fingerprint::from_text("migrations");
        let shaped = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            query,
        )
        .unwrap();

        assert_eq!(
            shaped.normalized_sql,
            "SELECT id, email, parent_id FROM users WHERE id = ?1 AND email = ?2"
        );
        assert_eq!(shaped.columns.len(), 3);
        assert_eq!(shaped.columns[0].name, "id");
        assert_eq!(shaped.columns[0].rust_type.0, "i64");
        assert_eq!(shaped.columns[0].nullable, Nullability::NonNull);
        assert_eq!(shaped.columns[2].name, "parent_id");
        assert_eq!(shaped.columns[2].nullable, Nullability::Nullable);
        assert_eq!(shaped.params.len(), 2);
        assert_eq!(shaped.params[0].name, "user_id");
        assert_eq!(shaped.params[0].rust_type.0, "i64");
        assert_eq!(shaped.params[0].db_type.as_deref(), Some("sqlite:INTEGER"));
        assert_eq!(shaped.params[1].name, "email");
        assert_eq!(shaped.params[1].rust_type.0, "String");
    }

    #[test]
    fn infers_mutation_param_types_from_target_table() {
        let catalog = catalog();
        let config = test_config(Vec::new());
        let migration_fingerprint = Fingerprint::from_text("migrations");

        let insert = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            ParsedQuery {
                name: "create_user".to_string(),
                source_file: PathBuf::from("queries/users.sql"),
                original_sql: "INSERT INTO users (email, org_id, active) VALUES (:email_address, :org_id, :active)".to_string(),
                cardinality: Cardinality::Exec,
            },
        )
        .unwrap();
        assert_eq!(
            insert.normalized_sql,
            "INSERT INTO users (email, org_id, active) VALUES (?1, ?2, ?3)"
        );
        assert_eq!(insert.params[0].name, "email_address");
        assert_eq!(insert.params[0].rust_type.0, "String");
        assert_eq!(insert.params[1].name, "org_id");
        assert_eq!(insert.params[1].rust_type.0, "i64");
        assert_eq!(insert.params[2].name, "active");
        assert_eq!(insert.params[2].rust_type.0, "bool");
        assert!(insert.columns.is_empty());

        let update = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            ParsedQuery {
                name: "rename_user".to_string(),
                source_file: PathBuf::from("queries/users.sql"),
                original_sql:
                    "UPDATE users SET email = :new_email, active = :is_active WHERE id = :user_id"
                        .to_string(),
                cardinality: Cardinality::Exec,
            },
        )
        .unwrap();
        assert_eq!(update.params[0].name, "new_email");
        assert_eq!(update.params[0].rust_type.0, "String");
        assert_eq!(update.params[1].name, "is_active");
        assert_eq!(update.params[1].rust_type.0, "bool");
        assert_eq!(update.params[2].name, "user_id");
        assert_eq!(update.params[2].rust_type.0, "i64");

        let delete = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            ParsedQuery {
                name: "delete_user".to_string(),
                source_file: PathBuf::from("queries/users.sql"),
                original_sql: "DELETE FROM users WHERE id = :user_id".to_string(),
                cardinality: Cardinality::Exec,
            },
        )
        .unwrap();
        assert_eq!(delete.params[0].name, "user_id");
        assert_eq!(delete.params[0].rust_type.0, "i64");
    }

    #[test]
    fn infers_star_and_builtin_expressions() {
        let catalog = catalog();
        let config = test_config(Vec::new());
        let query = ParsedQuery {
            name: "list_users".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql: "SELECT *, count(*) AS total, lower(email) AS lower_email, email || '' AS email_expr, coalesce(parent_id, 0) AS parent_fallback, ifnull(parent_id, 0) AS parent_ifnull, length(email) AS email_len, length(parent_id) AS parent_len FROM users".to_string(),
            cardinality: Cardinality::Many,
        };

        let migration_fingerprint = Fingerprint::from_text("migrations");
        let shaped = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            query,
        )
        .unwrap();

        assert!(shaped.columns.iter().any(|column| column.name == "id"));
        let total = shaped
            .columns
            .iter()
            .find(|column| column.name == "total")
            .unwrap();
        assert_eq!(total.rust_type.0, "i64");
        assert_eq!(total.nullable, Nullability::NonNull);
        assert_eq!(total.source, TypeSource::BuiltinFunctionRule);
        let lower_email = shaped
            .columns
            .iter()
            .find(|column| column.name == "lower_email")
            .unwrap();
        assert_eq!(lower_email.rust_type.0, "String");
        assert_eq!(lower_email.nullable, Nullability::NonNull);
        let email_expr = shaped
            .columns
            .iter()
            .find(|column| column.name == "email_expr")
            .unwrap();
        assert_eq!(email_expr.rust_type.0, "String");
        assert_eq!(email_expr.nullable, Nullability::NonNull);
        assert_eq!(email_expr.source, TypeSource::BuiltinFunctionRule);
        let parent_fallback = shaped
            .columns
            .iter()
            .find(|column| column.name == "parent_fallback")
            .unwrap();
        assert_eq!(parent_fallback.rust_type.0, "i64");
        assert_eq!(parent_fallback.nullable, Nullability::NonNull);
        assert_eq!(parent_fallback.source, TypeSource::BuiltinFunctionRule);
        let parent_ifnull = shaped
            .columns
            .iter()
            .find(|column| column.name == "parent_ifnull")
            .unwrap();
        assert_eq!(parent_ifnull.rust_type.0, "i64");
        assert_eq!(parent_ifnull.nullable, Nullability::NonNull);
        assert_eq!(parent_ifnull.source, TypeSource::BuiltinFunctionRule);
        let email_len = shaped
            .columns
            .iter()
            .find(|column| column.name == "email_len")
            .unwrap();
        assert_eq!(email_len.rust_type.0, "i64");
        assert_eq!(email_len.nullable, Nullability::NonNull);
        let parent_len = shaped
            .columns
            .iter()
            .find(|column| column.name == "parent_len")
            .unwrap();
        assert_eq!(parent_len.rust_type.0, "i64");
        assert_eq!(parent_len.nullable, Nullability::Nullable);
        assert_eq!(
            shaped.dependencies.functions,
            vec!["count", "lower", "coalesce", "ifnull", "length"]
        );
    }

    #[cfg(feature = "uuid-types")]
    #[test]
    fn infers_sqlite_uuid_extension_functions() {
        let mut config = test_config(Vec::new());
        config.type_mapping.uuid = UuidTypeMapping::Uuid;
        let tables = parse_schema_catalog_with_config(
            "CREATE TABLE users (id UUID PRIMARY KEY, parent_id UUID);",
            &config.type_mapping,
        )
        .unwrap();
        let catalog = SchemaCatalog {
            tables,
            fingerprint: Fingerprint::from_text("schema"),
        };
        let migration_fingerprint = Fingerprint::from_text("migrations");

        let generated = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            ParsedQuery {
                name: "new_uuid".to_string(),
                source_file: PathBuf::from("queries/uuid.sql"),
                original_sql: "SELECT uuid4() AS generated_id".to_string(),
                cardinality: Cardinality::One,
            },
        )
        .unwrap();
        assert_eq!(generated.columns[0].name, "generated_id");
        assert_eq!(generated.columns[0].rust_type.0, "uuid::Uuid");
        assert_eq!(generated.columns[0].db_type.as_deref(), Some("sqlite:UUID"));
        assert_eq!(generated.columns[0].nullable, Nullability::NonNull);
        assert_eq!(generated.dependencies.functions, vec!["uuid4"]);

        let converted = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            ParsedQuery {
                name: "convert_uuid".to_string(),
                source_file: PathBuf::from("queries/uuid.sql"),
                original_sql: "SELECT gen_random_uuid() AS random_id, uuid7() AS ordered_id, uuid_str(id) AS id_text, uuid_blob(id) AS id_blob, uuid7_timestamp_ms(id) AS id_timestamp_ms FROM users".to_string(),
                cardinality: Cardinality::Many,
            },
        )
        .unwrap();

        assert_eq!(converted.columns[0].rust_type.0, "uuid::Uuid");
        assert_eq!(converted.columns[1].rust_type.0, "uuid::Uuid");
        assert_eq!(converted.columns[2].rust_type.0, "uuid::Uuid");
        assert_eq!(converted.columns[3].rust_type.0, "uuid::Uuid");
        assert_eq!(converted.columns[3].db_type.as_deref(), Some("sqlite:BLOB"));
        assert_eq!(converted.columns[4].rust_type.0, "i64");
        assert_eq!(converted.columns[4].nullable, Nullability::Nullable);
        assert_eq!(
            converted.dependencies.functions,
            vec![
                "gen_random_uuid",
                "uuid7",
                "uuid_str",
                "uuid_blob",
                "uuid7_timestamp_ms"
            ]
        );
    }

    #[test]
    fn infers_join_projections_aliases_and_qualified_param_types() {
        let catalog = catalog();
        let config = test_config(Vec::new());
        let query = ParsedQuery {
            name: "get_user_with_org".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql:
                "SELECT u.id, u.email, o.name AS org_name, upper(o.slug) AS org_slug_upper \
                 FROM users AS u \
                 JOIN organizations o ON o.id = u.org_id \
                 WHERE u.id = :user_id AND o.slug = :org_slug"
                    .to_string(),
            cardinality: Cardinality::One,
        };

        let migration_fingerprint = Fingerprint::from_text("migrations");
        let shaped = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            query,
        )
        .unwrap();

        assert_eq!(
            shaped.normalized_sql,
            "SELECT u.id, u.email, o.name AS org_name, upper(o.slug) AS org_slug_upper \
             FROM users AS u \
             JOIN organizations o ON o.id = u.org_id \
             WHERE u.id = ?1 AND o.slug = ?2"
        );
        assert_eq!(shaped.dependencies.tables, vec!["users", "organizations"]);
        assert_eq!(shaped.columns.len(), 4);
        assert_eq!(shaped.columns[0].name, "id");
        assert_eq!(shaped.columns[0].rust_type.0, "i64");
        assert_eq!(shaped.columns[1].name, "email");
        assert_eq!(shaped.columns[1].nullable, Nullability::NonNull);
        assert_eq!(shaped.columns[2].name, "org_name");
        assert_eq!(shaped.columns[2].rust_type.0, "String");
        assert_eq!(shaped.columns[2].nullable, Nullability::NonNull);
        assert_eq!(shaped.columns[3].name, "org_slug_upper");
        assert_eq!(shaped.columns[3].source, TypeSource::BuiltinFunctionRule);
        assert_eq!(shaped.params[0].name, "user_id");
        assert_eq!(shaped.params[0].rust_type.0, "i64");
        assert_eq!(shaped.params[1].name, "org_slug");
        assert_eq!(shaped.params[1].rust_type.0, "String");
        assert_eq!(shaped.params[1].db_type.as_deref(), Some("sqlite:TEXT"));
    }

    #[test]
    fn infers_tuple_and_in_list_param_types_from_ast_pairs() {
        let catalog = catalog();
        let config = test_config(Vec::new());
        let query = ParsedQuery {
            name: "search_users".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql: "SELECT id FROM users u WHERE (u.id, u.org_id) = (:user_id, :organization_id) AND u.email IN (:primary_email, :secondary_email)".to_string(),
            cardinality: Cardinality::Many,
        };

        let migration_fingerprint = Fingerprint::from_text("migrations");
        let shaped = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            query,
        )
        .unwrap();

        assert_eq!(shaped.params.len(), 4);
        assert_eq!(shaped.params[0].name, "user_id");
        assert_eq!(shaped.params[0].rust_type.0, "i64");
        assert_eq!(shaped.params[1].name, "organization_id");
        assert_eq!(shaped.params[1].rust_type.0, "i64");
        assert_eq!(shaped.params[2].name, "primary_email");
        assert_eq!(shaped.params[2].rust_type.0, "String");
        assert_eq!(shaped.params[3].name, "secondary_email");
        assert_eq!(shaped.params[3].rust_type.0, "String");
    }

    #[test]
    fn infers_range_pattern_and_distinct_param_types_from_ast_pairs() {
        let catalog = catalog();
        let config = test_config(Vec::new());
        let query = ParsedQuery {
            name: "search_emails".to_string(),
            source_file: PathBuf::from("queries/emails.sql"),
            original_sql: "SELECT e.id FROM emails e \
                 WHERE e.created_at BETWEEN :start_at AND :end_at \
                   AND e.email LIKE :email_pattern \
                   AND e.kind IS NOT DISTINCT FROM :kind \
                   AND :user_id IS DISTINCT FROM e.user_id"
                .to_string(),
            cardinality: Cardinality::Many,
        };

        let migration_fingerprint = Fingerprint::from_text("migrations");
        let shaped = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            query,
        )
        .unwrap();

        assert_eq!(shaped.params.len(), 5);
        assert_eq!(shaped.params[0].name, "start_at");
        assert_eq!(shaped.params[0].rust_type.0, "String");
        assert_eq!(shaped.params[1].name, "end_at");
        assert_eq!(shaped.params[1].rust_type.0, "String");
        assert_eq!(shaped.params[2].name, "email_pattern");
        assert_eq!(shaped.params[2].rust_type.0, "String");
        assert_eq!(shaped.params[3].name, "kind");
        assert_eq!(shaped.params[3].rust_type.0, "String");
        assert_eq!(shaped.params[4].name, "user_id");
        assert_eq!(shaped.params[4].rust_type.0, "i64");
    }

    #[test]
    fn leaves_ambiguous_unqualified_join_columns_unknown() {
        let catalog = catalog();
        let config = test_config(Vec::new());
        let query = ParsedQuery {
            name: "ambiguous_join".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql:
                "SELECT id AS ambiguous_id FROM users u JOIN organizations o ON o.id = u.org_id WHERE id = :id"
                    .to_string(),
            cardinality: Cardinality::One,
        };

        let migration_fingerprint = Fingerprint::from_text("migrations");
        let shaped = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            query,
        )
        .unwrap();

        assert_eq!(shaped.columns.len(), 1);
        assert_eq!(shaped.columns[0].name, "ambiguous_id");
        assert_eq!(shaped.columns[0].source, TypeSource::ExpressionInference);
        assert_eq!(shaped.columns[0].confidence, InferenceConfidence::Weak);
        assert_eq!(shaped.params[0].source, TypeSource::Unknown);
    }

    #[test]
    fn infers_cte_and_derived_table_shapes() {
        let config = test_config(Vec::new());
        let catalog = catalog();
        let migration_fingerprint = Fingerprint::from_text("migrations");
        let cte = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            ParsedQuery {
                name: "cte_users".to_string(),
                source_file: PathBuf::from("queries/users.sql"),
                original_sql: "
                    WITH active_users AS (
                        SELECT id, email, org_id FROM users WHERE email = :email
                    )
                    SELECT active_users.id, active_users.email
                    FROM active_users
                    WHERE active_users.id = :id
                "
                .to_string(),
                cardinality: Cardinality::Many,
            },
        )
        .unwrap();

        assert_eq!(cte.columns.len(), 2);
        assert_eq!(cte.columns[0].name, "id");
        assert_eq!(cte.columns[0].rust_type.0, "i64");
        assert_eq!(cte.columns[0].nullable, Nullability::NonNull);
        assert_eq!(cte.columns[1].name, "email");
        assert_eq!(cte.columns[1].rust_type.0, "String");
        assert_eq!(cte.params[0].name, "email");
        assert_eq!(cte.params[0].rust_type.0, "String");
        assert_eq!(cte.params[1].name, "id");
        assert_eq!(cte.params[1].rust_type.0, "i64");

        let derived = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            ParsedQuery {
                name: "derived_users".to_string(),
                source_file: PathBuf::from("queries/users.sql"),
                original_sql: "
                    SELECT u.id, u.email
                    FROM (SELECT id, email FROM users WHERE org_id = :org_id) AS u
                    WHERE u.id = :id
                "
                .to_string(),
                cardinality: Cardinality::Many,
            },
        )
        .unwrap();

        assert_eq!(derived.columns.len(), 2);
        assert_eq!(derived.columns[0].name, "id");
        assert_eq!(derived.columns[1].name, "email");
        assert_eq!(derived.params[0].name, "org_id");
        assert_eq!(derived.params[0].rust_type.0, "i64");
        assert_eq!(derived.params[1].name, "id");
        assert_eq!(derived.params[1].rust_type.0, "i64");
    }

    #[test]
    fn infers_cte_declared_column_names() {
        let config = test_config(Vec::new());
        let catalog = catalog();
        let migration_fingerprint = Fingerprint::from_text("migrations");
        let shaped = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            ParsedQuery {
                name: "cte_declared_columns".to_string(),
                source_file: PathBuf::from("queries/users.sql"),
                original_sql: "
                    WITH active_users(user_id, user_email, user_org_id) AS (
                        SELECT id, email, org_id FROM users WHERE email = :email
                    )
                    SELECT active_users.user_id, active_users.user_email
                    FROM active_users
                    WHERE active_users.user_id = :id
                "
                .to_string(),
                cardinality: Cardinality::Many,
            },
        )
        .unwrap();

        assert_eq!(shaped.columns.len(), 2);
        assert_eq!(shaped.columns[0].name, "user_id");
        assert_eq!(shaped.columns[0].rust_type.0, "i64");
        assert_eq!(shaped.columns[0].nullable, Nullability::NonNull);
        assert_eq!(shaped.columns[1].name, "user_email");
        assert_eq!(shaped.columns[1].rust_type.0, "String");
        assert_eq!(shaped.params[0].name, "email");
        assert_eq!(shaped.params[0].rust_type.0, "String");
        assert_eq!(shaped.params[1].name, "id");
        assert_eq!(shaped.params[1].rust_type.0, "i64");
    }

    #[test]
    fn infers_lateral_derived_table_shapes_and_params() {
        let config = test_config(Vec::new());
        let catalog = catalog();
        let migration_fingerprint = Fingerprint::from_text("migrations");
        let shaped = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            ParsedQuery {
                name: "lateral_email".to_string(),
                source_file: PathBuf::from("queries/users.sql"),
                original_sql: "
                    SELECT u.id, recent.email
                    FROM users u
                    JOIN LATERAL (
                        SELECT e.email
                        FROM emails e
                        WHERE e.user_id = u.id AND e.kind = :kind
                        LIMIT 1
                    ) AS recent ON true
                    WHERE u.id = :id
                "
                .to_string(),
                cardinality: Cardinality::Many,
            },
        )
        .unwrap();

        assert_eq!(shaped.columns.len(), 2);
        assert_eq!(shaped.columns[0].name, "id");
        assert_eq!(shaped.columns[0].rust_type.0, "i64");
        assert_eq!(shaped.columns[1].name, "email");
        assert_eq!(shaped.columns[1].rust_type.0, "String");
        assert_eq!(shaped.params[0].name, "kind");
        assert_eq!(shaped.params[0].rust_type.0, "String");
        assert_eq!(shaped.params[1].name, "id");
        assert_eq!(shaped.params[1].rust_type.0, "i64");
    }

    #[test]
    fn infers_compound_select_branch_param_types() {
        let config = test_config(Vec::new());
        let catalog = catalog();
        let migration_fingerprint = Fingerprint::from_text("migrations");
        let shaped = shape_query(
            &config,
            &catalog,
            &catalog.fingerprint,
            &migration_fingerprint,
            &Fingerprint::from_text("type-mapping"),
            ParsedQuery {
                name: "search_users_or_orgs".to_string(),
                source_file: PathBuf::from("queries/search.sql"),
                original_sql: "
                    SELECT id, email FROM users WHERE id = :user_id
                    UNION ALL
                    SELECT id, slug FROM organizations WHERE slug = :org_slug
                "
                .to_string(),
                cardinality: Cardinality::Many,
            },
        )
        .unwrap();

        assert_eq!(shaped.columns.len(), 2);
        assert_eq!(shaped.columns[0].name, "id");
        assert_eq!(shaped.columns[0].rust_type.0, "i64");
        assert_eq!(shaped.columns[1].name, "email");
        assert_eq!(shaped.params.len(), 2);
        assert_eq!(shaped.params[0].name, "user_id");
        assert_eq!(shaped.params[0].rust_type.0, "i64");
        assert_eq!(shaped.params[1].name, "org_slug");
        assert_eq!(shaped.params[1].rust_type.0, "String");
        assert_eq!(shaped.dependencies.tables, vec!["users", "organizations"]);
    }

    #[test]
    fn loads_schema_files_for_inspect() {
        let dir = temp_dir("queryforge-libsql");
        let schema = dir.join("schema.sql");
        std::fs::write(
            &schema,
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
        )
        .unwrap();
        let config = test_config(vec![schema]);
        let parsed = vec![ParsedQuery {
            name: "get_user".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql: "SELECT id, email FROM users WHERE id = :id".to_string(),
            cardinality: Cardinality::One,
        }];

        let project = inspect(&config, parsed).unwrap();

        assert_eq!(project.queries.len(), 1);
        assert_eq!(project.queries[0].columns.len(), 2);
        assert_eq!(project.queries[0].params[0].rust_type.0, "i64");

        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(feature = "libsql-runtime")]
    #[test]
    fn loads_live_catalog_with_table_xinfo_when_schema_files_are_absent() {
        let dir = temp_dir("queryforge-libsql-live");
        let db_path = dir.join("catalog.db");
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let db = libsql::Builder::new_local(db_path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            let conn = db.connect().unwrap();
            libsql::Connection::execute(
                &conn,
                "CREATE TABLE organizations (
                    id INTEGER PRIMARY KEY,
                    slug TEXT UNIQUE NOT NULL
                )",
                (),
            )
            .await
            .unwrap();
            libsql::Connection::execute(
                &conn,
                "CREATE TABLE users (
                    id INTEGER PRIMARY KEY,
                    org_id INTEGER NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    email TEXT NOT NULL,
                    parent_id INTEGER,
                    active BOOLEAN NOT NULL,
                    email_upper TEXT GENERATED ALWAYS AS (upper(email)) VIRTUAL
                )",
                (),
            )
            .await
            .unwrap();
            libsql::Connection::execute(&conn, "CREATE INDEX users_email_idx ON users(email)", ())
                .await
                .unwrap();
        });

        let mut config = test_config(Vec::new());
        config.database.url = db_path.to_string_lossy().into_owned();
        let parsed = vec![ParsedQuery {
            name: "get_user".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql:
                "SELECT id, email, parent_id, active, email_upper FROM users WHERE id = :id"
                    .to_string(),
            cardinality: Cardinality::One,
        }];

        let project = inspect(&config, parsed).unwrap();
        let query = &project.queries[0];

        assert_eq!(query.columns.len(), 5);
        assert_eq!(query.columns[0].name, "id");
        assert_eq!(query.columns[0].rust_type.0, "i64");
        assert_eq!(query.columns[0].nullable, Nullability::NonNull);
        assert_eq!(query.columns[1].name, "email");
        assert_eq!(query.columns[1].rust_type.0, "String");
        assert_eq!(query.columns[1].nullable, Nullability::NonNull);
        assert_eq!(query.columns[2].name, "parent_id");
        assert_eq!(query.columns[2].nullable, Nullability::Nullable);
        assert_eq!(query.columns[3].name, "active");
        assert_eq!(query.columns[3].rust_type.0, "bool");
        assert_eq!(query.columns[4].name, "email_upper");
        assert_eq!(query.columns[4].rust_type.0, "String");
        assert_eq!(query.params[0].rust_type.0, "i64");
        assert_eq!(query.params[0].db_type.as_deref(), Some("sqlite:INTEGER"));

        let catalog = SchemaCatalog::load(&config).unwrap();
        let users = catalog.tables.get("users").unwrap();
        assert!(users.indexes.iter().any(|index| {
            index.name == "users_email_idx" && !index.unique && index.columns == ["email"]
        }));
        assert!(users.foreign_keys.iter().any(|foreign_key| {
            foreign_key.from_column == "org_id"
                && foreign_key.table == "organizations"
                && foreign_key.to_column.as_deref() == Some("id")
                && foreign_key.on_delete.as_deref() == Some("CASCADE")
        }));
        assert!(catalog.fingerprint.as_str().contains("fnv1a64:"));

        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(all(feature = "libsql-runtime", not(feature = "libsql-remote")))]
    #[test]
    fn reports_remote_catalog_urls_need_remote_feature_without_schema_files() {
        let mut config = test_config(Vec::new());
        config.database.url = "libsql://example.turso.io".to_string();

        let err = inspect(&config, Vec::new()).unwrap_err();

        assert!(err.to_string().contains(
            "remote libSQL catalog introspection for `libsql://example.turso.io` requires enabling the QueryForge `libsql-remote` feature"
        ));
        assert!(err
            .to_string()
            .contains("provide [schema].files for offline inference"));
    }

    #[cfg(all(feature = "libsql-runtime", feature = "libsql-remote"))]
    #[test]
    fn reports_remote_catalog_urls_need_auth_without_schema_files() {
        let mut config = test_config(Vec::new());
        config.database.url = "libsql://example.turso.io".to_string();

        let err = inspect(&config, Vec::new()).unwrap_err();

        assert!(err.to_string().contains(
            "remote libSQL catalog introspection for `libsql://example.turso.io` requires [database].auth_token or [database].auth_token_env"
        ));
        assert!(err
            .to_string()
            .contains("provide [schema].files for offline inference"));
    }

    #[cfg(all(feature = "libsql-runtime", feature = "uuid-types"))]
    #[test]
    fn remote_catalog_urls_with_schema_files_use_schema_fallback_without_auth() {
        let dir = temp_dir("queryforge-libsql-remote-schema-fallback");
        let schema = dir.join("schema.sql");
        std::fs::write(
            &schema,
            "CREATE TABLE users (id UUID PRIMARY KEY, email TEXT NOT NULL);",
        )
        .unwrap();

        let mut config = test_config(vec![schema]);
        config.database.url = "libsql://example.turso.io".to_string();
        config.type_mapping.uuid = crate::config::UuidTypeMapping::Uuid;
        let parsed = vec![ParsedQuery {
            name: "get_user".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql: "SELECT id, email FROM users WHERE id = :id".to_string(),
            cardinality: Cardinality::One,
        }];

        let project = inspect(&config, parsed).unwrap();

        assert_eq!(project.queries[0].columns[0].rust_type.0, "uuid::Uuid");
        assert_eq!(project.queries[0].params[0].rust_type.0, "uuid::Uuid");

        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(feature = "libsql-runtime")]
    #[test]
    fn falls_back_to_schema_files_when_live_catalog_is_empty() {
        let dir = temp_dir("queryforge-libsql-fallback");
        let db_path = dir.join("empty.db");
        let schema = dir.join("schema.sql");
        std::fs::write(
            &schema,
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
        )
        .unwrap();

        let mut config = test_config(vec![schema]);
        config.database.url = db_path.to_string_lossy().into_owned();
        let parsed = vec![ParsedQuery {
            name: "get_user".to_string(),
            source_file: PathBuf::from("queries/users.sql"),
            original_sql: "SELECT id, email FROM users WHERE id = :id".to_string(),
            cardinality: Cardinality::One,
        }];

        let project = inspect(&config, parsed).unwrap();

        assert_eq!(project.queries[0].columns.len(), 2);
        assert_eq!(project.queries[0].columns[1].name, "email");
        assert_eq!(project.queries[0].columns[1].nullable, Nullability::NonNull);

        std::fs::remove_dir_all(dir).ok();
    }

    fn catalog() -> SchemaCatalog {
        let tables = parse_schema_catalog(
            "
            CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                email TEXT NOT NULL,
                parent_id INTEGER,
                org_id INTEGER NOT NULL,
                active BOOLEAN NOT NULL
            );
            CREATE TABLE organizations (
                id INTEGER PRIMARY KEY,
                slug TEXT NOT NULL,
                name TEXT NOT NULL
            );
            CREATE TABLE emails (
                id INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL,
                email TEXT NOT NULL,
                kind TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            ",
        )
        .unwrap();
        SchemaCatalog {
            tables,
            fingerprint: Fingerprint::from_text("schema"),
        }
    }

    fn test_config(schema_files: Vec<PathBuf>) -> Config {
        Config {
            database: DatabaseConfig {
                backend: DatabaseBackend::Libsql,
                url: "file:test.db".to_string(),
                auth_token: None,
                auth_token_env: None,
            },
            codegen: CodegenConfig {
                out_dir: PathBuf::from("generated"),
                execution_target: ExecutionTarget::LibsqlNative,
                query_dir: PathBuf::from("queries"),
            },
            schema: SchemaConfig {
                files: schema_files,
            },
            migrations: MigrationsConfig::default(),
            build: BuildConfig::default(),
            inference: InferenceConfig::default(),
            type_mapping: crate::config::TypeMappingConfig::default(),
        }
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
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
