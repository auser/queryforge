use std::collections::{BTreeMap, BTreeSet};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::fingerprint::{Fingerprint, QUERYFORGE_CODEGEN_VERSION};
use crate::ir::{
    InferenceConfidence, Nullability, ParsedQuery, ProjectShape, QueryColumn, QueryDependencies,
    QueryParam, QueryShape, TypeSource,
};
use crate::sql_ir::{self, SelectProjection, SelectStatement};
use crate::type_map::{postgres_type_to_rust_with_config, type_mapping_fingerprint};

pub async fn inspect(config: &Config, parsed: Vec<ParsedQuery>) -> Result<ProjectShape> {
    crate::type_map::validate_type_mapping_features(config)?;

    let (client, connection) = tokio_postgres::connect(&config.database.url, tokio_postgres::NoTls)
        .await
        .map_err(|err| Error::Backend(format!("failed to connect to postgres: {err}")))?;

    tokio::spawn(async move {
        if let Err(err) = connection.await {
            eprintln!("queryforge postgres connection error: {err}");
        }
    });

    let mut queries = Vec::new();
    let schema_fingerprint = Fingerprint::from_text("postgres-live-schema-v0");
    let migration_fingerprint = Fingerprint::from_paths(&config.migrations.paths)?;
    let type_mapping_fingerprint = type_mapping_fingerprint(config);

    for query in parsed {
        let shaped = inspect_query(
            config,
            &schema_fingerprint,
            &migration_fingerprint,
            &type_mapping_fingerprint,
            &client,
            query,
        )
        .await?;
        queries.push(shaped);
    }

    let mut project_text = format!(
        "queryforge-version={}\nbackend={}\nexecution-target={}\ninference-policy={}\ntype-mapping={}\nschema={}\nmigrations={}\n",
        QUERYFORGE_CODEGEN_VERSION,
        config.database.backend,
        config.codegen.execution_target,
        config.inference.unknown_expression_policy,
        type_mapping_fingerprint,
        schema_fingerprint,
        migration_fingerprint
    );
    for query in &queries {
        project_text.push_str(query.fingerprint.as_str());
        project_text.push('\n');
    }

    Ok(ProjectShape {
        backend: config.database.backend.clone(),
        execution_target: config.codegen.execution_target.clone(),
        schema_fingerprint,
        migration_fingerprint,
        type_mapping_fingerprint,
        queries,
        fingerprint: Fingerprint::from_text(&project_text),
    })
}

async fn inspect_query(
    config: &Config,
    schema_fingerprint: &Fingerprint,
    migration_fingerprint: &Fingerprint,
    type_mapping_fingerprint: &Fingerprint,
    client: &tokio_postgres::Client,
    query: ParsedQuery,
) -> Result<QueryShape> {
    let normalized = normalize_postgres_params(&query.original_sql)?;

    let statement = client
        .prepare(&normalized.sql)
        .await
        .map_err(|err| Error::Backend(format!("failed to prepare `{}`: {err}", query.name)))?;

    let mut params: Vec<QueryParam> = statement
        .params()
        .iter()
        .enumerate()
        .map(|(index, ty)| QueryParam {
            name: normalized
                .param_names
                .get(index)
                .cloned()
                .unwrap_or_else(|| format!("param_{}", index + 1)),
            position: index + 1,
            db_type: Some(format!("postgres:{}", ty.name())),
            rust_type: postgres_type_to_rust_with_config(ty.name(), &config.type_mapping),
            source: TypeSource::DatabaseMetadata,
            confidence: InferenceConfidence::Exact,
        })
        .collect();
    for param in &mut params {
        if let Some(rust_type) = query.type_overrides.for_param(&param.name) {
            param.rust_type = rust_type.clone();
            param.source = TypeSource::UserOverride;
            param.confidence = InferenceConfidence::UserOverride;
        }
    }

    let parsed_select = sql_ir::parse_select(&query.original_sql);
    let nullable_join_tables = nullable_join_tables(parsed_select.as_ref());
    let expression_context = match parsed_select.as_ref() {
        Some(select) => PgExpressionContext::load(client, select).await?,
        None => PgExpressionContext::default(),
    };
    let expression_nullabilities = expression_nullabilities(
        parsed_select.as_ref(),
        &expression_context,
        statement.columns(),
    );
    let mut columns = Vec::new();
    for (index, column) in statement.columns().iter().enumerate() {
        let metadata_nullability =
            inspect_column_nullability(client, column, &nullable_join_tables).await?;
        columns.push(QueryColumn {
            name: column.name().to_string(),
            rust_name: crate::names::to_snake_case(column.name()),
            db_type: Some(format!("postgres:{}", column.type_().name())),
            rust_type: postgres_type_to_rust_with_config(
                column.type_().name(),
                &config.type_mapping,
            ),
            nullable: if metadata_nullability == Nullability::Unknown {
                expression_nullabilities
                    .get(index)
                    .cloned()
                    .unwrap_or(Nullability::Unknown)
            } else {
                metadata_nullability
            },
            source: TypeSource::DatabaseMetadata,
            confidence: InferenceConfidence::Exact,
        });
    }
    for column in &mut columns {
        if let Some(rust_type) = query
            .type_overrides
            .for_column(&column.name, &column.rust_name)
        {
            column.rust_type = rust_type.clone();
            column.source = TypeSource::UserOverride;
            column.confidence = InferenceConfidence::UserOverride;
        }
    }
    query
        .type_overrides
        .validate_matches(&query.name, &params, &columns)?;

    let fingerprint_input = format!(
        "queryforge-version={}\nbackend={}\nexecution-target={}\ninference-policy={}\ntype-mapping={}\nschema={}\nmigrations={}\nquery={}\ncardinality={:?}\nsql={}\nparams={:?}\ncolumns={:?}\n",
        QUERYFORGE_CODEGEN_VERSION,
        config.database.backend,
        config.codegen.execution_target,
        config.inference.unknown_expression_policy,
        type_mapping_fingerprint,
        schema_fingerprint,
        migration_fingerprint,
        query.name,
        query.cardinality,
        normalized.sql,
        params,
        columns
    );

    Ok(QueryShape {
        name: query.name,
        module_path: module_path(&query.source_file),
        source_file: query.source_file,
        original_sql: query.original_sql,
        normalized_sql: normalized.sql,
        cardinality: query.cardinality,
        params,
        columns,
        dependencies: QueryDependencies::default(),
        fingerprint: Fingerprint::from_text(&fingerprint_input),
    })
}

fn module_path(file: &std::path::Path) -> Vec<String> {
    let stem = file
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("queries");
    vec![crate::names::to_snake_case(stem)]
}

