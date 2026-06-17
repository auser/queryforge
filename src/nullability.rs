use crate::ir::Nullability;

pub fn wrap_nullable(base: &str, nullability: &Nullability) -> String {
    match nullability {
        Nullability::NonNull => base.to_string(),
        Nullability::Nullable | Nullability::Unknown => format!("Option<{base}>"),
    }
}
