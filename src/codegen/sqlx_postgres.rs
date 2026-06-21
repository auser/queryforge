use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Cardinality, Nullability, QueryShape};

use super::{
    format_query_tokens, lit_str, params_arg, pascal_ident, render_params_struct, rendered_columns,
    rendered_params, snake_ident, upper_ident, RenderedParam,
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
    let params_arg = params_arg(query);
    let params_struct = render_params_struct(query);
    let params = rendered_params(query);

    let body = match query.cardinality {
        Cardinality::Exec => {
            let query_expr = bind_all(quote! { sqlx::query(#sql_const) }, &params);
            quote! {
                pub async fn #fn_name<'e, E>(executor: E #params_arg) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error>
                where
                    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
                {
                    #query_expr.execute(executor).await
                }
            }
        }
        Cardinality::Optional => {
            let query_expr = bind_all(
                quote! { sqlx::query_as::<_, #row_name>(#sql_const) },
                &params,
            );
            quote! {
                #row_tokens
                pub async fn #fn_name<'e, E>(executor: E #params_arg) -> Result<Option<#row_name>, sqlx::Error>
                where
                    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
                {
                    #query_expr.fetch_optional(executor).await
                }
            }
        }
        Cardinality::Many | Cardinality::Stream | Cardinality::Batch => {
            let query_expr = bind_all(
                quote! { sqlx::query_as::<_, #row_name>(#sql_const) },
                &params,
            );
            quote! {
                #row_tokens
                pub async fn #fn_name<'e, E>(executor: E #params_arg) -> Result<Vec<#row_name>, sqlx::Error>
                where
                    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
                {
                    #query_expr.fetch_all(executor).await
                }
            }
        }
        Cardinality::One | Cardinality::Scalar => {
            let query_expr = bind_all(
                quote! { sqlx::query_as::<_, #row_name>(#sql_const) },
                &params,
            );
            quote! {
                #row_tokens
                pub async fn #fn_name<'e, E>(executor: E #params_arg) -> Result<#row_name, sqlx::Error>
                where
                    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
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
        #params_struct
        #body
    }
}

fn bind_all(mut query_expr: TokenStream, params: &[RenderedParam]) -> TokenStream {
    for param in params {
        let field = &param.field;
        let value = if param.encode_override {
            quote! { queryforge::QueryForgeEncode::queryforge_encode(params.#field) }
        } else {
            quote! { params.#field }
        };
        query_expr = quote! { #query_expr.bind(#value) };
    }
    query_expr
}

fn render_row_struct(query: &QueryShape, row_name: &proc_macro2::Ident) -> TokenStream {
    if query.columns.is_empty() {
        return TokenStream::new();
    }

    let columns = rendered_columns(query);
    let fields = columns.iter().map(|column| {
        let field = &column.field;
        let ty = &column.ty;
        quote! { pub #field: #ty }
    });
    let getters = columns.iter().map(|column| {
        let field = &column.field;
        let index = &column.index;
        if column.decode_override {
            let base_ty = &column.base_ty;
            match column.nullable {
                Nullability::Nullable => quote! {
                    #field: {
                        let raw: Option<<#base_ty as queryforge::QueryForgeDecode>::Storage> = row.try_get(#index)?;
                        raw.map(<#base_ty as queryforge::QueryForgeDecode>::queryforge_decode)
                            .transpose()
                            .map_err(|err| sqlx::Error::Decode(Box::new(err)))?
                    }
                },
                Nullability::NonNull | Nullability::Unknown => quote! {
                    #field: {
                        let raw: <#base_ty as queryforge::QueryForgeDecode>::Storage = row.try_get(#index)?;
                        <#base_ty as queryforge::QueryForgeDecode>::queryforge_decode(raw)
                            .map_err(|err| sqlx::Error::Decode(Box::new(err)))?
                    }
                },
            }
        } else {
            quote! { #field: row.try_get(#index)? }
        }
    });

    quote! {
        #[derive(Debug, Clone)]
        pub struct #row_name {
            #( #fields, )*
        }

        impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for #row_name {
            fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
                use sqlx::Row as _;
                Ok(Self {
                    #( #getters, )*
                })
            }
        }
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
    fn renders_executor_style_fetch_one_with_binds() {
        let rendered = render_query(&query(Cardinality::One));

        assert!(rendered.contains("E: sqlx::Executor<'e, Database = sqlx::Postgres>"));
        assert!(rendered.contains("pub struct GetUserParams"));
        assert!(rendered.contains("pub struct GetUserRow"));
        assert!(rendered.contains("pub id: i64"));
        assert!(rendered.contains("pub email: Option<String>"));
        assert!(rendered.contains(".bind(params.id)"));
        assert!(rendered.contains(".fetch_one(executor)"));
    }

    #[test]
    fn documents_transaction_compatible_executor_shape() {
        let rendered = render_query(&query(Cardinality::One));

        assert!(rendered.contains("pub async fn get_user<'e, E>("));
        assert!(rendered.contains("executor: E,"));
        assert!(rendered.contains("params: GetUserParams,"));
        assert!(rendered.contains("E: sqlx::Executor<'e, Database = sqlx::Postgres>"));
        assert!(rendered.contains(".fetch_one(executor)"));
    }

    #[test]
    fn renders_exec_with_query_result() {
        let rendered = render_query(&QueryShape {
            cardinality: Cardinality::Exec,
            columns: Vec::new(),
            ..query(Cardinality::Exec)
        });

        assert!(rendered.contains("Result<sqlx::postgres::PgQueryResult, sqlx::Error>"));
        assert!(rendered.contains("sqlx::query(GET_USER_SQL)"));
        assert!(rendered.contains(".execute(executor)"));
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
                "pub struct GetUserParams {\n",
                "    pub id: i64,\n",
                "}\n",
                "#[derive(Debug, Clone)]\n",
                "pub struct GetUserRow {\n",
                "    pub id: i64,\n",
                "    pub email: Option<String>,\n",
                "}\n",
                "impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for GetUserRow {\n",
                "    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {\n",
                "        use sqlx::Row as _;\n",
                "        Ok(Self {\n",
                "            id: row.try_get(0)?,\n",
                "            email: row.try_get(1)?,\n",
                "        })\n",
                "    }\n",
                "}\n",
                "pub async fn get_user<'e, E>(\n",
                "    executor: E,\n",
                "    params: GetUserParams,\n",
                ") -> Result<GetUserRow, sqlx::Error>\n",
                "where\n",
                "    E: sqlx::Executor<'e, Database = sqlx::Postgres>,\n",
                "{\n",
                "    sqlx::query_as::<_, GetUserRow>(GET_USER_SQL)\n",
                "        .bind(params.id)\n",
                "        .fetch_one(executor)\n",
                "        .await\n",
                "}\n"
            )
        );
    }

    #[test]
    fn renders_queryforge_scalar_overrides() {
        let mut query = query(Cardinality::One);
        query.params[0].rust_type = RustType::new("UserId");
        query.params[0].source = TypeSource::UserOverride;
        query.columns[0].rust_type = RustType::new("UserId");
        query.columns[0].source = TypeSource::UserOverride;
        let rendered = render_query(&query);

        assert!(rendered.contains("pub id: UserId"));
        assert!(rendered.contains("QueryForgeEncode"));
        assert!(rendered.contains("queryforge_encode"));
        assert!(rendered.contains("params.id"));
        assert!(rendered.contains("UserId as queryforge::QueryForgeDecode"));
        assert!(rendered.contains("try_get(0)?"));
        assert!(rendered.contains("queryforge_decode(raw)"));
        assert!(rendered.contains("sqlx::Error::Decode(Box::new(err))"));
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
