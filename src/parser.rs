use std::fs;
use std::path::Path;

use nom::bytes::complete::{tag, take_while, take_while1};
use nom::character::complete::{char, line_ending, not_line_ending, space0, space1};
use nom::combinator::{opt, recognize};
use nom::{IResult, Parser};

use crate::error::{Error, Result};
use crate::ir::{
    Cardinality, ParsedQuery, RustType, TypeOverride, TypeOverrideTarget, TypeOverrides,
};

pub fn parse_dir(path: &Path) -> Result<Vec<ParsedQuery>> {
    let mut out = Vec::new();
    let mut entries = fs::read_dir(path)
        .map_err(|err| Error::Config(format!("failed to read {}: {err}", path.display())))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|err| {
            Error::Config(format!("failed to read entry in {}: {err}", path.display()))
        })?;

    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();

        if path.is_dir() {
            out.extend(parse_dir(&path)?);
            continue;
        }

        if path.extension().and_then(|value| value.to_str()) != Some("sql") {
            continue;
        }

        out.extend(parse_file(&path)?);
    }

    Ok(out)
}

pub fn parse_queries_dir(path: &Path) -> Result<Vec<ParsedQuery>> {
    parse_dir(path)
}

pub fn parse_file(path: &Path) -> Result<Vec<ParsedQuery>> {
    let source = fs::read_to_string(path)
        .map_err(|err| Error::Config(format!("failed to read {}: {err}", path.display())))?;

    parse_queries(&source, path)
}

pub fn parse_queries(source: &str, path: &Path) -> Result<Vec<ParsedQuery>> {
    let blocks = split_query_blocks(source);

    let mut parsed = Vec::new();

    for block in blocks {
        let query = parse_query_block(block, path)?;
        parsed.push(query);
    }

    Ok(parsed)
}

fn split_query_blocks(source: &str) -> Vec<&str> {
    let mut starts = Vec::new();

    for (offset, line) in source.match_indices("--!") {
        if offset == 0 || source[..offset].ends_with('\n') {
            let after = &source[offset..];
            if after.starts_with("--!") && line == "--!" {
                starts.push(offset);
            }
        }
    }

    let mut blocks = Vec::new();

    for (index, start) in starts.iter().copied().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(source.len());
        let block = source[start..end].trim();

        if !block.is_empty() {
            blocks.push(block);
        }
    }

    blocks
}

fn parse_query_block(block: &str, path: &Path) -> Result<ParsedQuery> {
    let (_, header) = query_header(block).map_err(|err| {
        Error::Parse(format!(
            "failed to parse query header in {}: {err:?}",
            path.display()
        ))
    })?;

    let mut type_overrides = TypeOverrides::default();
    let mut sql_lines = Vec::new();
    for line in block.lines().skip(1) {
        let trimmed = line.trim_start();
        if let Some(directive) = trimmed.strip_prefix("--:") {
            type_overrides
                .entries
                .push(parse_type_override(directive.trim(), &header.name, path)?);
            continue;
        }
        if trimmed.starts_with("--#") {
            continue;
        }
        sql_lines.push(line);
    }

    let sql = sql_lines.join("\n").trim().to_string();

    if sql.is_empty() {
        return Err(Error::Parse(format!(
            "query `{}` in {} has no SQL body",
            header.name,
            path.display()
        )));
    }

    let cardinality = header
        .cardinality
        .unwrap_or_else(|| infer_cardinality_from_sql(&sql));

    Ok(ParsedQuery {
        name: header.name,
        source_file: path.to_path_buf(),
        original_sql: sql,
        cardinality,
        type_overrides,
    })
}

#[derive(Debug, Clone)]
struct QueryHeader {
    name: String,
    cardinality: Option<Cardinality>,
}

fn query_header(input: &str) -> IResult<&str, QueryHeader> {
    let (input, _) = tag("--!").parse(input)?;
    let (input, _) = space1.parse(input)?;
    let (input, name) = identifier.parse(input)?;
    let (input, _) = space0.parse(input)?;
    let (input, has_cardinality) = opt(char(':')).parse(input)?;
    let (input, cardinality) = if has_cardinality.is_some() {
        let (input, _) = space0.parse(input)?;
        let (input, cardinality) = cardinality.parse(input)?;
        (input, Some(cardinality))
    } else {
        (input, None)
    };
    let (input, _) = not_line_ending.parse(input)?;
    let (input, _) = opt(line_ending).parse(input)?;

    Ok((
        input,
        QueryHeader {
            name: name.to_string(),
            cardinality,
        },
    ))
}

