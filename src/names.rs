pub fn to_snake_case(input: &str) -> String {
    let mut out = String::new();
    let mut last_was_underscore = false;

    for (idx, ch) in input.chars().enumerate() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && idx > 0 && !last_was_underscore {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            last_was_underscore = false;
        } else if !last_was_underscore && !out.is_empty() {
            out.push('_');
            last_was_underscore = true;
        }
    }

    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        escape_rust_keyword(&trimmed)
    }
}

pub fn to_pascal_case(input: &str) -> String {
    let snake = to_snake_case(input);
    let mut out = String::new();
    for part in snake.trim_start_matches("r#").split('_') {
        if part.is_empty() {
            continue;
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
        }
    }
    if out.is_empty() {
        "Unnamed".to_string()
    } else {
        out
    }
}

pub fn escape_rust_keyword(name: &str) -> String {
    match name {
        "as" | "break" | "const" | "continue" | "crate" | "else" | "enum" | "extern" | "false"
        | "fn" | "for" | "if" | "impl" | "in" | "let" | "loop" | "match" | "mod" | "move"
        | "mut" | "pub" | "ref" | "return" | "self" | "Self" | "static" | "struct" | "super"
        | "trait" | "true" | "type" | "unsafe" | "use" | "where" | "while" | "async" | "await"
        | "dyn" => {
            format!("r#{name}")
        }
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_are_raw_identifiers() {
        assert_eq!(to_snake_case("type"), "r#type");
        assert_eq!(to_snake_case("userEmail"), "user_email");
    }
}
