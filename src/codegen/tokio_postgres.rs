use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Cardinality, Nullability, QueryShape};

use super::{
    format_query_tokens, lit_str, params_arg, pascal_ident, render_params_struct, rendered_columns,
    rendered_params, snake_ident, upper_ident,
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
    let uses_queryforge_result = query
        .params
        .iter()
        .any(|param| param.source == crate::ir::TypeSource::UserOverride)
        || query
            .columns
            .iter()
            .any(|column| column.source == crate::ir::TypeSource::UserOverride);
    let row_tokens = render_row_struct(query, &row_name, uses_queryforge_result);
    let params_arg = params_arg(query);
    let params_struct = render_params_struct(query);
    let param_slice = render_param_slice(query);

    let body = match query.cardinality {
        Cardinality::Exec => {
            if uses_queryforge_result {
                quote! {
                    pub async fn #fn_name<C>(client: &C #params_arg) -> queryforge::Result<u64>
                    where
                        C: tokio_postgres::GenericClient + Sync,
                    {
                        #param_slice
                        client
                            .execute(#sql_const, query_params)
                            .await
                            .map_err(|err| queryforge::Error::Backend(format!("tokio-postgres error: {err}")))
                    }
                }
            } else {
                quote! {
                    pub async fn #fn_name<C>(client: &C #params_arg) -> Result<u64, tokio_postgres::Error>
                    where
                        C: tokio_postgres::GenericClient + Sync,
                    {
                        #param_slice
                        client.execute(#sql_const, query_params).await
                    }
                }
            }
        }
        Cardinality::Optional => {
            if uses_queryforge_result {
                quote! {
                    #row_tokens
                    pub async fn #fn_name<C>(client: &C #params_arg) -> queryforge::Result<Option<#row_name>>
                    where
                        C: tokio_postgres::GenericClient + Sync,
                    {
                        #param_slice
                        client
                            .query_opt(#sql_const, query_params)
                            .await
                            .map_err(|err| queryforge::Error::Backend(format!("tokio-postgres error: {err}")))?
                            .map(#row_name::try_from)
                            .transpose()
                    }
                }
            } else {
                quote! {
                    #row_tokens
                    pub async fn #fn_name<C>(client: &C #params_arg) -> Result<Option<#row_name>, tokio_postgres::Error>
                    where
                        C: tokio_postgres::GenericClient + Sync,
                    {
                        #param_slice
                        client
                            .query_opt(#sql_const, query_params)
                            .await?
                            .map(#row_name::try_from)
                            .transpose()
                    }
                }
            }
        }
        Cardinality::Many | Cardinality::Stream | Cardinality::Batch => {
            if uses_queryforge_result {
                quote! {
                    #row_tokens
                    pub async fn #fn_name<C>(client: &C #params_arg) -> queryforge::Result<Vec<#row_name>>
                    where
                        C: tokio_postgres::GenericClient + Sync,
                    {
                        #param_slice
                        client
                            .query(#sql_const, query_params)
                            .await
                            .map_err(|err| queryforge::Error::Backend(format!("tokio-postgres error: {err}")))?
                            .into_iter()
                            .map(#row_name::try_from)
                            .collect()
                    }
                }
            } else {
                quote! {
                    #row_tokens
                    pub async fn #fn_name<C>(client: &C #params_arg) -> Result<Vec<#row_name>, tokio_postgres::Error>
                    where
                        C: tokio_postgres::GenericClient + Sync,
                    {
                        #param_slice
                        client
                            .query(#sql_const, query_params)
                            .await?
                            .into_iter()
                            .map(#row_name::try_from)
                            .collect()
                    }
                }
            }
        }
        Cardinality::One | Cardinality::Scalar => {
            if uses_queryforge_result {
                quote! {
                    #row_tokens
                    pub async fn #fn_name<C>(client: &C #params_arg) -> queryforge::Result<#row_name>
                    where
                        C: tokio_postgres::GenericClient + Sync,
                    {
                        #param_slice
                        let row = client
                            .query_one(#sql_const, query_params)
                            .await
                            .map_err(|err| queryforge::Error::Backend(format!("tokio-postgres error: {err}")))?;
                        #row_name::try_from(row)
                    }
                }
            } else {
                quote! {
                    #row_tokens
                    pub async fn #fn_name<C>(client: &C #params_arg) -> Result<#row_name, tokio_postgres::Error>
                    where
                        C: tokio_postgres::GenericClient + Sync,
                    {
                        #param_slice
                        let row = client.query_one(#sql_const, query_params).await?;
                        #row_name::try_from(row)
                    }
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

fn render_row_struct(
    query: &QueryShape,
    row_name: &proc_macro2::Ident,
    uses_queryforge_result: bool,
) -> TokenStream {
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
                        let raw: Option<<#base_ty as queryforge::QueryForgeDecode>::Storage> = row
                            .try_get(#index)
                            .map_err(|err| queryforge::Error::Backend(format!("tokio-postgres row decode error: {err}")))?;
                        raw.map(<#base_ty as queryforge::QueryForgeDecode>::queryforge_decode)
                            .transpose()?
                    }
                },
                Nullability::NonNull | Nullability::Unknown => quote! {
                    #field: {
                        let raw: <#base_ty as queryforge::QueryForgeDecode>::Storage = row
                            .try_get(#index)
                            .map_err(|err| queryforge::Error::Backend(format!("tokio-postgres row decode error: {err}")))?;
                        <#base_ty as queryforge::QueryForgeDecode>::queryforge_decode(raw)?
                    }
                },
            }
        } else if uses_queryforge_result {
            quote! {
                #field: row
                    .try_get(#index)
                    .map_err(|err| queryforge::Error::Backend(format!("tokio-postgres row decode error: {err}")))?
            }
        } else {
            quote! { #field: row.try_get(#index)? }
        }
    });

    if uses_queryforge_result {
        quote! {
            #[derive(Debug, Clone)]
            pub struct #row_name {
                #( #fields, )*
            }

            impl TryFrom<tokio_postgres::Row> for #row_name {
                type Error = queryforge::Error;

                fn try_from(row: tokio_postgres::Row) -> Result<Self, Self::Error> {
                    Ok(Self {
                        #( #getters, )*
                    })
                }
            }
        }
    } else {
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
}

fn render_param_slice(query: &QueryShape) -> TokenStream {
    if query.params.is_empty() {
        return quote! {
            let query_params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[];
        };
    }

    let params = rendered_params(query);
    let storage_bindings = params.iter().filter_map(|param| {
        if param.encode_override {
            let field = &param.field;
            let storage = snake_ident(&format!("{field}_queryforge_storage"));
            Some(quote! {
                let #storage = queryforge::QueryForgeEncode::queryforge_encode(params.#field);
            })
        } else {
            None
        }
    });
    let param_refs = params.iter().map(|param| {
        let field = &param.field;
        if param.encode_override {
            let storage = snake_ident(&format!("{field}_queryforge_storage"));
            quote! { &#storage }
        } else {
            quote! { &params.#field }
        }
    });
    quote! {
        #( #storage_bindings )*
        let query_params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[#( #param_refs ),*];
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
        assert!(rendered.contains("pub struct GetUserParams"));
        assert!(rendered.contains("pub struct GetUserRow"));
        assert!(rendered.contains("impl TryFrom<tokio_postgres::Row> for GetUserRow"));
        assert!(rendered.contains(
            "let query_params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[&params.id];"
        ));
        assert!(rendered.contains("client.query_one(GET_USER_SQL, query_params).await?"));
        assert!(rendered.contains("email: row.try_get(1)?"));
    }

    #[test]
    fn documents_transaction_compatible_generic_client_shape() {
        let rendered = render_query(&query(Cardinality::One));

        assert!(rendered.contains("pub async fn get_user<C>("));
        assert!(rendered.contains("client: &C,"));
        assert!(rendered.contains("params: GetUserParams,"));
        assert!(rendered.contains("C: tokio_postgres::GenericClient + Sync"));
        assert!(rendered.contains("client.query_one(GET_USER_SQL, query_params).await?"));
    }

    #[test]
    fn renders_optional_and_many_cardinalities() {
        let optional = render_query(&query(Cardinality::Optional));
        assert!(optional.contains(".query_opt(GET_USER_SQL, query_params)"));
        assert!(optional.contains(".transpose()"));

        let many = render_query(&query(Cardinality::Many));
        assert!(many.contains(".query(GET_USER_SQL, query_params)"));
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
        assert!(rendered.contains("client.execute(GET_USER_SQL, query_params).await"));
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
                "impl TryFrom<tokio_postgres::Row> for GetUserRow {\n",
                "    type Error = tokio_postgres::Error;\n",
                "    fn try_from(row: tokio_postgres::Row) -> Result<Self, Self::Error> {\n",
                "        Ok(Self {\n",
                "            id: row.try_get(0)?,\n",
                "            email: row.try_get(1)?,\n",
                "        })\n",
                "    }\n",
                "}\n",
                "pub async fn get_user<C>(\n",
                "    client: &C,\n",
                "    params: GetUserParams,\n",
                ") -> Result<GetUserRow, tokio_postgres::Error>\n",
                "where\n",
                "    C: tokio_postgres::GenericClient + Sync,\n",
                "{\n",
                "    let query_params: &[&(dyn tokio_postgres::types::ToSql + Sync)] = &[&params.id];\n",
                "    let row = client.query_one(GET_USER_SQL, query_params).await?;\n",
                "    GetUserRow::try_from(row)\n",
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
        assert!(rendered.contains("pub async fn get_user<C>("));
        assert!(rendered.contains(") -> queryforge::Result<GetUserRow>"));
        assert!(rendered.contains("id_queryforge_storage"));
        assert!(rendered.contains("QueryForgeEncode"));
        assert!(rendered.contains("queryforge_encode"));
        assert!(rendered.contains("params.id"));
        assert!(rendered.contains("&id_queryforge_storage"));
        assert!(rendered.contains("UserId as queryforge::QueryForgeDecode"));
        assert!(rendered.contains("queryforge_decode(raw)?"));
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
