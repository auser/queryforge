use std::fs;
use std::path::Path;

use nom::bytes::complete::{tag, take_while, take_while1};
use nom::character::complete::{char, line_ending, not_line_ending, space0, space1};
use nom::combinator::{opt, recognize};
use nom::{IResult, Parser};

use crate::error::{Error, Result};
use crate::ir::{Cardinality, ParsedQuery};

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

    let sql = block
        .lines()
        .skip(1)
        .filter(|line| !line.trim_start().starts_with("--:"))
        .filter(|line| !line.trim_start().starts_with("--#"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if sql.is_empty() {
        return Err(Error::Parse(format!(
            "query `{}` in {} has no SQL body",
            header.name,
            path.display()
        )));
    }

    Ok(ParsedQuery {
        name: header.name,
        source_file: path.to_path_buf(),
        original_sql: sql,
        cardinality: header.cardinality,
    })
}

#[derive(Debug, Clone)]
struct QueryHeader {
    name: String,
    cardinality: Cardinality,
}

fn query_header(input: &str) -> IResult<&str, QueryHeader> {
    let (input, _) = tag("--!").parse(input)?;
    let (input, _) = space1.parse(input)?;
    let (input, name) = identifier.parse(input)?;
    let (input, _) = space0.parse(input)?;
    let (input, _) = char(':').parse(input)?;
    let (input, _) = space0.parse(input)?;
    let (input, cardinality) = cardinality.parse(input)?;
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
    fn rejects_unknown_cardinality() {
        let err = parse_queries(
            "--! get_user : exactly_one\nSELECT 1;",
            Path::new("queries/users.sql"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("failed to parse query header"));
    }

    #[test]
    fn ignores_queryforge_annotation_lines_in_body() {
        let parsed = parse_queries(
            "--! annotated : many\n--: future type annotation\n--# future directive\nSELECT 1;",
            Path::new("queries/annotated.sql"),
        )
        .unwrap();

        assert_eq!(parsed[0].original_sql, "SELECT 1;");
    }
}