async fn inspect_column_nullability(
    client: &tokio_postgres::Client,
    column: &tokio_postgres::Column,
    nullable_join_tables: &BTreeSet<String>,
) -> Result<Nullability> {
    let (Some(table_oid), Some(column_id)) = (column.table_oid(), column.column_id()) else {
        return Ok(Nullability::Unknown);
    };

    let row = client
        .query_opt(
            "SELECT a.attnotnull, c.relname \
             FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             WHERE a.attrelid = $1::oid AND a.attnum = $2::int2 AND NOT a.attisdropped",
            &[&table_oid, &column_id],
        )
        .await
        .map_err(|err| {
            Error::Backend(format!(
                "failed to inspect nullability for `{}`: {err}",
                column.name()
            ))
        })?;

    Ok(match row {
        Some(row) => {
            let attnotnull = row.get::<_, bool>(0);
            let relname = row.get::<_, String>(1);
            if nullable_join_tables.contains(&normalize_pg_relation_name(&relname)) {
                Nullability::Nullable
            } else if attnotnull {
                Nullability::NonNull
            } else {
                Nullability::Nullable
            }
        }
        None => Nullability::Unknown,
    })
}

fn nullable_join_tables(select: Option<&SelectStatement>) -> BTreeSet<String> {
    select
        .map(|select| {
            select
                .table_refs
                .iter()
                .filter(|table| table.nullable)
                .map(|table| normalize_pg_relation_name(&table.name))
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_pg_relation_name(name: &str) -> String {
    let table = name
        .rsplit_once('.')
        .map(|(_, table)| table)
        .unwrap_or(name);
    sql_ir::strip_identifier_quotes(table).to_ascii_lowercase()
}

#[derive(Debug, Clone, Default)]
struct PgExpressionContext {
    qualified_columns: BTreeMap<(String, String), Nullability>,
    unqualified_columns: BTreeMap<String, Option<Nullability>>,
}

impl PgExpressionContext {
    async fn load(client: &tokio_postgres::Client, select: &SelectStatement) -> Result<Self> {
        Self::load_with_base(client, select, None).await
    }

    async fn load_with_base(
        client: &tokio_postgres::Client,
        select: &SelectStatement,
        base: Option<&Self>,
    ) -> Result<Self> {
        let mut context = base.cloned().unwrap_or_default();

        for cte in &select.ctes {
            let columns = synthetic_columns_for_cte_with_base(client, cte, Some(&context)).await?;
            context.add_synthetic_relation(&cte.name, None, false, columns);
        }

        for table in &select.table_refs {
            if let Some(cte) = select.ctes.iter().find(|cte| {
                normalize_pg_relation_name(&cte.name) == normalize_pg_relation_name(&table.name)
            }) {
                let columns =
                    synthetic_columns_for_cte_with_base(client, cte, Some(&context)).await?;
                context.add_synthetic_relation(
                    &table.name,
                    table.alias.as_deref(),
                    table.nullable,
                    columns,
                );
                continue;
            }

            if let Some(query) = &table.derived_query {
                let columns = if table.lateral {
                    synthetic_columns_for_select_with_base(client, query, Some(&context)).await?
                } else {
                    synthetic_columns_for_select(client, query).await?
                };
                context.add_synthetic_relation(
                    &table.name,
                    table.alias.as_deref(),
                    table.nullable,
                    columns,
                );
                continue;
            }

            let rows = client
                .query(
                    "SELECT a.attname, a.attnotnull \
                     FROM pg_attribute a \
                     WHERE a.attrelid = to_regclass($1) \
                       AND a.attnum > 0 \
                       AND NOT a.attisdropped",
                    &[&table.name],
                )
                .await
                .map_err(|err| {
                    Error::Backend(format!(
                        "failed to inspect columns for `{}`: {err}",
                        table.name
                    ))
                })?;

            let qualifiers = table_qualifiers(&table.name, table.alias.as_deref());
            for row in rows {
                let column = normalize_pg_relation_name(&row.get::<_, String>(0));
                let attnotnull = row.get::<_, bool>(1);
                let nullability = if table.nullable {
                    Nullability::Nullable
                } else if attnotnull {
                    Nullability::NonNull
                } else {
                    Nullability::Nullable
                };

                for qualifier in &qualifiers {
                    context
                        .qualified_columns
                        .insert((qualifier.clone(), column.clone()), nullability.clone());
                }

                context
                    .unqualified_columns
                    .entry(column)
                    .and_modify(|existing| {
                        if existing.as_ref() != Some(&nullability) {
                            *existing = None;
                        }
                    })
                    .or_insert(Some(nullability));
            }
        }

        Ok(context)
    }

    fn add_synthetic_relation(
        &mut self,
        name: &str,
        alias: Option<&str>,
        nullable_relation: bool,
        columns: Vec<(String, Nullability)>,
    ) {
        let qualifiers = table_qualifiers(name, alias);
        for (column, mut nullability) in columns {
            let column = normalize_pg_relation_name(&column);
            if nullable_relation {
                nullability = Nullability::Nullable;
            }

            for qualifier in &qualifiers {
                self.qualified_columns
                    .insert((qualifier.clone(), column.clone()), nullability.clone());
            }

            self.unqualified_columns
                .entry(column)
                .and_modify(|existing| {
                    if existing.as_ref() != Some(&nullability) {
                        *existing = None;
                    }
                })
                .or_insert(Some(nullability));
        }
    }

    fn column_nullability(&self, expr: &str) -> Option<Nullability> {
        let (qualifier, column) = sql_ir::split_qualified_name(expr);
        let column = normalize_pg_relation_name(&column);
        match qualifier {
            Some(qualifier) => self
                .qualified_columns
                .get(&(normalize_pg_relation_name(&qualifier), column))
                .cloned(),
            None => self.unqualified_columns.get(&column).cloned().flatten(),
        }
    }
}

fn synthetic_columns_for_select<'a>(
    client: &'a tokio_postgres::Client,
    sql: &'a str,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<(String, Nullability)>>> + 'a>> {
    synthetic_columns_for_select_with_base(client, sql, None)
}

fn synthetic_columns_for_select_with_base<'a>(
    client: &'a tokio_postgres::Client,
    sql: &'a str,
    base: Option<&'a PgExpressionContext>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<(String, Nullability)>>> + 'a>> {
    Box::pin(async move {
        let Some(select) = sql_ir::parse_select(sql) else {
            return Ok(Vec::new());
        };
        let context = PgExpressionContext::load_with_base(client, &select, base).await?;
        Ok(select
            .projections
            .iter()
            .filter_map(|projection| synthetic_projection_column(projection, &context))
            .collect())
    })
}

fn synthetic_columns_for_cte_with_base<'a>(
    client: &'a tokio_postgres::Client,
    cte: &'a sql_ir::CommonTableExpression,
    base: Option<&'a PgExpressionContext>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<(String, Nullability)>>> + 'a>> {
    Box::pin(async move {
        let Some(select) = sql_ir::parse_select(&cte.query) else {
            return Ok(Vec::new());
        };

        let context = PgExpressionContext::load_with_base(client, &select, base).await?;
        let mut columns = select
            .projections
            .iter()
            .filter_map(|projection| synthetic_projection_column(projection, &context))
            .collect::<Vec<_>>();
        columns = apply_declared_column_names(columns, &cte.columns);

        for compound in &select.compound {
            let mut recursive_base;
            let branch_base = if cte.recursive {
                recursive_base = base.cloned().unwrap_or_default();
                recursive_base.add_synthetic_relation(&cte.name, None, false, columns.clone());
                if let Some(branch_select) = sql_ir::parse_select(&compound.query) {
                    for table in branch_select.table_refs.iter().filter(|table| {
                        normalize_pg_relation_name(&table.name)
                            == normalize_pg_relation_name(&cte.name)
                    }) {
                        recursive_base.add_synthetic_relation(
                            &cte.name,
                            table.alias.as_deref(),
                            table.nullable,
                            columns.clone(),
                        );
                    }
                }
                Some(&recursive_base)
            } else {
                base
            };
            let branch_columns =
                synthetic_columns_for_select_with_base(client, &compound.query, branch_base)
                    .await?;
            merge_compound_columns(&mut columns, &branch_columns);
        }

        Ok(columns)
    })
}

fn merge_compound_columns(
    columns: &mut [(String, Nullability)],
    branch_columns: &[(String, Nullability)],
) {
    for (idx, (_name, nullability)) in columns.iter_mut().enumerate() {
        *nullability = match branch_columns
            .get(idx)
            .map(|(_name, nullability)| nullability)
        {
            Some(branch_nullability) => {
                combine_compound_nullability(nullability.clone(), branch_nullability.clone())
            }
            None => Nullability::Unknown,
        };
    }
}

fn combine_compound_nullability(left: Nullability, right: Nullability) -> Nullability {
    match (left, right) {
        (Nullability::Unknown, _) | (_, Nullability::Unknown) => Nullability::Unknown,
        (Nullability::Nullable, _) | (_, Nullability::Nullable) => Nullability::Nullable,
        (Nullability::NonNull, Nullability::NonNull) => Nullability::NonNull,
    }
}

fn apply_declared_column_names(
    columns: Vec<(String, Nullability)>,
    declared_names: &[String],
) -> Vec<(String, Nullability)> {
    if declared_names.is_empty() {
        return columns;
    }

    columns
        .into_iter()
        .enumerate()
        .map(|(idx, (name, nullability))| {
            let name = declared_names.get(idx).cloned().unwrap_or(name);
            (name, nullability)
        })
        .collect()
}

fn synthetic_projection_column(
    projection: &SelectProjection,
    context: &PgExpressionContext,
) -> Option<(String, Nullability)> {
    let name = projection
        .alias
        .clone()
        .or_else(|| projection_column_name(&projection.expr))?;
    Some((name, infer_expression_nullability(projection, context)))
}

fn projection_column_name(expr: &str) -> Option<String> {
    let expr = strip_outer_parens(strip_top_level_cast(expr.trim()));
    if expr == "*" || expr.ends_with(".*") {
        return None;
    }
    let (_qualifier, column) = sql_ir::split_qualified_name(expr);
    if column
        .chars()
        .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        Some(column)
    } else {
        None
    }
}

fn table_qualifiers(table_name: &str, alias: Option<&str>) -> Vec<String> {
    let mut qualifiers = Vec::new();
    if let Some(alias) = alias {
        qualifiers.push(normalize_pg_relation_name(alias));
    }
    let normalized_table = normalize_pg_relation_name(table_name);
    if !qualifiers.contains(&normalized_table) {
        qualifiers.push(normalized_table);
    }
    let stripped = sql_ir::strip_identifier_quotes(table_name).to_ascii_lowercase();
    if !qualifiers.contains(&stripped) {
        qualifiers.push(stripped);
    }
    qualifiers
}

fn expression_nullabilities(
    select: Option<&SelectStatement>,
    context: &PgExpressionContext,
    columns: &[tokio_postgres::Column],
) -> Vec<Nullability> {
    let Some(select) = select else {
        return Vec::new();
    };
    if select.projections.len() != columns.len() {
        return Vec::new();
    }
    select
        .projections
        .iter()
        .map(|projection| infer_expression_nullability(projection, context))
        .collect()
}

fn infer_expression_nullability(
    projection: &SelectProjection,
    context: &PgExpressionContext,
) -> Nullability {
    infer_expr_nullability(&projection.expr, context)
}

fn infer_expr_nullability(expr: &str, context: &PgExpressionContext) -> Nullability {
    let expr = strip_outer_parens(strip_top_level_cast(expr.trim()));
    if expr.is_empty() {
        return Nullability::Unknown;
    }

    if is_null_literal(expr) {
        return Nullability::Nullable;
    }
    if is_non_null_literal(expr) {
        return Nullability::NonNull;
    }
    if is_bind_param(expr) {
        return Nullability::NonNull;
    }
    if let Some(nullability) = context.column_nullability(expr) {
        return nullability;
    }
    if let Some(nullability) = scalar_subquery_nullability(expr, context) {
        return nullability;
    }
    if function_name(expr).is_some_and(|name| name.eq_ignore_ascii_case("count")) {
        return Nullability::NonNull;
    }
    if let Some(args) = function_args(expr, "coalesce") {
        return coalesce_nullability(args, context);
    }
    if function_args(expr, "nullif").is_some() {
        return Nullability::Nullable;
    }
    if starts_case_expression(expr) {
        return case_nullability(expr, context);
    }
    if let Some(inner) = strip_prefix_keyword(expr, "not") {
        return boolean_operand_nullability(inner, context);
    }
    if is_is_null_expression(expr) || is_is_distinct_from_expression(expr) {
        return Nullability::NonNull;
    }
    if let Some(nullability) = between_nullability(expr, context) {
        return nullability;
    }
    if let Some(nullability) = in_list_nullability(expr, context) {
        return nullability;
    }
    if let Some(nullability) = pattern_match_nullability(expr, context) {
        return nullability;
    }

    for operator in [" and ", " or "] {
        let parts = split_top_level_operator_case_insensitive(expr, operator);
        if parts.len() > 1 {
            return nullable_if_any_nullable(&parts, context);
        }
    }

    for operator in [" is not ", " is "] {
        let parts = split_top_level_operator_case_insensitive(expr, operator);
        if parts.len() == 2 && !parts[1].eq_ignore_ascii_case("null") {
            return nullable_if_any_nullable(&parts, context);
        }
    }

    for operator in ["<=", ">=", "<>", "!=", "=", "<", ">"] {
        let parts = split_top_level_operator(expr, operator);
        if parts.len() == 2 {
            return nullable_if_any_nullable(&parts, context);
        }
    }

    for operator in ["+", "-", "*", "/", "%"] {
        let parts = split_top_level_operator(expr, operator);
        if parts.len() > 1 {
            return nullable_if_any_nullable(&parts, context);
        }
    }

    let concat_parts = split_top_level_operator(expr, "||");
    if concat_parts.len() > 1 {
        return concat_nullability(&concat_parts, context);
    }

    Nullability::Unknown
}

fn scalar_subquery_nullability(expr: &str, context: &PgExpressionContext) -> Option<Nullability> {
    let select = sql_ir::parse_select(expr)?;
    let projection = select.projections.first()?;
    let projected = infer_expr_nullability(&projection.expr, context);
    if scalar_subquery_guarantees_row(expr, projection) {
        return Some(projected);
    }

    Some(match projected {
        Nullability::NonNull | Nullability::Nullable => Nullability::Nullable,
        Nullability::Unknown => Nullability::Unknown,
    })
}

fn scalar_subquery_guarantees_row(sql: &str, projection: &SelectProjection) -> bool {
    if has_top_level_group_by(sql) {
        return false;
    }

    let expr = strip_outer_parens(strip_top_level_cast(projection.expr.trim()));
    if function_name(expr).is_some_and(|name| name.eq_ignore_ascii_case("count")) {
        return true;
    }
    if let Some(args) = function_args(expr, "coalesce") {
        return args.into_iter().any(|arg| {
            function_name(strip_outer_parens(strip_top_level_cast(arg.trim())))
                .is_some_and(|name| name.eq_ignore_ascii_case("count"))
        });
    }

    false
}

fn has_top_level_group_by(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut depth = 0_i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut idx = 0;

    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' if !in_double => {
                in_single = !in_single;
                idx += 1;
                continue;
            }
            b'"' if !in_single => {
                in_double = !in_double;
                idx += 1;
                continue;
            }
            b'(' if !in_single && !in_double => depth += 1,
            b')' if !in_single && !in_double => depth -= 1,
            _ => {}
        }

        if depth == 0
            && !in_single
            && !in_double
            && keyword_at(sql, idx, "group")
            && next_keyword_after(sql, idx + "group".len()) == Some("by")
        {
            return true;
        }
        idx += 1;
    }

    false
}

