use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Cardinality, QueryShape};

use super::{
    format_query_tokens, lit_str, parse_type, pascal_ident, rust_type, snake_ident, upper_ident,
};

pub fn render_query(query: &QueryShape) -> String {
    format_query_tokens(render_query_tokens(query))
}

fn render_query_tokens(query: &QueryShape) -> TokenStream {
    let fn_name = snake_ident(&query.name);
    let sql_const = upper_ident(&query.name, "SQL");
    let fingerprint_const = upper_ident(&query.name, "FINGERPRINT");
    let sql_fn = snake_ident(&format!("{}_sql", query.name));
    let sql = lit_str(&query.normalized_sql);
    let fingerprint = lit_str(query.fingerprint.as_str());
    let row_name = pascal_ident(&format!("{}_row", query.name));
    let row_tokens = render_row_struct(query, &row_name);
    let param_args = render_param_args(query);
    let param_idents = query
        .params
        .iter()
        .map(|param| snake_ident(&param.name))
        .collect::<Vec<_>>();

    let body = match query.cardinality {
        Cardinality::Exec => {
            let query_expr = bind_all(quote! { sqlx::query(#sql_const) }, &param_idents);
            quote! {
                pub async fn #fn_name<'e, E>(executor: E #(, #param_args)*) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error>
                where
                    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
                {
                    #query_expr.execute(executor).await
                }
            }
        }
        Cardinality::Optional => {
            let query_expr = bind_all(
                quote! { sqlx::query_as::<_, #row_name>(#sql_const) },
                &param_idents,
            );
            quote! {
                #row_tokens
                pub async fn #fn_name<'e, E>(executor: E #(, #param_args)*) -> Result<Option<#row_name>, sqlx::Error>
                where
                    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
                {
                    #query_expr.fetch_optional(executor).await
                }
            }
        }
        Cardinality::Many | Cardinality::Stream | Cardinality::Batch => {
            let query_expr = bind_all(
                quote! { sqlx::query_as::<_, #row_name>(#sql_const) },
                &param_idents,
            );
            quote! {
                #row_tokens
                pub async fn #fn_name<'e, E>(executor: E #(, #param_args)*) -> Result<Vec<#row_name>, sqlx::Error>
                where
                    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
                {
                    #query_expr.fetch_all(executor).await
                }
            }
        }
        Cardinality::One | Cardinality::Scalar => {
            let query_expr = bind_all(
                quote! { sqlx::query_as::<_, #row_name>(#sql_const) },
                &param_idents,
            );
            quote! {
                #row_tokens
                pub async fn #fn_name<'e, E>(executor: E #(, #param_args)*) -> Result<#row_name, sqlx::Error>
                where
                    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
                {
                    #query_expr.fetch_one(executor).await
                }
            }
        }
    };

    quote! {
        pub const #sql_const: &str = #sql;
        pub const #fingerprint_const: &str = #fingerprint;
        pub fn #sql_fn() -> &'static str {
            #sql_const
        }
        #body
    }
}

fn bind_all(mut query_expr: TokenStream, params: &[proc_macro2::Ident]) -> TokenStream {
    for param in params {
        query_expr = quote! { #query_expr.bind(#param) };
    }
    query_expr
}

fn render_row_struct(query: &QueryShape, row_name: &proc_macro2::Ident) -> TokenStream {
    if query.columns.is_empty() {
        return TokenStream::new();
    }

    let fields = query.columns.iter().map(|column| {
        let field = snake_ident(&column.rust_name);
        let ty = rust_type(&column.rust_type.0, &column.nullable);
        quote! { pub #field: #ty }
    });

    quote! {
        #[derive(Debug, Clone, sqlx::FromRow)]
        pub struct #row_name {
            #( #fields, )*
        }
    }
}

fn render_param_args(query: &QueryShape) -> Vec<TokenStream> {
    query
        .params
        .iter()
        .map(|param| {
            let name = snake_ident(&param.name);
            let ty = parse_type(&param.rust_type.0);
            quote! { #name: #ty }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::Fingerprint;
    use crate::ir::{
        InferenceConfidence, Nullability, QueryColumn, QueryDependencies, QueryParam, RustType,
        TypeSource,
    };
    use std::path::PathBuf;

    #[test]
    fn renders_transaction_compatible_executor_shape() {
        let rendered = render_query(&query(Cardinality::One));

        assert!(rendered.contains("pub async fn get_user<'e, E>(executor: E, id: i64)"));
        assert!(rendered.contains("E: sqlx::Executor<'e, Database = sqlx::Sqlite>"));
        assert!(rendered.contains(".fetch_one(executor)"));
    }

    #[test]
    fn render_query_matches_snapshot() {
        assert_eq!(
            render_query(&query(Cardinality::One)),
            concat!(
                "pub const GET_USER_SQL: &str = \"SELECT id, email FROM users WHERE id = ?1\";\n",
                "pub const GET_USER_FINGERPRINT: &str = \"fnv1a64:6d9ac38b89586d5b\";\n",
                "pub fn get_user_sql() -> &'static str {\n",
                "    GET_USER_SQL\n",
                "}\n",
                "#[derive(Debug, Clone, sqlx::FromRow)]\n",
                "pub struct GetUserRow {\n",
                "    pub id: i64,\n",
                "    pub email: Option<String>,\n",
                "}\n",
                "pub async fn get_user<'e, E>(executor: E, id: i64) -> Result<GetUserRow, sqlx::Error>\n",
                "where\n",
                "    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,\n",
                "{\n",
                "    sqlx::query_as::<_, GetUserRow>(GET_USER_SQL).bind(id).fetch_one(executor).await\n",
                "}\n"
            )
        );
    }

    fn query(cardinality: Cardinality) -> QueryShape {
        QueryShape {
            name: "get_user".to_string(),
            module_path: vec!["users".to_string()],
            source_file: PathBuf::from("queries/users.sql"),
            original_sql: "SELECT id, email FROM users WHERE id = ?1".to_string(),
            normalized_sql: "SELECT id, email FROM users WHERE id = ?1".to_string(),
            cardinality,
            params: vec![QueryParam {
                name: "id".to_string(),
                position: 1,
                db_type: Some("sqlite:INTEGER".to_string()),
                rust_type: RustType::new("i64"),
                source: TypeSource::SchemaCatalog,
                confidence: InferenceConfidence::Strong,
            }],
            columns: vec![
                QueryColumn {
                    name: "id".to_string(),
                    rust_name: "id".to_string(),
                    db_type: Some("sqlite:INTEGER".to_string()),
                    rust_type: RustType::new("i64"),
                    nullable: Nullability::NonNull,
                    source: TypeSource::SchemaCatalog,
                    confidence: InferenceConfidence::Strong,
                },
                QueryColumn {
                    name: "email".to_string(),
                    rust_name: "email".to_string(),
                    db_type: Some("sqlite:TEXT".to_string()),
                    rust_type: RustType::string(),
                    nullable: Nullability::Nullable,
                    source: TypeSource::SchemaCatalog,
                    confidence: InferenceConfidence::Strong,
                },
            ],
            dependencies: QueryDependencies::default(),
            fingerprint: Fingerprint::from_text("get_user"),
        }
    }
}
