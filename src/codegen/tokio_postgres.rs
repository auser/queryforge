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
    let param_slice = render_param_slice(query);

    let body = match query.cardinality {
        Cardinality::Exec => quote! {
            pub async fn #fn_name<C>(client: &C #(, #param_args)*) -> Result<u64, tokio_postgres::Error>
            where
                C: tokio_postgres::GenericClient + Sync,
            {
                #param_slice
                client.execute(#sql_const, params).await
            }
        },
        Cardinality::Optional => quote! {
            #row_tokens
            pub async fn #fn_name<C>(client: &C #(, #param_args)*) -> Result<Option<#row_name>, tokio_postgres::Error>
            where
                C: tokio_postgres::GenericClient + Sync,
            {
                #param_slice
                client
                    .query_opt(#sql_const, params)
                    .await?
                    .map(#row_name::try_from)
                    .transpose()
            }
        },
        Cardinality::Many | Cardinality::Stream | Cardinality::Batch => quote! {
            #row_tokens
            pub async fn #fn_name<C>(client: &C #(, #param_args)*) -> Result<Vec<#row_name>, tokio_postgres::Error>
            where
                C: tokio_postgres::GenericClient + Sync,
            {
                #param_slice
                client
                    .query(#sql_const, params)
                    .await?
                    .into_iter()
                    .map(#row_name::try_from)
                    .collect()
            }
        },
        Cardinality::One | Cardinality::Scalar => quote! {
            #row_tokens
            pub async fn #fn_name<C>(client: &C #(, #param_args)*) -> Result<#row_name, tokio_postgres::Error>
            where
                C: tokio_postgres::GenericClient + Sync,
            {
                #param_slice
                let row = client.query_one(#sql_const, params).await?;
                #row_name::try_from(row)
            }
        },
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

fn render_row_struct(query: &QueryShape, row_name: &proc_macro2::Ident) -> TokenStream {
    if query.columns.is_empty() {
        return TokenStream::new();
    }

    let fields = query.columns.iter().map(|column| {
        let field = snake_ident(&column.rust_name);
        let ty = rust_type(&column.rust_type.0, &column.nullable);
        quote! { pub #field: #ty }
    });
    let getters = query.columns.iter().map(|column| {
        let field = snake_ident(&column.rust_name);
        let name = lit_str(&column.name);
        quote! { #field: row.try_get(#name)? }
    });

    quote! {
        #[derive(Debug, Clone)]
        pub struct #row_name {
            #( #fields, )*
        }

        impl TryFrom<tokio_postgres::Row> for #row_name {
            type Error = tokio_postgres::Error;

            fn try_from(row: tokio_postgres::Row) -> Result<Self, Self::Error> {
                Ok(Self {
                    #( #getters, )*
                })
            }
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

fn render_param_slice(query: &QueryShape) -> TokenStream {
    if query.params.is_empty() {
        return quote! {
            let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[];
        };
    }

    let params = query.params.iter().map(|param| snake_ident(&param.name));
    quote! {
        let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[#( &#params ),*];
    }
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
    fn renders_generic_client_fetch_one_with_row_mapping() {
        let rendered = render_query(&query(Cardinality::One));

        assert!(rendered.contains("C: tokio_postgres::GenericClient + Sync"));
        assert!(rendered.contains("pub struct GetUserRow"));
        assert!(rendered.contains("impl TryFrom<tokio_postgres::Row> for GetUserRow"));
        assert!(rendered
            .contains("let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[&id];"));
        assert!(rendered.contains("client.query_one(GET_USER_SQL, params).await?"));
        assert!(rendered.contains("email: row.try_get(\"email\")?"));
    }

    #[test]
    fn documents_transaction_compatible_generic_client_shape() {
        let rendered = render_query(&query(Cardinality::One));

        assert!(rendered.contains("pub async fn get_user<C>("));
        assert!(rendered.contains("client: &C,"));
        assert!(rendered.contains("id: i64,"));
        assert!(rendered.contains("C: tokio_postgres::GenericClient + Sync"));
        assert!(rendered.contains("client.query_one(GET_USER_SQL, params).await?"));
    }

    #[test]
    fn renders_optional_and_many_cardinalities() {
        let optional = render_query(&query(Cardinality::Optional));
        assert!(optional.contains(".query_opt(GET_USER_SQL, params)"));
        assert!(optional.contains(".transpose()"));

        let many = render_query(&query(Cardinality::Many));
        assert!(many.contains(".query(GET_USER_SQL, params)"));
        assert!(many.contains(".collect()"));
    }

    #[test]
    fn renders_exec_as_execute_count() {
        let rendered = render_query(&QueryShape {
            cardinality: Cardinality::Exec,
            columns: Vec::new(),
            ..query(Cardinality::Exec)
        });

        assert!(rendered.contains("Result<u64, tokio_postgres::Error>"));
        assert!(rendered.contains("client.execute(GET_USER_SQL, params).await"));
    }

    #[test]
    fn render_query_matches_snapshot() {
        assert_eq!(
            render_query(&query(Cardinality::One)),
            concat!(
                "pub const GET_USER_SQL: &str = \"SELECT id, email FROM users WHERE id = $1\";\n",
                "pub const GET_USER_FINGERPRINT: &str = \"fnv1a64:6d9ac38b89586d5b\";\n",
                "pub fn get_user_sql() -> &'static str {\n",
                "    GET_USER_SQL\n",
                "}\n",
                "#[derive(Debug, Clone)]\n",
                "pub struct GetUserRow {\n",
                "    pub id: i64,\n",
                "    pub email: Option<String>,\n",
                "}\n",
                "impl TryFrom<tokio_postgres::Row> for GetUserRow {\n",
                "    type Error = tokio_postgres::Error;\n",
                "    fn try_from(row: tokio_postgres::Row) -> Result<Self, Self::Error> {\n",
                "        Ok(Self {\n",
                "            id: row.try_get(\"id\")?,\n",
                "            email: row.try_get(\"email\")?,\n",
                "        })\n",
                "    }\n",
                "}\n",
                "pub async fn get_user<C>(\n",
                "    client: &C,\n",
                "    id: i64,\n",
                ") -> Result<GetUserRow, tokio_postgres::Error>\n",
                "where\n",
                "    C: tokio_postgres::GenericClient + Sync,\n",
                "{\n",
                "    let params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[&id];\n",
                "    let row = client.query_one(GET_USER_SQL, params).await?;\n",
                "    GetUserRow::try_from(row)\n",
                "}\n"
            )
        );
    }

    fn query(cardinality: Cardinality) -> QueryShape {
        QueryShape {
            name: "get_user".to_string(),
            module_path: vec!["users".to_string()],
            source_file: PathBuf::from("queries/users.sql"),
            original_sql: "SELECT id, email FROM users WHERE id = $1".to_string(),
            normalized_sql: "SELECT id, email FROM users WHERE id = $1".to_string(),
            cardinality,
            params: vec![QueryParam {
                name: "id".to_string(),
                position: 1,
                db_type: Some("postgres:int8".to_string()),
                rust_type: RustType::new("i64"),
                source: TypeSource::DatabaseMetadata,
                confidence: InferenceConfidence::Exact,
            }],
            columns: vec![
                QueryColumn {
                    name: "id".to_string(),
                    rust_name: "id".to_string(),
                    db_type: Some("postgres:int8".to_string()),
                    rust_type: RustType::new("i64"),
                    nullable: Nullability::NonNull,
                    source: TypeSource::DatabaseMetadata,
                    confidence: InferenceConfidence::Exact,
                },
                QueryColumn {
                    name: "email".to_string(),
                    rust_name: "email".to_string(),
                    db_type: Some("postgres:text".to_string()),
                    rust_type: RustType::string(),
                    nullable: Nullability::Nullable,
                    source: TypeSource::DatabaseMetadata,
                    confidence: InferenceConfidence::Exact,
                },
            ],
            dependencies: QueryDependencies::default(),
            fingerprint: Fingerprint::from_text("get_user"),
        }
    }
}
