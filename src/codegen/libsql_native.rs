use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Cardinality, QueryShape};

use super::{
    format_query_tokens, lit_str, parse_type, pascal_ident, rendered_columns, snake_ident,
    upper_ident,
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
    let param_values = render_param_values(query);

    let body = match query.cardinality {
        Cardinality::Exec => quote! {
            pub async fn #fn_name<E>(executor: &E #(, #param_args)*) -> queryforge::Result<u64>
            where
                E: queryforge::runtime::libsql_executor::LibsqlExecutor + ?Sized,
            {
                #param_values
                executor.execute(#sql_const, &params).await
            }
        },
        Cardinality::Optional => quote! {
            #row_tokens
            pub async fn #fn_name<E>(executor: &E #(, #param_args)*) -> queryforge::Result<Option<#row_name>>
            where
                E: queryforge::runtime::libsql_executor::LibsqlExecutor + ?Sized,
            {
                #param_values
                executor
                    .query_optional(#sql_const, &params)
                    .await?
                    .map(#row_name::try_from)
                    .transpose()
            }
        },
        Cardinality::Many | Cardinality::Stream | Cardinality::Batch => quote! {
            #row_tokens
            pub async fn #fn_name<E>(executor: &E #(, #param_args)*) -> queryforge::Result<Vec<#row_name>>
            where
                E: queryforge::runtime::libsql_executor::LibsqlExecutor + ?Sized,
            {
                #param_values
                executor
                    .query_many(#sql_const, &params)
                    .await?
                    .into_iter()
                    .map(#row_name::try_from)
                    .collect()
            }
        },
        Cardinality::One | Cardinality::Scalar => quote! {
            #row_tokens
            pub async fn #fn_name<E>(executor: &E #(, #param_args)*) -> queryforge::Result<#row_name>
            where
                E: queryforge::runtime::libsql_executor::LibsqlExecutor + ?Sized,
            {
                #param_values
                let row = executor.query_one(#sql_const, &params).await?;
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

    let columns = rendered_columns(query);
    let fields = columns.iter().map(|column| {
        let field = &column.field;
        let ty = &column.ty;
        quote! { pub #field: #ty }
    });
    let getters = columns.iter().map(|column| {
        let field = &column.field;
        let index = &column.index;
        quote! { #field: row.try_get_index(#index)? }
    });

    quote! {
        #[derive(Debug, Clone)]
        pub struct #row_name {
            #( #fields, )*
        }

        impl TryFrom<queryforge::runtime::libsql_executor::LibsqlRow> for #row_name {
            type Error = queryforge::Error;

            fn try_from(row: queryforge::runtime::libsql_executor::LibsqlRow) -> Result<Self, Self::Error> {
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

fn render_param_values(query: &QueryShape) -> TokenStream {
    if query.params.is_empty() {
        return quote! {
            let params: Vec<queryforge::runtime::libsql_executor::LibsqlValue> = Vec::new();
        };
    }

    let values = query.params.iter().map(|param| {
        let name = snake_ident(&param.name);
        quote! { queryforge::runtime::libsql_executor::LibsqlValue::from(#name) }
    });
    quote! {
        let params = vec![#( #values ),*];
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
    fn renders_transaction_shaped_executor_signature() {
        let rendered = render_query(&query(Cardinality::One));

        assert!(rendered.contains("pub async fn get_user<E>(executor: &E, id: i64)"));
        assert!(
            rendered.contains("E: queryforge::runtime::libsql_executor::LibsqlExecutor + ?Sized")
        );
        assert!(rendered.contains("queryforge::Result<GetUserRow>"));
        assert!(rendered.contains("queryforge::runtime::libsql_executor::LibsqlValue::from(id)"));
        assert!(rendered.contains("executor.query_one(GET_USER_SQL, &params).await?"));
        assert!(rendered.contains(
            "impl TryFrom<queryforge::runtime::libsql_executor::LibsqlRow> for GetUserRow"
        ));
    }

    #[test]
    fn renders_exec_as_affected_row_count_shape() {
        let rendered = render_query(&QueryShape {
            cardinality: Cardinality::Exec,
            columns: Vec::new(),
            ..query(Cardinality::Exec)
        });

        assert!(rendered.contains("queryforge::Result<u64>"));
        assert!(rendered.contains("executor.execute(GET_USER_SQL, &params).await"));
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
