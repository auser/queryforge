use nom::bytes::complete::{tag_no_case, take_while, take_while1};
use nom::character::complete::{char, multispace0, multispace1};
use nom::combinator::{map, opt, recognize};
use nom::sequence::delimited;
use nom::{IResult, Parser};

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
    pub query: String,
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
    let cleaned = trim_trailing_semicolon(sql.trim());
    let (select_sql, ctes) = split_leading_ctes(cleaned)?;
    let (select_sql, compound) = split_compound_selects(select_sql);
    let Ok((after_select, _)) = select_prefix(select_sql) else {
        return None;
    };
    let select_len = select_sql.len() - after_select.len();
    let from_idx = find_keyword_top_level(select_sql, "from")?;
    let projection_sql = select_sql[select_len..from_idx].trim();
    let after_from = select_sql[from_idx + "from".len()..].trim();
    let from_clause = leading_from_clause(after_from);
    let table_refs = parse_table_references(from_clause);
    let table = table_refs.first()?.name.clone();

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
    let mut ctes = Vec::new();
    loop {
        let (name, name_len) = parse_identifier_prefix(rest)?;
        rest = rest[name_len..].trim_start();

        if rest.starts_with('(') {
            let close = find_matching_paren(rest, 0)?;
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
            query: rest[1..close].trim().to_string(),
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

fn parse_table_references(from_clause: &str) -> Vec<TableReference> {
    let mut refs = Vec::new();
    let mut rest = from_clause.trim();

    if let Some((table_ref, consumed)) = parse_table_reference_prefix(rest, false) {
        refs.push(table_ref);
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
        if let Some((table_ref, consumed)) =
            parse_table_reference_prefix(rest, join_nullability.joined_nullable)
        {
            refs.push(table_ref);
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

fn parse_table_reference_prefix(input: &str, nullable: bool) -> Option<(TableReference, usize)> {
    let leading_ws = input.len() - input.trim_start().len();
    let input = input.trim_start();
    if input.starts_with('(') {
        let close = find_matching_paren(input, 0)?;
        let derived_query = input[1..close].trim().to_string();
        let mut consumed = close + 1;
        let (alias, alias_len) = parse_optional_alias(&input[consumed..]);
        consumed += alias_len;
        let alias = alias.unwrap_or_else(|| "subquery".to_string());
        return Some((
            TableReference {
                name: alias.clone(),
                alias: Some(alias),
                derived_query: Some(derived_query),
                nullable,
            },
            leading_ws + consumed,
        ));
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
        TableReference {
            name: table,
            alias,
            derived_query: None,
            nullable,
        },
        leading_ws + consumed,
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
                    nullable: false
                },
                TableReference {
                    name: "organizations".to_string(),
                    alias: Some("o".to_string()),
                    derived_query: None,
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
                    nullable: true
                },
                TableReference {
                    name: "organizations".to_string(),
                    alias: Some("o".to_string()),
                    derived_query: None,
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
                query: "SELECT id, email FROM users WHERE active = true".to_string(),
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