fn identifier(input: &str) -> IResult<&str, &str> {
    recognize((take_while1(is_ident_start), take_while(is_ident_continue))).parse(input)
}

fn cardinality(input: &str) -> IResult<&str, Cardinality> {
    let (rest, value) = identifier(input)?;
    let cardinality = Cardinality::parse(value)
        .map_err(|_| nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))?;
    Ok((rest, cardinality))
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn infer_cardinality_from_sql(sql: &str) -> Cardinality {
    match first_sql_keyword(sql).as_deref() {
        Some("select" | "with") => Cardinality::Many,
        Some("insert") if contains_sql_keyword(sql, "returning") => Cardinality::One,
        Some("update" | "delete") if contains_sql_keyword(sql, "returning") => Cardinality::Many,
        _ => Cardinality::Exec,
    }
}

fn parse_type_override(directive: &str, query_name: &str, path: &Path) -> Result<TypeOverride> {
    let Some((target, rust_type)) = directive.split_once(':') else {
        return Err(Error::Parse(format!(
            "invalid type override for query `{query_name}` in {}; expected `--: name: RustType`, `--: param.name: RustType`, or `--: column.name: RustType`",
            path.display()
        )));
    };
    let target = target.trim();
    let rust_type = rust_type.trim();
    if target.is_empty() || rust_type.is_empty() {
        return Err(Error::Parse(format!(
            "invalid type override for query `{query_name}` in {}; override target and type must be non-empty",
            path.display()
        )));
    }

    let (target_kind, name) = if let Some(name) = target.strip_prefix("param.") {
        (TypeOverrideTarget::Param, name)
    } else if let Some(name) = target.strip_prefix("column.") {
        (TypeOverrideTarget::Column, name)
    } else {
        (TypeOverrideTarget::Any, target)
    };

    if name.is_empty() {
        return Err(Error::Parse(format!(
            "invalid type override for query `{query_name}` in {}; override name must be non-empty",
            path.display()
        )));
    }

    Ok(TypeOverride {
        target: target_kind,
        name: name.to_string(),
        rust_type: RustType::new(rust_type),
    })
}

fn first_sql_keyword(sql: &str) -> Option<String> {
    let bytes = sql.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        while bytes.get(idx).is_some_and(u8::is_ascii_whitespace) {
            idx += 1;
        }

        if bytes.get(idx..idx + 2) == Some(b"--") {
            idx += 2;
            while idx < bytes.len() && bytes[idx] != b'\n' {
                idx += 1;
            }
            continue;
        }

        if bytes.get(idx..idx + 2) == Some(b"/*") {
            idx += 2;
            while idx + 1 < bytes.len() && bytes.get(idx..idx + 2) != Some(b"*/") {
                idx += 1;
            }
            idx = (idx + 2).min(bytes.len());
            continue;
        }

        if bytes
            .get(idx)
            .is_some_and(|byte| byte.is_ascii_alphabetic())
        {
            let start = idx;
            idx += 1;
            while bytes
                .get(idx)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            {
                idx += 1;
            }
            return Some(sql[start..idx].to_ascii_lowercase());
        }

        return None;
    }

    None
}