fn keyword_at(input: &str, idx: usize, keyword: &str) -> bool {
    let Some(candidate) = input.get(idx..idx + keyword.len()) else {
        return false;
    };
    if !candidate.eq_ignore_ascii_case(keyword) {
        return false;
    }
    let before = idx
        .checked_sub(1)
        .and_then(|idx| input.as_bytes().get(idx))
        .copied();
    let after = input.as_bytes().get(idx + keyword.len()).copied();
    !before.is_some_and(is_ident_continue_byte) && !after.is_some_and(is_ident_continue_byte)
}

fn next_keyword_after(input: &str, mut idx: usize) -> Option<&str> {
    let bytes = input.as_bytes();
    while bytes.get(idx).is_some_and(u8::is_ascii_whitespace) {
        idx += 1;
    }
    if keyword_at(input, idx, "by") {
        Some("by")
    } else {
        None
    }
}

fn is_ident_continue_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

fn is_bind_param(expr: &str) -> bool {
    let bytes = expr.as_bytes();
    if bytes.first() == Some(&b'$') {
        return bytes.len() > 1 && bytes[1..].iter().all(u8::is_ascii_digit);
    }

    if bytes.first() != Some(&b':') || bytes.get(1) == Some(&b':') {
        return false;
    }

    let Some(first) = bytes.get(1) else {
        return false;
    };
    (first.is_ascii_alphabetic() || *first == b'_')
        && bytes[2..]
            .iter()
            .all(|byte| *byte == b'_' || byte.is_ascii_alphanumeric())
}

