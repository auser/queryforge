use nom::bytes::complete::{tag_no_case, take_while, take_while1};
use nom::character::complete::{char, multispace0, multispace1};
use nom::combinator::{map, opt, recognize};
use nom::sequence::delimited;
use nom::{IResult, Parser};
use sqlparser::ast::{
    AssignmentTarget, BinaryOperator, ColumnDef as AstColumnDef, ColumnOption, ConnectByKind,
    CreateTable, Delete, Expr, FromTable, FunctionArg, FunctionArgExpr, FunctionArgumentClause,
    FunctionArguments, GroupByExpr, Insert, JoinConstraint, JoinOperator, LimitClause, ObjectName,
    ObjectNamePart, OrderBy, OrderByKind, Query, Select, SelectItem,
    SelectItemQualifiedWildcardKind, SetExpr, SetOperator, SetQuantifier,
    Statement as AstStatement, TableAlias, TableConstraint, TableFactor, TableObject,
    TableWithJoins, TopQuantity, Update, Value,
};
use sqlparser::dialect::{PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser as SqlParser;

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlStatement {
    CreateTable(CreateTableStatement),
    Select(SelectStatement),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTableStatement {
    pub table: String,
    pub columns: Vec<ColumnDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDefinition {
    pub name: String,
    pub declared_type: String,
    pub nullable: ColumnNullability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnNullability {
    NonNull,
    Nullable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectStatement {
    pub ctes: Vec<CommonTableExpression>,
    pub projections: Vec<SelectProjection>,
    pub table: String,
    pub table_refs: Vec<TableReference>,
    pub equality_params: Vec<EqualityParam>,
    pub compound: Vec<CompoundSelect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonTableExpression {
    pub name: String,
    pub columns: Vec<String>,
    pub query: String,
    pub recursive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompoundSelect {
    pub operator: CompoundOperator,
    pub query: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundOperator {
    Union,
    UnionAll,
    Intersect,
    Except,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableReference {
    pub name: String,
    pub alias: Option<String>,
    pub derived_query: Option<String>,
    pub lateral: bool,
    pub nullable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectProjection {
    pub expr: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EqualityParam {
    pub qualifier: Option<String>,
    pub column: String,
    pub param: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationStatement {
    pub table: String,
    pub column_params: Vec<MutationColumnParam>,
    pub equality_params: Vec<EqualityParam>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationColumnParam {
    pub column: String,
    pub param: String,
}

pub fn parse_statements(sql: &str) -> Result<Vec<SqlStatement>> {
    split_sql_statements(sql)
        .into_iter()
        .map(parse_statement)
        .collect()
}

pub fn parse_statement(sql: &str) -> Result<SqlStatement> {
    if let Some(statement) = parse_create_table(sql)? {
        return Ok(SqlStatement::CreateTable(statement));
    }

    if let Some(statement) = parse_select(sql) {
        return Ok(SqlStatement::Select(statement));
    }

    Ok(SqlStatement::Other)
}

pub fn parse_create_table(sql: &str) -> Result<Option<CreateTableStatement>> {
    if let Some(statement) = parse_create_table_with_sqlparser(sql) {
        return Ok(Some(statement));
    }

    parse_create_table_heuristic(sql)
}

fn parse_create_table_with_sqlparser(sql: &str) -> Option<CreateTableStatement> {
    let cleaned = trim_trailing_semicolon(sql.trim());
    let statement = parse_sqlparser_statement(cleaned)?;
    let AstStatement::CreateTable(create_table) = statement else {
        return None;
    };
    Some(lower_create_table(&create_table))
}

fn lower_create_table(create_table: &CreateTable) -> CreateTableStatement {
    let primary_key_columns = create_table
        .constraints
        .iter()
        .filter_map(|constraint| match constraint {
            TableConstraint::PrimaryKey(primary_key) => Some(primary_key),
            _ => None,
        })
        .flat_map(|primary_key| primary_key.columns.iter())
        .filter_map(|column| column_name_from_expr(&column.column.expr))
        .map(|(_, column)| normalize_sqlparser_ident(&column))
        .collect::<std::collections::BTreeSet<_>>();

    let columns = create_table
        .columns
        .iter()
        .map(|column| lower_column_definition(column, &primary_key_columns))
        .collect();

    CreateTableStatement {
        table: object_name_to_string(&create_table.name),
        columns,
    }
}

fn lower_column_definition(
    column: &AstColumnDef,
    primary_key_columns: &std::collections::BTreeSet<String>,
) -> ColumnDefinition {
    let is_primary_key = primary_key_columns
        .contains(&normalize_sqlparser_ident(&column.name.value))
        || column
            .options
            .iter()
            .any(|option| matches!(option.option, ColumnOption::PrimaryKey(_)));
    let is_not_null = column
        .options
        .iter()
        .any(|option| matches!(option.option, ColumnOption::NotNull));

    ColumnDefinition {
        name: column.name.value.clone(),
        declared_type: column.data_type.to_string(),
        nullable: if is_primary_key || is_not_null {
            ColumnNullability::NonNull
        } else {
            ColumnNullability::Nullable
        },
    }
}

fn normalize_sqlparser_ident(value: &str) -> String {
    value.to_ascii_lowercase()
}

fn parse_create_table_heuristic(sql: &str) -> Result<Option<CreateTableStatement>> {
    let trimmed = sql.trim();
    let Ok((rest, table)) = create_table_prefix(trimmed) else {
        return Ok(None);
    };
    let rest_start = trimmed.len() - rest.len();
    let Some(open_paren) =
        find_top_level_char(&trimmed[rest_start..], '(').map(|idx| rest_start + idx)
    else {
        return Err(Error::Parse(format!(
            "CREATE TABLE statement has no column list: {trimmed}"
        )));
    };
    let Some(close_paren) = find_matching_paren(trimmed, open_paren) else {
        return Err(Error::Parse(format!(
            "CREATE TABLE statement has an unterminated column list: {trimmed}"
        )));
    };

    let columns = split_comma_separated(&trimmed[open_paren + 1..close_paren])
        .into_iter()
        .filter_map(parse_column_definition)
        .collect();

    Ok(Some(CreateTableStatement { table, columns }))
}

pub fn parse_select(sql: &str) -> Option<SelectStatement> {
    parse_select_with_sqlparser(sql).or_else(|| parse_select_heuristic(sql))
}

pub fn parse_mutation(sql: &str) -> Option<MutationStatement> {
    let cleaned = trim_trailing_semicolon(sql.trim());
    match parse_sqlparser_statement(cleaned)? {
        AstStatement::Insert(insert) => lower_insert_mutation(&insert),
        AstStatement::Update(update) => lower_update_mutation(&update),
        AstStatement::Delete(delete) => lower_delete_mutation(&delete),
        _ => None,
    }
}

fn lower_insert_mutation(insert: &Insert) -> Option<MutationStatement> {
    let table = match &insert.table {
        TableObject::TableName(name) => object_name_to_string(name),
        TableObject::TableFunction(_) | TableObject::TableQuery(_) => return None,
    };
    let mut column_params = Vec::new();

    for assignment in &insert.assignments {
        let AssignmentTarget::ColumnName(column) = &assignment.target else {
            continue;
        };
        if let Some(param) = named_param_from_expr(&assignment.value) {
            column_params.push(MutationColumnParam {
                column: last_object_name_part(column)?,
                param,
            });
        }
    }

    if let Some(source) = &insert.source {
        if let SetExpr::Values(values) = &*source.body {
            if let Some(row) = values.rows.first() {
                for (column, value) in insert.columns.iter().zip(row.iter()) {
                    if let Some(param) = named_param_from_expr(value) {
                        column_params.push(MutationColumnParam {
                            column: last_object_name_part(column)?,
                            param,
                        });
                    }
                }
            }
        }
    }

    Some(MutationStatement {
        table,
        column_params,
        equality_params: Vec::new(),
    })
}

fn lower_update_mutation(update: &Update) -> Option<MutationStatement> {
    let table = mutation_table_from_table_factor(&update.table.relation)?;
    let mut column_params = Vec::new();

    for assignment in &update.assignments {
        match &assignment.target {
            AssignmentTarget::ColumnName(column) => {
                if let Some(param) = named_param_from_expr(&assignment.value) {
                    column_params.push(MutationColumnParam {
                        column: last_object_name_part(column)?,
                        param,
                    });
                }
            }
            AssignmentTarget::Tuple(columns) => {
                let Expr::Tuple(values) = &assignment.value else {
                    continue;
                };
                for (column, value) in columns.iter().zip(values.iter()) {
                    if let Some(param) = named_param_from_expr(value) {
                        column_params.push(MutationColumnParam {
                            column: last_object_name_part(column)?,
                            param,
                        });
                    }
                }
            }
        }
    }

    let mut equality_params = Vec::new();
    if let Some(selection) = &update.selection {
        equality_params = infer_equality_param_pairs_from_expr(selection);
    }

    Some(MutationStatement {
        table,
        column_params,
        equality_params,
    })
}

fn lower_delete_mutation(delete: &Delete) -> Option<MutationStatement> {
    let table = match &delete.from {
        FromTable::WithFromKeyword(tables) | FromTable::WithoutKeyword(tables) => {
            let first = tables.first()?;
            mutation_table_from_table_factor(&first.relation)?
        }
    };

    let mut equality_params = Vec::new();
    if let Some(selection) = &delete.selection {
        equality_params = infer_equality_param_pairs_from_expr(selection);
    }

    Some(MutationStatement {
        table,
        column_params: Vec::new(),
        equality_params,
    })
}

fn mutation_table_from_table_factor(table_factor: &TableFactor) -> Option<String> {
    match table_factor {
        TableFactor::Table { name, .. } => Some(object_name_to_string(name)),
        _ => None,
    }
}

fn last_object_name_part(name: &ObjectName) -> Option<String> {
    name.0.last().map(|part| match part {
        ObjectNamePart::Identifier(ident) => ident.value.clone(),
        ObjectNamePart::Function(function) => function.to_string(),
    })
}

fn parse_select_with_sqlparser(sql: &str) -> Option<SelectStatement> {
    let cleaned = trim_trailing_semicolon(sql.trim());
    let statement = parse_sqlparser_statement(cleaned)?;
    let AstStatement::Query(query) = statement else {
        return None;
    };
    lower_query(&query)
}

fn parse_sqlparser_statement(sql: &str) -> Option<AstStatement> {
    let postgres = PostgreSqlDialect {};
    let sqlite = SQLiteDialect {};

    for dialect in [
        &postgres as &dyn sqlparser::dialect::Dialect,
        &sqlite as &dyn sqlparser::dialect::Dialect,
    ] {
        let Ok(mut statements) = SqlParser::parse_sql(dialect, sql) else {
            continue;
        };
        if statements.len() == 1 {
            return statements.pop();
        }
    }

    None
}

fn lower_query(query: &Query) -> Option<SelectStatement> {
    let ctes = query
        .with
        .as_ref()
        .map(|with| {
            with.cte_tables
                .iter()
                .map(|cte| CommonTableExpression {
                    name: cte.alias.name.value.clone(),
                    columns: cte
                        .alias
                        .columns
                        .iter()
                        .map(|column| column.name.value.clone())
                        .collect(),
                    query: cte.query.to_string(),
                    recursive: with.recursive,
                })
                .collect()
        })
        .unwrap_or_default();

    let (select, compound) = lower_set_expr(&query.body)?;
    let projections = select.projection.iter().map(lower_select_item).collect();
    let table_refs = lower_table_with_joins(&select.from);
    let table = table_refs
        .first()
        .map(|table_ref| table_ref.name.clone())
        .unwrap_or_default();
    let equality_params = if compound.is_empty() {
        infer_equality_param_pairs_from_query(query, true)
    } else {
        infer_equality_param_pairs_from_select(select)
    };

    Some(SelectStatement {
        ctes,
        projections,
        table,
        table_refs,
        equality_params,
        compound,
    })
}

fn infer_equality_param_pairs_from_query(
    query: &Query,
    include_compound_branches: bool,
) -> Vec<EqualityParam> {
    if !include_compound_branches {
        return lower_set_expr(&query.body)
            .map(|(select, _)| infer_equality_param_pairs_from_select(select))
            .unwrap_or_default();
    }

    let mut visitor = EqualityParamVisitor::default();
    walk_query(query, include_compound_branches, &mut visitor);
    visitor.pairs
}

fn infer_equality_param_pairs_from_select(select: &Select) -> Vec<EqualityParam> {
    let mut visitor = EqualityParamVisitor::default();
    walk_select(select, &mut visitor);
    visitor.pairs
}

fn infer_equality_param_pairs_from_expr(expr: &Expr) -> Vec<EqualityParam> {
    let mut visitor = EqualityParamVisitor::default();
    walk_expr(expr, &mut visitor);
    visitor.pairs
}

#[derive(Default)]
struct EqualityParamVisitor {
    pairs: Vec<EqualityParam>,
}

impl AstVisitor for EqualityParamVisitor {
    fn enter_expr(&mut self, expr: &Expr) {
        collect_equality_param_pairs_from_expr(expr, &mut self.pairs);
    }
}

trait AstVisitor {
    fn enter_query(&mut self, _query: &Query) {}
    fn enter_select(&mut self, _select: &Select) {}
    fn enter_table_factor(&mut self, _table_factor: &TableFactor) {}
    fn enter_expr(&mut self, _expr: &Expr) {}
}

fn walk_query<V: AstVisitor + ?Sized>(
    query: &Query,
    include_compound_branches: bool,
    visitor: &mut V,
) {
    visitor.enter_query(query);

    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            walk_query(&cte.query, true, visitor);
        }
    }

    walk_set_expr(&query.body, include_compound_branches, visitor);

    if let Some(order_by) = &query.order_by {
        walk_order_by(order_by, visitor);
    }
    if let Some(limit_clause) = &query.limit_clause {
        walk_limit_clause(limit_clause, visitor);
    }
    if let Some(fetch) = &query.fetch {
        if let Some(quantity) = &fetch.quantity {
            walk_expr(quantity, visitor);
        }
    }
}

fn walk_set_expr<V: AstVisitor + ?Sized>(
    set_expr: &SetExpr,
    include_compound_branches: bool,
    visitor: &mut V,
) {
    match set_expr {
        SetExpr::Select(select) => walk_select(select, visitor),
        SetExpr::Query(query) => walk_query(query, include_compound_branches, visitor),
        SetExpr::SetOperation { left, right, .. } => {
            walk_set_expr(left, include_compound_branches, visitor);
            if include_compound_branches {
                walk_set_expr(right, include_compound_branches, visitor);
            }
        }
        SetExpr::Values(values) => {
            for row in &values.rows {
                for expr in row.iter() {
                    walk_expr(expr, visitor);
                }
            }
        }
        SetExpr::Insert(_)
        | SetExpr::Update(_)
        | SetExpr::Delete(_)
        | SetExpr::Merge(_)
        | SetExpr::Table(_) => {}
    }
}

fn walk_select<V: AstVisitor + ?Sized>(select: &Select, visitor: &mut V) {
    visitor.enter_select(select);

    if let Some(top) = &select.top {
        if let Some(TopQuantity::Expr(quantity)) = &top.quantity {
            walk_expr(quantity, visitor);
        }
    }

    for projection in &select.projection {
        match projection {
            SelectItem::UnnamedExpr(expr)
            | SelectItem::ExprWithAlias { expr, .. }
            | SelectItem::ExprWithAliases { expr, .. } => walk_expr(expr, visitor),
            SelectItem::QualifiedWildcard(_, _) | SelectItem::Wildcard(_) => {}
        }
    }

    for table_with_joins in &select.from {
        walk_table_with_joins(table_with_joins, visitor);
    }
    for lateral_view in &select.lateral_views {
        walk_expr(&lateral_view.lateral_view, visitor);
    }
    if let Some(prewhere) = &select.prewhere {
        walk_expr(prewhere, visitor);
    }
    if let Some(selection) = &select.selection {
        walk_expr(selection, visitor);
    }
    for connect_by in &select.connect_by {
        match connect_by {
            ConnectByKind::ConnectBy { relationships, .. } => {
                for relationship in relationships {
                    walk_expr(relationship, visitor);
                }
            }
            ConnectByKind::StartWith { condition, .. } => walk_expr(condition, visitor),
        }
    }
    walk_group_by(&select.group_by, visitor);
    for expr in &select.cluster_by {
        walk_expr(expr, visitor);
    }
    for expr in &select.distribute_by {
        walk_expr(expr, visitor);
    }
    for order_by in &select.sort_by {
        walk_order_by_expr(order_by, visitor);
    }
    if let Some(having) = &select.having {
        walk_expr(having, visitor);
    }
    if let Some(qualify) = &select.qualify {
        walk_expr(qualify, visitor);
    }
}

fn walk_table_with_joins<V: AstVisitor + ?Sized>(
    table_with_joins: &TableWithJoins,
    visitor: &mut V,
) {
    walk_table_factor(&table_with_joins.relation, visitor);
    for join in &table_with_joins.joins {
        walk_table_factor(&join.relation, visitor);
        walk_join_constraint(&join.join_operator, visitor);
    }
}

fn walk_table_factor<V: AstVisitor + ?Sized>(table_factor: &TableFactor, visitor: &mut V) {
    visitor.enter_table_factor(table_factor);

    match table_factor {
        TableFactor::Table { with_hints, .. } => {
            for hint in with_hints {
                walk_expr(hint, visitor);
            }
        }
        TableFactor::Derived { subquery, .. } => walk_query(subquery, true, visitor),
        TableFactor::TableFunction { expr, .. } => walk_expr(expr, visitor),
        TableFactor::Function { args, .. } => {
            for arg in args {
                walk_function_arg(arg, visitor);
            }
        }
        TableFactor::UNNEST { array_exprs, .. } => {
            for expr in array_exprs {
                walk_expr(expr, visitor);
            }
        }
        TableFactor::JsonTable { json_expr, .. } | TableFactor::OpenJsonTable { json_expr, .. } => {
            walk_expr(json_expr, visitor)
        }
        TableFactor::NestedJoin {
            table_with_joins, ..
        } => walk_table_with_joins(table_with_joins, visitor),
        TableFactor::Pivot {
            table,
            aggregate_functions,
            value_column,
            default_on_null,
            ..
        } => {
            walk_table_factor(table, visitor);
            for expr_with_alias in aggregate_functions {
                walk_expr(&expr_with_alias.expr, visitor);
            }
            for expr in value_column {
                walk_expr(expr, visitor);
            }
            if let Some(default_on_null) = default_on_null {
                walk_expr(default_on_null, visitor);
            }
        }
        TableFactor::Unpivot {
            table,
            value,
            columns,
            ..
        } => {
            walk_table_factor(table, visitor);
            walk_expr(value, visitor);
            for expr_with_alias in columns {
                walk_expr(&expr_with_alias.expr, visitor);
            }
        }
        TableFactor::MatchRecognize {
            table,
            partition_by,
            order_by,
            measures,
            symbols,
            ..
        } => {
            walk_table_factor(table, visitor);
            for expr in partition_by {
                walk_expr(expr, visitor);
            }
            for order_by in order_by {
                walk_order_by_expr(order_by, visitor);
            }
            for measure in measures {
                walk_expr(&measure.expr, visitor);
            }
            for symbol in symbols {
                walk_expr(&symbol.definition, visitor);
            }
        }
        TableFactor::XmlTable { row_expression, .. } => walk_expr(row_expression, visitor),
        TableFactor::SemanticView {
            dimensions,
            metrics,
            facts,
            where_clause,
            ..
        } => {
            for expr in dimensions {
                walk_expr(expr, visitor);
            }
            for expr in metrics {
                walk_expr(expr, visitor);
            }
            for expr in facts {
                walk_expr(expr, visitor);
            }
            if let Some(where_clause) = where_clause {
                walk_expr(where_clause, visitor);
            }
        }
    }
}

fn walk_join_constraint<V: AstVisitor + ?Sized>(operator: &JoinOperator, visitor: &mut V) {
    let constraint = match operator {
        JoinOperator::Join(constraint)
        | JoinOperator::Inner(constraint)
        | JoinOperator::Left(constraint)
        | JoinOperator::LeftOuter(constraint)
        | JoinOperator::Right(constraint)
        | JoinOperator::RightOuter(constraint)
        | JoinOperator::FullOuter(constraint)
        | JoinOperator::CrossJoin(constraint)
        | JoinOperator::Semi(constraint)
        | JoinOperator::LeftSemi(constraint)
        | JoinOperator::RightSemi(constraint)
        | JoinOperator::Anti(constraint)
        | JoinOperator::LeftAnti(constraint)
        | JoinOperator::RightAnti(constraint)
        | JoinOperator::StraightJoin(constraint)
        | JoinOperator::AsOf { constraint, .. } => constraint,
        JoinOperator::CrossApply
        | JoinOperator::OuterApply
        | JoinOperator::ArrayJoin
        | JoinOperator::LeftArrayJoin
        | JoinOperator::InnerArrayJoin => return,
    };

    if let JoinConstraint::On(expr) = constraint {
        walk_expr(expr, visitor);
    }
}

fn walk_group_by<V: AstVisitor + ?Sized>(group_by: &GroupByExpr, visitor: &mut V) {
    match group_by {
        GroupByExpr::All(_) => {}
        GroupByExpr::Expressions(exprs, modifiers) => {
            for expr in exprs {
                walk_expr(expr, visitor);
            }
            for modifier in modifiers {
                if let sqlparser::ast::GroupByWithModifier::GroupingSets(expr) = modifier {
                    walk_expr(expr, visitor);
                }
            }
        }
    }
}

fn walk_order_by<V: AstVisitor + ?Sized>(order_by: &OrderBy, visitor: &mut V) {
    if let OrderByKind::Expressions(exprs) = &order_by.kind {
        for expr in exprs {
            walk_order_by_expr(expr, visitor);
        }
    }
}

fn walk_order_by_expr<V: AstVisitor + ?Sized>(
    order_by: &sqlparser::ast::OrderByExpr,
    visitor: &mut V,
) {
    walk_expr(&order_by.expr, visitor);
}

fn walk_limit_clause<V: AstVisitor + ?Sized>(limit_clause: &LimitClause, visitor: &mut V) {
    match limit_clause {
        LimitClause::LimitOffset {
            limit,
            offset,
            limit_by,
        } => {
            if let Some(limit) = limit {
                walk_expr(limit, visitor);
            }
            if let Some(offset) = offset {
                walk_expr(&offset.value, visitor);
            }
            for expr in limit_by {
                walk_expr(expr, visitor);
            }
        }
        LimitClause::OffsetCommaLimit { offset, limit } => {
            walk_expr(offset, visitor);
            walk_expr(limit, visitor);
        }
    }
}

fn walk_function_arguments<V: AstVisitor + ?Sized>(arguments: &FunctionArguments, visitor: &mut V) {
    match arguments {
        FunctionArguments::None => {}
        FunctionArguments::Subquery(query) => walk_query(query, true, visitor),
        FunctionArguments::List(arguments) => {
            for arg in &arguments.args {
                walk_function_arg(arg, visitor);
            }
            for clause in &arguments.clauses {
                match clause {
                    FunctionArgumentClause::OrderBy(order_by) => {
                        for order_by in order_by {
                            walk_order_by_expr(order_by, visitor);
                        }
                    }
                    FunctionArgumentClause::Limit(limit) => walk_expr(limit, visitor),
                    FunctionArgumentClause::Having(bound) => walk_expr(&bound.1, visitor),
                    FunctionArgumentClause::IgnoreOrRespectNulls(_)
                    | FunctionArgumentClause::OnOverflow(_)
                    | FunctionArgumentClause::Separator(_)
                    | FunctionArgumentClause::JsonNullClause(_)
                    | FunctionArgumentClause::JsonReturningClause(_) => {}
                }
            }
        }
    }
}

fn walk_function_arg<V: AstVisitor + ?Sized>(arg: &FunctionArg, visitor: &mut V) {
    match arg {
        FunctionArg::Named { arg, .. } | FunctionArg::Unnamed(arg) => {
            walk_function_arg_expr(arg, visitor);
        }
        FunctionArg::ExprNamed { name, arg, .. } => {
            walk_expr(name, visitor);
            walk_function_arg_expr(arg, visitor);
        }
    }
}

fn walk_function_arg_expr<V: AstVisitor + ?Sized>(arg: &FunctionArgExpr, visitor: &mut V) {
    if let FunctionArgExpr::Expr(expr) = arg {
        walk_expr(expr, visitor);
    }
}

fn walk_expr<V: AstVisitor + ?Sized>(expr: &Expr, visitor: &mut V) {
    visitor.enter_expr(expr);

    match expr {
        Expr::CompoundFieldAccess { root, access_chain } => {
            walk_expr(root, visitor);
            for access in access_chain {
                match access {
                    sqlparser::ast::AccessExpr::Dot(expr) => walk_expr(expr, visitor),
                    sqlparser::ast::AccessExpr::Subscript(subscript) => {
                        walk_subscript(subscript, visitor);
                    }
                }
            }
        }
        Expr::JsonAccess { value, .. } => walk_expr(value, visitor),
        Expr::IsFalse(expr)
        | Expr::IsNotFalse(expr)
        | Expr::IsTrue(expr)
        | Expr::IsNotTrue(expr)
        | Expr::IsNull(expr)
        | Expr::IsNotNull(expr)
        | Expr::IsUnknown(expr)
        | Expr::IsNotUnknown(expr)
        | Expr::IsNormalized { expr, .. }
        | Expr::Nested(expr)
        | Expr::UnaryOp { expr, .. }
        | Expr::Convert { expr, .. }
        | Expr::Cast { expr, .. }
        | Expr::Extract { expr, .. }
        | Expr::Ceil { expr, .. }
        | Expr::Floor { expr, .. }
        | Expr::Collate { expr, .. }
        | Expr::Prefixed { value: expr, .. }
        | Expr::Named { expr, .. }
        | Expr::OuterJoin(expr)
        | Expr::Prior(expr) => walk_expr(expr, visitor),
        Expr::IsDistinctFrom(left, right)
        | Expr::IsNotDistinctFrom(left, right)
        | Expr::BinaryOp { left, right, .. }
        | Expr::AnyOp { left, right, .. }
        | Expr::AllOp { left, right, .. } => {
            walk_expr(left, visitor);
            walk_expr(right, visitor);
        }
        Expr::InList { expr, list, .. } => {
            walk_expr(expr, visitor);
            for item in list {
                walk_expr(item, visitor);
            }
        }
        Expr::InSubquery { expr, subquery, .. } => {
            walk_expr(expr, visitor);
            walk_query(subquery, true, visitor);
        }
        Expr::InUnnest {
            expr, array_expr, ..
        } => {
            walk_expr(expr, visitor);
            walk_expr(array_expr, visitor);
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            walk_expr(expr, visitor);
            walk_expr(low, visitor);
            walk_expr(high, visitor);
        }
        Expr::Like { expr, pattern, .. }
        | Expr::ILike { expr, pattern, .. }
        | Expr::SimilarTo { expr, pattern, .. }
        | Expr::RLike { expr, pattern, .. } => {
            walk_expr(expr, visitor);
            walk_expr(pattern, visitor);
        }
        Expr::AtTimeZone {
            timestamp,
            time_zone,
        } => {
            walk_expr(timestamp, visitor);
            walk_expr(time_zone, visitor);
        }
        Expr::Position { expr, r#in } => {
            walk_expr(expr, visitor);
            walk_expr(r#in, visitor);
        }
        Expr::Substring {
            expr,
            substring_from,
            substring_for,
            ..
        } => {
            walk_expr(expr, visitor);
            if let Some(substring_from) = substring_from {
                walk_expr(substring_from, visitor);
            }
            if let Some(substring_for) = substring_for {
                walk_expr(substring_for, visitor);
            }
        }
        Expr::Trim {
            trim_what,
            expr,
            trim_characters,
            ..
        } => {
            if let Some(trim_what) = trim_what {
                walk_expr(trim_what, visitor);
            }
            walk_expr(expr, visitor);
            if let Some(trim_characters) = trim_characters {
                for trim_character in trim_characters {
                    walk_expr(trim_character, visitor);
                }
            }
        }
        Expr::Overlay {
            expr,
            overlay_what,
            overlay_from,
            overlay_for,
        } => {
            walk_expr(expr, visitor);
            walk_expr(overlay_what, visitor);
            walk_expr(overlay_from, visitor);
            if let Some(overlay_for) = overlay_for {
                walk_expr(overlay_for, visitor);
            }
        }
        Expr::Function(function) => {
            walk_function_arguments(&function.parameters, visitor);
            walk_function_arguments(&function.args, visitor);
            if let Some(filter) = &function.filter {
                walk_expr(filter, visitor);
            }
            for order_by in &function.within_group {
                walk_order_by_expr(order_by, visitor);
            }
        }
        Expr::Case {
            operand,
            conditions,
            else_result,
            ..
        } => {
            if let Some(operand) = operand {
                walk_expr(operand, visitor);
            }
            for condition in conditions {
                walk_expr(&condition.condition, visitor);
                walk_expr(&condition.result, visitor);
            }
            if let Some(else_result) = else_result {
                walk_expr(else_result, visitor);
            }
        }
        Expr::Exists { subquery, .. } | Expr::Subquery(subquery) => {
            walk_query(subquery, true, visitor);
        }
        Expr::GroupingSets(groups) | Expr::Cube(groups) | Expr::Rollup(groups) => {
            for group in groups {
                for item in group {
                    walk_expr(item, visitor);
                }
            }
        }
        Expr::Tuple(items) => {
            for item in items {
                walk_expr(item, visitor);
            }
        }
        Expr::Struct { values, .. } => {
            for value in values {
                walk_expr(value, visitor);
            }
        }
        Expr::Dictionary(fields) => {
            for field in fields {
                walk_expr(&field.value, visitor);
            }
        }
        Expr::Map(map) => {
            for entry in &map.entries {
                walk_expr(&entry.key, visitor);
                walk_expr(&entry.value, visitor);
            }
        }
        Expr::Array(array) => {
            for elem in &array.elem {
                walk_expr(elem, visitor);
            }
        }
        Expr::Interval(interval) => {
            walk_expr(&interval.value, visitor);
        }
        Expr::Lambda(lambda) => walk_expr(&lambda.body, visitor),
        Expr::MemberOf(member_of) => {
            walk_expr(&member_of.value, visitor);
            walk_expr(&member_of.array, visitor);
        }
        Expr::Identifier(_)
        | Expr::CompoundIdentifier(_)
        | Expr::Value(_)
        | Expr::TypedString(_)
        | Expr::MatchAgainst { .. }
        | Expr::Wildcard(_)
        | Expr::QualifiedWildcard(_, _) => {}
    }
}

fn walk_subscript<V: AstVisitor + ?Sized>(subscript: &sqlparser::ast::Subscript, visitor: &mut V) {
    match subscript {
        sqlparser::ast::Subscript::Index { index } => walk_expr(index, visitor),
        sqlparser::ast::Subscript::Slice {
            lower_bound,
            upper_bound,
            stride,
        } => {
            if let Some(lower_bound) = lower_bound {
                walk_expr(lower_bound, visitor);
            }
            if let Some(upper_bound) = upper_bound {
                walk_expr(upper_bound, visitor);
            }
            if let Some(stride) = stride {
                walk_expr(stride, visitor);
            }
        }
    }
}

fn collect_equality_param_pairs_from_expr(expr: &Expr, pairs: &mut Vec<EqualityParam>) {
    match expr {
        Expr::BinaryOp { left, op, right } => {
            if *op == BinaryOperator::Eq {
                collect_equality_param_pairs_from_exprs(left, right, pairs);
            }
        }
        Expr::InList { expr, list, .. } => {
            collect_in_list_param_pairs(expr, list, pairs);
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            collect_between_param_pairs(expr, low, high, pairs);
        }
        Expr::Like { expr, pattern, .. }
        | Expr::ILike { expr, pattern, .. }
        | Expr::SimilarTo { expr, pattern, .. }
        | Expr::RLike { expr, pattern, .. } => {
            collect_binary_param_pair(expr, pattern, pairs);
        }
        Expr::IsDistinctFrom(left, right) | Expr::IsNotDistinctFrom(left, right) => {
            collect_null_safe_equality_param_pairs_from_exprs(left, right, pairs);
        }
        _ => {}
    }
}

fn collect_equality_param_pairs_from_exprs(
    left: &Expr,
    right: &Expr,
    pairs: &mut Vec<EqualityParam>,
) {
    if let (Some(left_items), Some(right_items)) =
        (tuple_items_from_expr(left), tuple_items_from_expr(right))
    {
        for (left_item, right_item) in left_items.iter().zip(right_items.iter()) {
            if let Some(pair) = equality_param_from_exprs(left_item, right_item) {
                pairs.push(pair);
            }
        }
        return;
    }

    if let Some(pair) = equality_param_from_exprs(left, right) {
        pairs.push(pair);
    }
}

fn collect_in_list_param_pairs(expr: &Expr, list: &[Expr], pairs: &mut Vec<EqualityParam>) {
    if let Some((qualifier, column)) = column_name_from_expr(expr) {
        for item in list {
            if let Some(param) = named_param_from_expr(item) {
                pairs.push(EqualityParam {
                    qualifier: qualifier.clone(),
                    column: column.clone(),
                    param,
                });
            }
        }
        return;
    }

    let Some(columns) = tuple_items_from_expr(expr) else {
        return;
    };
    for item in list {
        let Some(values) = tuple_items_from_expr(item) else {
            continue;
        };
        for (column, value) in columns.iter().zip(values.iter()) {
            if let (Some((qualifier, column)), Some(param)) =
                (column_name_from_expr(column), named_param_from_expr(value))
            {
                pairs.push(EqualityParam {
                    qualifier,
                    column,
                    param,
                });
            }
        }
    }
}

fn collect_between_param_pairs(
    expr: &Expr,
    low: &Expr,
    high: &Expr,
    pairs: &mut Vec<EqualityParam>,
) {
    collect_binary_param_pair(expr, low, pairs);
    collect_binary_param_pair(expr, high, pairs);
}

fn collect_binary_param_pair(left: &Expr, right: &Expr, pairs: &mut Vec<EqualityParam>) {
    if let Some(pair) = equality_param_from_exprs(left, right) {
        pairs.push(pair);
    }
}

fn collect_null_safe_equality_param_pairs_from_exprs(
    left: &Expr,
    right: &Expr,
    pairs: &mut Vec<EqualityParam>,
) {
    let before = pairs.len();
    collect_equality_param_pairs_from_exprs(left, right, pairs);
    if pairs.len() != before {
        return;
    }

    // sqlparser-rs 0.62 can include a trailing boolean chain in the RHS of
    // `IS [NOT] DISTINCT FROM`. The immediate left operand of that chain is
    // the value being compared; the recursive visitor handles the rest.
    if let Expr::BinaryOp {
        left: immediate_right,
        op,
        ..
    } = right
    {
        if matches!(op, BinaryOperator::And | BinaryOperator::Or) {
            collect_equality_param_pairs_from_exprs(left, immediate_right, pairs);
        }
    }
}

fn equality_param_from_exprs(left: &Expr, right: &Expr) -> Option<EqualityParam> {
    if let (Some((qualifier, column)), Some(param)) =
        (column_name_from_expr(left), named_param_from_expr(right))
    {
        return Some(EqualityParam {
            qualifier,
            column,
            param,
        });
    }

    let (Some((qualifier, column)), Some(param)) =
        (column_name_from_expr(right), named_param_from_expr(left))
    else {
        return None;
    };
    Some(EqualityParam {
        qualifier,
        column,
        param,
    })
}

fn tuple_items_from_expr(expr: &Expr) -> Option<&[Expr]> {
    match expr {
        Expr::Tuple(items) => Some(items),
        Expr::Nested(expr) => tuple_items_from_expr(expr),
        _ => None,
    }
}

fn column_name_from_expr(expr: &Expr) -> Option<(Option<String>, String)> {
    match expr {
        Expr::Identifier(ident) => Some((None, ident.value.clone())),
        Expr::CompoundIdentifier(idents) => {
            let column = idents.last()?.value.clone();
            let qualifier = match idents.len() {
                0 | 1 => None,
                _ => Some(
                    idents[..idents.len() - 1]
                        .iter()
                        .map(|ident| ident.value.clone())
                        .collect::<Vec<_>>()
                        .join("."),
                ),
            };
            Some((qualifier, column))
        }
        Expr::Nested(expr) | Expr::Cast { expr, .. } => column_name_from_expr(expr),
        _ => None,
    }
}

fn named_param_from_expr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Value(value) => {
            let Value::Placeholder(name) = &value.value else {
                return None;
            };
            name.strip_prefix(':').map(ToString::to_string)
        }
        Expr::Nested(expr) | Expr::Cast { expr, .. } => named_param_from_expr(expr),
        _ => None,
    }
}

fn lower_set_expr(set_expr: &SetExpr) -> Option<(&Select, Vec<CompoundSelect>)> {
    match set_expr {
        SetExpr::Select(select) => Some((select, Vec::new())),
        SetExpr::Query(query) => lower_set_expr(&query.body),
        SetExpr::SetOperation {
            left,
            op,
            set_quantifier,
            right,
        } => {
            let (select, mut compound) = lower_set_expr(left)?;
            collect_compound_selects(right, *op, *set_quantifier, &mut compound);
            Some((select, compound))
        }
        _ => None,
    }
}

fn collect_compound_selects(
    set_expr: &SetExpr,
    op: SetOperator,
    quantifier: SetQuantifier,
    compound: &mut Vec<CompoundSelect>,
) {
    match set_expr {
        SetExpr::SetOperation {
            left,
            op: next_op,
            set_quantifier: next_quantifier,
            right,
        } => {
            compound.push(CompoundSelect {
                operator: lower_compound_operator(op, quantifier),
                query: left.to_string(),
            });
            collect_compound_selects(right, *next_op, *next_quantifier, compound);
        }
        SetExpr::Query(query) => collect_compound_selects(&query.body, op, quantifier, compound),
        other => compound.push(CompoundSelect {
            operator: lower_compound_operator(op, quantifier),
            query: other.to_string(),
        }),
    }
}

fn lower_compound_operator(op: SetOperator, quantifier: SetQuantifier) -> CompoundOperator {
    match op {
        SetOperator::Union if quantifier == SetQuantifier::All => CompoundOperator::UnionAll,
        SetOperator::Union => CompoundOperator::Union,
        SetOperator::Intersect => CompoundOperator::Intersect,
        SetOperator::Except | SetOperator::Minus => CompoundOperator::Except,
    }
}

fn lower_select_item(item: &SelectItem) -> SelectProjection {
    match item {
        SelectItem::UnnamedExpr(expr) => SelectProjection {
            expr: expr.to_string(),
            alias: None,
        },
        SelectItem::ExprWithAlias { expr, alias } => SelectProjection {
            expr: expr.to_string(),
            alias: Some(alias.value.clone()),
        },
        SelectItem::ExprWithAliases { expr, aliases } => SelectProjection {
            expr: expr.to_string(),
            alias: aliases.first().map(|alias| alias.value.clone()),
        },
        SelectItem::QualifiedWildcard(kind, _) => SelectProjection {
            expr: match kind {
                SelectItemQualifiedWildcardKind::ObjectName(name) => {
                    format!("{}.*", object_name_to_string(name))
                }
                SelectItemQualifiedWildcardKind::Expr(expr) => format!("{expr}.*"),
            },
            alias: None,
        },
        SelectItem::Wildcard(_) => SelectProjection {
            expr: "*".to_string(),
            alias: None,
        },
    }
}

fn lower_table_with_joins(table_with_joins: &[TableWithJoins]) -> Vec<TableReference> {
    let mut refs = Vec::new();
    for table_with_join in table_with_joins {
        refs.extend(lower_table_factor(&table_with_join.relation, false));
        for join in &table_with_join.joins {
            let nullability = join_nullability_from_operator(&join.join_operator);
            if nullability.previous_nullable {
                for table_ref in &mut refs {
                    table_ref.nullable = true;
                }
            }

            let mut joined_refs = lower_table_factor(&join.relation, nullability.joined_nullable);
            refs.append(&mut joined_refs);
        }
    }
    refs
}

fn lower_table_factor(table_factor: &TableFactor, nullable: bool) -> Vec<TableReference> {
    match table_factor {
        TableFactor::Table { name, alias, .. } => vec![TableReference {
            name: object_name_to_string(name),
            alias: alias_name(alias),
            derived_query: None,
            lateral: false,
            nullable,
        }],
        TableFactor::Derived {
            lateral,
            subquery,
            alias,
            ..
        } => {
            let alias = alias_name(alias).unwrap_or_else(|| "subquery".to_string());
            vec![TableReference {
                name: alias.clone(),
                alias: Some(alias),
                derived_query: Some(subquery.to_string()),
                lateral: *lateral,
                nullable,
            }]
        }
        TableFactor::NestedJoin {
            table_with_joins,
            alias: _,
        } => {
            let mut refs = lower_table_with_joins(std::slice::from_ref(table_with_joins));
            if nullable {
                for table_ref in &mut refs {
                    table_ref.nullable = true;
                }
            }
            refs
        }
        TableFactor::TableFunction { expr, alias } => vec![TableReference {
            name: alias_name(alias).unwrap_or_else(|| expr.to_string()),
            alias: alias_name(alias),
            derived_query: None,
            lateral: false,
            nullable,
        }],
        TableFactor::Function {
            lateral,
            name,
            alias,
            ..
        } => vec![TableReference {
            name: alias_name(alias).unwrap_or_else(|| object_name_to_string(name)),
            alias: alias_name(alias),
            derived_query: None,
            lateral: *lateral,
            nullable,
        }],
        _ => Vec::new(),
    }
}

fn join_nullability_from_operator(operator: &JoinOperator) -> JoinNullability {
    match operator {
        JoinOperator::Left(_) | JoinOperator::LeftOuter(_) | JoinOperator::OuterApply => {
            JoinNullability {
                previous_nullable: false,
                joined_nullable: true,
            }
        }
        JoinOperator::Right(_) | JoinOperator::RightOuter(_) => JoinNullability {
            previous_nullable: true,
            joined_nullable: false,
        },
        JoinOperator::FullOuter(_) => JoinNullability {
            previous_nullable: true,
            joined_nullable: true,
        },
        _ => JoinNullability {
            previous_nullable: false,
            joined_nullable: false,
        },
    }
}

fn object_name_to_string(name: &ObjectName) -> String {
    name.0
        .iter()
        .map(|part| match part {
            ObjectNamePart::Identifier(ident) => ident.value.clone(),
            ObjectNamePart::Function(function) => function.to_string(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn alias_name(alias: &Option<TableAlias>) -> Option<String> {
    alias.as_ref().map(|alias| alias.name.value.clone())
}

fn parse_select_heuristic(sql: &str) -> Option<SelectStatement> {
    let cleaned = trim_trailing_semicolon(sql.trim());
    let (select_sql, ctes) = split_leading_ctes(cleaned)?;
    let (select_sql, compound) = split_compound_selects(select_sql);
    let Ok((after_select, _)) = select_prefix(select_sql) else {
        return None;
    };
    let select_len = select_sql.len() - after_select.len();
    let (projection_sql, table_refs) =
        if let Some(from_idx) = find_keyword_top_level(select_sql, "from") {
            let projection_sql = select_sql[select_len..from_idx].trim();
            let after_from = select_sql[from_idx + "from".len()..].trim();
            let from_clause = leading_from_clause(after_from);
            (projection_sql, parse_table_references(from_clause))
        } else {
            let after_select = select_sql[select_len..].trim();
            (leading_projection_clause(after_select), Vec::new())
        };
    let table = table_refs
        .first()
        .map(|table_ref| table_ref.name.clone())
        .unwrap_or_default();

    let projections = split_comma_separated(projection_sql)
        .into_iter()
        .map(parse_projection)
        .collect();
    let equality_params = infer_equality_param_pairs(select_sql);

    Some(SelectStatement {
        ctes,
        projections,
        table,
        table_refs,
        equality_params,
        compound,
    })
}

fn create_table_prefix(input: &str) -> IResult<&str, String> {
    let (input, _) = multispace0.parse(input)?;
    let (input, _) = tag_no_case("create").parse(input)?;
    let (input, _) = multispace1.parse(input)?;
    let (input, _) = tag_no_case("table").parse(input)?;
    let (input, _) = multispace1.parse(input)?;
    let (input, _) = opt((
        tag_no_case("if"),
        multispace1,
        tag_no_case("not"),
        multispace1,
        tag_no_case("exists"),
        multispace1,
    ))
    .parse(input)?;
    let (input, table) = sql_identifier.parse(input)?;

    Ok((input, table))
}

fn select_prefix(input: &str) -> IResult<&str, ()> {
    let (input, _) = multispace0.parse(input)?;
    let (input, _) = tag_no_case("select").parse(input)?;
    let (input, _) = multispace1.parse(input)?;
    Ok((input, ()))
}

fn parse_column_definition(definition: &str) -> Option<ColumnDefinition> {
    let tokens = tokenize_words(definition);
    let name = tokens.first()?;

    if is_table_constraint(name) {
        return None;
    }

    let constraint_idx = tokens
        .iter()
        .position(|token| is_column_constraint(token))
        .unwrap_or(tokens.len());
    let declared_type = tokens
        .get(1..constraint_idx)
        .unwrap_or_default()
        .join(" ")
        .trim()
        .to_string();
    let declared_type = if declared_type.is_empty() {
        "TEXT".to_string()
    } else {
        declared_type
    };
    let upper_definition = definition.to_ascii_uppercase();
    let nullable = if upper_definition.contains("NOT NULL")
        || upper_definition.contains("PRIMARY KEY")
        || upper_definition.contains("WITHOUT NULL")
    {
        ColumnNullability::NonNull
    } else {
        ColumnNullability::Nullable
    };

    Some(ColumnDefinition {
        name: strip_identifier_quotes(name).to_string(),
        declared_type,
        nullable,
    })
}

fn parse_projection(projection: &str) -> SelectProjection {
    let (expr, alias) = split_projection_alias(projection);
    SelectProjection {
        expr: expr.trim().to_string(),
        alias: alias.map(ToString::to_string),
    }
}

fn split_projection_alias(projection: &str) -> (&str, Option<&str>) {
    if let Some(as_idx) = find_keyword_top_level(projection, "as") {
        let expr = projection[..as_idx].trim();
        let alias = projection[as_idx + "as".len()..].trim();
        return (expr, Some(strip_identifier_quotes(alias)));
    }

    (projection, None)
}

fn split_leading_ctes(sql: &str) -> Option<(&str, Vec<CommonTableExpression>)> {
    if !starts_with_keyword(sql, "with") {
        return Some((sql, Vec::new()));
    }

    let mut rest = sql["with".len()..].trim_start();
    let recursive = starts_with_keyword(rest, "recursive");
    if recursive {
        rest = rest["recursive".len()..].trim_start();
    }
    let mut ctes = Vec::new();
    loop {
        let (name, name_len) = parse_identifier_prefix(rest)?;
        rest = rest[name_len..].trim_start();

        let mut columns = Vec::new();
        if rest.starts_with('(') {
            let close = find_matching_paren(rest, 0)?;
            columns = split_comma_separated(&rest[1..close])
                .into_iter()
                .map(|column| strip_identifier_quotes(column).to_string())
                .collect();
            rest = rest[close + 1..].trim_start();
        }

        if !starts_with_keyword(rest, "as") {
            return None;
        }
        rest = rest["as".len()..].trim_start();
        if !rest.starts_with('(') {
            return None;
        }
        let close = find_matching_paren(rest, 0)?;
        ctes.push(CommonTableExpression {
            name: strip_identifier_quotes(&name).to_string(),
            columns,
            query: rest[1..close].trim().to_string(),
            recursive,
        });
        rest = rest[close + 1..].trim_start();

        if rest.starts_with(',') {
            rest = rest[1..].trim_start();
            continue;
        }

        return Some((rest, ctes));
    }
}

fn split_compound_selects(sql: &str) -> (&str, Vec<CompoundSelect>) {
    let mut head = sql.trim();
    let mut compound = Vec::new();

    let Some((idx, operator, operator_len)) = find_next_compound_operator(head) else {
        return (head, compound);
    };

    let first = head[..idx].trim();
    let mut rest = head[idx + operator_len..].trim();
    let mut current_operator = operator;
    loop {
        if let Some((next_idx, next_operator, next_operator_len)) =
            find_next_compound_operator(rest)
        {
            compound.push(CompoundSelect {
                operator: current_operator,
                query: rest[..next_idx].trim().to_string(),
            });
            rest = rest[next_idx + next_operator_len..].trim();
            current_operator = next_operator;
        } else {
            compound.push(CompoundSelect {
                operator: current_operator,
                query: rest.trim().to_string(),
            });
            break;
        }
    }

    head = first;
    (head, compound)
}

fn find_next_compound_operator(input: &str) -> Option<(usize, CompoundOperator, usize)> {
    let candidates = [
        ("union", CompoundOperator::Union),
        ("intersect", CompoundOperator::Intersect),
        ("except", CompoundOperator::Except),
    ];

    candidates
        .into_iter()
        .filter_map(|(keyword, operator)| {
            let idx = find_keyword_top_level(input, keyword)?;
            let mut len = keyword.len();
            let mut operator = operator;
            if keyword == "union" {
                let after_union = &input[idx + keyword.len()..];
                let whitespace = after_union.len() - after_union.trim_start().len();
                let after_whitespace = after_union.trim_start();
                if starts_with_keyword(after_whitespace, "all") {
                    len += whitespace + "all".len();
                    operator = CompoundOperator::UnionAll;
                }
            }
            Some((idx, operator, len))
        })
        .min_by_key(|(idx, _, _)| *idx)
}

fn sql_identifier(input: &str) -> IResult<&str, String> {
    map(
        recognize((
            take_while1(is_ident_start_char),
            take_while(is_ident_continue_char),
        )),
        ToString::to_string,
    )
    .or(map(
        delimited(char('"'), take_while(|ch| ch != '"'), char('"')),
        ToString::to_string,
    ))
    .or(map(
        delimited(char('`'), take_while(|ch| ch != '`'), char('`')),
        ToString::to_string,
    ))
    .or(map(
        delimited(char('['), take_while(|ch| ch != ']'), char(']')),
        ToString::to_string,
    ))
    .parse(input)
}

fn parse_identifier_prefix(input: &str) -> Option<(String, usize)> {
    let trimmed = input.trim_start();
    let leading_ws = input.len() - trimmed.len();
    let bytes = trimmed.as_bytes();
    let first = *bytes.first()?;

    if matches!(first, b'"' | b'`' | b'[') {
        let close = if first == b'[' { b']' } else { first };
        let mut idx = 1;
        while idx < bytes.len() {
            if bytes[idx] == close {
                let token = &trimmed[..=idx];
                return Some((token.to_string(), leading_ws + idx + 1));
            }
            idx += 1;
        }
        return None;
    }

    if !is_ident_start_byte(first) {
        return None;
    }

    let mut idx = 1;
    while idx < bytes.len()
        && (bytes[idx].is_ascii_alphanumeric() || bytes[idx] == b'_' || bytes[idx] == b'.')
    {
        idx += 1;
    }
    Some((trimmed[..idx].to_string(), leading_ws + idx))
}

fn infer_equality_param_pairs(sql: &str) -> Vec<EqualityParam> {
    let tokens = sql_tokens(sql);
    let mut pairs = Vec::new();
    for window in tokens.windows(3) {
        if window[1] != "=" {
            continue;
        }

        if let Some(param) = window[2].strip_prefix(':') {
            let (qualifier, column) = split_qualified_name(&window[0]);
            pairs.push(EqualityParam {
                qualifier,
                column,
                param: param.to_string(),
            });
        } else if let Some(param) = window[0].strip_prefix(':') {
            let (qualifier, column) = split_qualified_name(&window[2]);
            pairs.push(EqualityParam {
                qualifier,
                column,
                param: param.to_string(),
            });
        }
    }
    pairs
}

fn leading_from_clause(after_from: &str) -> &str {
    let mut end = after_from.len();
    for keyword in [
        "where",
        "group",
        "order",
        "limit",
        "having",
        "union",
        "returning",
    ] {
        if let Some(idx) = find_keyword_top_level(after_from, keyword) {
            end = end.min(idx);
        }
    }
    after_from[..end].trim()
}

fn leading_projection_clause(after_select: &str) -> &str {
    let mut end = after_select.len();
    for keyword in ["where", "group", "order", "limit", "having", "returning"] {
        if let Some(idx) = find_keyword_top_level(after_select, keyword) {
            end = end.min(idx);
        }
    }
    after_select[..end].trim()
}

fn parse_table_references(from_clause: &str) -> Vec<TableReference> {
    split_comma_separated(from_clause)
        .into_iter()
        .flat_map(parse_joined_table_references)
        .collect()
}

fn parse_joined_table_references(from_clause: &str) -> Vec<TableReference> {
    let mut refs = Vec::new();
    let mut rest = from_clause.trim();

    if let Some((table_refs, consumed)) = parse_table_reference_prefix(rest, false) {
        refs.extend(table_refs);
        rest = rest[consumed..].trim_start();
    }

    while let Some(join_idx) = find_keyword_top_level(rest, "join") {
        let before_join = &rest[..join_idx];
        let tokens = tokenize_words(before_join);
        let join_nullability = join_nullability_from_tokens(&tokens);
        if join_nullability.previous_nullable {
            for table_ref in &mut refs {
                table_ref.nullable = true;
            }
        }
        rest = rest[join_idx + "join".len()..].trim_start();
        if let Some((table_refs, consumed)) =
            parse_table_reference_prefix(rest, join_nullability.joined_nullable)
        {
            refs.extend(table_refs);
            rest = rest[consumed..].trim_start();
        } else {
            break;
        }
    }

    refs
}

#[derive(Debug, Clone, Copy)]
struct JoinNullability {
    previous_nullable: bool,
    joined_nullable: bool,
}

fn join_nullability_before(tokens: &[String], join_idx: usize) -> JoinNullability {
    let mut idx = join_idx;
    while idx > 0 {
        idx -= 1;
        let token = tokens[idx].to_ascii_lowercase();
        match token.as_str() {
            "outer" => continue,
            "left" => {
                return JoinNullability {
                    previous_nullable: false,
                    joined_nullable: true,
                };
            }
            "right" => {
                return JoinNullability {
                    previous_nullable: true,
                    joined_nullable: false,
                };
            }
            "full" => {
                return JoinNullability {
                    previous_nullable: true,
                    joined_nullable: true,
                };
            }
            "inner" | "cross" | "natural" => break,
            _ => break,
        }
    }

    JoinNullability {
        previous_nullable: false,
        joined_nullable: false,
    }
}

fn join_nullability_from_tokens(tokens: &[String]) -> JoinNullability {
    join_nullability_before(tokens, tokens.len())
}

fn parse_table_reference_prefix(
    input: &str,
    nullable: bool,
) -> Option<(Vec<TableReference>, usize)> {
    let leading_ws = input.len() - input.trim_start().len();
    let mut input = input.trim_start();
    let mut lateral = false;
    let mut prefix_consumed = 0;

    if starts_with_keyword(input, "lateral") {
        lateral = true;
        let after_lateral = input["lateral".len()..].trim_start();
        prefix_consumed += "lateral".len() + input["lateral".len()..].len() - after_lateral.len();
        input = after_lateral;
    }

    if input.starts_with('(') {
        let close = find_matching_paren(input, 0)?;
        let inner = input[1..close].trim();
        let mut consumed = close + 1;
        if starts_with_keyword(inner, "select") || starts_with_keyword(inner, "with") {
            let (alias, alias_len) = parse_optional_alias(&input[consumed..]);
            consumed += alias_len;
            let alias = alias.unwrap_or_else(|| "subquery".to_string());
            return Some((
                vec![TableReference {
                    name: alias.clone(),
                    alias: Some(alias),
                    derived_query: Some(inner.to_string()),
                    lateral,
                    nullable,
                }],
                leading_ws + prefix_consumed + consumed,
            ));
        }

        let mut table_refs = parse_table_references(inner);
        if table_refs.is_empty() {
            return None;
        }
        if nullable {
            for table_ref in &mut table_refs {
                table_ref.nullable = true;
            }
        }
        let (_alias, alias_len) = parse_optional_alias(&input[consumed..]);
        consumed += alias_len;
        return Some((table_refs, leading_ws + prefix_consumed + consumed));
    }

    let (table, table_len) = parse_identifier_prefix(input)?;
    if is_from_join_keyword(&table) {
        return None;
    }
    let table = strip_identifier_quotes(&table).to_string();
    let mut consumed = table_len;
    let (alias, alias_len) = parse_optional_alias(&input[consumed..]);
    consumed += alias_len;

    Some((
        vec![TableReference {
            name: table,
            alias,
            derived_query: None,
            lateral,
            nullable,
        }],
        leading_ws + prefix_consumed + consumed,
    ))
}

fn parse_optional_alias(input: &str) -> (Option<String>, usize) {
    let mut consumed = input.len() - input.trim_start().len();
    let mut rest = input.trim_start();

    if starts_with_keyword(rest, "as") {
        let after_as = rest["as".len()..].trim_start();
        consumed += "as".len() + rest["as".len()..].len() - after_as.len();
        rest = after_as;
    }

    parse_identifier_prefix(rest)
        .filter(|(alias, _)| !is_from_join_keyword(alias))
        .map(|(alias, len)| {
            (
                Some(strip_identifier_quotes(&alias).to_string()),
                consumed + len,
            )
        })
        .unwrap_or((None, consumed))
}

fn is_from_join_keyword(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "join"
            | "on"
            | "using"
            | "where"
            | "left"
            | "right"
            | "inner"
            | "outer"
            | "full"
            | "cross"
            | "natural"
            | "lateral"
            | "group"
            | "order"
            | "limit"
            | "having"
    )
}

fn split_sql_statements(sql: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let mut start = 0;
    let mut in_single = false;
    let mut in_double = false;
    let bytes = sql.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b';' if !in_single && !in_double => {
                let statement = sql[start..idx].trim();
                if !statement.is_empty() {
                    statements.push(statement);
                }
                start = idx + 1;
            }
            _ => {}
        }
        idx += 1;
    }

    let tail = sql[start..].trim();
    if !tail.is_empty() {
        statements.push(tail);
    }

    statements
}

pub(crate) fn split_comma_separated(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_i32;
    let mut in_single = false;
    let mut in_double = false;
    let bytes = input.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'(' if !in_single && !in_double => depth += 1,
            b')' if !in_single && !in_double => depth -= 1,
            b',' if depth == 0 && !in_single && !in_double => {
                parts.push(input[start..idx].trim());
                start = idx + 1;
            }
            _ => {}
        }
        idx += 1;
    }

    let tail = input[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

fn find_top_level_char(input: &str, needle: char) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in input.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            value if value == needle && !in_single && !in_double => return Some(idx),
            _ => {}
        }
    }
    None
}

fn find_matching_paren(input: &str, open: usize) -> Option<usize> {
    let mut depth = 0_i32;
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in input[open..].char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '(' if !in_single && !in_double => depth += 1,
            ')' if !in_single && !in_double => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_keyword_top_level(input: &str, keyword: &str) -> Option<usize> {
    let lower = input.to_ascii_lowercase();
    let needle = keyword.to_ascii_lowercase();
    let bytes = input.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut depth = 0_i32;
    let mut idx = 0;

    while idx + needle.len() <= bytes.len() {
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

        if depth == 0
            && !in_single
            && !in_double
            && lower[idx..].starts_with(&needle)
            && is_keyword_boundary(bytes.get(idx.wrapping_sub(1)).copied())
            && is_keyword_boundary(bytes.get(idx + needle.len()).copied())
        {
            return Some(idx);
        }

        idx += 1;
    }

    None
}

fn starts_with_keyword(input: &str, keyword: &str) -> bool {
    let trimmed = input.trim_start();
    trimmed
        .get(..keyword.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(keyword))
        && is_keyword_boundary(trimmed.as_bytes().get(keyword.len()).copied())
}

fn tokenize_words(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() {
            break;
        }

        if matches!(bytes[idx], b'"' | b'`' | b'[') {
            let close = match bytes[idx] {
                b'[' => b']',
                other => other,
            };
            let start = idx;
            idx += 1;
            while idx < bytes.len() && bytes[idx] != close {
                idx += 1;
            }
            idx = (idx + 1).min(bytes.len());
            tokens.push(input[start..idx].to_string());
            continue;
        }

        let start = idx;
        while idx < bytes.len()
            && !bytes[idx].is_ascii_whitespace()
            && !matches!(bytes[idx], b',' | b'(' | b')' | b';')
        {
            idx += 1;
        }
        if start != idx {
            tokens.push(input[start..idx].to_string());
        } else {
            idx += 1;
        }
    }

    tokens
}

fn sql_tokens(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        if bytes[idx].is_ascii_whitespace() {
            idx += 1;
            continue;
        }
        if bytes[idx] == b'\'' {
            skip_single_quoted(input, &mut idx);
            continue;
        }
        if bytes[idx] == b'"' {
            skip_double_quoted(input, &mut idx);
            continue;
        }
        if bytes[idx] == b'-' && bytes.get(idx + 1).copied() == Some(b'-') {
            skip_line_comment(input, &mut idx);
            continue;
        }
        if bytes[idx] == b'/' && bytes.get(idx + 1).copied() == Some(b'*') {
            skip_block_comment(input, &mut idx);
            continue;
        }
        if bytes[idx] == b':' && bytes.get(idx + 1).copied().is_some_and(is_ident_start_byte) {
            let start = idx;
            idx += 2;
            while idx < bytes.len() && is_ident_continue(bytes[idx]) {
                idx += 1;
            }
            tokens.push(input[start..idx].to_string());
            continue;
        }
        if bytes[idx].is_ascii_alphabetic() || bytes[idx] == b'_' {
            let start = idx;
            idx += 1;
            while idx < bytes.len()
                && (bytes[idx].is_ascii_alphanumeric() || bytes[idx] == b'_' || bytes[idx] == b'.')
            {
                idx += 1;
            }
            tokens.push(input[start..idx].to_string());
            continue;
        }
        if bytes[idx] == b'=' {
            tokens.push("=".to_string());
        }
        idx += 1;
    }

    tokens
}

fn trim_trailing_semicolon(input: &str) -> &str {
    input.trim_end().trim_end_matches(';').trim_end()
}

pub(crate) fn strip_identifier_quotes(input: &str) -> &str {
    let trimmed = input.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('`') && trimmed.ends_with('`'))
            || (trimmed.starts_with('[') && trimmed.ends_with(']')))
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

pub(crate) fn split_qualified_name(input: &str) -> (Option<String>, String) {
    let trimmed = input.trim();
    if let Some((qualifier, column)) = trimmed.rsplit_once('.') {
        (
            Some(strip_identifier_quotes(qualifier).to_string()),
            strip_identifier_quotes(column).to_string(),
        )
    } else {
        (None, strip_identifier_quotes(trimmed).to_string())
    }
}

fn is_keyword_boundary(byte: Option<u8>) -> bool {
    byte.is_none_or(|byte| !byte.is_ascii_alphanumeric() && byte != b'_')
}

fn is_table_constraint(value: &str) -> bool {
    matches!(
        value.to_ascii_uppercase().as_str(),
        "PRIMARY" | "FOREIGN" | "UNIQUE" | "CHECK" | "CONSTRAINT"
    )
}

fn is_column_constraint(value: &str) -> bool {
    matches!(
        value.to_ascii_uppercase().as_str(),
        "PRIMARY"
            | "NOT"
            | "NULL"
            | "DEFAULT"
            | "COLLATE"
            | "REFERENCES"
            | "CHECK"
            | "UNIQUE"
            | "GENERATED"
            | "AS"
    )
}

fn skip_single_quoted(sql: &str, idx: &mut usize) {
    let bytes = sql.as_bytes();
    *idx += 1;
    while *idx < bytes.len() {
        if bytes[*idx] == b'\'' {
            *idx += 1;
            if bytes.get(*idx).copied() == Some(b'\'') {
                *idx += 1;
                continue;
            }
            break;
        }
        *idx += 1;
    }
}

fn skip_double_quoted(sql: &str, idx: &mut usize) {
    let bytes = sql.as_bytes();
    *idx += 1;
    while *idx < bytes.len() {
        if bytes[*idx] == b'"' {
            *idx += 1;
            if bytes.get(*idx).copied() == Some(b'"') {
                *idx += 1;
                continue;
            }
            break;
        }
        *idx += 1;
    }
}

fn skip_line_comment(sql: &str, idx: &mut usize) {
    let bytes = sql.as_bytes();
    while *idx < bytes.len() {
        *idx += 1;
        if bytes[*idx - 1] == b'\n' {
            break;
        }
    }
}

fn skip_block_comment(sql: &str, idx: &mut usize) {
    let bytes = sql.as_bytes();
    while *idx < bytes.len() {
        *idx += 1;
        if *idx >= 2 && bytes[*idx - 2] == b'*' && bytes[*idx - 1] == b'/' {
            break;
        }
    }
}

fn is_ident_continue(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || ch == b'_'
}

fn is_ident_start_byte(ch: u8) -> bool {
    ch.is_ascii_alphabetic() || ch == b'_'
}

fn is_ident_start_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue_char(ch: char) -> bool {
    ch == '_' || ch == '.' || ch.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_create_table_into_ir() {
        let statement = parse_create_table(
            "CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY,
                email TEXT NOT NULL,
                parent_id INTEGER,
                CONSTRAINT users_email_unique UNIQUE (email)
            );",
        )
        .unwrap()
        .unwrap();

        assert_eq!(statement.table, "users");
        assert_eq!(statement.columns.len(), 3);
        assert_eq!(statement.columns[0].name, "id");
        assert_eq!(statement.columns[0].declared_type, "INTEGER");
        assert_eq!(statement.columns[0].nullable, ColumnNullability::NonNull);
        assert_eq!(statement.columns[2].name, "parent_id");
        assert_eq!(statement.columns[2].nullable, ColumnNullability::Nullable);
    }

    #[test]
    fn sqlparser_ast_lowerer_parses_create_table_schema() {
        let statement = parse_create_table_with_sqlparser(
            r#"
            CREATE TABLE "accounts" (
                "id" BIGINT,
                "email" TEXT NOT NULL,
                "display_name" TEXT,
                CONSTRAINT accounts_pk PRIMARY KEY ("id")
            );
            "#,
        )
        .unwrap();

        assert_eq!(statement.table, "accounts");
        assert_eq!(statement.columns.len(), 3);
        assert_eq!(statement.columns[0].name, "id");
        assert_eq!(statement.columns[0].declared_type, "BIGINT");
        assert_eq!(statement.columns[0].nullable, ColumnNullability::NonNull);
        assert_eq!(statement.columns[1].name, "email");
        assert_eq!(statement.columns[1].nullable, ColumnNullability::NonNull);
        assert_eq!(statement.columns[2].name, "display_name");
        assert_eq!(statement.columns[2].nullable, ColumnNullability::Nullable);
    }

    #[test]
    fn sqlparser_ast_lowerer_parses_mutation_params() {
        let insert = parse_mutation(
            "INSERT INTO users (email, org_id) VALUES (:email_address, CAST(:org_id AS INTEGER))",
        )
        .unwrap();
        assert_eq!(insert.table, "users");
        assert_eq!(
            insert.column_params,
            vec![
                MutationColumnParam {
                    column: "email".to_string(),
                    param: "email_address".to_string()
                },
                MutationColumnParam {
                    column: "org_id".to_string(),
                    param: "org_id".to_string()
                }
            ]
        );

        let update = parse_mutation(
            "UPDATE users SET email = :new_email, active = :active WHERE id = :user_id",
        )
        .unwrap();
        assert_eq!(update.table, "users");
        assert_eq!(
            update.column_params,
            vec![
                MutationColumnParam {
                    column: "email".to_string(),
                    param: "new_email".to_string()
                },
                MutationColumnParam {
                    column: "active".to_string(),
                    param: "active".to_string()
                }
            ]
        );
        assert_eq!(
            update.equality_params,
            vec![EqualityParam {
                qualifier: None,
                column: "id".to_string(),
                param: "user_id".to_string()
            }]
        );

        let delete = parse_mutation("DELETE FROM users WHERE id = :user_id").unwrap();
        assert_eq!(delete.table, "users");
        assert_eq!(
            delete.equality_params,
            vec![EqualityParam {
                qualifier: None,
                column: "id".to_string(),
                param: "user_id".to_string()
            }]
        );
    }

    #[test]
    fn parses_select_into_ir() {
        let statement = parse_select(
            "SELECT id, lower(email) AS lower_email, email || '' AS email_expr FROM users WHERE id = :id AND email = :email;",
        )
        .unwrap();

        assert_eq!(statement.table, "users");
        assert!(statement.ctes.is_empty());
        assert!(statement.compound.is_empty());
        assert_eq!(statement.projections.len(), 3);
        assert_eq!(statement.projections[1].expr, "lower(email)");
        assert_eq!(
            statement.projections[1].alias.as_deref(),
            Some("lower_email")
        );
        assert_eq!(
            statement.equality_params,
            vec![
                EqualityParam {
                    qualifier: None,
                    column: "id".to_string(),
                    param: "id".to_string()
                },
                EqualityParam {
                    qualifier: None,
                    column: "email".to_string(),
                    param: "email".to_string()
                }
            ]
        );
    }

    #[test]
    fn parses_select_without_from_into_ir() {
        let statement =
            parse_select("SELECT u.email || '' AS email_expr, :id AS requested_id WHERE true")
                .unwrap();

        assert_eq!(statement.table, "");
        assert!(statement.table_refs.is_empty());
        assert_eq!(statement.projections.len(), 2);
        assert_eq!(statement.projections[0].expr, "u.email || ''");
        assert_eq!(
            statement.projections[0].alias.as_deref(),
            Some("email_expr")
        );
        assert_eq!(statement.projections[1].expr, ":id");
        assert_eq!(
            statement.projections[1].alias.as_deref(),
            Some("requested_id")
        );
    }

    #[test]
    fn ignores_params_in_quoted_text_and_comments_for_predicate_pairs() {
        let statement = parse_select(
            "SELECT id FROM users WHERE note = ':ignored' AND id = :id -- email = :comment\n/* parent_id = :block */",
        )
        .unwrap();

        assert_eq!(
            statement.equality_params,
            vec![EqualityParam {
                qualifier: None,
                column: "id".to_string(),
                param: "id".to_string()
            }]
        );
    }

    #[test]
    fn parses_join_tables_aliases_and_qualified_params() {
        let statement = parse_select(
            "SELECT u.id, o.name AS org_name FROM users AS u LEFT JOIN organizations o ON o.id = u.org_id WHERE u.id = :id AND o.slug = :org_slug",
        )
        .unwrap();

        assert_eq!(statement.table, "users");
        assert_eq!(
            statement.table_refs,
            vec![
                TableReference {
                    name: "users".to_string(),
                    alias: Some("u".to_string()),
                    derived_query: None,
                    lateral: false,
                    nullable: false
                },
                TableReference {
                    name: "organizations".to_string(),
                    alias: Some("o".to_string()),
                    derived_query: None,
                    lateral: false,
                    nullable: true
                }
            ]
        );
        assert_eq!(
            statement.equality_params,
            vec![
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "id".to_string(),
                    param: "id".to_string()
                },
                EqualityParam {
                    qualifier: Some("o".to_string()),
                    column: "slug".to_string(),
                    param: "org_slug".to_string()
                }
            ]
        );
    }

    #[test]
    fn parses_outer_join_nullability_into_table_refs() {
        let right = parse_select(
            "SELECT u.id, o.id FROM users u RIGHT JOIN organizations o ON o.id = u.org_id",
        )
        .unwrap();
        assert_eq!(
            right.table_refs,
            vec![
                TableReference {
                    name: "users".to_string(),
                    alias: Some("u".to_string()),
                    derived_query: None,
                    lateral: false,
                    nullable: true
                },
                TableReference {
                    name: "organizations".to_string(),
                    alias: Some("o".to_string()),
                    derived_query: None,
                    lateral: false,
                    nullable: false
                }
            ]
        );

        let full = parse_select(
            "SELECT u.id, o.id FROM users u FULL OUTER JOIN organizations o ON o.id = u.org_id",
        )
        .unwrap();
        assert!(full.table_refs.iter().all(|table| table.nullable));
    }

    #[test]
    fn parses_parenthesized_join_groups() {
        let statement = parse_select(
            "SELECT u.id, o.name, a.slug
             FROM users u
             LEFT JOIN (organizations o JOIN accounts a ON a.org_id = o.id) ON o.id = u.org_id
             WHERE u.id = :id",
        )
        .unwrap();
        assert_eq!(
            statement.table_refs,
            vec![
                TableReference {
                    name: "users".to_string(),
                    alias: Some("u".to_string()),
                    derived_query: None,
                    lateral: false,
                    nullable: false
                },
                TableReference {
                    name: "organizations".to_string(),
                    alias: Some("o".to_string()),
                    derived_query: None,
                    lateral: false,
                    nullable: true
                },
                TableReference {
                    name: "accounts".to_string(),
                    alias: Some("a".to_string()),
                    derived_query: None,
                    lateral: false,
                    nullable: true
                }
            ]
        );
    }

    #[test]
    fn parses_lateral_derived_tables_inside_parenthesized_join_groups() {
        let statement = parse_select(
            "SELECT u.id, org_meta.org_expr
             FROM users u
             LEFT JOIN (
                organizations o
                JOIN LATERAL (SELECT o.name || '' AS org_expr) org_meta ON true
             ) ON o.id = u.org_id
             WHERE u.id = :id",
        )
        .unwrap();

        assert_eq!(statement.table_refs.len(), 3);
        assert_eq!(statement.table_refs[0].name, "users");
        assert!(!statement.table_refs[0].nullable);
        assert!(!statement.table_refs[0].lateral);
        assert_eq!(statement.table_refs[1].name, "organizations");
        assert_eq!(statement.table_refs[1].alias.as_deref(), Some("o"));
        assert!(statement.table_refs[1].nullable);
        assert!(!statement.table_refs[1].lateral);
        assert_eq!(statement.table_refs[2].name, "org_meta");
        assert_eq!(statement.table_refs[2].alias.as_deref(), Some("org_meta"));
        assert_eq!(
            statement.table_refs[2].derived_query.as_deref(),
            Some("SELECT o.name || '' AS org_expr")
        );
        assert!(statement.table_refs[2].nullable);
        assert!(statement.table_refs[2].lateral);
    }

    #[test]
    fn sqlparser_ast_lowerer_handles_quoted_lateral_join_shapes() {
        let statement = parse_select_with_sqlparser(
            r#"
            SELECT "u"."id" AS user_id, e.email_expr
            FROM "users" AS "u"
            LEFT JOIN LATERAL (
                SELECT "u"."email" || '' AS email_expr
            ) AS e ON true
            "#,
        )
        .unwrap();

        assert_eq!(statement.table, "users");
        assert_eq!(statement.projections.len(), 2);
        assert_eq!(statement.projections[0].alias.as_deref(), Some("user_id"));
        assert_eq!(
            statement.table_refs,
            vec![
                TableReference {
                    name: "users".to_string(),
                    alias: Some("u".to_string()),
                    derived_query: None,
                    lateral: false,
                    nullable: false
                },
                TableReference {
                    name: "e".to_string(),
                    alias: Some("e".to_string()),
                    derived_query: Some("SELECT \"u\".\"email\" || '' AS email_expr".to_string()),
                    lateral: true,
                    nullable: true
                }
            ]
        );
    }

    #[test]
    fn sqlparser_ast_lowerer_extracts_named_equality_params() {
        let statement = parse_select_with_sqlparser(
            "
            WITH filtered AS (
                SELECT id FROM users WHERE email = :cte_email
            )
            SELECT u.id
            FROM users u
            LEFT JOIN LATERAL (
                SELECT e.email
                FROM emails e
                WHERE e.user_id = u.id AND e.kind = :kind
            ) recent ON true
            JOIN organizations o ON o.id = u.org_id AND o.slug = :org_slug
            WHERE u.id = :id AND :parent_id = u.parent_id AND ':ignored' = u.note
            ",
        )
        .unwrap();
        assert_eq!(
            statement.equality_params,
            vec![
                EqualityParam {
                    qualifier: None,
                    column: "email".to_string(),
                    param: "cte_email".to_string()
                },
                EqualityParam {
                    qualifier: Some("e".to_string()),
                    column: "kind".to_string(),
                    param: "kind".to_string()
                },
                EqualityParam {
                    qualifier: Some("o".to_string()),
                    column: "slug".to_string(),
                    param: "org_slug".to_string()
                },
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "id".to_string(),
                    param: "id".to_string()
                },
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "parent_id".to_string(),
                    param: "parent_id".to_string()
                }
            ]
        );
    }

    #[test]
    fn sqlparser_ast_lowerer_extracts_tuple_and_in_list_params() {
        let statement = parse_select_with_sqlparser(
            "
            SELECT u.id
            FROM users u
            WHERE (u.id, u.org_id) = (:id, :org_id)
              AND u.email IN (:email_one, :email_two)
              AND (u.parent_id, u.active) IN ((:parent_id, :active))
            ",
        )
        .unwrap();

        assert_eq!(
            statement.equality_params,
            vec![
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "id".to_string(),
                    param: "id".to_string()
                },
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "org_id".to_string(),
                    param: "org_id".to_string()
                },
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "email".to_string(),
                    param: "email_one".to_string()
                },
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "email".to_string(),
                    param: "email_two".to_string()
                },
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "parent_id".to_string(),
                    param: "parent_id".to_string()
                },
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "active".to_string(),
                    param: "active".to_string()
                }
            ]
        );
    }

    #[test]
    fn sqlparser_ast_lowerer_extracts_range_pattern_and_distinct_params() {
        let statement = parse_select_with_sqlparser(
            "
            SELECT u.id
            FROM users u
            WHERE u.created_at BETWEEN :start_at AND :end_at
              AND u.email LIKE :email_pattern
              AND u.slug ILIKE :slug_pattern
              AND u.external_id IS NOT DISTINCT FROM :external_id
              AND :parent_id IS DISTINCT FROM u.parent_id
            ",
        )
        .unwrap();

        assert_eq!(
            statement.equality_params,
            vec![
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "created_at".to_string(),
                    param: "start_at".to_string()
                },
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "created_at".to_string(),
                    param: "end_at".to_string()
                },
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "email".to_string(),
                    param: "email_pattern".to_string()
                },
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "slug".to_string(),
                    param: "slug_pattern".to_string()
                },
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "external_id".to_string(),
                    param: "external_id".to_string()
                },
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "parent_id".to_string(),
                    param: "parent_id".to_string()
                }
            ]
        );
    }

    #[test]
    fn ast_visitor_extracts_params_from_non_where_query_paths() {
        let statement = parse_select_with_sqlparser(
            "
            SELECT count(*) FILTER (WHERE e.kind = :kind) AS filtered_count
            FROM emails e
            GROUP BY e.user_id
            HAVING e.created_at = :created_at
            ORDER BY CASE WHEN e.email = :email THEN 0 ELSE 1 END
            LIMIT CASE WHEN e.id = :limit_id THEN 1 ELSE 2 END
            ",
        )
        .unwrap();

        assert_eq!(
            statement.equality_params,
            vec![
                EqualityParam {
                    qualifier: Some("e".to_string()),
                    column: "kind".to_string(),
                    param: "kind".to_string()
                },
                EqualityParam {
                    qualifier: Some("e".to_string()),
                    column: "created_at".to_string(),
                    param: "created_at".to_string()
                },
                EqualityParam {
                    qualifier: Some("e".to_string()),
                    column: "email".to_string(),
                    param: "email".to_string()
                },
                EqualityParam {
                    qualifier: Some("e".to_string()),
                    column: "id".to_string(),
                    param: "limit_id".to_string()
                }
            ]
        );
    }

    #[test]
    fn parses_comma_separated_table_references() {
        let statement = parse_select(
            "SELECT u.id, o.name
             FROM users u, organizations o
             WHERE u.org_id = o.id AND u.id = :id",
        )
        .unwrap();

        assert_eq!(
            statement.table_refs,
            vec![
                TableReference {
                    name: "users".to_string(),
                    alias: Some("u".to_string()),
                    derived_query: None,
                    lateral: false,
                    nullable: false
                },
                TableReference {
                    name: "organizations".to_string(),
                    alias: Some("o".to_string()),
                    derived_query: None,
                    lateral: false,
                    nullable: false
                }
            ]
        );
    }

    #[test]
    fn nullable_parenthesized_groups_apply_to_comma_table_references() {
        let statement = parse_select(
            "SELECT u.id, o.name, a.slug
             FROM users u
             LEFT JOIN (organizations o, accounts a) ON o.id = u.org_id AND a.org_id = o.id
             WHERE u.id = :id",
        )
        .unwrap();

        assert_eq!(statement.table_refs.len(), 3);
        assert!(!statement.table_refs[0].nullable);
        assert!(statement.table_refs[1].nullable);
        assert!(statement.table_refs[2].nullable);
    }

    #[test]
    fn parses_cte_prefix_into_ir() {
        let statement = parse_select(
            "WITH filtered_users AS (
                SELECT id, email FROM users WHERE active = true
            )
            SELECT filtered_users.id FROM filtered_users WHERE filtered_users.id = :id",
        )
        .unwrap();

        assert_eq!(
            statement.ctes,
            vec![CommonTableExpression {
                name: "filtered_users".to_string(),
                columns: Vec::new(),
                query: "SELECT id, email FROM users WHERE active = true".to_string(),
                recursive: false,
            }]
        );
        assert_eq!(statement.table, "filtered_users");
        assert_eq!(
            statement.equality_params,
            vec![EqualityParam {
                qualifier: Some("filtered_users".to_string()),
                column: "id".to_string(),
                param: "id".to_string()
            }]
        );
    }

    #[test]
    fn parses_recursive_cte_prefix_into_ir() {
        let statement = parse_select(
            "WITH RECURSIVE tree(node_id, parent_node_id) AS (
                SELECT id, parent_id FROM nodes WHERE id = :root_id
                UNION ALL
                SELECT n.id, n.parent_id FROM nodes n JOIN tree t ON n.parent_id = t.id
            )
            SELECT tree.node_id FROM tree",
        )
        .unwrap();

        assert_eq!(statement.ctes.len(), 1);
        assert_eq!(statement.ctes[0].name, "tree");
        assert_eq!(
            statement.ctes[0].columns,
            vec!["node_id".to_string(), "parent_node_id".to_string()]
        );
        assert!(statement.ctes[0].recursive);
        assert_eq!(statement.table, "tree");
    }

    #[test]
    fn parses_derived_table_references() {
        let statement = parse_select(
            "SELECT u.id, o.name
             FROM (SELECT id, org_id FROM users WHERE active = true) AS u
             LEFT JOIN (SELECT id, name FROM organizations) o ON o.id = u.org_id
             WHERE u.id = :id",
        )
        .unwrap();

        assert_eq!(statement.table, "u");
        assert_eq!(statement.table_refs.len(), 2);
        assert_eq!(statement.table_refs[0].name, "u");
        assert_eq!(statement.table_refs[0].alias.as_deref(), Some("u"));
        assert_eq!(
            statement.table_refs[0].derived_query.as_deref(),
            Some("SELECT id, org_id FROM users WHERE active = true")
        );
        assert!(!statement.table_refs[0].nullable);
        assert_eq!(statement.table_refs[1].name, "o");
        assert_eq!(statement.table_refs[1].alias.as_deref(), Some("o"));
        assert_eq!(
            statement.table_refs[1].derived_query.as_deref(),
            Some("SELECT id, name FROM organizations")
        );
        assert!(statement.table_refs[1].nullable);
    }

    #[test]
    fn parses_lateral_derived_table_references() {
        let statement = parse_select(
            "SELECT u.id, recent.email
             FROM users u
             LEFT JOIN LATERAL (
                SELECT e.email
                FROM emails e
                WHERE e.user_id = u.id AND e.kind = :kind
                ORDER BY e.created_at DESC
                LIMIT 1
             ) AS recent ON true
             WHERE u.id = :id",
        )
        .unwrap();

        assert_eq!(statement.table, "users");
        assert_eq!(statement.table_refs.len(), 2);
        assert_eq!(statement.table_refs[0].name, "users");
        assert!(!statement.table_refs[0].lateral);
        assert_eq!(statement.table_refs[1].name, "recent");
        assert_eq!(statement.table_refs[1].alias.as_deref(), Some("recent"));
        assert!(statement.table_refs[1].lateral);
        assert!(statement.table_refs[1].nullable);
        assert_eq!(
            statement.table_refs[1].derived_query.as_deref(),
            Some(
                "SELECT e.email FROM emails e WHERE e.user_id = u.id AND e.kind = :kind ORDER BY e.created_at DESC LIMIT 1"
            )
        );
        assert_eq!(
            statement.equality_params,
            vec![
                EqualityParam {
                    qualifier: Some("e".to_string()),
                    column: "kind".to_string(),
                    param: "kind".to_string()
                },
                EqualityParam {
                    qualifier: Some("u".to_string()),
                    column: "id".to_string(),
                    param: "id".to_string()
                }
            ]
        );
    }

    #[test]
    fn ignores_nested_from_when_finding_top_level_select_parts() {
        let statement = parse_select(
            "SELECT id, (SELECT count(*) FROM organizations) AS org_count
             FROM users
             WHERE id = :id",
        )
        .unwrap();

        assert_eq!(statement.table, "users");
        assert_eq!(statement.projections.len(), 2);
        assert_eq!(
            statement.projections[1].expr,
            "(SELECT count(*) FROM organizations)"
        );
        assert_eq!(statement.projections[1].alias.as_deref(), Some("org_count"));
    }

    #[test]
    fn parses_compound_selects_into_ir() {
        let statement = parse_select(
            "SELECT id, email FROM users WHERE id = :id
             UNION ALL
             SELECT id, slug FROM organizations WHERE slug = :slug
             EXCEPT
             SELECT id, email FROM banned_users WHERE email = :email",
        )
        .unwrap();

        assert_eq!(statement.table, "users");
        assert_eq!(
            statement.equality_params,
            vec![EqualityParam {
                qualifier: None,
                column: "id".to_string(),
                param: "id".to_string()
            }]
        );
        assert_eq!(
            statement.compound,
            vec![
                CompoundSelect {
                    operator: CompoundOperator::UnionAll,
                    query: "SELECT id, slug FROM organizations WHERE slug = :slug".to_string(),
                },
                CompoundSelect {
                    operator: CompoundOperator::Except,
                    query: "SELECT id, email FROM banned_users WHERE email = :email".to_string(),
                }
            ]
        );
    }
}