fn contains_sql_keyword(sql: &str, keyword: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut idx = 0;
    let keyword = keyword.to_ascii_lowercase();

    while idx < bytes.len() {
        if bytes.get(idx..idx + 2) == Some(b"--") {
            idx += 2;
            while idx < bytes.len() && bytes[idx] != b'\n' {
                idx += 1;
            }
            continue;
        }

        if bytes.get(idx..idx + 2) == Some(b"/*") {
            idx += 2;
            while idx + 1 < bytes.len() && bytes.get(idx..idx + 2) != Some(b"*/") {
                idx += 1;
            }
            idx = (idx + 2).min(bytes.len());
            continue;
        }

        if bytes.get(idx) == Some(&b'\'') {
            idx += 1;
            while idx < bytes.len() {
                if bytes[idx] == b'\'' {
                    idx += 1;
                    if bytes.get(idx) == Some(&b'\'') {
                        idx += 1;
                        continue;
                    }
                    break;
                }
                idx += 1;
            }
            continue;
        }

        if bytes.get(idx) == Some(&b'"') {
            idx += 1;
            while idx < bytes.len() {
                if bytes[idx] == b'"' {
                    idx += 1;
                    if bytes.get(idx) == Some(&b'"') {
                        idx += 1;
                        continue;
                    }
                    break;
                }
                idx += 1;
            }
            continue;
        }

        if bytes
            .get(idx)
            .is_some_and(|byte| byte.is_ascii_alphabetic())
        {
            let start = idx;
            idx += 1;
            while bytes
                .get(idx)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            {
                idx += 1;
            }
            if sql[start..idx].eq_ignore_ascii_case(&keyword) {
                return true;
            }
            continue;
        }

        idx += 1;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_query_blocks() {
        let source = "--! get_user : one\nSELECT id FROM users WHERE id = :id;\n\n--! list_users : many\nSELECT id FROM users;";
        let parsed = parse_queries(source, Path::new("queries/users.sql")).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "get_user");
        assert_eq!(parsed[0].cardinality, Cardinality::One);
        assert_eq!(
            parsed[0].original_sql,
            "SELECT id FROM users WHERE id = :id;"
        );
        assert_eq!(parsed[0].source_file, Path::new("queries/users.sql"));
        assert_eq!(parsed[1].name, "list_users");
        assert_eq!(parsed[1].cardinality, Cardinality::Many);
    }

    #[test]
    fn infers_cardinality_when_header_omits_it() {
        let source = "--! insert_user\nINSERT INTO users (email) VALUES (:email);\n\n--! list_users\nSELECT id FROM users;";
        let parsed = parse_queries(source, Path::new("queries/users.sql")).unwrap();

        assert_eq!(parsed[0].name, "insert_user");
        assert_eq!(parsed[0].cardinality, Cardinality::Exec);
        assert_eq!(parsed[1].name, "list_users");
        assert_eq!(parsed[1].cardinality, Cardinality::Many);
    }

    #[test]
    fn infers_returning_mutation_cardinality_when_header_omits_it() {
        let source = "--! insert_user\nINSERT INTO users (email) VALUES (:email) RETURNING id;\n\n--! update_users\nUPDATE users SET active = true RETURNING id;\n\n--! delete_users\nDELETE FROM users WHERE active = false RETURNING id;";
        let parsed = parse_queries(source, Path::new("queries/users.sql")).unwrap();

        assert_eq!(parsed[0].name, "insert_user");
        assert_eq!(parsed[0].cardinality, Cardinality::One);
        assert_eq!(parsed[1].name, "update_users");
        assert_eq!(parsed[1].cardinality, Cardinality::Many);
        assert_eq!(parsed[2].name, "delete_users");
        assert_eq!(parsed[2].cardinality, Cardinality::Many);
    }

    #[test]
    fn explicit_cardinality_overrides_sql_inference() {
        let source =
            "--! insert_user : one\nINSERT INTO users (email) VALUES (:email) RETURNING id;";
        let parsed = parse_queries(source, Path::new("queries/users.sql")).unwrap();

        assert_eq!(parsed[0].cardinality, Cardinality::One);
    }

    #[test]
    fn rejects_unknown_cardinality() {
        let err = parse_queries(
            "--! get_user : exactly_one\nSELECT 1;",
            Path::new("queries/users.sql"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("failed to parse query header"));
    }

    #[test]
    fn parses_type_override_annotation_lines() {
        let parsed = parse_queries(
            "--! annotated : many\n--: param.id: UserId\n--: column.email: EmailAddress\n--: country: CountryCode\n--# future directive\nSELECT id, email, country FROM users WHERE id = :id;",
            Path::new("queries/annotated.sql"),
        )
        .unwrap();

        assert_eq!(
            parsed[0].original_sql,
            "SELECT id, email, country FROM users WHERE id = :id;"
        );
        assert_eq!(parsed[0].type_overrides.entries.len(), 3);
        assert_eq!(
            parsed[0].type_overrides.entries[0].target,
            TypeOverrideTarget::Param
        );
        assert_eq!(parsed[0].type_overrides.entries[0].name, "id");
        assert_eq!(parsed[0].type_overrides.entries[0].rust_type.0, "UserId");
        assert_eq!(
            parsed[0].type_overrides.entries[1].target,
            TypeOverrideTarget::Column
        );
        assert_eq!(parsed[0].type_overrides.entries[1].name, "email");
        assert_eq!(
            parsed[0].type_overrides.entries[1].rust_type.0,
            "EmailAddress"
        );
        assert_eq!(
            parsed[0].type_overrides.entries[2].target,
            TypeOverrideTarget::Any
        );
        assert_eq!(parsed[0].type_overrides.entries[2].name, "country");
        assert_eq!(
            parsed[0].type_overrides.entries[2].rust_type.0,
            "CountryCode"
        );
    }
}