fn coalesce_nullability(args: Vec<&str>, context: &PgExpressionContext) -> Nullability {
    let mut saw_unknown = false;
    for arg in args {
        match infer_expr_nullability(arg, context) {
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

fn case_nullability(expr: &str, context: &PgExpressionContext) -> Nullability {
    let Some((then_exprs, else_expr)) = case_result_expressions(expr) else {
        return Nullability::Unknown;
    };
    let mut result_exprs = then_exprs;
    let has_else = else_expr.is_some();
    if let Some(else_expr) = else_expr {
        result_exprs.push(else_expr);
    }

    let result = nullability_for_exprs(&result_exprs, context);
    if !has_else {
        return match result {
            Nullability::Unknown => Nullability::Unknown,
            Nullability::NonNull | Nullability::Nullable => Nullability::Nullable,
        };
    }
    result
}

fn case_result_expressions(expr: &str) -> Option<(Vec<&str>, Option<&str>)> {
    let positions = top_level_keyword_positions(expr, &["then", "when", "else", "end"]);
    let end_idx = positions.iter().find(|(_, keyword)| *keyword == "end")?.0;
    let mut then_exprs = Vec::new();
    let mut else_expr = None;

    for (idx, (pos, keyword)) in positions.iter().enumerate() {
        match *keyword {
            "then" => {
                let start = pos + "then".len();
                let end = positions
                    .iter()
                    .skip(idx + 1)
                    .find(|(_, keyword)| matches!(*keyword, "when" | "else" | "end"))
                    .map(|(pos, _)| *pos)
                    .unwrap_or(end_idx);
                let result = expr[start..end].trim();
                if !result.is_empty() {
                    then_exprs.push(result);
                }
            }
            "else" => {
                let start = pos + "else".len();
                let result = expr[start..end_idx].trim();
                if !result.is_empty() {
                    else_expr = Some(result);
                }
            }
            _ => {}
        }
    }

    if then_exprs.is_empty() {
        None
    } else {
        Some((then_exprs, else_expr))
    }
}

fn boolean_operand_nullability(expr: &str, context: &PgExpressionContext) -> Nullability {
    match infer_expr_nullability(expr, context) {
        Nullability::NonNull => Nullability::NonNull,
        Nullability::Nullable => Nullability::Nullable,
        Nullability::Unknown => Nullability::Unknown,
    }
}

fn between_nullability(expr: &str, context: &PgExpressionContext) -> Option<Nullability> {
    for operator in [" not between ", " between "] {
        let parts = split_top_level_operator_case_insensitive(expr, operator);
        if parts.len() != 2 {
            continue;
        }
        let bounds = split_top_level_operator_case_insensitive(parts[1], " and ");
        if bounds.len() != 2 {
            return Some(Nullability::Unknown);
        }
        return Some(nullable_if_any_nullable(
            &[parts[0], bounds[0], bounds[1]],
            context,
        ));
    }
    None
}

fn in_list_nullability(expr: &str, context: &PgExpressionContext) -> Option<Nullability> {
    for operator in [" not in ", " in "] {
        let parts = split_top_level_operator_case_insensitive(expr, operator);
        if parts.len() != 2 {
            continue;
        }
        let Some(items) = parenthesized_list(parts[1]) else {
            return Some(Nullability::Unknown);
        };
        if items
            .first()
            .is_some_and(|item| sql_ir::parse_select(item).is_some())
        {
            return Some(match infer_expr_nullability(parts[0], context) {
                Nullability::Nullable => Nullability::Nullable,
                Nullability::NonNull | Nullability::Unknown => Nullability::Unknown,
            });
        }

        let mut expressions = Vec::with_capacity(items.len() + 1);
        expressions.push(parts[0]);
        expressions.extend(items);
        return Some(nullable_if_any_nullable(&expressions, context));
    }
    None
}

fn pattern_match_nullability(expr: &str, context: &PgExpressionContext) -> Option<Nullability> {
    for operator in [" not ilike ", " not like ", " ilike ", " like "] {
        let parts = split_top_level_operator_case_insensitive(expr, operator);
        if parts.len() == 2 {
            return Some(nullable_if_any_nullable(&parts, context));
        }
    }
    None
}

fn parenthesized_list(expr: &str) -> Option<Vec<&str>> {
    let expr = expr.trim();
    if !expr.starts_with('(') {
        return None;
    }
    let close = matching_trailing_paren(expr, 0)?;
    if close + 1 != expr.len() {
        return None;
    }
    let expr = expr[1..close].trim();
    if expr.is_empty() {
        return None;
    }
    Some(sql_ir::split_comma_separated(expr))
}

fn nullable_if_any_nullable(parts: &[&str], context: &PgExpressionContext) -> Nullability {
    nullability_for_exprs(parts, context)
}

fn nullability_for_exprs(parts: &[&str], context: &PgExpressionContext) -> Nullability {
    let mut saw_nullable = false;
    for part in parts {
        match infer_expr_nullability(part, context) {
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

fn concat_nullability(parts: &[&str], context: &PgExpressionContext) -> Nullability {
    nullability_for_exprs(parts, context)
}

fn function_name(expr: &str) -> Option<&str> {
    let open = expr.find('(')?;
    let name = expr[..open].trim();
    if name
        .chars()
        .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        Some(name)
    } else {
        None
    }
}

fn function_args<'a>(expr: &'a str, expected_name: &str) -> Option<Vec<&'a str>> {
    let name = function_name(expr)?;
    if !name.eq_ignore_ascii_case(expected_name) {
        return None;
    }
    let open = expr.find('(')?;
    let close = matching_trailing_paren(expr, open)?;
    if expr[close + 1..].trim().is_empty() {
        Some(sql_ir::split_comma_separated(&expr[open + 1..close]))
    } else {
        None
    }
}

fn strip_top_level_cast(expr: &str) -> &str {
    let bytes = expr.as_bytes();
    let mut depth = 0_i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut idx = 0;
    while idx + 1 < bytes.len() {
        match bytes[idx] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'(' if !in_single && !in_double => depth += 1,
            b')' if !in_single && !in_double => depth -= 1,
            b':' if depth == 0
                && !in_single
                && !in_double
                && bytes.get(idx + 1).copied() == Some(b':') =>
            {
                if is_type_cast_suffix(&expr[idx + 2..]) {
                    return expr[..idx].trim();
                }
            }
            _ => {}
        }
        idx += 1;
    }
    expr
}

fn is_type_cast_suffix(suffix: &str) -> bool {
    let suffix = suffix.trim();
    if suffix.is_empty() {
        return false;
    }

    let bytes = suffix.as_bytes();
    let mut depth = 0_i32;
    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            byte if depth == 0
                && !(byte.is_ascii_alphanumeric()
                    || matches!(byte, b'_' | b'.' | b'"' | b'[' | b']')) =>
            {
                return false;
            }
            _ => {}
        }
        idx += 1;
    }

    depth == 0
}

fn strip_outer_parens(mut expr: &str) -> &str {
    loop {
        let trimmed = expr.trim();
        if !trimmed.starts_with('(') {
            return trimmed;
        }
        let Some(close) = matching_trailing_paren(trimmed, 0) else {
            return trimmed;
        };
        if close + 1 == trimmed.len() {
            expr = &trimmed[1..close];
        } else {
            return trimmed;
        }
    }
}

fn matching_trailing_paren(input: &str, open: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut depth = 0_i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut idx = open;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'(' if !in_single && !in_double => depth += 1,
            b')' if !in_single && !in_double => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
        idx += 1;
    }
    None
}

fn is_null_literal(expr: &str) -> bool {
    expr.eq_ignore_ascii_case("null")
}

fn is_non_null_literal(expr: &str) -> bool {
    let trimmed = expr.trim();
    if matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "true" | "false" | "current_date" | "current_time" | "current_timestamp"
    ) {
        return true;
    }
    if trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2 {
        return true;
    }
    trimmed.parse::<i64>().is_ok() || trimmed.parse::<f64>().is_ok()
}

fn starts_case_expression(expr: &str) -> bool {
    let trimmed = expr.trim_start();
    keyword_at(trimmed, 0, "case")
}

fn strip_prefix_keyword<'a>(expr: &'a str, keyword: &str) -> Option<&'a str> {
    let trimmed = expr.trim_start();
    if keyword_at(trimmed, 0, keyword) {
        Some(trimmed[keyword.len()..].trim_start())
    } else {
        None
    }
}

fn is_is_null_expression(expr: &str) -> bool {
    let parts = split_top_level_operator_case_insensitive(expr, " is not ");
    if parts.len() == 2 && parts[1].eq_ignore_ascii_case("null") {
        return true;
    }
    let parts = split_top_level_operator_case_insensitive(expr, " is ");
    parts.len() == 2 && parts[1].eq_ignore_ascii_case("null")
}

fn is_is_distinct_from_expression(expr: &str) -> bool {
    split_top_level_operator_case_insensitive(expr, " is distinct from ").len() == 2
        || split_top_level_operator_case_insensitive(expr, " is not distinct from ").len() == 2
}

fn top_level_keyword_positions<'a>(expr: &str, keywords: &[&'a str]) -> Vec<(usize, &'a str)> {
    let bytes = expr.as_bytes();
    let mut positions = Vec::new();
    let mut depth = 0_i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut idx = 0;

    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' if !in_double => {
                in_single = !in_single;
                idx += 1;
                continue;
            }
            b'"' if !in_single => {
                in_double = !in_double;
                idx += 1;
                continue;
            }
            b'(' if !in_single && !in_double => {
                depth += 1;
                idx += 1;
                continue;
            }
            b')' if !in_single && !in_double => {
                depth -= 1;
                idx += 1;
                continue;
            }
            _ => {}
        }

        if depth == 0 && !in_single && !in_double {
            if let Some(keyword) = keywords
                .iter()
                .copied()
                .find(|keyword| keyword_at(expr, idx, keyword))
            {
                positions.push((idx, keyword));
                idx += keyword.len();
                continue;
            }
        }

        idx += 1;
    }

    positions
}

fn split_top_level_operator<'a>(expr: &'a str, operator: &str) -> Vec<&'a str> {
    let bytes = expr.as_bytes();
    let op = operator.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut idx = 0;

    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'(' if !in_single && !in_double => depth += 1,
            b')' if !in_single && !in_double => depth -= 1,
            _ => {}
        }

        if depth == 0
            && !in_single
            && !in_double
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

fn split_top_level_operator_case_insensitive<'a>(expr: &'a str, operator: &str) -> Vec<&'a str> {
    let lower = expr.to_ascii_lowercase();
    let operator = operator.to_ascii_lowercase();
    let mut parts = Vec::new();
    let bytes = expr.as_bytes();
    let mut start = 0;
    let mut depth = 0_i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut idx = 0;

    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'(' if !in_single && !in_double => depth += 1,
            b')' if !in_single && !in_double => depth -= 1,
            _ => {}
        }

        if depth == 0
            && !in_single
            && !in_double
            && lower
                .get(idx..idx + operator.len())
                .is_some_and(|candidate| candidate == operator)
        {
            parts.push(expr[start..idx].trim());
            idx += operator.len();
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NormalizedSql {
    sql: String,
    param_names: Vec<String>,
}

pub(crate) fn normalize_postgres_params(sql: &str) -> Result<NormalizedSql> {
    let mut out = String::with_capacity(sql.len());
    let mut param_positions = BTreeMap::<String, usize>::new();
    let mut param_names = Vec::<String>::new();

    let mut chars = sql.char_indices().peekable();

    while let Some((start, ch)) = chars.next() {
        match ch {
            '\'' => copy_single_quoted_string(&mut out, &mut chars),
            '"' => copy_double_quoted_identifier(&mut out, &mut chars),
            '$' if copy_dollar_quoted_string(sql, start, &mut out, &mut chars)? => {}
            '-' if peek_char(&mut chars) == Some('-') => {
                out.push('-');
                let (_, second_dash) = chars.next().expect("peeked dash must exist");
                out.push(second_dash);
                copy_line_comment(&mut out, &mut chars);
            }
            '/' if peek_char(&mut chars) == Some('*') => {
                out.push('/');
                let (_, star) = chars.next().expect("peeked star must exist");
                out.push(star);
                copy_block_comment(&mut out, &mut chars)?;
            }
            ':' => {
                let Some(next) = peek_char(&mut chars) else {
                    out.push(':');
                    continue;
                };

                // Leave casts like `value::text` alone.
                if next == ':' {
                    out.push(':');
                    let (_, second_colon) = chars.next().expect("peeked colon must exist");
                    out.push(second_colon);
                    continue;
                }

                if !is_ident_start(next) {
                    out.push(':');
                    continue;
                }

                let name = take_identifier(&mut chars);

                let position = match param_positions.get(&name).copied() {
                    Some(position) => position,
                    None => {
                        let position = param_names.len() + 1;
                        param_positions.insert(name.clone(), position);
                        param_names.push(name);
                        position
                    }
                };

                out.push('$');
                out.push_str(&position.to_string());
            }
            _ => out.push(ch),
        }
    }

    Ok(NormalizedSql {
        sql: out,
        param_names,
    })
}

fn copy_dollar_quoted_string(
    sql: &str,
    start: usize,
    out: &mut String,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
) -> Result<bool> {
    let Some(delimiter) = dollar_quote_delimiter_at(sql, start) else {
        out.push('$');
        return Ok(false);
    };
    let search_start = start + delimiter.len();
    let Some(close_offset) = sql[search_start..].find(delimiter) else {
        return Err(Error::Parse(
            "unterminated dollar-quoted string in SQL".to_string(),
        ));
    };
    let copy_end = search_start + close_offset + delimiter.len();

    out.push_str(&sql[start..copy_end]);
    while chars
        .peek()
        .map(|(offset, _)| *offset < copy_end)
        .unwrap_or(false)
    {
        chars.next();
    }

    Ok(true)
}

fn dollar_quote_delimiter_at(sql: &str, start: usize) -> Option<&str> {
    let bytes = sql.as_bytes();
    if bytes.get(start).copied() != Some(b'$') {
        return None;
    }

    let mut end = start + 1;
    while let Some(byte) = bytes.get(end).copied() {
        if byte == b'$' {
            return Some(&sql[start..=end]);
        }

        if byte.is_ascii_alphanumeric() || byte == b'_' {
            end += 1;
            continue;
        }

        return None;
    }

    None
}

fn peek_char(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) -> Option<char> {
    chars.peek().map(|(_, ch)| *ch)
}

fn take_identifier(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) -> String {
    let mut name = String::new();

    while let Some((_, ch)) = chars.peek().copied() {
        if !is_ident_continue(ch) {
            break;
        }

        name.push(ch);
        chars.next();
    }

    name
}

fn copy_single_quoted_string(
    out: &mut String,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
) {
    out.push('\'');

    while let Some((_, ch)) = chars.next() {
        out.push(ch);

        if ch != '\'' {
            continue;
        }

        // SQL escapes a single quote inside a string as two quotes: ''.
        if peek_char(chars) == Some('\'') {
            let (_, escaped_quote) = chars.next().expect("peeked quote must exist");
            out.push(escaped_quote);
            continue;
        }

        break;
    }
}

fn copy_double_quoted_identifier(
    out: &mut String,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
) {
    out.push('"');

    while let Some((_, ch)) = chars.next() {
        out.push(ch);

        if ch != '"' {
            continue;
        }

        // SQL escapes a double quote inside an identifier as two quotes: "".
        if peek_char(chars) == Some('"') {
            let (_, escaped_quote) = chars.next().expect("peeked quote must exist");
            out.push(escaped_quote);
            continue;
        }

        break;
    }
}

fn copy_line_comment(out: &mut String, chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) {
    while let Some((_, ch)) = chars.next() {
        out.push(ch);

        if ch == '\n' {
            break;
        }
    }
}

fn copy_block_comment(
    out: &mut String,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
) -> Result<()> {
    let mut previous = '\0';

    while let Some((_, ch)) = chars.next() {
        out.push(ch);

        if previous == '*' && ch == '/' {
            return Ok(());
        }

        previous = ch;
    }

    Err(Error::Parse(
        "unterminated block comment in SQL".to_string(),
    ))
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalized(sql: &str) -> NormalizedSql {
        normalize_postgres_params(sql).unwrap()
    }

    #[test]
    fn normalizes_basic_named_params() {
        assert_eq!(
            normalized("WHERE id = :id AND org_id = :org_id"),
            NormalizedSql {
                sql: "WHERE id = $1 AND org_id = $2".to_string(),
                param_names: vec!["id".to_string(), "org_id".to_string()],
            }
        );
    }

    #[test]
    fn reuses_repeated_params() {
        assert_eq!(
            normalized("WHERE id = :id OR parent_id = :id"),
            NormalizedSql {
                sql: "WHERE id = $1 OR parent_id = $1".to_string(),
                param_names: vec!["id".to_string()],
            }
        );
    }

    #[test]
    fn leaves_postgres_casts_intact() {
        assert_eq!(normalized("SELECT :id::bigint").sql, "SELECT $1::bigint");
    }

    #[test]
    fn does_not_rewrite_single_quoted_strings() {
        assert_eq!(
            normalized("SELECT ':id', :id, 'it''s :name'").sql,
            "SELECT ':id', $1, 'it''s :name'"
        );
    }

    #[test]
    fn does_not_rewrite_double_quoted_identifiers() {
        assert_eq!(
            normalized(r#"SELECT ":id", :id, "a "" :name "" b""#).sql,
            r#"SELECT ":id", $1, "a "" :name "" b""#
        );
    }

    #[test]
    fn does_not_rewrite_line_comments() {
        assert_eq!(
            normalized("-- :ignored\nSELECT :id").sql,
            "-- :ignored\nSELECT $1"
        );
    }

    #[test]
    fn does_not_rewrite_block_comments() {
        assert_eq!(
            normalized("/* :ignored */ SELECT :id").sql,
            "/* :ignored */ SELECT $1"
        );
    }

    #[test]
    fn does_not_rewrite_dollar_quoted_strings() {
        assert_eq!(
            normalized("SELECT $$:ignored$$, $tag$ :also_ignored $tag$, :id").sql,
            "SELECT $$:ignored$$, $tag$ :also_ignored $tag$, $1"
        );
    }

    #[test]
    fn rejects_unterminated_dollar_quoted_strings() {
        let err = normalize_postgres_params("SELECT $$:ignored").unwrap_err();
        assert!(err
            .to_string()
            .contains("unterminated dollar-quoted string"));
    }

    #[test]
    fn infers_simple_expression_nullability() {
        let context = PgExpressionContext {
            qualified_columns: BTreeMap::from([
                (("u".to_string(), "email".to_string()), Nullability::NonNull),
                (
                    ("u".to_string(), "parent_id".to_string()),
                    Nullability::Nullable,
                ),
            ]),
            unqualified_columns: BTreeMap::from([
                ("email".to_string(), Some(Nullability::NonNull)),
                ("parent_id".to_string(), Some(Nullability::Nullable)),
            ]),
        };

        assert_eq!(
            infer_expr_nullability("u.email || ''", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("u.parent_id || ''", &context),
            Nullability::Nullable
        );
        assert_eq!(
            infer_expr_nullability("coalesce(u.parent_id, 0)", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("count(*)", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("unknown_fn(u.email)", &context),
            Nullability::Unknown
        );
    }

    #[test]
    fn expression_inference_handles_casts_and_parentheses() {
        let context = PgExpressionContext {
            qualified_columns: BTreeMap::from([(
                ("u".to_string(), "email".to_string()),
                Nullability::NonNull,
            )]),
            unqualified_columns: BTreeMap::from([(
                "email".to_string(),
                Some(Nullability::NonNull),
            )]),
        };

        assert_eq!(
            infer_expr_nullability("(u.email)::text", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("(u.email || '')::text", &context),
            Nullability::NonNull
        );
    }

    #[test]
    fn expression_inference_handles_scalar_subqueries_conservatively() {
        let context = PgExpressionContext {
            qualified_columns: BTreeMap::from([(
                ("u".to_string(), "email".to_string()),
                Nullability::NonNull,
            )]),
            unqualified_columns: BTreeMap::from([(
                "email".to_string(),
                Some(Nullability::NonNull),
            )]),
        };

        assert_eq!(
            infer_expr_nullability(
                "(SELECT count(*) FROM posts p WHERE p.user_id = u.id)",
                &context
            ),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability(
                "(SELECT coalesce(count(*), 0) FROM posts p WHERE p.user_id = u.id)",
                &context
            ),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("(SELECT u.email FROM users u WHERE u.id = 1)", &context),
            Nullability::Nullable
        );
        assert_eq!(
            infer_expr_nullability("(SELECT count(*) FROM posts GROUP BY user_id)", &context),
            Nullability::Nullable
        );
    }

    #[test]
    fn expression_inference_handles_case_nullif_boolean_and_arithmetic() {
        let context = PgExpressionContext {
            qualified_columns: BTreeMap::from([
                (("u".to_string(), "email".to_string()), Nullability::NonNull),
                (
                    ("u".to_string(), "active".to_string()),
                    Nullability::NonNull,
                ),
                (("u".to_string(), "score".to_string()), Nullability::NonNull),
                (
                    ("u".to_string(), "parent_id".to_string()),
                    Nullability::Nullable,
                ),
            ]),
            unqualified_columns: BTreeMap::new(),
        };

        assert_eq!(
            infer_expr_nullability(
                "CASE WHEN u.active THEN u.email ELSE 'inactive@example.com' END",
                &context
            ),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("CASE WHEN u.active THEN u.email END", &context),
            Nullability::Nullable
        );
        assert_eq!(
            infer_expr_nullability(
                "CASE WHEN u.active THEN u.email ELSE u.parent_id::text END",
                &context
            ),
            Nullability::Nullable
        );
        assert_eq!(
            infer_expr_nullability("nullif(u.email, '')", &context),
            Nullability::Nullable
        );
        assert_eq!(
            infer_expr_nullability("u.email = 'a@example.com'", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("u.parent_id = 1", &context),
            Nullability::Nullable
        );
        assert_eq!(
            infer_expr_nullability("u.parent_id = :parent_id", &context),
            Nullability::Nullable
        );
        assert_eq!(
            infer_expr_nullability("u.score = $1", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("u.parent_id IS NULL", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("u.email IS DISTINCT FROM ''", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("u.score BETWEEN 1 AND 10", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("u.parent_id BETWEEN 1 AND 10", &context),
            Nullability::Nullable
        );
        assert_eq!(
            infer_expr_nullability("u.email IN ('a@example.com', 'b@example.com')", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("u.parent_id NOT IN (1, 2)", &context),
            Nullability::Nullable
        );
        assert_eq!(
            infer_expr_nullability("u.email LIKE '%@example.com'", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("u.parent_id::text NOT ILIKE '1%'", &context),
            Nullability::Nullable
        );
        assert_eq!(
            infer_expr_nullability("u.active AND u.email = 'a@example.com'", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("u.parent_id = 1 OR u.active", &context),
            Nullability::Nullable
        );
        assert_eq!(
            infer_expr_nullability("NOT u.active", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("u.score + 1", &context),
            Nullability::NonNull
        );
        assert_eq!(
            infer_expr_nullability("u.parent_id + 1", &context),
            Nullability::Nullable
        );
    }

    #[test]
    fn builds_synthetic_projection_columns() {
        let context = PgExpressionContext {
            qualified_columns: BTreeMap::from([
                (("u".to_string(), "email".to_string()), Nullability::NonNull),
                (
                    ("u".to_string(), "parent_id".to_string()),
                    Nullability::Nullable,
                ),
            ]),
            unqualified_columns: BTreeMap::new(),
        };

        assert_eq!(
            synthetic_projection_column(
                &SelectProjection {
                    expr: "u.email || ''".to_string(),
                    alias: Some("email_expr".to_string()),
                },
                &context,
            ),
            Some(("email_expr".to_string(), Nullability::NonNull))
        );
        assert_eq!(
            synthetic_projection_column(
                &SelectProjection {
                    expr: "u.parent_id".to_string(),
                    alias: None,
                },
                &context,
            ),
            Some(("parent_id".to_string(), Nullability::Nullable))
        );
    }

    #[test]
    fn applies_declared_cte_column_names_to_synthetic_columns() {
        let columns = apply_declared_column_names(
            vec![
                ("id".to_string(), Nullability::NonNull),
                ("parent_id".to_string(), Nullability::Nullable),
                ("label".to_string(), Nullability::NonNull),
            ],
            &[
                "node_id".to_string(),
                "parent_node_id".to_string(),
                "node_label".to_string(),
            ],
        );

        assert_eq!(
            columns,
            vec![
                ("node_id".to_string(), Nullability::NonNull),
                ("parent_node_id".to_string(), Nullability::Nullable),
                ("node_label".to_string(), Nullability::NonNull),
            ]
        );
    }

    #[test]
    fn merges_compound_branch_nullability_conservatively() {
        let mut columns = vec![
            ("id".to_string(), Nullability::NonNull),
            ("maybe_parent_id".to_string(), Nullability::NonNull),
            ("unknown_expr".to_string(), Nullability::NonNull),
        ];
        let branch_columns = vec![
            ("id".to_string(), Nullability::NonNull),
            ("parent_id".to_string(), Nullability::Nullable),
            ("expr".to_string(), Nullability::Unknown),
        ];

        merge_compound_columns(&mut columns, &branch_columns);

        assert_eq!(
            columns,
            vec![
                ("id".to_string(), Nullability::NonNull),
                ("maybe_parent_id".to_string(), Nullability::Nullable),
                ("unknown_expr".to_string(), Nullability::Unknown),
            ]
        );
    }

    #[test]
    fn detects_nullable_tables_from_outer_joins() {
        assert_eq!(
            nullable_join_tables(
                sql_ir::parse_select(
                    "SELECT u.id, o.name FROM users u LEFT JOIN organizations o ON o.id = u.org_id"
                )
                .as_ref()
            ),
            BTreeSet::from(["organizations".to_string()])
        );
        assert_eq!(
            nullable_join_tables(sql_ir::parse_select(
                "SELECT u.id, o.name FROM users u RIGHT JOIN public.organizations o ON o.id = u.org_id"
            )
            .as_ref()),
            BTreeSet::from(["users".to_string()])
        );
        assert_eq!(
            nullable_join_tables(sql_ir::parse_select(
                "SELECT u.id, o.name FROM public.users u FULL OUTER JOIN organizations o ON o.id = u.org_id"
            )
            .as_ref()),
            BTreeSet::from(["organizations".to_string(), "users".to_string()])
        );
        assert_eq!(
            nullable_join_tables(
                sql_ir::parse_select(
                    "SELECT u.id, recent.email \
                     FROM users u \
                     LEFT JOIN LATERAL (SELECT email FROM emails e WHERE e.user_id = u.id LIMIT 1) recent ON true"
                )
                .as_ref()
            ),
            BTreeSet::from(["recent".to_string()])
        );
        assert_eq!(
            nullable_join_tables(
                sql_ir::parse_select(
                    "SELECT u.id, o.name, a.slug \
                     FROM users u \
                     LEFT JOIN (organizations o JOIN accounts a ON a.org_id = o.id) ON o.id = u.org_id"
                )
                .as_ref()
            ),
            BTreeSet::from(["accounts".to_string(), "organizations".to_string()])
        );
        assert_eq!(
            nullable_join_tables(
                sql_ir::parse_select(
                    "SELECT u.id, o.name, a.slug \
                     FROM users u \
                     LEFT JOIN (organizations o, accounts a) ON o.id = u.org_id AND a.org_id = o.id"
                )
                .as_ref()
            ),
            BTreeSet::from(["accounts".to_string(), "organizations".to_string()])
        );
        assert_eq!(
            nullable_join_tables(
                sql_ir::parse_select(
                    "SELECT u.id, o.name, a.slug \
                     FROM (users u LEFT JOIN organizations o ON o.id = u.org_id) \
                     RIGHT JOIN accounts a ON a.org_id = o.id"
                )
                .as_ref()
            ),
            BTreeSet::from(["organizations".to_string(), "users".to_string()])
        );
        assert_eq!(
            nullable_join_tables(
                sql_ir::parse_select(
                    "SELECT u.id, o.name, a.slug, m.role \
                     FROM users u \
                     LEFT JOIN (organizations o RIGHT JOIN accounts a ON a.org_id = o.id \
                       LEFT JOIN memberships m ON m.account_id = a.id) ON o.id = u.org_id"
                )
                .as_ref()
            ),
            BTreeSet::from([
                "accounts".to_string(),
                "memberships".to_string(),
                "organizations".to_string()
            ])
        );
    }
}
